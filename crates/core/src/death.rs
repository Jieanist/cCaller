//! Death-test isolation (FR-T-05): run one case in a child process and
//! judge it by how the child terminated.
//!
//! A `should_panic` test asserts that the wrapper *crashes* - a segfault,
//! an abort, a stack overflow. That signal must not take the whole run
//! down, and a mere `Err` return must not be mistaken for a crash, so
//! the case runs in an isolated child process and only the child's
//! *termination kind* matters:
//!
//! - terminated by a signal (Unix) or a fatal NTSTATUS (Windows):
//!   the death test **passed**;
//! - exited normally, whatever its code: the death test **failed** -
//!   the wrapper survived when it was expected to crash.
//!
//! The executor knows *what* must run in isolation; *how* a child is
//! spawned belongs to the embedder, because only it knows its own
//! binary and command-line shape (architecture rule AR-01 keeps core
//! free of CLI knowledge). The CLI re-executes itself; a test harness
//! can re-execute the test binary. Without a launcher the executor
//! keeps the FR-T-05 skip behaviour: a death test without isolation is
//! skipped and never judged a failure.

use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

/// One death-test isolation request: everything a launcher needs to
/// re-run exactly one sub-case in a child process.
#[derive(Debug, Clone)]
pub struct DeathTestRequest {
    /// Library description file the child must load.
    pub lib_path: PathBuf,
    /// Case configuration file the child must load.
    pub cases_path: PathBuf,
    /// Name of the test owning the sub-case.
    pub test: String,
    /// Display name of the sub-case (Q-05) to execute.
    pub subcase: String,
}

/// Spawns one isolated child for a death-test execution (FR-T-05).
///
/// Implementations return the spawned [`Child`] with its streams
/// already configured; the executor polls, times out, and classifies.
/// The child must run the requested sub-case exactly once, as a normal
/// (non-death) test, and exit normally when it completes - see the CLI
/// child mode for the reference protocol.
pub trait DeathLauncher: Send + Sync {
    /// Spawns the child for `request`.
    ///
    /// # Errors
    /// Returns the spawn failure (e.g. the binary vanished); the
    /// executor turns it into a failed death-test execution.
    fn spawn(&self, request: &DeathTestRequest) -> io::Result<Child>;
}

/// How the executor isolates death tests (FR-T-05).
#[derive(Default)]
pub enum DeathIsolation {
    /// No launcher available: death tests are skipped with a warning
    /// and never judged a failure (the FR-T-05 fallback).
    #[default]
    Skip,
    /// Isolate every death-test execution through this launcher.
    Child(Box<dyn DeathLauncher>),
}

impl std::fmt::Debug for DeathIsolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeathIsolation::Skip => f.write_str("Skip"),
            DeathIsolation::Child(_) => f.write_str("Child(<launcher>)"),
        }
    }
}

/// Default wall-clock budget for one isolated child (decision: generous
/// enough for a slow wrapper under load, short enough that a hanging
/// death test does not hang the whole run).
pub const DEFAULT_DEATH_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the executor polls a running child.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How one isolated child terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeathOutcome {
    /// The child crashed: killed by a signal (Unix) or a fatal
    /// NTSTATUS (Windows). The death test passed.
    Crashed(String),
    /// The child exited normally; carries its exit code. The death
    /// test failed - the wrapper survived.
    Survived(Option<i32>),
    /// The child exceeded its time budget and was killed. The death
    /// test failed - hanging is not crashing.
    TimedOut(Duration),
}

/// Waits for `child` up to `timeout`, then classifies its termination.
///
/// A timed-out child is killed and reaped here, so the returned
/// outcome never leaves a grandchild behind. A child that exits in any
/// other way - including with an error code - is [`DeathOutcome::Survived`],
/// because only a crash satisfies a death test.
pub fn isolate_child(mut child: Child, timeout: Duration) -> Result<DeathOutcome, io::Error> {
    let started = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(classify_exit_status(status)),
            None => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    // Reap the killed child; its status is irrelevant
                    // because the timeout already decided the outcome.
                    let _ = child.wait();
                    return Ok(DeathOutcome::TimedOut(timeout));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// Classifies an exit status as crash or survival.
fn classify_exit_status(status: std::process::ExitStatus) -> DeathOutcome {
    // Unix: death by signal is the crash contract, and it is the only
    // way a child cannot report "I survived" itself.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return DeathOutcome::Crashed(format!("killed by signal {signal}"));
        }
    }
    // Windows: a crash surfaces as a fatal NTSTATUS exit code
    // (0xC0000005 access violation, 0xC0000409 fail-fast, ...). The
    // documented fatal range is 0xC0000000 and above; a normal exit -
    // including our own 0/1/2 protocol codes - is below it.
    #[cfg(windows)]
    {
        if let Some(code) = status.code() {
            if (u32::try_from(code).unwrap_or(0)) >= 0xC000_0000 {
                return DeathOutcome::Crashed(format!(
                    "terminated with fatal status 0x{:08X}",
                    u32::try_from(code).unwrap_or(0)
                ));
            }
        }
    }
    DeathOutcome::Survived(status.code())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Spawns `sh -c <script>` with silenced output.
    fn shell(script: &str) -> std::process::Child {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("sh is available on the test host")
    }

    #[test]
    fn a_running_command_that_exits_normally_survives__F_T_05() {
        let child = shell("exit 0");
        let outcome = isolate_child(child, Duration::from_secs(10)).expect("wait must succeed");
        assert_eq!(outcome, DeathOutcome::Survived(Some(0)));
    }

    #[test]
    fn a_nonzero_exit_code_is_still_survival__F_T_05() {
        // Exiting with an error is the child's own report, not a crash.
        let child = shell("exit 3");
        let outcome = isolate_child(child, Duration::from_secs(10)).expect("wait must succeed");
        assert_eq!(outcome, DeathOutcome::Survived(Some(3)));
    }

    #[test]
    fn a_signalled_child_crashes__F_T_05() {
        let child = shell("kill -SEGV $$");
        let outcome = isolate_child(child, Duration::from_secs(10)).expect("wait must succeed");
        match outcome {
            DeathOutcome::Crashed(detail) => assert!(detail.contains("signal"), "{detail}"),
            other => panic!("expected a crash, got {other:?}"),
        }
    }

    #[test]
    fn a_hanging_child_times_out_and_is_killed__F_T_05() {
        let child = shell("sleep 30");
        let outcome = isolate_child(child, Duration::from_millis(150)).expect("wait must succeed");
        assert_eq!(outcome, DeathOutcome::TimedOut(Duration::from_millis(150)));
    }
}
