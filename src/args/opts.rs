//! Command-line options which can be passed to `docker-compose`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::ops::{Deref, DerefMut};

use args::ToArgs;

/// An empty set of options, used for `docker-compose` subcommands for
/// which don't need any.
#[derive(Debug, Clone, Copy)]
#[allow(missing_copy_implementations)]
pub struct Empty;

impl ToArgs for Empty {
    fn to_args(&self) -> Vec<OsString> {
        vec![]
    }
}

/// Command-line flags with for `docker-compose up`.
#[derive(Debug, Default, Clone)]
#[allow(missing_copy_implementations)]
pub struct Up {
    /// Should we initialize each pod after we start it up?
    pub init: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    _nonexhaustive: (),
}

impl Up {
    /// Create new `Up` options.
    pub fn new(init: bool) -> Up {
        Up {
            init: init,
            _nonexhaustive: (),
        }
    }
}

impl ToArgs for Up {
    fn to_args(&self) -> Vec<OsString> {
        // For now, this one is hard-coded.  We always pass `-d` because we
        // need to detach from each pod to launch the next.  To avoid this,
        // we'd need to use multiple parallel threads and maybe some
        // intelligent output buffering.
        vec!["-d".into()]
    }
}

/// Command-line flags which can be passed to `docker-compose exec` and `run`.
#[derive(Debug, Clone)]
pub struct Process {
    /// Should we execute this command in the background?
    pub detached: bool,

    /// An optional user as whom we should run the command.
    ///
    /// TODO LOW: Is this technically "user[:group]"?  If so, we need
    /// support for that type in `compose_yml` and use it here.
    pub user: Option<String>,

    /// Should we allocate a TTY when executing the command?
    /// Defaults to true for `docker-compose`.
    pub allocate_tty: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl ToArgs for Process {
    fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if self.detached {
            args.push(OsStr::new("-d").to_owned());
        }
        if let Some(ref user) = self.user {
            args.push(OsStr::new("--user").to_owned());
            args.push(user.into());
        }
        if !self.allocate_tty {
            args.push(OsStr::new("-T").to_owned());
        }
        args
    }
}

#[test]
fn process_options_to_args_returns_empty_for_default_opts() {
    assert_eq!(Process::default().to_args(), Vec::<OsString>::new());
}

