//! Plugin which issues vault tokens to services.

use compose_yml::v2 as dc;
use std::result;
use std::collections::BTreeMap;
use std::env;
use std::fmt::Debug;
use std::fs;
use std::io::{self, Read};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::{Arc, RwLock};
use vault;
use vault::client::VaultDuration;

use errors::*;
use plugins;
use plugins::{Operation, PluginGenerate, PluginNew, PluginTransform};
use project::Project;
use serde_helpers::load_yaml;
use util::err;

// TODO: This old-style serde `include!` should be inline or a module.
include!("vault_config.in.rs");

/// Load a vault token from `~/.vault-token`, where the command line client
/// puts it.
fn load_vault_token_from_file() -> Result<String> {
    let path = env::home_dir()
        .ok_or_else(|| err("You do not appear to have a home directory"))?
        .join(".vault-token");
    let mkerr = || ErrorKind::CouldNotReadFile(path.clone());
    let f = fs::File::open(&path).chain_err(&mkerr)?;
    let mut reader = io::BufReader::new(f);
    let mut result = String::new();
    reader.read_to_string(&mut result).chain_err(&mkerr)?;
    Ok(result.trim().to_owned())
}

/// Find the vault token we'll use to generate new tokens.
fn find_vault_token() -> Result<String> {
    env::var("VAULT_MASTER_TOKEN")
        .or_else(|_| env::var("VAULT_TOKEN"))
        .or_else(|_| load_vault_token_from_file())
        .map_err(|e| {
            err!(
                "{}.  You probably want to log in using the vault client or set \
                 VAULT_MASTER_TOKEN",
                e
            )
        })
}

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
    fn var(&self, key: &str) -> result::Result<String, env::VarError> {
        let result = match key {
            "PROJECT" => Ok(self.ctx.project.name()),
            "TARGET" => Ok(self.ctx.project.current_target().name()),
            "POD" => Ok(self.ctx.pod.name()),
            "SERVICE" => Ok(self.service),
            _ => Err(env::VarError::NotPresent),
        };
        result.map(|s| s.to_owned())
    }
}

/// An abstract interface to Vault's token-generation capabilities.  We use
/// this to mock vault during tests.
trait GenerateToken: Debug + Sync {
    /// Get a `VAULT_ADDR` value to use along with this token.
    fn addr(&self) -> &str;
    /// Generate a token with the specified parameters.
    fn generate_token(
        &self,
        display_name: &str,
        policies: Vec<String>,
        ttl: VaultDuration,
    ) -> Result<String>;
}

/// A list of calls made to a `MockVault` instance.
#[cfg(test)]
type MockVaultCalls = Arc<RwLock<Vec<(String, Vec<String>, VaultDuration)>>>;

/// A fake interface to vault for testing purposes.
#[derive(Debug)]
#[cfg(test)]
struct MockVault {
    /// The tokens we were asked to generate.  We store these in a RwLock
    /// so that we can have "interior" mutability, because we don't want
    /// `generate_token` to be `&mut self` in the general case.
    calls: MockVaultCalls,
}

