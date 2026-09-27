//! Command-line front end for the cCaller test framework.
//!
//! M1 surface: argument parsing (`--version`, `--help`, `-l/--log`,
//! `-t/--test`, `-i/--lib`) plus the `check` subcommand (FR-X-03) with
//! text and JSON output. `run` and the remaining subcommands land with
//! milestone M2+; until then a bare invocation prints the help text and
//! exits successfully.

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

use std::fmt;
use std::path::{Path, PathBuf};

use ccaller_core::config::{run_check, CheckReport};
use ccaller_core::error::CoreError;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

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
    /// Test case configuration (requirement spec 7.3).
    #[arg(short, long, global = true, alias = "test_case", value_name = "FILE")]
    test: Option<PathBuf>,
    /// Library description (requirement spec 7.2).
    #[arg(short = 'i', long, global = true, alias = "input", value_name = "FILE")]
    lib: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Sub>,
}

/// The subcommands of ccaller (FR-X-01); `run` lands in M2.
#[derive(Subcommand)]
enum Sub {
    /// Load-time validation and static slot analysis; nothing is executed.
    Check {
        /// Output format (requirement spec 7.6).
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

/// Output format of `check` (requirement spec 7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Human-readable findings (the default).
    Text,
    /// The machine-readable 7.6 JSON contract.
    Json,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Text => "text",
            Format::Json => "json",
        })
    }
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
    match cli.command {
        Some(Sub::Check { format }) => check(cli.test.as_deref(), cli.lib.as_deref(), format),
        None => {
            // The run subcommand lands in M2; until then a bare
            // invocation shows the help.
            if let Err(error) = Cli::command().print_help() {
                log::warn!("failed to print help: {error}");
            }
            std::process::ExitCode::SUCCESS
        }
    }
}

/// Runs the `check` subcommand (FR-X-03).
///
/// Exit codes follow FR-X-02: 0 when the configuration is clean, 2 for
/// load-time findings, missing options, and unreadable files.
fn check(test: Option<&Path>, lib: Option<&Path>, format: Format) -> std::process::ExitCode {
    let (Some(test), Some(lib)) = (test, lib) else {
        eprintln!("error: `check` requires both --test and --lib");
        return std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8);
    };
    match run_check(lib, test) {
        Ok(report) => {
            match format {
                Format::Text => print_check_text(&report),
                Format::Json => println!("{}", report.to_json()),
            }
            let code = if report.ok() {
                exit_code::ExitCode::Success
            } else {
                exit_code::ExitCode::ConfigError
            };
            std::process::ExitCode::from(code as u8)
        }
        // An unreadable file is an environment problem (exit 2), not a
        // config finding, so the report is bypassed entirely.
        Err(error @ CoreError::Io { .. }) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        // CoreError is non_exhaustive; run_check promises Io only, so
        // anything else is a framework bug (exit 3).
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(exit_code::ExitCode::InternalError as u8)
        }
    }
}

/// Prints the human-readable form of a check report to stdout.
///
/// Findings come first, one per line and prefixed with their source
/// location and code, then a single summary line. Results go to stdout,
/// logs to stderr (FR-R-01/02 separation).
fn print_check_text(report: &CheckReport) {
    for diagnostic in &report.errors {
        println!(
            "{}:{}:{}: error[{}]: {}",
            diagnostic.location.file,
            diagnostic.location.line,
            diagnostic.location.column,
            diagnostic.code,
            diagnostic.message
        );
    }
    if report.ok() {
        println!(
            "ok: {} tests, {} subcases, {} commands",
            report.stats.tests, report.stats.subcases, report.stats.cmds
        );
    } else {
        println!("error: {} finding(s)", report.errors.len());
    }
}