#[test]
fn process_options_to_args_returns_appropriate_flags() {
    let opts = Process {
        detached: true,
        user: Some("root".to_owned()),
        allocate_tty: false,
        ..Default::default()
    };
    let raw_expected = &["-d", "--user", "root", "-T"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

impl Default for Process {
    fn default() -> Process {
        Process {
            detached: false,
            user: None,
            allocate_tty: true, // Not false!
            _nonexhaustive: (),
        }
    }
}

/// Options for `docker_compose exec`.
#[derive(Debug, Clone, Default)]
pub struct Exec {
    /// Our "superclass", faked using `Deref`.
    pub process: Process,

    /// Should we run this command with elevated privileges?
    pub privileged: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl Deref for Exec {
    type Target = Process;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl DerefMut for Exec {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl ToArgs for Exec {
    fn to_args(&self) -> Vec<OsString> {
        let mut args = self.process.to_args();
        if self.privileged {
            args.push(OsStr::new("--privileged").to_owned());
        }
        args
    }
}

#[test]
fn exec_options_to_args_returns_appropriate_flags() {
    let mut opts = Exec::default();
    opts.detached = true;
    opts.privileged = true;
    let raw_expected = &["-d", "--privileged"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

/// Options for `docker_compose run`.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// Our "superclass", faked using `Deref`.
    pub process: Process,

    /// Extra environment variables to pass in.
    pub environment: BTreeMap<String, String>,

    /// Override the container's entrypoint.  Specify `Some("".to_owned())`
    /// to reset the entrypoint to the default.
    pub entrypoint: Option<String>,

    /// Don't start linked services when running specified service,
    /// default: false
    pub no_deps: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl Deref for Run {
    type Target = Process;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl DerefMut for Run {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl ToArgs for Run {
    fn to_args(&self) -> Vec<OsString> {
        let mut args = self.process.to_args();
        for (var, val) in &self.environment {
            args.push(OsStr::new("-e").to_owned());
            args.push(format!("{}={}", var, val).into());
        }
        if let Some(ref entrypoint) = self.entrypoint {
            args.push(OsStr::new("--entrypoint").to_owned());
            args.push(entrypoint.into());
        }
        if self.no_deps {
            args.push(OsStr::new("--no-deps").to_owned());
        }
        args
    }
}

#[test]
fn run_options_to_args_returns_appropriate_flags() {
    let mut opts = Run::default();
    opts.detached = true;
    opts.environment.insert("FOO".to_owned(), "foo".to_owned());
    opts.entrypoint = Some("/helper.sh".to_owned());
    let raw_expected = &["-d", "-e", "FOO=foo", "--entrypoint", "/helper.sh"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

/// Command-line flags for `docker-compose kill`.
#[derive(Debug, Clone, Default)]
#[allow(missing_copy_implementations)]
pub struct Kill {
    /// The signal to deliver, if specified.
    pub signal: Option<String>,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl ToArgs for Kill {
    fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if let Some(ref signal) = self.signal {
            args.push(OsStr::new("-s").to_owned());
            args.push(OsStr::new(signal).to_owned());
        }
        args
    }
}

#[test]
fn kill_options_to_args_returns_appropriate_flags() {
    let mut opts = Kill::default();
    opts.signal = Some("FOO".to_owned());
    let raw_expected = &["-s", "FOO"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

/// Command-line flags with for `docker-compose logs`.
#[derive(Debug, Clone, Default)]
#[allow(missing_copy_implementations)]
pub struct Logs {
    /// Whether to interactively display logs as they're output
    pub follow: bool,

    /// Number of lines from end of log output to display
    pub number: Option<String>,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl ToArgs for Logs {
    fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if self.follow {
            args.push(OsStr::new("-f").to_owned());
        }
        if let Some(ref number) = self.number {
            args.push(OsStr::new(&format!("--tail={}", number)).to_owned());
        }
        args
    }
}

#[test]
fn logs_options_to_args_returns_appropriate_flags() {
    let mut opts = Logs::default();
    opts.follow = true;
    opts.number = Some("12".to_owned());
    let raw_expected = &["-f", "--tail=12"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

/// Command-line flags for use with `docker-compose ps`.
#[derive(Debug, Clone, Default)]
#[allow(missing_copy_implementations)]
pub struct Ps {
    /// Only show ids.
    pub only_ids: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl ToArgs for Ps {
    fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if self.only_ids {
            args.push(OsStr::new("-q").to_owned());
        }
        args
    }
}

#[test]
fn ps_options_to_args_returns_appropriate_flags() {
    let mut opts = Ps::default();
    opts.only_ids = true;
    let raw_expected = &["-q"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}

/// Command-line flags for use with `docker-compose rm`.
#[derive(Debug, Clone, Default)]
#[allow(missing_copy_implementations)]
pub struct Rm {
    /// Delete containers without confirmation.
    pub force: bool,

    /// Delete any anonymous volumes attached to containers.
    pub remove_volumes: bool,

    /// PRIVATE: This field is a stand-in for future options.
    /// See http://stackoverflow.com/q/39277157/12089
    #[doc(hidden)]
    pub _nonexhaustive: (),
}

impl ToArgs for Rm {
    fn to_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![];
        if self.force {
            args.push(OsStr::new("-f").to_owned());
        }
        if self.remove_volumes {
            args.push(OsStr::new("-v").to_owned());
        }
        args
    }
}

#[test]
fn rm_options_to_args_returns_appropriate_flags() {
    let mut opts = Rm::default();
    opts.force = true;
    opts.remove_volumes = true;
    let raw_expected = &["-f", "-v"];
    let expected: Vec<OsString> = raw_expected
        .iter()
        .map(|s| OsStr::new(s).to_owned())
        .collect();
    assert_eq!(opts.to_args(), expected);
}
