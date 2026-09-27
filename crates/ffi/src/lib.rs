//! FFI layer of the cCaller test framework.
//!
//! This is the only crate allowed to contain `unsafe` (architecture
//! rule AR-04). It owns dynamic-library loading, the unified wrapper
//! ABI contract (requirement specification section 7.1), and per-thread
//! param_page management. The concrete loader lands with milestone M2;
//! until then this crate pins the architectural boundary only.
