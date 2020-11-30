//! Our main CLI tool.

#[macro_use]
extern crate clap;
#[macro_use]
extern crate log;

use cage::{
    cmd::*,
    command_runner::{Command, CommandRunner, OsCommandRunner},
    ErrorKind, Project, Result,
};
use colored::Colorize;
use itertools::Itertools;
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
    process,
};
use yaml_rust::yaml;

/// Load our command-line interface definitions from an external `clap`
/// YAML file.  We could create these using code, but at the cost of more
/// verbosity.
fn cli(yaml: &yaml::Yaml) -> clap::App<'_, '_> {
    clap::App::from_yaml(yaml).version(crate_version!())
}

/// Custom methods we want to add to `clap::App`.
trait ArgMatchesExt {
    /// Do we need to generate `.cage/pods`?  This will probably be
    /// refactored in the future.
    fn should_output_project(&self) -> bool;

    /// Get either the specified target name, or a reasonable default.
    fn target_name(&self) -> &str;

    /// Determine what pods or services we're supposed to act on.
    fn to_acts_on(&self, arg_name: &str, include_tasks: bool) -> cage::args::ActOn;

    /// Determine what sources we're supposed to act on.
    fn to_acts_on_sources(
        &self,
        project: &Project,
    ) -> Result<cage::args::ActOnSources>;

    /// Extract options shared by `exec` and `run` from our command-line
    /// arguments.
    fn to_process_options(&self) -> cage::args::opts::Process;

    /// Extract `exec` options from our command-line arguments.
    fn to_exec_options(&self) -> cage::args::opts::Exec;

    /// Extract `run` options from our command-line arguments.
    fn to_run_options(&self) -> cage::args::opts::Run;

    /// Extract `exec::Command` from our command-line arguments.
    fn to_exec_command(&self) -> Option<cage::args::Command>;

    /// Extract 'logs' options from our command-line arguments.
    fn to_logs_options(&self) -> cage::args::opts::Logs;

    /// Extract 'rm' options from our command-line arguments.
    fn to_rm_options(&self) -> cage::args::opts::Rm;
}

impl<'a> ArgMatchesExt for clap::ArgMatches<'a> {
    fn should_output_project(&self) -> bool {
        self.subcommand_name() != Some("export")
    }

    fn target_name(&self) -> &str {
        self.value_of("target").unwrap_or_else(|| {
            if self.subcommand_name() == Some("test") {
                "test"
            } else {
                "development"
            }
        })
    }

    fn to_acts_on(&self, arg_name: &str, include_tasks: bool) -> cage::args::ActOn {
        let names: Vec<String> = self
            .values_of(arg_name)
            .map_or_else(Vec::new, |p| p.collect())
            .iter()
            .map(|&p| p.to_string())
            .collect();
        if names.is_empty() {
            if include_tasks {
                cage::args::ActOn::All
            } else {
                cage::args::ActOn::AllExceptTasks
            }
        } else {
            cage::args::ActOn::Named(names)
        }
    }

