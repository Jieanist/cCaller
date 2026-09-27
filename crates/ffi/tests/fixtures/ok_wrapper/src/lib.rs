//! Test fixture: a well-formed wrapper for the loader tests.
//!
//! Exports the handshake symbol (returning the version this framework
//! expects) and two functions with the unified signature.

/// ABI version reported by this fixture; matches `CCALLER_ABI_VERSION`.
#[no_mangle]
pub extern "C" fn CCaller_abi_version() -> i64 {
    1
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

/// Exists so duplicate-detection tests can pair this library with
/// `dup_wrapper`, which exports the same symbol.
#[no_mangle]
pub extern "C" fn Call_dup(_param_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    0
}

/// Returns the length of the NUL-terminated string whose pointer is
/// the first parameter; any other count is a wrapper failure. Proves
/// that string pointers are valid for the duration of the call
/// (decision Q-07).
#[no_mangle]
pub extern "C" fn Call_strlen(_param_page: *mut u64, params: *const i64, param_len: i64) -> i64 {
    if param_len != 1 {
        return -1;
    }
    // SAFETY: the framework guarantees the string pointer in
    // params[0] is valid for the duration of this call (Q-07); the
    // scan stops at the NUL terminator inside that buffer.
    unsafe {
        let mut cursor = *params as *const u8;
        let mut length = 0i64;
        while *cursor != 0 {
            cursor = cursor.add(1);
            length += 1;
        }
        length
    }
}
