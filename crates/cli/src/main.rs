//! Command-line front end for the cCaller test framework.
//!
//! The surface is argument parsing (`--version`, `--help`, `-l/--log`,
//! `-t/--test`, `-i/--lib`, `-d/--debug`, `-m/--max-thread`) plus two
//! subcommands: `check` (FR-X-03) with text and JSON output, and `run`
//! (FR-X-01, the default) which executes a configuration end to end
//! with text, JSON, or JUnit output.

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

mod death;
mod exit_code;
mod logging;

use std::fmt;
use std::path::{Path, PathBuf};

use ccaller_core::config::{run_check, CheckReport};
use ccaller_core::death::DeathIsolation;
use ccaller_core::error::CoreError;
use ccaller_core::{
    ConsoleReporter, IsolationTarget, JsonReporter, JunitReporter, RunError, RunOptions,
};
use clap::{Parser, Subcommand, ValueEnum};

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
    /// Run only the named test (FR-T-08); the configuration's `debug_test` wins.
    #[arg(short, long, global = true, value_name = "NAME")]
    debug: Option<String>,
    /// Maximum concurrent worker threads (FR-T-04); at least 1.
    #[arg(
        short = 'm',
        long,
        global = true,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    max_thread: Option<u64>,

    #[command(subcommand)]
    command: Option<Sub>,
}

/// The subcommands of ccaller (FR-X-01); `run` is the default.
#[derive(Subcommand)]
enum Sub {
    /// Execute the configuration end to end (the default subcommand).
    Run {
        /// Allow a run that executes zero cases to exit 0 (decision Q-08).
        #[arg(long)]
        allow_empty: bool,
        /// Force every test's sub-cases to run serially (FR-T-09).
        #[arg(long)]
        serial: bool,
        /// Output format of the run report.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Internal: the test a death-test child executes (FR-T-05).
        #[arg(long, hide = true)]
        isolate_test: Option<String>,
        /// Internal: the sub-case a death-test child executes (FR-T-05).
        #[arg(long, hide = true)]
        isolate_subcase: Option<String>,
    },
    /// Load-time validation and static slot analysis; nothing is executed.
    Check {
        /// Output format (requirement spec 7.6).
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

/// Output format of `check` and `run` (requirement spec 7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Human-readable findings (the default).
    Text,
    /// The machine-readable 7.6 JSON contract.
    Json,
    /// The JUnit XML schema CI systems ingest.
    Junit,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Text => "text",
            Format::Json => "json",
            Format::Junit => "junit",
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
    match cli.command.unwrap_or(Sub::Run {
        allow_empty: false,
        serial: false,
        format: Format::Text,
        isolate_test: None,
        isolate_subcase: None,
    }) {
        Sub::Run {
            allow_empty,
            serial,
            format,
            isolate_test,
            isolate_subcase,
        } => run(RunCli {
            test: cli.test,
            lib: cli.lib,
            debug: cli.debug,
            max_thread: cli.max_thread,
            allow_empty,
            serial,
            format,
            isolate_test,
            isolate_subcase,
        }),
        Sub::Check { format } => check(cli.test.as_deref(), cli.lib.as_deref(), format),
    }
}

/// The `run` subcommand's full invocation, gathered from the global and
/// subcommand options.
struct RunCli {
    test: Option<PathBuf>,
    lib: Option<PathBuf>,
    debug: Option<String>,
    max_thread: Option<u64>,
    allow_empty: bool,
    serial: bool,
    format: Format,
    isolate_test: Option<String>,
    isolate_subcase: Option<String>,
}

/// Runs the `run` subcommand (FR-X-01, the default).
///
/// Load-time findings reuse the `check` gate (exit 2); library-load
/// failures, unreadable files, and an unknown `-d` name are environment
/// or user errors (exit 2); executed cases map to 0 when nothing failed
/// and 1 otherwise. A run that executes zero cases exits 1 unless
/// `--allow-empty` is given (Q-08). `--serial` (FR-T-09) and `-m`
/// (FR-T-04) are forwarded to the executor. A death-test child (the
/// hidden `--isolate-*` flags, FR-T-05) runs exactly one sub-case and
/// reports to stderr so the parent's stdout stays clean.
fn run(invocation: RunCli) -> std::process::ExitCode {
    let (Some(test), Some(lib)) = (invocation.test.as_deref(), invocation.lib.as_deref()) else {
        eprintln!("error: `run` requires both --test and --lib");
        return std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8);
    };

