//! The `run` command.

use crate::args::{self, ToArgs};
#[cfg(test)]
use crate::command_runner::TestCommandRunner;
use crate::command_runner::{Command, CommandRunner};
use crate::errors::*;
use crate::ext::service::ServiceExt;
use crate::project::Project;

/// We implement `run` with a trait so we put it in its own module.
pub trait CommandRun {
    /// Run a specific pod as a one-shot task.
    fn run<CR>(
        &self,
        runner: &CR,
        service: &str,
        command: Option<&args::Command>,
        opts: &args::opts::Run,
    ) -> Result<()>
    where
        CR: CommandRunner;

    /// Execute tests inside a fresh container.
    fn test<CR>(
        &self,
        runner: &CR,
        service: &str,
        command: Option<&args::Command>,
        opts: &args::opts::Run,
    ) -> Result<()>
    where
        CR: CommandRunner;
}

impl CommandRun for Project {
    fn run<CR>(
        &self,
        runner: &CR,
        service: &str,
        command: Option<&args::Command>,
        opts: &args::opts::Run,
    ) -> Result<()>
    where
        CR: CommandRunner,
    {
        let (pod, service_name) = self.service_or_err(service)?;

        // Build and run our command.
        let command_args = if let Some(c) = command {
            c.to_args()
        } else {
            vec![]
        };
        runner
            .build("docker-compose")
            .args(&pod.compose_args(self)?)
            .arg("run")
            .args(&opts.to_args())
            .arg(service_name)
            .args(&command_args)
            .exec()
    }

    fn test<CR>(
        &self,
        runner: &CR,
        service_name: &str,
        command: Option<&args::Command>,
        opts: &args::opts::Run,
    ) -> Result<()>
    where
        CR: CommandRunner,
    {
        let target = self.current_target();
        let (pod, service_name) = self.service_or_err(service_name)?;

        // If we don't have any mounted sources, warn.
        let service = pod.service_or_err(target, service_name)?;
        let sources = service.sources(self.sources())?.collect::<Vec<_>>();
        let sources_dirs = self.sources_dirs();
        let mount_count = sources
            .iter()
            .cloned()
            .filter(|ref source_mount| {
                source_mount.source.is_available_locally(&sources_dirs)
                    && source_mount.source.mounted()
            })
            .count();
        if mount_count == 0 {
            warn!(
                "No source code mounted into '{}/{}'",
                pod.name(),
                service_name
            );
        }

        let command_args = if let Some(c) = command {
            c.to_args()
        } else {
            let service = pod.service_or_err(target, service_name)?;
            service.test_command()?.iter().map(|s| s.into()).collect()
        };
        runner
            .build("docker-compose")
            .args(&pod.compose_args(self)?)
            .arg("run")
            .args(&opts.to_args())
            .arg("--rm")
            .arg("--no-deps")
            .arg(service_name)
            .args(&command_args)
            .exec()
    }
}

#[test]
fn fails_on_a_multi_service_pod() {
    let _ = env_logger::try_init();
    let proj = Project::from_example("hello").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("run").unwrap();
    let opts = Default::default();
    assert!(proj.run(&runner, "frontend", None, &opts).is_err());
}

#[test]
fn runs_a_single_service_pod() {
    let _ = env_logger::try_init();
    let proj = Project::from_example("rails_hello").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("run").unwrap();
    let cmd = args::Command::new("db:migrate");
    let mut opts = args::opts::Run::default();
    opts.allocate_tty = false;
    proj.run(&runner, "rake", Some(&cmd), &opts).unwrap();
    assert_ran!(runner, {
        [
            "docker-compose",
            "-p",
            "railshello",
            "-f",
            proj.output_dir().join("pods").join("rake.yml"),
            "run",
            "-T",
            "rake",
            "db:migrate",
        ]
    });
    proj.remove_test_output().unwrap();
}

#[test]
fn runs_tests() {
    let _ = env_logger::try_init();
    let mut proj = Project::from_example("hello").unwrap();
    proj.set_current_target_name("test").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("test").unwrap();
    let opts = Default::default();

    proj.test(&runner, "frontend/proxy", None, &opts).unwrap();

    assert_ran!(runner, {
        [
            "docker-compose",
            "-p",
            "hellotest",
            "-f",
            proj.output_pods_dir().join("frontend.yml"),
            "run",
            "--rm",
            "--no-deps",
            "proxy",
            "echo",
            "All tests passed",
        ]
    });

    proj.remove_test_output().unwrap();
}

#[test]
fn runs_tests_with_custom_command() {
    let _ = env_logger::try_init();
    let mut proj = Project::from_example("hello").unwrap();
    proj.set_current_target_name("test").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("test").unwrap();
    let opts = Default::default();

    let cmd = args::Command::new("rspec").with_args(&["-t", "foo"]);
    proj.test(&runner, "proxy", Some(&cmd), &opts).unwrap();

    assert_ran!(runner, {
        [
            "docker-compose",
            "-p",
            "hellotest",
            "-f",
            proj.output_pods_dir().join("frontend.yml"),
            "run",
            "--rm",
            "--no-deps",
            "proxy",
            "rspec",
            "-t",
            "foo",
        ]
    });

    proj.remove_test_output().unwrap();
}
