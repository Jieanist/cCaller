//! Per-command timeout watchdog (FR-X-03).
//!
//! Every `Call_<name>` invocation is bracketed by [`begin`] and the returned
//! guard's `Drop`, which registers the call in a global in-flight table with
//! its per-command budget. A detached watchdog thread polls that table; when
//! a call stays in flight past its budget it writes a loud diagnostic and
//! exits the process with code 1 (the `TestFailed` contract of FR-X-02), so
//! a stuck wrapper/driver cannot occupy a CI machine forever.
//!
//! The watchdog can only observe a hung *wrapper* call: cCaller's own code
//! never blocks on the device, so the FFI boundary is the discriminator. A
//! hang with no in-flight FFI entry is a framework defect (not the driver)
//! and is outside this watchdog's scope.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

/// Poll period of the watchdog thread.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One in-flight `Call_<name>` invocation awaiting return.
struct InFlight {
    /// Symbol name of the call, for the diagnostic.
    func: String,
    /// When the call was entered.
    started: Instant,
    /// Budget in seconds; the entry only exists when this is `> 0`.
    timeout_secs: u64,
}

/// The global registry of calls currently inside the wrapper. Keyed by the
/// worker thread that made the call: each worker runs one command at a
/// time, so a thread has at most one in-flight entry.
static IN_FLIGHT: LazyLock<Mutex<HashMap<ThreadId, InFlight>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers the current thread as inside `func` with the given budget and
/// returns a guard that deregisters it when the call returns. A budget of
/// `0` disables the watchdog for this call: the guard still drops, but
/// nothing was inserted.
pub fn begin(func: &str, timeout_secs: u64) -> InFlightGuard {
    if timeout_secs > 0 {
        let mut table = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        table.insert(
            thread::current().id(),
            InFlight {
                func: func.to_string(),
                started: Instant::now(),
                timeout_secs,
            },
        );
    }
    InFlightGuard
}

/// Removes the current thread's in-flight entry on drop (the call returned).
pub struct InFlightGuard;

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut table = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        table.remove(&thread::current().id());
    }
}

/// A call that has exceeded its budget, ready to be reported.
struct TimeoutInfo {
    func: String,
    timeout_secs: u64,
    elapsed: Duration,
}

/// Returns the first in-flight call that has exceeded its budget, if any.
fn expired_timeout() -> Option<TimeoutInfo> {
    let table = IN_FLIGHT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    table.values().find_map(|entry| {
        let elapsed = entry.started.elapsed();
        (elapsed >= Duration::from_secs(entry.timeout_secs)).then(|| TimeoutInfo {
            func: entry.func.clone(),
            timeout_secs: entry.timeout_secs,
            elapsed,
        })
    })
}

/// Reports a timeout to stderr and exits with code 1 (`TestFailed`, FR-X-02).
fn report_and_exit(info: &TimeoutInfo) -> ! {
    eprintln!(
        "\nccaller: TIMEOUT — command `{}` did not return within its {}s timeout \
         (in flight {:.1}s).\n\
         This is a wrapper/driver hang: the FFI call never returned, so the .so \
         (or the driver underneath it) is stuck — not cCaller.\n\
         Check `dmesg` for a driver Oops or deadlock. Exiting with code 1 \
         (test failed) to release the CI machine.\n",
        info.func,
        info.timeout_secs,
        info.elapsed.as_secs_f64(),
    );
    let _ = std::io::stderr().flush();
    std::process::exit(1);
}

/// The detached watchdog thread handle; dropping it signals the thread to
/// stop on its next poll, so a completed run does not leak a thread.
pub struct Watchdog {
    done: Arc<AtomicBool>,
}

impl Watchdog {
    /// Spawns the watchdog thread for the duration of a run.
    ///
    /// # Errors
    /// Returns the OS spawn failure (e.g. thread exhaustion); the caller
    /// surfaces it as a framework internal error.
    pub fn start() -> std::io::Result<Self> {
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        thread::Builder::new()
            .name("ccaller-timeout-watchdog".to_string())
            .spawn(move || loop {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(info) = expired_timeout() {
                    report_and_exit(&info);
                }
                thread::sleep(POLL_INTERVAL);
            })?;
        Ok(Watchdog { done })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_registers_and_guard_removes__F_X_03() {
        let guard = begin("Call_probe", 60);
        assert_eq!(in_flight_len(), 1);
        // A fresh call is within budget: nothing has expired.
        assert!(expired_timeout().is_none());
        drop(guard);
        assert_eq!(in_flight_len(), 0);
    }

    #[test]
    fn zero_budget_disables_registration__F_X_03() {
        let _guard = begin("Call_probe", 0);
        assert_eq!(in_flight_len(), 0);
    }

    fn in_flight_len() -> usize {
        IN_FLIGHT.lock().unwrap().len()
    }
}
