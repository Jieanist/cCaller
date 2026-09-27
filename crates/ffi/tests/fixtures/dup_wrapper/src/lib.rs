//! Test fixture: a second library exporting the same `Call_dup`
//! symbol as `ok_wrapper`, for the cross-library duplicate check
//! (FR-A-01).

/// ABI version reported by this fixture; matches the framework.
#[no_mangle]
pub extern "C" fn CCaller_abi_version() -> i64 {
    1
}

/// The symbol that collides with `ok_wrapper`'s export.
#[no_mangle]
pub extern "C" fn Call_dup(_param_page: *mut u64, _params: *const i64, _param_len: i64) -> i64 {
    0
}