    /// Determine what pods or services we're supposed to act on.
    fn to_acts_on_sources(&self, proj: &Project) -> Result<cage::args::ActOnSources> {
        if self.is_present("ALL") {
            Ok(cage::args::ActOnSources::All)
        } else if let Some(aliases) = self.values_of("ALIASES") {
            let aliases = aliases
                .map(|a| -> Result<String> {
                    if proj.sources().find_by_alias(a).is_none() {
                        Err(ErrorKind::UnknownSource(a.to_owned()).into())
                    } else {
                        Ok(a.to_owned())
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(cage::args::ActOnSources::Named(aliases))
        } else {
            panic!("clap source always require --all or a list of sources");
        }
    }

    fn to_process_options(&self) -> cage::args::opts::Process {
        let mut opts = cage::args::opts::Process::default();
        opts.detached = self.is_present("detached");
        opts.user = self.value_of("user").map(|v| v.to_owned());
        opts.allocate_tty = !self.is_present("no-allocate-tty");
        opts
    }

    fn to_exec_options(&self) -> cage::args::opts::Exec {
        let mut opts = cage::args::opts::Exec::default();
        opts.process = self.to_process_options();
        opts.privileged = self.is_present("privileged");
        opts
    }

    fn to_run_options(&self) -> cage::args::opts::Run {
        let mut opts = cage::args::opts::Run::default();
        opts.process = self.to_process_options();
        opts.entrypoint = self.value_of("entrypoint").map(|v| v.to_owned());
        if let Some(environment) = self.values_of("environment") {
            let environment: Vec<&str> = environment.collect();
            for env_val in environment.chunks(2) {
                if env_val.len() != 2 {
                    // Clap should prevent this.
                    panic!("Environment binding '{}' has no value", env_val[0]);
                }
                opts.environment
                    .insert(env_val[0].to_owned(), env_val[1].to_owned());
            }
        }
        opts.no_deps = self.is_present("no-deps");
        opts.service_ports = self.is_present("service-ports");
        opts
    }

    fn to_logs_options(&self) -> cage::args::opts::Logs {
        let mut opts = cage::args::opts::Logs::default();
        opts.follow = self.is_present("follow");
        opts.number = self.value_of("number").map(|v| v.to_owned());
        opts
    }

    fn to_rm_options(&self) -> cage::args::opts::Rm {
        let mut opts = cage::args::opts::Rm::default();
        opts.force = self.is_present("force");
        opts.remove_volumes = self.is_present("remove-volumes");
        opts
    }

    fn to_exec_command(&self) -> Option<cage::args::Command> {
        if self.is_present("COMMAND") {
            let values: Vec<&str> = self.values_of("COMMAND").unwrap().collect();
            assert!(!values.is_empty(), "too few values from CLI parser");
            Some(cage::args::Command::new(values[0]).with_args(&values[1..]))
        } else {
            None
        }
    }
}

/// Display a warning if we think some of our services should be running but
/// they're not.
fn warn_if_pods_are_enabled_but_not_running(project: &cage::Project) -> Result<()> {
    let pods = project.enabled_pods_that_are_not_running()?;
    if !pods.is_empty() {
        let pod_names = pods.iter().map(|p| p.name());
        warn!(
            "You might want to start the following pods first: {0} (see \
             `cage --target={1} up` or `cage --target={1} status`)",
            pod_names.format(", "),
            project.current_target().name()
        );
    }
    Ok(())
}

/// The function which does the real work.  Unlike `main`, we have a return
/// type of `Result` and may therefore use `?` to handle errors.
fn run(matches: &clap::ArgMatches<'_>) -> Result<()> {
    // We know that we always have a subcommand because our `cli.yml`
    // requires this and `clap` is supposed to enforce it.
    let sc_name = matches.subcommand_name().unwrap();
    let sc_matches: &clap::ArgMatches<'_> =
        matches.subcommand_matches(sc_name).unwrap();

    // Handle any subcommands that we can handle without a project
    // directory.
    match sc_name {
        "sysinfo" => {
            all_versions()?;
            return Ok(());
        }
        "new" => {
            cage::Project::generate_new(
                &env::current_dir()?,
                sc_matches.value_of("NAME").unwrap(),
            )?;
            return Ok(());
        }
        _ => {}
    }

    // Handle our standard arguments that apply to all subcommands.
    let mut proj = cage::Project::from_current_dir()?;
    if let Some(project_name) = matches.value_of("project-name") {
        proj.set_name(project_name);
    }
    if let Some(default_tags_path) = matches.value_of("default-tags") {
        let f = fs::File::open(default_tags_path)?;
        let reader = io::BufReader::new(f);
        proj.set_default_tags(cage::DefaultTags::read(reader)?);
    }
    proj.set_current_target_name(matches.target_name())?;

    // Output our project's `*.yml` files for `docker-compose` if we'll
    // need it.
    if matches.should_output_project() {
        proj.output(sc_name)?;
    }

    // Handle our subcommands that require a `Project`.
    let runner = OsCommandRunner::new();
    match sc_name {
        "status" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            proj.status(&runner, &acts_on)?;
        }
        "pull" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            let mut opts = cage::args::opts::Pull::default();
            opts.quiet = sc_matches.is_present("quiet");
            proj.pull(&runner, &acts_on, &opts)?;
        }
        "build" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            let opts = cage::args::opts::Empty;
            proj.compose(&runner, "build", &acts_on, &opts)?;
        }
        "up" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", false);
            let opts = cage::args::opts::Up::new(sc_matches.is_present("init"));
            proj.up(&runner, &acts_on, &opts)?;
        }
        "restart" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", false);
            let opts = cage::args::opts::Empty;
            proj.compose(&runner, "restart", &acts_on, &opts)?;
        }
        "stop" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", false);
            let opts = cage::args::opts::Empty;
            proj.compose(&runner, "stop", &acts_on, &opts)?;
        }
        "rm" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            let opts = sc_matches.to_rm_options();
            proj.compose(&runner, "rm", &acts_on, &opts)?;
        }
        "run" => {
            warn_if_pods_are_enabled_but_not_running(&proj)?;
            let opts = sc_matches.to_run_options();
            let cmd = sc_matches.to_exec_command();
            let service = sc_matches.value_of("SERVICE").unwrap();
            proj.run(&runner, service, cmd.as_ref(), &opts)?;
        }
        "run-script" => {
            warn_if_pods_are_enabled_but_not_running(&proj)?;
            let opts = sc_matches.to_run_options();
            let script_name = sc_matches.value_of("SCRIPT_NAME").unwrap();
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            proj.run_script(&runner, &acts_on, script_name.as_ref(), &opts)?;
        }
        "exec" => {
            warn_if_pods_are_enabled_but_not_running(&proj)?;
            let service = sc_matches.value_of("SERVICE").unwrap();
            let opts = sc_matches.to_exec_options();
            let cmd = sc_matches.to_exec_command().unwrap();
            proj.exec(&runner, service, &cmd, &opts)?;
        }
        "shell" => {
            warn_if_pods_are_enabled_but_not_running(&proj)?;
            let service = sc_matches.value_of("SERVICE").unwrap();
            let opts = sc_matches.to_exec_options();
            proj.shell(&runner, service, &opts)?;
        }
        "test" => {
            warn_if_pods_are_enabled_but_not_running(&proj)?;
            let opts = sc_matches.to_run_options();
            let cmd = sc_matches.to_exec_command();
            let service = sc_matches.value_of("SERVICE").unwrap();
            proj.test(&runner, service, cmd.as_ref(), &opts)?;
        }
        "source" => run_source(&runner, &mut proj, sc_matches)?,
        "generate" => run_generate(&runner, &proj, sc_matches)?,
        "logs" => {
            let acts_on = sc_matches.to_acts_on("POD_OR_SERVICE", true);
            let opts = sc_matches.to_logs_options();
            proj.logs(&runner, &acts_on, &opts)?;
        }
        "export" => {
            let dir = sc_matches.value_of("DIR").unwrap();
            proj.export(Path::new(dir))?;
        }
        unknown => unreachable!("Unexpected subcommand '{}'", unknown),
    }

    Ok(())
}

