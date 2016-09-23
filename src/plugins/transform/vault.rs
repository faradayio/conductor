//! Plugin which issues vault tokens to services.

use docker_compose::v2 as dc;
use serde_yaml;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;
#[cfg(test)]
use std::rc::Rc;
use vault;
use vault::client::VaultDuration;

use plugins;
use plugins::transform::Operation;
use plugins::transform::{Plugin as PluginTransform, PluginNew};
use project::Project;
use util::Error;

#[cfg(feature = "serde_macros")]
include!(concat!("vault_config.in.rs"));
#[cfg(feature = "serde_codegen")]
include!(concat!(env!("OUT_DIR"), "/plugins/transform/vault_config.rs"));

/// The "environment" in which to interpret a configuration file.  We don't
/// want to use the OS environment variables, but rather a fake environment
/// with a few carefully selected values.
#[derive(Debug)]
struct ConfigEnvironment<'a> {
    /// The context for our transformation, including the project, pod,
    /// etc.
    ctx: &'a plugins::Context<'a>,
    /// The name of the current service.
    service: &'a str,
}

impl<'a> dc::Environment for ConfigEnvironment<'a> {
    fn var(&self, key: &str) -> Result<String, env::VarError> {
        let result = match key {
            "PROJECT" => Ok(self.ctx.project.name()),
            "OVERRIDE" => Ok(self.ctx.ovr.name()),
            "POD" => Ok(self.ctx.pod.name()),
            "SERVICE" => Ok(self.service),
            _ => Err(env::VarError::NotPresent),
        };
        result.map(|s| s.to_owned())
    }
}

/// An abstract interface to Vault's token-generation capabilities.  We use
/// this to mock vault during tests.
trait GenerateToken: Debug {
    /// Get a `VAULT_ADDR` value to use along with this token.
    fn addr(&self) -> &str;
    /// Generate a token with the specified parameters.
    fn generate_token(&self,
                      display_name: &str,
                      policies: Vec<String>,
                      ttl: VaultDuration)
                      -> Result<String, Error>;
}

/// A list of calls made to a `MockVault` instance.
#[cfg(test)]
type MockVaultCalls = Rc<RefCell<Vec<(String, Vec<String>, VaultDuration)>>>;

/// A fake interface to vault for testing purposes.
#[derive(Debug)]
#[cfg(test)]
struct MockVault {
    /// The tokens we were asked to generate.  We store these in a RefCell
    /// so that we can have "interior" mutability, because we don't want
    /// `generate_token` to be `&mut self` in the general case.
    calls: MockVaultCalls,
}

#[cfg(test)]
impl MockVault {
    /// Create a new MockVault.
    fn new() -> MockVault {
        MockVault { calls: Rc::new(RefCell::new(vec![])) }
    }

    /// Return a reference to record of calls made to our vault.
    fn calls(&self) -> MockVaultCalls {
        self.calls.clone()
    }
}

#[cfg(test)]
impl GenerateToken for MockVault {
    fn addr(&self) -> &str {
        "http://example.com:8200/"
    }

    fn generate_token(&self,
                      display_name: &str,
                      policies: Vec<String>,
                      ttl: VaultDuration)
                      -> Result<String, Error> {
        self.calls.borrow_mut().push((display_name.to_owned(), policies, ttl));
        Ok("fake_token".to_owned())
    }
}

/// An interface to an actual vault server.
#[derive(Debug)]
struct Vault {
    /// The address of our vault server.
    addr: String,
    /// The master token that we'll use to issue new tokens.
    token: String,
}

impl Vault {
    /// Create a new vault client.
    fn new() -> Result<Vault, Error> {
        let addr = try!(env::var("VAULT_ADDR"));
        let token = try!(env::var("VAULT_MASTER_TOKEN"));
        Ok(Vault {
            addr: addr,
            token: token,
        })
    }
}

impl GenerateToken for Vault {
    fn addr(&self) -> &str {
        &self.addr
    }

    fn generate_token(&self,
                      display_name: &str,
                      policies: Vec<String>,
                      ttl: VaultDuration)
                      -> Result<String, Error> {
        // We can't store `client` in `self`, because it has some obnoxious
        // lifetime parameters.  So we'll just recreate it.  This is
        // probably not the worst idea, because it uses `hyper` for HTTP,
        // and `hyper` HTTP connections used to have expiration issues that
        // were tricky for clients to deal with correctly.
        let client = try!(vault::Client::new(&self.addr, &self.token));
        let opts = vault::client::TokenOptions::default()
            .display_name(display_name)
            .renewable(false)
            .ttl(ttl)
            .policies(policies);
        let auth = try!(client.create_token(&opts));
        Ok(auth.client_token)
    }
}

