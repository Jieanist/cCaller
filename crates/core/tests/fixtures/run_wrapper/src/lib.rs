//! Test fixture: a wrapper exporting the handshake plus a few functions
//! that let the core run integration test exercise pass, fail, skip, and
//! env-failure outcomes.
//!
//! Deliberately free of raw-pointer dereferencing: that stays exclusive
//! to `crates/ffi` (architecture rule AR-04), and the run test only needs
//! return-code and arity plumbing, not memory access. The lifecycle and
//! ordering tests observe call order through a safe atomic counter, so
//! they also need no page access.

use std::sync::atomic::{AtomicUsize, Ordering};

/// ABI version reported by this fixture; matches `CCALLER_ABI_VERSION`.
#[no_mangle]
pub extern "C" fn CCaller_abi_version() -> i64 {
    1
}

/// Call counter for the env lifecycle/order tests; reset by `Call_reset`.
static TICKS: AtomicUsize = AtomicUsize::new(0);

/// Resets the lifecycle counter and returns 0.
#[no_mangle]
pub extern "C" fn Call_reset(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    TICKS.store(0, Ordering::SeqCst);
    0
}

/// Returns the current tick count and then increments it.
///
/// Combined with `Call_reset`, this lets a test assert the exact order in
/// which env phases and test commands ran, without touching `param_page`.
#[no_mangle]
pub extern "C" fn Call_tick(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    TICKS.fetch_add(1, Ordering::SeqCst) as i64
}

/// Returns the number of arguments the framework passed (`param_len`),
/// proving that marshalled arguments reach the wrapper.
#[no_mangle]
pub extern "C" fn Call_argc(_page: *mut u64, _params: *const i64, param_len: i64) -> i64 {
    param_len
}

/// Always returns success.
#[no_mangle]
pub extern "C" fn Call_ok(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    0
}

/// Always skips (Q-01): returns `CCALLER_ERR_SKIP` = -255.
#[no_mangle]
pub extern "C" fn Call_skip(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    -255
}

/// Always fails with a wrapper-custom code (-42).
#[no_mangle]
pub extern "C" fn Call_fail(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    -42
}

/// Fails with -1; used to exercise env init/exit failure visibility.
#[no_mangle]
pub extern "C" fn Call_env_fail(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    -1
}

/// Crashes the process: `std::process::abort` raises SIGABRT without a
/// single line of unsafe code. The death-test child that runs this is
/// killed by the signal, which is exactly what `should_panic` asserts.
#[no_mangle]
pub extern "C" fn Call_abort(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    std::process::abort()
}

/// Sleeps for a long time and then returns 0; a death-test child
/// running this neither crashes nor completes in any sane budget, so
/// the executor's timeout (and its kill) is what the parent observes.
#[no_mangle]
pub extern "C" fn Call_hang(_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    std::thread::sleep(std::time::Duration::from_secs(30));
    0
}

/// Tracks how many wrapper calls are in flight and the high-water mark
/// of concurrent calls; `Call_concurrency_reset` clears both counters.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static MAX_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Resets the concurrency counters and returns 0.
#[no_mangle]
pub extern "C" fn Call_concurrency_reset(
    _page: *mut u64,
    _params: *const i64,
    _param_len: i64,
) -> i64 {
    IN_FLIGHT.store(0, Ordering::SeqCst);
    MAX_IN_FLIGHT.store(0, Ordering::SeqCst);
    0
}

/// Marks one call in flight, sleeps briefly so concurrent calls
/// overlap, and returns the high-water mark observed so far.
///
/// With real threads the maximum grows with the number of overlapping
/// calls, which is how the parallel scheduler is verified to actually
/// run executions concurrently (FR-T-02/T-03) and to respect the `-m`
/// cap (FR-T-04).
#[no_mangle]
pub extern "C" fn Call_concurrency(
    _page: *mut u64,
    _params: *const i64,
    _param_len: i64,
) -> i64 {
    let now = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
    // Publish the new high-water mark; a racing call may publish a
    // smaller value afterwards, so use a compare-and-swap loop.
    let mut seen = MAX_IN_FLIGHT.load(Ordering::SeqCst);
    while now > seen {
        match MAX_IN_FLIGHT.compare_exchange(
            seen,
            now,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => break,
            Err(actual) => seen = actual,
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    MAX_IN_FLIGHT.load(Ordering::SeqCst) as i64
}