#[cfg(test)]
impl MockVault {
    /// Create a new MockVault.
    fn new() -> MockVault {
        MockVault {
            calls: Arc::new(RwLock::new(vec![])),
        }
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

    fn generate_token(
        &self,
        display_name: &str,
        policies: Vec<String>,
        ttl: VaultDuration,
    ) -> Result<String> {
        self.calls
            .write()
            .unwrap()
            .push((display_name.to_owned(), policies, ttl));
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
    fn new() -> Result<Vault> {
        let mut addr = env::var("VAULT_ADDR").map_err(|_| {
            err(
                "Please set the environment variable VAULT_ADDR to the URL of \
                 your vault server",
            )
        })?;
        // TODO MED: Temporary fix because of broken URL handling in
        // hashicorp_vault.  Upstream bug:
        // https://github.com/ChrisMacNaughton/vault-rs/issues/14
        if addr.ends_with('/') {
            let new_len = addr.len() - 1;
            addr.truncate(new_len);
        }
        let token = find_vault_token()?;
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

    fn generate_token(
        &self,
        display_name: &str,
        policies: Vec<String>,
        ttl: VaultDuration,
    ) -> Result<String> {
        let mkerr = || ErrorKind::VaultError(self.addr.clone());

        // We can't store `client` in `self`, because it has some obnoxious
        // lifetime parameters.  So we'll just recreate it.  This is
        // probably not the worst idea, because it uses `hyper` for HTTP,
        // and `hyper` HTTP connections used to have expiration issues that
        // were tricky for clients to deal with correctly.
        let client = vault::Client::new(&self.addr, &self.token).chain_err(&mkerr)?;
        let opts = vault::client::TokenOptions::default()
            .display_name(display_name)
            .renewable(true)
            .ttl(ttl)
            .policies(policies);
        let auth = client.create_token(&opts).chain_err(&mkerr)?;
        Ok(auth.client_token)
    }
}

/// Issues `VAULT_TOKEN` values to services.
#[derive(Debug)]
pub struct Plugin {
    /// Our `config/vault.yml` YAML file, parsed and read into memory.
    /// Optional because if we're being run as a `PluginGenerate`, we won't
    /// have it (but it's guaranteed otherwise).
    config: Option<Config>,
    /// Our source of tokens.
    generator: Option<Box<GenerateToken>>,
}

impl Plugin {
    /// Get the path to this plugin's config file.
    fn config_path(project: &Project) -> PathBuf {
        project.root_dir().join("config").join("vault.yml")
    }

    /// Create a new plugin, specifying an alternate source for tokens.
    fn new_with_generator<G>(project: &Project, generator: Option<G>) -> Result<Plugin>
    where
        G: GenerateToken + 'static,
    {
        let path = Self::config_path(project);
        let config = if path.exists() {
            Some(load_yaml(&path)?)
        } else {
            None
        };
        Ok(Plugin {
            config: config,
            generator: generator.map(|gen: G| -> Box<GenerateToken> { Box::new(gen) }),
        })
    }
}

impl plugins::Plugin for Plugin {
    fn name(&self) -> &'static str {
        Self::plugin_name()
    }
}

impl PluginNew for Plugin {
    fn plugin_name() -> &'static str {
        "vault"
    }

    fn is_configured_for(project: &Project) -> Result<bool> {
        let path = Self::config_path(project);
        Ok(path.exists())
    }

    fn new(project: &Project) -> Result<Self> {
        // An annoying special case.  We may be called as a code generator,
        // in which case we don't want to try to create a `GenerateToken`
        // instance.
        let token_gen = if Self::is_configured_for(project)? {
            Some(Vault::new()?)
        } else {
            None
        };
        Self::new_with_generator(project, token_gen)
    }
}

impl PluginGenerate for Plugin {
    fn generator_description(&self) -> &'static str {
        "Get passwords & other secrets from a Vault server"
    }
}

