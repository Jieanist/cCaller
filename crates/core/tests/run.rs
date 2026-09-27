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
use std::sync::Mutex;

use ccaller_core::{execute, ConsoleReporter, FailedCase, RunOptions, RunSummary};

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
     { name = \"Call_reset\",    paras = [] },\n\
     { name = \"Call_tick\",     paras = [] },\n\
]\n";

fn run_cases(cases_text: &str) -> (RunSummary, Vec<FailedCase>) {
    let fixture = build_fixture();
    let dir = std::env::temp_dir().join(format!("ccaller-core-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let libs = LIBS_HEADER.replace("{lib}", &toml_path(&fixture));
    let lib_path = write_config(&dir, "libs.toml", &libs);
    let cases_path = write_config(&dir, "cases.toml", cases_text);

    let reporter = Box::new(ConsoleReporter::new(Vec::<u8>::new()));
    let report = execute(&lib_path, &cases_path, RunOptions::default(), reporter).unwrap();
    (report.summary, report.failures)
}

/// Serializes the tests that observe call order through the fixture's
/// shared tick counter, so parallel test threads do not interleave their
/// `Call_tick` sequences across `execute` runs.
static TICK_LOCK: Mutex<()> = Mutex::new(());

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

#[test]
fn global_env_runs_once_per_test_not_per_subcase__F_E_02() {
    // FR-E-02: the global env init/exit frame the whole test, not each
    // sub-case. The tick values only line up if init runs once before the
    // two sub-cases and exit once after; the previous per-sub-case walk
    // would make the exit observe tick 2 instead of 3.
    let _guard = TICK_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [env]\ninit = [\n\
                 \x20 { opfunc = \"Call_reset\", expect_eq = 0 },\n\
                 \x20 { opfunc = \"Call_tick\",  expect_eq = 0 },\n]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 3 }]\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_tick\", expect_eq = \"$a\" },\n]\n\
                 [[tests.inputs]]\nname = \"g\"\nargs = { a = [1, 2] }\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(
        summary,
        RunSummary {
            total: 2,
            success: 2,
            failure: 0,
            skipped: 0,
        }
    );
    assert!(failures.is_empty());
}

#[test]
fn env_layer_order_is_process_global_case_thread__Q_13() {
    // Q-13: entry order process, global, case, thread with exits reversed.
    // Each env phase and the test body consumes the tick counter in that
    // exact order, so any reordering fails an expectation. This is the
    // runtime twin of the def-use order test.
    let _guard = TICK_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [process_env]\ninit = [\n\
                 \x20 { opfunc = \"Call_reset\", expect_eq = 0 },\n\
                 \x20 { opfunc = \"Call_tick\",  expect_eq = 0 },\n]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 8 }]\n\n\
                 [env]\ninit = [{ opfunc = \"Call_tick\", expect_eq = 1 }]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 7 }]\n\n\
                 [thread_env]\ninit = [{ opfunc = \"Call_tick\", expect_eq = 3 }]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 5 }]\n\n\
                 [[envs]]\nname = \"e\"\n\
                 init = [{ opfunc = \"Call_tick\", expect_eq = 2 }]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 6 }]\n\
                 tests = [\"t\"]\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_tick\", expect_eq = 4 },\n]\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(
        summary,
        RunSummary {
            total: 1,
            success: 1,
            failure: 0,
            skipped: 0,
        }
    );
    assert!(failures.is_empty());
}

#[test]
fn should_panic_is_skipped_not_failed__F_T_05() {
    // FR-T-05: until isolation lands a death test is skipped, counted in
    // the summary, and never a failure. The command would fail if it ran,
    // so a clean summary proves the body was not executed.
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_env_fail\", expect_eq = 0, args = [] },\n]\n";
    let (summary, failures) = run_cases(cases);
    assert_eq!(
        summary,
        RunSummary {
            total: 1,
            success: 0,
            failure: 0,
            skipped: 1,
        }
    );
    assert!(failures.is_empty());
}