    // The hidden isolation flags are a pair: one without the other is a
    // broken launcher, not something a user should ever type.
    let isolate = match (invocation.isolate_test, invocation.isolate_subcase) {
        (None, None) => None,
        (Some(test), Some(subcase)) => Some(IsolationTarget { test, subcase }),
        _ => {
            eprintln!("error: --isolate-test and --isolate-subcase must be given together");
            return std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8);
        }
    };
    let child_mode = isolate.is_some();

    let options = RunOptions {
        serial: invocation.serial,
        max_threads: invocation
            .max_thread
            .and_then(|threads| usize::try_from(threads).ok()),
        debug: invocation.debug,
        isolate,
        // Death-test children never spawn grandchildren; every other
        // run isolates through the re-execution launcher (FR-T-05).
        death: if child_mode {
            DeathIsolation::Skip
        } else {
            DeathIsolation::Child(Box::new(death::Relaunch))
        },
        ..RunOptions::default()
    };

    // A death-test child reports to stderr: its report is debugging
    // context for the parent, and the parent owns stdout (FR-R-01).
    let reporter: Box<dyn ccaller_core::Reporter> = match invocation.format {
        Format::Text if child_mode => Box::new(ConsoleReporter::new(std::io::stderr())),
        Format::Text => Box::new(ConsoleReporter::new(std::io::stdout())),
        Format::Json => Box::new(JsonReporter::new(std::io::stdout())),
        Format::Junit => Box::new(JunitReporter::new(std::io::stdout())),
    };

    match ccaller_core::execute(lib, test, options, reporter) {
        Ok(report) => {
            let code = if report.summary.total == 0 {
                if invocation.allow_empty {
                    exit_code::ExitCode::Success
                } else {
                    exit_code::ExitCode::TestFailed
                }
            } else if report.summary.failure == 0 {
                exit_code::ExitCode::Success
            } else {
                exit_code::ExitCode::TestFailed
            };
            std::process::ExitCode::from(code as u8)
        }
        Err(RunError::Io { path, source }) => {
            eprintln!("error: failed to read `{path}`: {source}");
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        Err(RunError::Config(diagnostics)) => {
            print_config_findings(&diagnostics);
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        Err(RunError::TestNotFound(name)) => {
            eprintln!("error: no test or sub-case named `{name}` exists");
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        Err(RunError::Load(error)) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        Err(RunError::Internal(message)) => {
            eprintln!("error: internal framework error: {message}");
            std::process::ExitCode::from(exit_code::ExitCode::InternalError as u8)
        }
        Err(RunError::Report(error)) => {
            eprintln!("error: failed to write the run report: {error}");
            std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8)
        }
        // RunError is non_exhaustive: an unknown variant is a framework
        // defect, mapped to the internal-error exit code.
        Err(other) => {
            eprintln!("error: internal framework error: {other}");
            std::process::ExitCode::from(exit_code::ExitCode::InternalError as u8)
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
                // check validates a configuration; it executes nothing,
                // so there is no case data to fill a JUnit document with.
                Format::Junit => {
                    eprintln!("error: `check` supports --format text|json");
                    return std::process::ExitCode::from(exit_code::ExitCode::ConfigError as u8);
                }
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

/// Prints load-time findings to stderr (run has no result to report, so
/// the diagnostics are errors, not check's stdout findings).
fn print_config_findings(diagnostics: &[ccaller_core::config::Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "{}:{}:{}: error[{}]: {}",
            diagnostic.location.file,
            diagnostic.location.line,
            diagnostic.location.column,
            diagnostic.code,
            diagnostic.message
        );
    }
}