impl PluginTransform for Plugin {
    fn transform(
        &self,
        _op: Operation,
        ctx: &plugins::Context,
        file: &mut dc::File,
    ) -> Result<()> {
        // Get our plugin config.
        let config = self.config
            .as_ref()
            .expect("config should always be present for transform");
        let generator = self.generator
            .as_ref()
            .expect("generator should always be present for transform");

        // Should this plugin be excluded in this target?
        let target = ctx.project.current_target();
        if !target.is_enabled_by(&config.enable_in_targets) {
            return Ok(());
        }

        // Apply to each service.
        for (name, service) in &mut file.services {
            // Set up a ConfigEnvironment that we can use to perform
            // interpolations of values like `$SERVICE` in our config file.
            let env = ConfigEnvironment {
                ctx: ctx,
                service: name,
            };

            // Define a local helper function to interpolate
            // `RawOr<String>` values using `env`.
            let interpolated = |raw_val: &dc::RawOr<String>| -> Result<String> {
                let mut val = raw_val.to_owned();
                Ok(val.interpolate_env(&env)?.to_owned())
            };

            // Insert our VAULT_ADDR value into the generated files.
            service
                .environment
                .insert("VAULT_ADDR".to_owned(), Some(dc::escape(generator.addr())?));

            // Get a list of policy "patterns" that apply to this service.
            let mut raw_policies = config.default_policies.clone();
            raw_policies.extend(
                config
                    .pods
                    .get(ctx.pod.name())
                    .and_then(|pod| pod.get(name))
                    .map_or_else(|| vec![], |s| s.policies.clone()),
            );

            // Interpolate the variables found in our policy patterns.
            let mut policies = vec![];
            for result in raw_policies.iter().map(|p| interpolated(p)) {
                // We'd like to use std::result::fold here but it's unstable.
                policies.push(result?);
            }
            debug!(
                "Generating token for '{}' with policies {:?}",
                name,
                &policies
            );

            // Generate a VAULT_TOKEN.
            let display_name = format!(
                "{}_{}_{}_{}",
                ctx.project.name(),
                ctx.project.current_target().name(),
                ctx.pod.name(),
                name
            );
            let ttl = VaultDuration::seconds(config.default_ttl);
            let token = generator
                .generate_token(&display_name, policies, ttl)
                .chain_err(|| format!("could not generate token for '{}'", name))?;
            service
                .environment
                .insert("VAULT_TOKEN".to_owned(), Some(dc::escape(token)?));

            // Add in any extra environment variables.
            for (var, val) in &config.extra_environment {
                service
                    .environment
                    .insert(var.to_owned(), Some(dc::escape(interpolated(val)?)?));
            }
        }
        Ok(())
    }
}

#[test]
fn interpolates_policies() {
    use env_logger;
    let _ = env_logger::init();

    env::set_var("VAULT_ADDR", "http://example.com:8200/");
    env::set_var("VAULT_MASTER_TOKEN", "fake master token");

    let mut proj = Project::from_example("vault_integration").unwrap();
    proj.set_current_target_name("production").unwrap();

    let vault = MockVault::new();
    let calls = vault.calls();
    let plugin = Plugin::new_with_generator(&proj, Some(vault)).unwrap();

    let frontend = proj.pod("frontend").unwrap();
    let ctx = plugins::Context::new(&proj, frontend, "up");
    let mut file = frontend.merged_file(proj.current_target()).unwrap();
    plugin
        .transform(Operation::Output, &ctx, &mut file)
        .unwrap();
    let web = file.services.get("web").unwrap();
    let vault_addr = web.environment.get("VAULT_ADDR").expect("has VAULT_ADDR");
    assert_eq!(vault_addr.value().unwrap(), "http://example.com:8200/");
    let vault_token = web.environment.get("VAULT_TOKEN").expect("has VAULT_TOKEN");
    assert_eq!(vault_token.value().unwrap(), "fake_token");
    let vault_env = web.environment.get("VAULT_ENV").expect("has VAULT_ENV");
    assert_eq!(vault_env.value().unwrap(), "production");

    let calls = calls.read().unwrap();
    assert_eq!(calls.len(), 1);
    let (ref display_name, ref policies, ref ttl) = calls[0];
    assert_eq!(display_name, "vault_integration_production_frontend_web");
    assert_eq!(
        policies,
        &[
            "vault_integration-production".to_owned(),
            "vault_integration-production-frontend-web".to_owned(),
            "vault_integration-production-ssl".to_owned()
        ]
    );
    assert_eq!(ttl, &VaultDuration::seconds(2592000));
}

#[test]
fn only_applied_in_specified_targets() {
    use env_logger;
    let _ = env_logger::init();

    let mut proj = Project::from_example("vault_integration").unwrap();
    proj.set_current_target_name("test").unwrap();
    let target = proj.current_target();

    let vault = MockVault::new();
    let plugin = Plugin::new_with_generator(&proj, Some(vault)).unwrap();

    let frontend = proj.pod("frontend").unwrap();
    let ctx = plugins::Context::new(&proj, frontend, "test");
    let mut file = frontend.merged_file(target).unwrap();
    plugin
        .transform(Operation::Output, &ctx, &mut file)
        .unwrap();
    let web = file.services.get("web").unwrap();
    assert_eq!(web.environment.get("VAULT_ADDR"), None);
}
