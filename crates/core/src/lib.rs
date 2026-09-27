//! Core domain layer of the cCaller test framework.
//!
//! This crate owns the domain model (tests, commands, environments,
//! input groups), load-time validation, scheduling, the assertion
//! registry, and reporting abstractions. It must stay independent of
//! any CLI concern (argument parsing, exit codes, terminal colors) and
//! of any concrete FFI mechanism; see architecture rules AR-01/AR-02 in
//! the requirement specification.

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

pub mod assertion;
pub mod config;
pub mod error;
pub mod plan;
pub mod report;
pub mod run;
pub mod runtime;

pub use error::{CoreError, Location};
pub use plan::{build_plan, Plan, PlanOutcome};
pub use report::{ConsoleReporter, FailedCase, Reporter, RunReport, RunSummary};
pub use run::{execute, RunError, Runner};
