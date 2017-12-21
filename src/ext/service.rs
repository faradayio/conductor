//! Extension methods for `compose_yml::v2::Service`.

use compose_yml::v2 as dc;
use shlex;
use std::vec;

use errors::*;
#[cfg(test)]
use project::Project;
use sources::{self, Source};
use ext::context::ContextExt;
use util::err;

/// These methods will appear as regular methods on `Service` in any module
/// which includes `ServiceExt`.
pub trait ServiceExt {
    /// The build context associated with this service (either a git
    /// repository URL or a local directory).
    fn context(&self) -> Result<Option<&dc::Context>>;

    /// The directory in which to mount our source code if it's checked
    /// out.
    fn source_mount_dir(&self) -> Result<String>;

    /// The subdirectory inside the source where the code for this service is located.
    /// `Ok(None)` either means that this service has no build context, or that
    /// its context is not a git repository, or that its context is a git repository
    /// without a subdirectory. `Ok(Some(dir)` means that the service's build context
    /// is a git repository, with a subdirectory of `dir`.
    fn repository_subdirectory(&self) -> Result<Option<String>>;

    /// Get the default shell associated with this service.  Used for
    /// getting interactive access to a container.
    fn shell(&self) -> Result<String>;

    /// Get the test command associated with this service.
    fn test_command(&self) -> Result<Vec<String>>;

    /// All the `Source` trees which can be mounted into this `Service`.
    /// Note that this iterator does not hold any references to this
    /// `Service` object, so you can use it to decide how you want to
    /// update other fields of this object without running afoul of the
    /// borrow checker.
    fn sources<'a, 'b>(&'a self, sources: &'b sources::Sources)
        -> Result<Sources<'b>>;
}

impl ServiceExt for dc::Service {
    fn context(&self) -> Result<Option<&dc::Context>> {
        if let Some(ref build) = self.build {
            Ok(Some(build.context.value()?))
        } else {
            Ok(None)
        }
    }

    fn source_mount_dir(&self) -> Result<String> {
        let default = dc::escape("/app")?;
        let srcdir = match self.labels
            .get("io.fdy.cage.srcdir") {
            Some(&Some(ref v)) => v,
            _ => &default,
        };
        Ok(srcdir.value()?.to_owned())
    }

    fn repository_subdirectory(&self) -> Result<Option<String>> {
        if let Some(context) = self.context()? {
            return match *context {
                dc::Context::Dir(_) => Ok(None),
                dc::Context::GitUrl(ref git_url) => {
                    Ok(git_url.subdirectory().map(|subdir| subdir.to_string()))
                }
            };
        }
        Ok(None)
    }

    fn shell(&self) -> Result<String> {
        let default = dc::escape("sh")?;
        let shell = match self.labels
            .get("io.fdy.cage.shell") {
            Some(&Some(ref v)) => v,
            _ => &default,
        };
        Ok(shell.value()?.to_owned())
    }

    fn test_command(&self) -> Result<Vec<String>> {
        let raw = self.labels.get("io.fdy.cage.test").and_then(|v| v.clone()).ok_or_else(|| {
            err("specify a value for the label io.fdy.cage.test to run tests")
        })?;
        let mut lexer = shlex::Shlex::new(raw.value()?);
        let result: Vec<String> = lexer.by_ref().map(|w| w.to_owned()).collect();
        if lexer.had_error {
            Err(err!("cannot parse <{}> into shell words", raw))
        } else {
            Ok(result)
        }
    }

    fn sources<'a, 'b>(
        &'a self,
        sources: &'b sources::Sources,
    ) -> Result<Sources<'b>> {
        // Get our `context`, if any.
        let container_path = self.source_mount_dir()?;
        let source_subdirectory = self.repository_subdirectory()?;

        let context = self.context()?
            .and_then(|ctx| {
                // human_alias is called on every context when constructing Sources
                let alias = &ctx.human_alias()
                    .expect("human_alias failed on a context that worked previously");
                sources.find_by_alias(alias)
            })
            .and_then(|source| {
                Some(SourceMount {
                    container_path,
                    source,
                    source_subdirectory,
                })
            });

