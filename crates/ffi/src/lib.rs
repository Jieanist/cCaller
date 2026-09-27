//! FFI layer of the cCaller test framework.
//!
//! This is the only crate allowed to contain `unsafe` (architecture
//! rule AR-04). It owns dynamic-library loading ([`loader`]), the
//! unified wrapper ABI contract (requirement spec 7.1; constants in
//! [`abi`] mirrored from `include/ccaller.h`), per-thread param_page
//! memory ([`page`]), and the call bridge that invokes a wrapper with
//! one Cmd's arguments ([`call`]).

// The `__F_xx_nn` test-name suffixes mandated by verification plan
// section 9.1 are intentionally upper-case; exempt test builds only.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        non_snake_case
    )
)]

pub mod abi;
pub mod call;
pub mod loader;
pub mod page;
