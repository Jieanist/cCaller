//! FFI layer of the cCaller test framework.
//!
//! This is the only crate allowed to contain `unsafe` (architecture
//! rule AR-04). It owns dynamic-library loading, the unified wrapper
//! ABI contract (requirement spec 7.1), and per-thread param_page
//! management. The contract constants facing C consumers live in
//! `include/ccaller.h`; the [`abi`] module mirrors them in Rust and a
//! unit test keeps both sides in sync (style guide 8.4). Loading and
//! the version handshake live in [`loader`].

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
pub mod loader;