/// Our `source` subcommand.
fn run_source<R>(
    runner: &R,
    proj: &mut cage::Project,
    matches: &clap::ArgMatches<'_>,
) -> Result<()>
where
    R: CommandRunner,
{
    // We know that we always have a subcommand because our `cli.yml`
    // requires this and `clap` is supposed to enforce it.
    let sc_name = matches.subcommand_name().unwrap();
    let sc_matches: &clap::ArgMatches<'_> =
        matches.subcommand_matches(sc_name).unwrap();

    // Dispatch our subcommand.
    let mut re_output = true;
    match sc_name {
        "ls" => {
            re_output = false;
            proj.source_list(runner)?;
        }
        "clone" => {
            let alias = sc_matches.value_of("ALIAS").unwrap();
            proj.source_clone(runner, alias)?;
        }
        "mount" => {
            let act_on_sources = sc_matches.to_acts_on_sources(proj)?;
            proj.source_set_mounted(runner, act_on_sources, true)?;
        }
        "unmount" => {
            let act_on_sources = sc_matches.to_acts_on_sources(proj)?;
            proj.source_set_mounted(runner, act_on_sources, false)?;
        }
        unknown => unreachable!("Unexpected subcommand '{}'", unknown),
    }

    // Regenerate our output if it might have changed.
    if re_output {
        proj.output(sc_name)?;
    }

    Ok(())
}

