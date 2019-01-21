//! The `kill` command.

use args;
use cmd::CommandCompose;
use command_runner::CommandRunner;
#[cfg(test)]
use command_runner::TestCommandRunner;
use errors::*;
use project::Project;

/// We implement `kill` with a trait so we put it in its own module.
pub trait CommandKill {
    /// Kill given service
    fn kill<CR>(
        &self,
        runner: &CR,
        act_on: &args::ActOn,
        opts: &args::opts::Kill,
    ) -> Result<()>
    where
        CR: CommandRunner;
}

impl CommandKill for Project {
    fn kill<CR>(
        &self,
        runner: &CR,
        act_on: &args::ActOn,
        opts: &args::opts::Kill,
    ) -> Result<()>
    where
        CR: CommandRunner,
    {
        match *act_on {
            args::ActOn::Named(ref names) if names.len() == 1 => {
                self.compose(runner, "kill", act_on, opts)
            }
            _ => Err("You may only specify a single service or pod".into()),
        }
    }
}

#[test]
fn runs_docker_compose_kill() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("rails_hello").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("kill").unwrap();

    let mut opts = args::opts::Kill::default();
    opts.signal = Some(String::from("FOO"));
    proj.kill(
        &runner,
        &args::ActOn::Named(vec!["frontend".to_owned()]),
        &opts,
    ).unwrap();
    assert_ran!(runner, {
        [
            "docker-compose",
            "-p",
            "railshello",
            "-f",
            proj.output_dir().join("pods").join("frontend.yml"),
            "kill",
            "-s",
            "FOO"
        ]
    });

    proj.remove_test_output().unwrap();
}

#[test]
fn errors_when_act_on_specifies_multiple_containers() {
    use env_logger;
    let _ = env_logger::init();
    let proj = Project::from_example("rails_hello").unwrap();
    let runner = TestCommandRunner::new();
    proj.output("kill").unwrap();

    let opts = args::opts::Kill::default();
    assert!(proj.kill(&runner, &args::ActOn::All, &opts).is_err());

    proj.remove_test_output().unwrap();
}
