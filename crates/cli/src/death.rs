//! Death-test isolation for the CLI: re-execute this binary (FR-T-05).
//!
//! The core executor knows what a death test must run; only the CLI
//! knows how to spawn another `ccaller` process (architecture rule
//! AR-01). This launcher re-executes the current binary with the
//! hidden `--isolate-test` / `--isolate-subcase` flags, which make the
//! child run exactly one sub-case as a normal test:
//!
//! - the child frames its own env stack (process, global, case,
//!   thread) exactly as the static analysis models it;
//! - a crashing wrapper kills the child, which is the pass condition;
//! - a completing child exits through the normal run exit codes, which
//!   the parent reads as "survived".
//!
//! The child's report goes to the inherited stderr so a death-test
//! failure can be debugged from the parent's console; stdout is
//! dropped to keep the parent's own report clean (FR-R-01).

use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use ccaller_core::death::{DeathLauncher, DeathTestRequest};

/// The production launcher: re-executes the ccaller binary.
#[derive(Debug, Default)]
pub struct Relaunch;

impl DeathLauncher for Relaunch {
    fn spawn(&self, request: &DeathTestRequest) -> io::Result<Child> {
        relaunch(&std::env::current_exe()?, request).spawn()
    }
}

/// Builds the re-execution command for one isolated death-test child.
///
/// Split from [`Relaunch`] so the argument protocol is testable without
/// spawning a process: the launcher only resolves the binary.
pub fn relaunch(exe: &Path, request: &DeathTestRequest) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("-t")
        .arg(&request.cases_path)
        .arg("-i")
        .arg(&request.lib_path)
        .arg("run")
        .arg("--isolate-test")
        .arg(&request.test)
        .arg("--isolate-subcase")
        .arg(&request.subcase)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relaunch_command_carries_the_isolation_protocol__F_T_05() {
        let request = DeathTestRequest {
            lib_path: PathBuf::from("/tmp/cases.toml"),
            cases_path: PathBuf::from("/tmp/libs.toml"),
            test: "t".to_string(),
            subcase: "t/g#0[x=1]".to_string(),
        };
        let command = relaunch(Path::new("/usr/bin/ccaller"), &request);
        assert_eq!(command.get_program(), "/usr/bin/ccaller");
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-t",
                "/tmp/libs.toml",
                "-i",
                "/tmp/cases.toml",
                "run",
                "--isolate-test",
                "t",
                "--isolate-subcase",
                "t/g#0[x=1]",
            ]
        );
    }
}
