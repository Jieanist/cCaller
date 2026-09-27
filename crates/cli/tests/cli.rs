//! End-to-end checks of the ccaller binary surface (M0 skeleton).

// Integration tests are separate crates, so the crate-level test
// exemptions in main.rs do not reach here; style guide section 6.3
// allows unwrap/expect/panic freely in test code, and the __F_xx_nn
// suffixes from verification plan section 9.1 are upper-case on purpose.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    non_snake_case
)]

use std::process::Command;

fn ccaller() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ccaller"))
}

#[test]
fn version_flag_prints_binary_name_and_exits_zero__F_G_03() {
    let output = ccaller().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("ccaller"), "stdout was: {stdout}");
}

#[test]
fn unknown_flag_exits_with_config_error_code__F_G_03() {
    // Usage mistakes share exit code 2 with load-time config errors.
    let output = ccaller().arg("--definitely-not-a-flag").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn invalid_log_level_exits_with_config_error_code__F_R_02() {
    let output = ccaller().args(["--log", "verbose"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid log level"), "stderr was: {stderr}");
}

#[test]
fn help_flag_prints_help_and_exits_zero__F_X_01() {
    // `run` is now the default subcommand, so the way to ask for help is
    // an explicit --help, not a bare invocation (FR-X-01).
    let output = ccaller().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: ccaller"), "stdout was: {stdout}");
}

#[test]
fn bare_invocation_requires_test_and_lib__F_X_01() {
    // The default `run` subcommand needs both --test and --lib; a bare
    // invocation is therefore a usage error (exit 2), not help (FR-X-01).
    let output = ccaller().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--test") && stderr.contains("--lib"),
        "stderr was: {stderr}"
    );
}

#[test]
fn run_subcommand_exposes_help__F_X_01() {
    // The ABI conformance kit probes `ccaller run --help` to detect that
    // the subcommand exists, so this must succeed without side effects.
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "stdout was: {stdout}");
    assert!(stdout.contains("run"), "stdout was: {stdout}");
}

#[test]
fn run_help_lists_serial_flag__F_T_09() {
    // The `--serial` override (FR-T-09) is part of the run surface, so
    // the plumbing is observable through the generated help.
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--serial"), "stdout was: {stdout}");
}
