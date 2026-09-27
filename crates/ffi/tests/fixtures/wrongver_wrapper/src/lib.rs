//! Test fixture: a wrapper built against a different ABI version.
//!
//! The handshake reports 2 while the framework expects 1; the loader
//! must reject it and report both numbers (FR-A-04).

/// ABI version reported by this fixture; deliberately NOT the version
/// the current framework expects.
#[no_mangle]
pub extern "C" fn CCaller_abi_version() -> i64 {
    2
}

/// Adds two i64 parameters; any other count is a wrapper failure.
#[no_mangle]
pub extern "C" fn Call_add(_param_page: *mut u64, params: *const i64, param_len: i64) -> i64 {
    if param_len != 2 {
        return -1;
    }
    // SAFETY: param_len == 2 was checked above, so both reads stay
    // inside the caller-provided argument slice.
    unsafe { (*params.add(0)).wrapping_add(*params.add(1)) }
}
