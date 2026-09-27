//! Command-line front end for the cCaller test framework.
//!
//! M0 milestone skeleton: the binary exists, defines its exit-code
//! contract, and exits successfully. Subcommands (`run`, `check`,
//! `expand`, ...) and the logging bootstrap land within this crate in
//! milestone M1+.

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

mod exit_code;

fn main() {
    std::process::exit(exit_code::ExitCode::Success as i32);
}
