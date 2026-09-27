//! Test fixture: a wrapper that forgot the handshake symbol.
//!
//! Exports `Call_add` only; the loader must reject it with a
//! missing-`CCaller_abi_version` error (FR-A-04).

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
