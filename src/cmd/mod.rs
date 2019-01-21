//! Various commands which can be performed on a project, corresponding to
//! CLI entry points.
//!
//! To gain access to all commands at once:
//!
//! ```
//! use cage::cmd::*;
//! ```

// We're allowed to print things to the user in the `cmd` submodule.
#![cfg_attr(feature="clippy", allow(print_stdout))]

pub use self::compose::CommandCompose;
pub use self::exec::CommandExec;
pub use self::generate::CommandGenerate;
pub use self::kill::CommandKill;
pub use self::logs::CommandLogs;
pub use self::ps::CommandPs;
pub use self::pull::CommandPull;
pub use self::run::CommandRun;
pub use self::run_script::CommandRunScript;
pub use self::source::CommandSource;
pub use self::status::CommandStatus;
pub use self::up::CommandUp;

mod compose;
mod exec;
mod generate;
mod kill;
mod logs;
mod ps;
mod pull;
mod run;
mod run_script;
mod source;
mod status;
mod up;