        // Get our library keys and mount points.
        let mut libs = vec![];
        for (label, mount_as) in &self.labels {
            let prefix = "io.fdy.cage.lib.";
            if label.starts_with(prefix) {
                let lib_name = label[prefix.len()..].to_string();
                let source = sources.find_by_lib_key(&lib_name).ok_or_else(
                    || -> Error { ErrorKind::UnknownLibKey(lib_name).into() },
                )?;

                let mount_as = mount_as.as_ref().ok_or_else(|| {
                    err!("specify a value for {}", prefix)
                })?;

                libs.push(SourceMount {
                    container_path: mount_as.value()?.to_owned(),
                    source,
                    source_subdirectory: None,
                })
            }
        }

        Ok(Sources {
            context,
            libs: libs.into_iter(),
        })
    }
}

/// Iterator over all the `Source` trees which can be mounted into this
/// `Service`.
pub struct Sources<'a> {
    /// Do we need to iterate over our `context` field?
    context: Option<SourceMount<'a>>,
    /// Libraries
    libs: vec::IntoIter<SourceMount<'a>>,
}

#[derive(Clone)]
pub struct SourceMount<'a> {
    pub container_path: String,
    pub source: &'a Source,
    pub source_subdirectory: Option<String>,
}

impl<'a> Iterator for Sources<'a> {
    type Item = SourceMount<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Check for a `SourceMount` using `take`, which moves data out of
        // an `Option` value and leaves `None` in its place,
        // simulataneously updating our internal state and keeping the
        // borrow checker happy.
        self.context.take().or_else(|| {
            // If there is no SourceMount for the service itself, iterate over libs
            self.libs.next()
        })
    }
}

#[test]
fn src_dir_returns_the_source_directory_for_this_service() {
    use env_logger;
    let _ = env_logger::init();
    let proj: Project = Project::from_example("rails_hello").unwrap();
    let target = proj.target("development").unwrap();

    // Default value.
    let db = proj.pod("db").unwrap();
    let merged = db.merged_file(target).unwrap();
    let db = merged.services.get("db").unwrap();
    assert_eq!(db.source_mount_dir().unwrap(), "/app");

    // Custom value.
    let frontend = proj.pod("frontend").unwrap();
    let merged = frontend.merged_file(target).unwrap();
    let proxy = merged.services.get("web").unwrap();
    assert_eq!(proxy.source_mount_dir().unwrap(), "/usr/src/app");
}

#[test]
fn build_context_can_specify_a_subdirectory() {
    use env_logger;
    let _ = env_logger::init();
    let proj: Project = Project::from_fixture("with_repo_subdir").unwrap();
    let target = proj.target("development").unwrap();

    // Default value.
    let db = proj.pod("db").unwrap();
    let merged = db.merged_file(target).unwrap();
    let db = merged.services.get("db").unwrap();
    assert_eq!(db.repository_subdirectory().unwrap(), None);

    // Custom value.
    let frontend = proj.pod("frontend").unwrap();
    let merged = frontend.merged_file(target).unwrap();
    let web = merged.services.get("web").unwrap();
    assert_eq!(
        web.repository_subdirectory().unwrap(),
        Some("myfolder".to_string())
    );
}

#[test]
fn shell_returns_preferred_shell_for_this_service() {
    use env_logger;
    let _ = env_logger::init();
    let proj: Project = Project::from_example("hello").unwrap();
    let target = proj.target("development").unwrap();
    let frontend = proj.pod("frontend").unwrap();
    let merged = frontend.merged_file(target).unwrap();

    // Default value.
    let web = merged.services.get("web").unwrap();
    assert_eq!(web.shell().unwrap(), "sh");

    // Custom value.
    let proxy = merged.services.get("proxy").unwrap();
    assert_eq!(proxy.shell().unwrap(), "/bin/sh");
}
