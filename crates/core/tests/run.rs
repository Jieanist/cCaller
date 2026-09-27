//! End-to-end execution tests: build a real wrapper fixture and run a
//! configuration through [`ccaller_core::execute`].

// Integration tests are separate crates, so the crate-level test
// exemptions in lib.rs do not reach here; style guide section 6.3 allows
// unwrap/expect/panic freely in test code, and the __F_xx_nn suffixes
// from verification plan section 9.1 are upper-case on purpose.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    non_snake_case
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use ccaller_core::{execute, ConsoleReporter, FailedCase, RunSummary};

/// Builds the fixture cdylib and returns its path.
fn build_fixture() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("run_wrapper")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .expect("cargo must be available to build the fixture");
    assert!(
        output.status.success(),
        "fixture failed to build:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = if cfg!(windows) {
        "run_wrapper.dll"
    } else if cfg!(target_os = "macos") {
        "librun_wrapper.dylib"
    } else {
        "librun_wrapper.so"
    };
    let path = manifest
        .parent()
        .expect("the manifest path always has a parent")
        .join("target")
        .join("debug")
        .join(file);
    assert!(
        path.exists(),
        "fixture artifact not found at {}",
        path.display()
    );
    path
}

/// Writes a config file into the given directory and returns its path.
fn write_config(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// A forward-slash rendering of `path` safe inside a TOML basic string.
fn toml_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

const LIBS_HEADER: &str = "version = 1\n\n[[libs]]\npath = \"{lib}\"\nfuncs = [\n\
     { name = \"Call_argc\",     paras = [\"a\", \"b\"] },\n\
     { name = \"Call_ok\",       paras = [] },\n\
     { name = \"Call_skip\",     paras = [] },\n\
     { name = \"Call_fail\",     paras = [] },\n\
     { name = \"Call_env_fail\", paras = [] },\n\
]\n";

fn run_cases(cases_text: &str) -> (RunSummary, Vec<FailedCase>) {
    let fixture = build_fixture();
    let dir = std::env::temp_dir().join(format!("ccaller-core-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let libs = LIBS_HEADER.replace("{lib}", &toml_path(&fixture));
    let lib_path = write_config(&dir, "libs.toml", &libs);
    let cases_path = write_config(&dir, "cases.toml", cases_text);

    let reporter = Box::new(ConsoleReporter::new(Vec::<u8>::new()));
    let report = execute(&lib_path, &cases_path, reporter).unwrap();
    (report.summary, report.failures)
}

#[test]
fn execute_runs_pass_fail_and_skip_end_to_end__F_T_01() {
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_pass\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_argc\", expect_eq = 2, args = [\"a=2\", \"b=40\"] },\n]\n\n\
                 [[tests]]\nname = \"t_fail\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0, args = [] },\n]\n\n\
                 [[tests]]\nname = \"t_skip\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_skip\", expect_eq = 0, args = [] },\n\
                 \x20 { opfunc = \"Call_ok\",   expect_eq = 0, args = [] },\n]\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(
        summary,
        RunSummary {
            total: 3,
            success: 2,
            failure: 1,
            skipped: 1,
        }
    );
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name, "t_fail");
    assert!(failures[0].reason.contains("-42"), "{}", failures[0].reason);
}

#[test]
fn env_init_failure_fails_the_subcase__F_E_03() {
    let cases = "version = 1\n\n\
                 [env]\ninit = [{ opfunc = \"Call_env_fail\", args = [] }]\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0, args = [] },\n]\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(summary.total, 1);
    assert_eq!(summary.failure, 1);
    assert_eq!(summary.success, 0);
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].reason.contains("env init failed"),
        "{}",
        failures[0].reason
    );
}

#[test]
fn env_exit_failure_fails_the_subcase__F_E_03() {
    let cases = "version = 1\n\n\
                 [env]\nexit = [{ opfunc = \"Call_env_fail\", args = [] }]\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0, args = [] },\n]\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(summary.total, 1);
    assert_eq!(summary.failure, 1);
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].reason.contains("env exit failed"),
        "{}",
        failures[0].reason
    );
}