/// Issues `VAULT_TOKEN` values to services.
#[derive(Debug)]
pub struct Plugin {
    /// Our `config/vault.yml` YAML file, parsed and read into memory.
    config: Config,
    /// Our source of tokens.
    generator: Box<GenerateToken>,
}

impl Plugin {
    /// Get the path to this plugin's config file.
    fn config_path(project: &Project) -> PathBuf {
        project.root_dir().join("config/vault.yml")
    }

    /// Create a new plugin, specifying an alternate source for tokens.
    fn new_with_generator<G>(project: &Project, generator: G) -> Result<Plugin, Error>
        where G: GenerateToken + 'static
    {
        let f = try!(fs::File::open(&Self::config_path(project)));
        let config = try!(serde_yaml::from_reader(f));
        Ok(Plugin {
            config: config,
            generator: Box::new(generator),
        })
    }
}

impl PluginTransform for Plugin {
    fn name(&self) -> &'static str {
        "vault"
    }

    fn transform(&self,
                 _op: Operation,
                 ctx: &plugins::Context,
                 file: &mut dc::File)
                 -> Result<(), Error> {

        for (name, mut service) in &mut file.services {
            // Set up a ConfigEnvironment that we can use to perform
            // interpolations of values like `$SERVICE` in our config file.
            let env = ConfigEnvironment {
                ctx: ctx,
                service: name,
            };

            // Define a local helper function to interpolate
            // `RawOr<String>` values using `env`.
            let interpolated = |raw_val: &dc::RawOr<String>| -> Result<String, Error> {
                let mut val = raw_val.to_owned();
                Ok(try!(val.interpolate_env(&env)).to_owned())
            };

            // Insert our VAULT_ADDR value into the generated files.
            service.environment
                .insert("VAULT_ADDR".to_owned(), self.generator.addr().to_owned());

            // Get a list of policy "patterns" that apply to this service.
            let mut raw_policies = self.config.default_policies.clone();
            raw_policies.extend(self.config
                .pods
                .get(ctx.pod.name())
                .and_then(|pod| pod.get(name))
                .map_or_else(|| vec![], |s| s.policies.clone()));

            // Interpolate the variables found in our policy patterns.
            let mut policies = vec![];
            for result in raw_policies.iter().map(|p| interpolated(p)) {
                // We'd like to use std::result::fold here but it's unstable.
                policies.push(try!(result));
            }

            // Generate a VAULT_TOKEN.
            let display_name = format!("{}-{}-{}-{}",
                                       ctx.project.name(),
                                       ctx.ovr.name(),
                                       ctx.pod.name(),
                                       name);
            let ttl = VaultDuration::seconds(self.config.default_ttl);
            let token = try!(self.generator
                .generate_token(&display_name, policies, ttl));
            service.environment.insert("VAULT_TOKEN".to_owned(), token);

            // Add in any extra environment variables.
            for (var, val) in &self.config.extra_environment {
                service.environment.insert(var.to_owned(), try!(interpolated(val)));
            }
        }
        Ok(())
    }
}

impl PluginNew for Plugin {
    /// Should we enable this plugin for this project?
    fn should_enable_for(project: &Project) -> Result<bool, Error> {
        let path = Self::config_path(project);
        Ok(path.exists())
    }

    /// Create a new plugin.
    fn new(project: &Project) -> Result<Self, Error> {
        Self::new_with_generator(project, try!(Vault::new()))
    }
}

#[test]
fn interpolates_policies() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("rails_hello").unwrap();
    let ovr = proj.ovr("production").unwrap();

    let vault = MockVault::new();
    let calls = vault.calls();
    let plugin = Plugin::new_with_generator(&proj, vault).unwrap();

    let frontend = proj.pod("frontend").unwrap();
    let ctx = plugins::Context::new(&proj, ovr, frontend);
    let mut file = frontend.merged_file(ovr).unwrap();
    plugin.transform(Operation::Output, &ctx, &mut file).unwrap();
    let web = file.services.get("web").unwrap();
    assert_eq!(web.environment.get("VAULT_ADDR").expect("has VAULT_ADDR"),
               "http://example.com:8200/");
    assert_eq!(web.environment.get("VAULT_TOKEN").expect("has VAULT_TOKEN"),
               "fake_token");
    assert_eq!(web.environment.get("VAULT_ENV").expect("has VAULT_ENV"),
               "production");

    let calls = calls.borrow();
    assert_eq!(calls.len(), 1);
    let (ref display_name, ref policies, ref ttl) = calls[0];
    assert_eq!(display_name, "rails_hello-production-frontend-web");
    assert_eq!(policies,
               &["rails_hello-production".to_owned(),
                 "rails_hello-production-frontend-web".to_owned(),
                 "rails_hello-production-ssl".to_owned()]);
    assert_eq!(ttl, &VaultDuration::seconds(2592000));
}