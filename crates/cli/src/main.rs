//! Command-line front end for the cCaller test framework.
//!
//! M0 milestone skeleton: argument parsing (`--version`, `--help`,
//! `-l/--log`) plus the logging bootstrap on stderr. The subcommands
//! (`run`, `check`, `expand`, ...) land with milestone M1+; until then a
//! bare invocation prints the help text and exits successfully.

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
mod logging;

use clap::{CommandFactory, Parser};

/// Generic C-interface test execution framework.
#[derive(Parser)]
#[command(
    name = "ccaller",
    version,
    about = "Generic C-interface test execution framework"
)]
struct Cli {
    /// Log level: 1-4 (error/warn/info/debug) or a level name; RUST_LOG takes precedence.
    #[arg(short, long, global = true, value_name = "LEVEL")]
    log: Option<String>,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let cli_level = match cli.log.as_deref().map(logging::parse_log_level) {
        Some(Ok(level)) => Some(level),
        Some(Err(message)) => {
            // Usage-class mistake: treat like a load-time config error (exit 2).
            eprintln!("error: {message}");
            return std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8);
        }
        None => None,
    };
    logging::init(cli_level);
    // M0 skeleton: no subcommands yet, so a bare run shows the help.
    if let Err(error) = Cli::command().print_help() {
        log::warn!("failed to print help: {error}");
    }
    std::process::ExitCode::SUCCESS
}
