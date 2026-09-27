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
