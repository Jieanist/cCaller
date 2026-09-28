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

#[test]
fn help_lists_debug_and_max_thread_flags__F_T_08_F_T_04() {
    // The global -d/--debug (FR-T-08) and -m/--max-thread (FR-T-04)
    // options are part of the run surface and appear in the top-level
    // help.
    let output = ccaller().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--debug"), "stdout was: {stdout}");
    assert!(stdout.contains("--max-thread"), "stdout was: {stdout}");
    // The internal isolation flags are hidden from users (FR-T-05).
    assert!(!stdout.contains("--isolate-test"), "stdout was: {stdout}");
}

#[test]
fn max_thread_zero_is_a_usage_error__F_T_04() {
    // A zero cap would silently disable parallelism; clap's range
    // parser rejects it with the usage exit code.
    let output = ccaller().args(["--max-thread", "0"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value"), "stderr was: {stderr}");
}

#[test]
fn run_help_lists_the_format_flag__F_R_06() {
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--format"), "stdout was: {stdout}");
}

#[test]
fn unknown_debug_name_exits_two_before_any_execution__F_T_08() {
    // The selection is resolved before libraries load, so a typo'd -d
    // name is reported as the user mistake it is - the library file
    // never needs to exist, but its declaration must validate.
    let dir = std::env::temp_dir().join("ccaller-cli-debug");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = dir.join("cases.toml");
    let libs = dir.join("libs.toml");
    std::fs::write(
        &cases,
        "version = 1\n\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ok\", expect_eq = 0 }]\n",
    )
    .unwrap();
    std::fs::write(
        &libs,
        "version = 1\n\n[[libs]]\npath = \"missing.so\"\nfuncs = [{ name = \"Call_ok\", paras = [] }]\n",
    )
    .unwrap();
    let output = ccaller()
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&libs)
        .arg("-d")
        .arg("ghost")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ghost"), "stderr was: {stderr}");
}

#[test]
fn isolate_flags_come_in_pairs__F_T_05() {
    // The hidden isolation flags form one protocol; one without the
    // other is a usage error rather than a half-run. The config files
    // are never touched (the pair check fires first).
    let dummy = std::env::temp_dir().join("ccaller-cli-nothing.toml");
    let output = ccaller()
        .arg("-t")
        .arg(&dummy)
        .arg("-i")
        .arg(&dummy)
        .args(["run", "--isolate-test", "t"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("together"), "stderr was: {stderr}");
}

#[test]
fn relaunch_protocol_reaches_the_child_and_exits_normally__F_T_05() {
    // The launcher's re-execution protocol, exercised against the real
    // binary: the child of a death test runs exactly one sub-case and
    // exits through the normal run codes. A missing config file makes
    // the child exit 2 (an environment error) - a normal exit, which is
    // the "survived" answer the parent expects when nothing crashed.
    let dir = std::env::temp_dir().join("ccaller-cli-relaunch");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("missing.toml");
    let output = ccaller()
        .arg("-t")
        .arg(&missing)
        .arg("-i")
        .arg(&missing)
        .arg("run")
        .arg("--isolate-test")
        .arg("t")
        .arg("--isolate-subcase")
        .arg("t")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"), "stderr was: {stderr}");
}