/// Our `generate` subcommand.
fn run_generate<R>(
    _runner: &R,
    proj: &cage::Project,
    matches: &clap::ArgMatches<'_>,
) -> Result<()>
where
    R: CommandRunner,
{
    // We know that we always have a subcommand because our `cli.yml`
    // requires this and `clap` is supposed to enforce it.
    let sc_name = matches.subcommand_name().unwrap();
    let sc_matches: &clap::ArgMatches<'_> =
        matches.subcommand_matches(sc_name).unwrap();

    match sc_name {
        // TODO LOW: Allow running this without a project?
        "completion" => {
            let shell = match sc_matches.value_of("SHELL").unwrap() {
                "bash" => clap::Shell::Bash,
                "fish" => clap::Shell::Fish,
                unknown => unreachable!("Unknown shell '{}'", unknown),
            };
            let cli_yaml = load_yaml!("cli.yml");
            cli(cli_yaml).gen_completions("cage", shell, proj.root_dir());
        }
        other => proj.generate(other)?,
    }
    Ok(())
}

/// Print the version of this executable.
fn version() {
    println!("cage {}", cage::version());
}

/// Print the version of this executable and also the versions of several
/// tools we use.
fn all_versions() -> Result<()> {
    version();

    let runner = OsCommandRunner::new();
    for tool in &["docker", "docker-compose", "git"] {
        runner.build(tool).arg("--version").exec()?;
    }
    Ok(())
}

fn log_level_label(level: log::Level) -> colored::ColoredString {
    match level {
        log::Level::Error => "ERROR:".red().bold(),
        log::Level::Warn => "WARNING:".yellow().bold(),
        log::Level::Info => "INFO:".bold(),
        log::Level::Debug => "DEBUG:".normal(),
        log::Level::Trace => "TRACE:".normal(),
    }
}

/// Our main entry point.
fn main() {
    openssl_probe::init_ssl_cert_env_vars();

    // Initialize logging with some custom options, mostly so we can see
    // our own warnings.
    let mut builder = env_logger::Builder::new();
    builder.filter(Some("compose_yml"), log::LevelFilter::Warn);
    builder.filter(Some("compose_yml::v2::validate"), log::LevelFilter::Error);
    builder.filter(Some("cage"), log::LevelFilter::Warn);
    builder.format(
        |f: &mut env_logger::fmt::Formatter, record: &log::Record<'_>| {
            let msg = format!(
                "{} {} (from {})",
                log_level_label(record.level()),
                record.args(),
                record.target()
            );
            if record.level() > log::Level::Info {
                writeln!(f, "{}", msg.dimmed())
            } else {
                writeln!(f, "{}", msg)
            }
        },
    );
    if let Ok(config) = env::var("RUST_LOG") {
        builder.parse_filters(&config);
    }
    builder.init();

    // Parse our command-line arguments.
    let cli_yaml = load_yaml!("cli.yml");
    let matches: clap::ArgMatches<'_> = cli(cli_yaml).get_matches();
    debug!("Arguments: {:?}", &matches);

    // Defer all our real work to `run`, and handle any errors.  This is a
    // standard Rust pattern to make error-handling in `main` nicer.
    if let Err(ref err) = run(&matches) {
        // We use `unwrap` here to turn I/O errors into application panics.
        // If we can't print a message to stderr without an I/O error,
        // the situation is hopeless.
        eprint!("Error: ");
        for e in err.iter() {
            eprintln!("{}", e);
        }
        if let Some(backtrace) = err.backtrace() {
            eprintln!("{:?}", backtrace);
        }
        process::exit(1);
    }
}
