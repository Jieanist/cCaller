//! Core domain layer of the cCaller test framework.
//!
//! This crate owns the domain model (tests, commands, environments,
//! input groups), load-time validation, scheduling, the assertion
//! registry, and reporting abstractions. It must stay independent of
//! any CLI concern (argument parsing, exit codes, terminal colors) and
//! of any concrete FFI mechanism; see architecture rules AR-01/AR-02 in
//! the requirement specification.
