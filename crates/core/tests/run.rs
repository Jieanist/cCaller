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

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use ccaller_core::{
    execute, CaseStatus, ConsoleReporter, DeathLauncher, DeathTestRequest, IsolationTarget,
    RunOptions, RunReport, RunSummary,
};

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
      { name = \"Call_abort\",    paras = [] },\n\
      { name = \"Call_hang\",     paras = [] },\n\
      { name = \"Call_concurrency_reset\", paras = [] },\n\
      { name = \"Call_concurrency\",        paras = [] },\n\
 ]\n";

/// The environment variables the death-test child harness reads.
const HARNESS_LIB: &str = "CCALLER_ITEST_LIB";
const HARNESS_CASES: &str = "CCALLER_ITEST_CASES";
const HARNESS_TEST: &str = "CCALLER_ITEST_TEST";
const HARNESS_SUBCASE: &str = "CCALLER_ITEST_SUBCASE";

/// A process-unique counter so parallel tests never share a config dir.
static DIR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Runs `cases_text` with `options` and returns the full report.
fn run_cases_with(cases_text: &str, options: RunOptions) -> RunReport {
    let fixture = build_fixture();
    let seq = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("ccaller-core-run-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let libs = LIBS_HEADER.replace("{lib}", &toml_path(&fixture));
    let lib_path = write_config(&dir, "libs.toml", &libs);
    let cases_path = write_config(&dir, "cases.toml", cases_text);

    let reporter = Box::new(ConsoleReporter::new(Vec::<u8>::new()));
    execute(&lib_path, &cases_path, options, reporter).expect("execute must succeed")
}

/// Runs `cases_text` with default options and returns the summary plus
/// the failed cases as (name, reason) pairs.
fn run_cases(cases_text: &str) -> (RunSummary, Vec<(String, String)>) {
    let report = run_cases_with(cases_text, RunOptions::default());
    let failures = report
        .failures()
        .into_iter()
        .map(|case| (case.name.clone(), case.reason.clone().unwrap_or_default()))
        .collect();
    (report.summary, failures)
}

/// Serializes the tests that observe call order through the fixture's
/// shared tick counter, so parallel test threads do not interleave their
/// `Call_tick` sequences across `execute` runs.
static TICK_LOCK: Mutex<()> = Mutex::new(());

/// Serializes the tests that use the fixture's concurrency counters,
/// which live in the once-loaded wrapper library.
static CONCURRENCY_LOCK: Mutex<()> = Mutex::new(());

/// Serializes death tests: their children re-execute this test binary,
/// and the harness entries below must not race the parent's assertions
/// on process-global state.
static DEATH_LOCK: Mutex<()> = Mutex::new(());

/// A death-test launcher that re-executes this very test binary with
/// libtest's `--exact` filtering, running only [`death_child_harness__internal`].
struct TestBinaryLauncher;

impl DeathLauncher for TestBinaryLauncher {
    fn spawn(&self, request: &DeathTestRequest) -> io::Result<std::process::Child> {
        Command::new(std::env::current_exe()?)
            .arg("death_child_harness__internal")
            .arg("--exact")
            .arg("--nocapture")
            .env(HARNESS_LIB, &request.lib_path)
            .env(HARNESS_CASES, &request.cases_path)
            .env(HARNESS_TEST, &request.test)
            .env(HARNESS_SUBCASE, &request.subcase)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

/// The in-child half of the death tests: when the harness environment
/// variables are set, run exactly the requested sub-case in child-isolation
/// mode; without them (the normal suite run) it is a no-op that passes.
///
/// The parent classifies only *how* this process terminated: a wrapper
/// crash kills the whole process, and any normal completion - pass or
/// fail - is the "survived" answer.
#[test]
fn death_child_harness__internal() {
    let (Ok(lib), Ok(cases), Ok(test), Ok(subcase)) = (
        std::env::var(HARNESS_LIB),
        std::env::var(HARNESS_CASES),
        std::env::var(HARNESS_TEST),
        std::env::var(HARNESS_SUBCASE),
    ) else {
        return;
    };
    let options = RunOptions {
        isolate: Some(IsolationTarget { test, subcase }),
        ..RunOptions::default()
    };
    let reporter = ConsoleReporter::new(io::stderr());
    match execute(
        Path::new(&lib),
        Path::new(&cases),
        options,
        Box::new(reporter),
    ) {
        Ok(report) => eprintln!("death child completed: {:?}", report.summary),
        Err(error) => eprintln!("death child failed to run: {error}"),
    }
}

/// Runs a death test through the child-isolation path with a short
/// timeout, mirroring what the CLI does with `Relaunch`.
fn run_death_cases(cases_text: &str) -> RunReport {
    run_cases_with(
        cases_text,
        RunOptions {
            death: ccaller_core::DeathIsolation::Child(Box::new(TestBinaryLauncher)),
            death_timeout: Some(Duration::from_millis(4000)),
            ..RunOptions::default()
        },
    )
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
    assert_eq!(failures[0].0, "t_fail");
    assert!(failures[0].1.contains("-42"), "{}", failures[0].1);
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
        failures[0].1.contains("env init failed"),
        "{}",
        failures[0].1
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
    assert_eq!(summary.success, 0);
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0].1.contains("env exit failed"),
        "{}",
        failures[0].1
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
fn should_panic_without_isolation_is_skipped_not_failed__F_T_05() {
    // FR-T-05: without a launcher a death test is skipped, counted in
    // the summary, and never a failure. The command would fail if it
    // ran, so a clean summary proves the body was not executed.
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

#[test]
fn thread_num_runs_replicas_on_real_worker_threads__F_T_02() {
    // FR-T-02: thread_num = 4 schedules 4 executions on 4 worker
    // threads. The fixture counts how many calls overlap: every call
    // must observe at least 2 in flight, which only happens with real
    // concurrency.
    let _guard = CONCURRENCY_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [process_env]\ninit = [{ opfunc = \"Call_concurrency_reset\", expect_eq = 0 }]\n\n\
                 [[tests]]\nname = \"t\"\nthread_num = 4\ncmds = [\n\
                 \x20 { opfunc = \"Call_concurrency\", expect_ge = 2 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(
        report.summary,
        RunSummary {
            total: 4,
            success: 4,
            failure: 0,
            skipped: 0,
        }
    );
    // Every replica is its own execution with its own @k display name.
    let names: Vec<&str> = report.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["t@0", "t@1", "t@2", "t@3"]);
}

#[test]
fn serial_forces_replicas_onto_one_thread__F_T_09() {
    // FR-T-09: a serial test runs every replica on the calling thread,
    // so no two calls ever overlap.
    let _guard = CONCURRENCY_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [process_env]\ninit = [{ opfunc = \"Call_concurrency_reset\", expect_eq = 0 }]\n\n\
                 [[tests]]\nname = \"t\"\nthread_num = 3\nserial = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_concurrency\", expect_eq = 1 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.total, 3);
    assert_eq!(report.summary.success, 3);
}

#[test]
fn max_thread_caps_the_worker_count__F_T_04() {
    // FR-T-04: thread_num = 4 with -m 2 runs on 2 workers, so at most 2
    // calls are ever in flight (each call observes exactly 2: the
    // first wave overlaps by construction of the fixture's sleep).
    let _guard = CONCURRENCY_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [process_env]\ninit = [{ opfunc = \"Call_concurrency_reset\", expect_eq = 0 }]\n\n\
                 [[tests]]\nname = \"t\"\nthread_num = 4\ncmds = [\n\
                 \x20 { opfunc = \"Call_concurrency\", expect_eq = 2 },\n]\n";
    let report = run_cases_with(
        cases,
        RunOptions {
            max_threads: Some(2),
            ..RunOptions::default()
        },
    );
    assert_eq!(report.summary.total, 4);
    assert_eq!(report.summary.success, 4);
}

#[test]
fn group_should_panic_override_partitions_one_test__F_T_05() {
    // FR-T-05 group override: one test, two input groups. The test is a
    // death test, but the second group opts out. Without a launcher the
    // death executions skip; the opted-out execution really RUNS - its
    // failing expectation is a failure, not a skip and not a crash
    // verdict.
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_env_fail\", expect_eq = 0 },\n]\n\
                 [[tests.inputs]]\nname = \"death\"\nargs = { a = [1] }\n\
                 [[tests.inputs]]\nname = \"normal\"\nshould_panic = false\nargs = { a = [2] }\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.total, 2);
    assert_eq!(report.summary.skipped, 1);
    assert_eq!(report.summary.failure, 1);
    let failed: Vec<&str> = report
        .failures()
        .into_iter()
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(failed, vec!["t/normal#0[a=2]"]);
    let skipped: Vec<&str> = report
        .cases
        .iter()
        .filter(|case| case.status == CaseStatus::Skipped)
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(skipped, vec!["t/death#0[a=1]"]);
}

#[test]
fn group_break_if_fail_override_stops_or_continues_per_subcase__F_T_01() {
    // FR-T-01 group override: the same failing command sequence, one
    // group stopping at the first failure and one running both. The
    // reasons differ only in how many failures they carry.
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\nbreak_if_fail = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n]\n\
                 [[tests.inputs]]\nname = \"stops\"\nargs = { a = [1] }\n\
                 [[tests.inputs]]\nname = \"continues\"\nbreak_if_fail = false\nargs = { a = [2] }\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.total, 2);
    assert_eq!(report.summary.failure, 2);
    let by_name: std::collections::BTreeMap<&str, &str> = report
        .cases
        .iter()
        .map(|case| {
            (
                case.name.as_str(),
                case.reason.as_deref().unwrap_or_default(),
            )
        })
        .collect();
    // The stopping group carries one failure; the continuing one both.
    let stops = by_name["t/stops#0[a=1]"];
    let continues = by_name["t/continues#0[a=2]"];
    assert_eq!(stops.matches("-42").count(), 1, "reason was: {stops}");
    assert_eq!(
        continues.matches("-42").count(),
        2,
        "reason was: {continues}"
    );
}

#[test]
fn concurrence_group_runs_members_in_parallel__F_T_03() {
    // FR-T-03: two group members run on separate threads; the fixture's
    // overlap counter proves they really ran concurrently.
    let _guard = CONCURRENCY_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [process_env]\ninit = [{ opfunc = \"Call_concurrency_reset\", expect_eq = 0 }]\n\n\
                 [[concurrences]]\nname = \"cg\"\ntests = [\"t_a\", \"t_b\"]\n\n\
                 [[tests]]\nname = \"t_a\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_concurrency\", expect_ge = 2 },\n]\n\n\
                 [[tests]]\nname = \"t_b\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_concurrency\", expect_ge = 2 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(
        report.summary,
        RunSummary {
            total: 2,
            success: 2,
            failure: 0,
            skipped: 0,
        }
    );
    let names: Vec<&str> = report.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["cg/t_a", "cg/t_b"]);
}

#[test]
fn group_members_do_not_run_standalone__F_T_03() {
    // FR-T-03: a grouped test runs only inside its group. If t_fail ran
    // standalone as well, its failure would be counted twice.
    let cases = "version = 1\n\n\
                 [[concurrences]]\nname = \"cg\"\ntests = [\"t_pass\", \"t_fail\"]\n\n\
                 [[tests]]\nname = \"t_pass\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n\n\
                 [[tests]]\nname = \"t_fail\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.total, 2);
    assert_eq!(report.summary.success, 1);
    assert_eq!(report.summary.failure, 1);
    let failed: Vec<&str> = report
        .failures()
        .into_iter()
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(failed, vec!["cg/t_fail"]);
}

#[test]
fn perf_flag_times_the_invocation_into_the_report__F_P_01() {
    // FR-P-01: commands with perf = true carry their wall-clock
    // duration into the report; commands without it do not.
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0, perf = true },\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.success, 1);
    assert_eq!(report.perf.len(), 1);
    assert_eq!(report.perf[0].opfunc, "Call_ok");
    assert_eq!(report.perf[0].context, "t cmd 0");
    assert!(report.perf[0].duration > Duration::ZERO);
}

#[test]
fn config_debug_test_selects_the_named_tests__F_T_08() {
    // FR-T-08: the configuration's debug_test list runs only those
    // tests; the other is neither executed nor counted.
    let cases = "version = 1\n\ndebug_test = [\"t_b\"]\n\n\
                 [[tests]]\nname = \"t_a\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n]\n\n\
                 [[tests]]\nname = \"t_b\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.success, 1);
    let names: Vec<&str> = report.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["t_b"]);
}

#[test]
fn config_debug_test_wins_over_the_cli_flag__F_T_08() {
    // FR-T-08: precedence is configuration debug_test, then CLI -d.
    let cases = "version = 1\n\ndebug_test = [\"t_b\"]\n\n\
                 [[tests]]\nname = \"t_a\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n\n\
                 [[tests]]\nname = \"t_b\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(
        cases,
        RunOptions {
            debug: Some("t_a".to_string()),
            ..RunOptions::default()
        },
    );
    let names: Vec<&str> = report.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["t_b"]);
}

#[test]
fn cli_debug_flag_selects_the_named_test__F_T_08() {
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_a\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n]\n\n\
                 [[tests]]\nname = \"t_b\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(
        cases,
        RunOptions {
            debug: Some("t_b".to_string()),
            ..RunOptions::default()
        },
    );
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.success, 1);
}

#[test]
fn unknown_cli_debug_name_is_a_selection_error__F_T_08() {
    // A typo'd -d name is a user mistake, rejected before anything is
    // loaded or executed (exit-2 class in the CLI mapping).
    let fixture = build_fixture();
    let seq = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("ccaller-core-run-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lib_path = write_config(
        &dir,
        "libs.toml",
        &LIBS_HEADER.replace("{lib}", &toml_path(&fixture)),
    );
    let cases_path = write_config(
        &dir,
        "cases.toml",
        "version = 1\n\n[[tests]]\nname = \"t\"\ncmds = [\n\x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n",
    );
    let options = RunOptions {
        debug: Some("ghost".to_string()),
        ..RunOptions::default()
    };
    match execute(
        &lib_path,
        &cases_path,
        options,
        Box::new(ConsoleReporter::new(Vec::<u8>::new())),
    ) {
        Err(ccaller_core::RunError::TestNotFound(name)) => assert_eq!(name, "ghost"),
        other => panic!("expected TestNotFound, got {other:?}"),
    }
}

#[test]
fn a_crashing_death_test_passes__F_T_05() {
    // FR-T-05: should_panic + a wrapper that aborts. The isolated child
    // dies from SIGABRT, which is exactly what the death test asserts.
    let _guard = DEATH_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_death\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_abort\", expect_eq = 0 },\n]\n";
    let report = run_death_cases(cases);
    assert_eq!(
        report.summary,
        RunSummary {
            total: 1,
            success: 1,
            failure: 0,
            skipped: 0,
        }
    );
}

#[test]
fn a_surviving_death_test_fails__F_T_05() {
    // FR-T-05: the wrapper returns normally, so the isolated child
    // exits normally - the death test expected a crash and failed.
    let _guard = DEATH_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_survivor\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0 },\n]\n";
    let report = run_death_cases(cases);
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.failure, 1);
    let failures = report.failures();
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("did not crash"),
        "reason was: {:?}",
        failures[0].reason
    );
}

#[test]
fn a_hanging_death_test_times_out_and_fails__F_T_05() {
    // FR-T-05: a hanging wrapper neither crashes nor completes; the
    // executor's budget kills the child and the death test fails with
    // the timeout reason.
    let _guard = DEATH_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_hang\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_hang\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(
        cases,
        RunOptions {
            death: ccaller_core::DeathIsolation::Child(Box::new(TestBinaryLauncher)),
            death_timeout: Some(Duration::from_millis(300)),
            ..RunOptions::default()
        },
    );
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.failure, 1);
    let failures = report.failures();
    assert!(
        failures[0].reason.as_deref().unwrap_or("").contains("hung"),
        "reason was: {:?}",
        failures[0].reason
    );
}

#[test]
fn death_test_replicas_each_get_their_own_child__F_T_05() {
    // FR-T-05 + FR-T-02: every replica of a death test is isolated on
    // its own; each child crashes, so every replica passes.
    let _guard = DEATH_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t_death\"\nshould_panic = true\nthread_num = 2\ncmds = [\n\
                 \x20 { opfunc = \"Call_abort\", expect_eq = 0 },\n]\n";
    let report = run_death_cases(cases);
    assert_eq!(report.summary.total, 2);
    assert_eq!(report.summary.success, 2);
    let names: Vec<&str> = report.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["t_death@0", "t_death@1"]);
}

#[test]
fn death_test_reports_skipped_status_in_the_case_list__F_T_05() {
    // The skipped death case appears as a case with status skipped,
    // which the JSON and JUnit reporters render.
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_env_fail\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(cases, RunOptions::default());
    assert_eq!(report.cases.len(), 1);
    assert_eq!(report.cases[0].status, CaseStatus::Skipped);
}

#[test]
fn child_isolation_runs_the_requested_subcase_in_process__F_T_05() {
    // The child half of the protocol: `isolate` selects exactly one
    // sub-case and neutralizes should_panic, so the cmds really run in
    // this process - the failing expectation is visible as a failure,
    // not as a skip.
    let _guard = DEATH_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_fail\", expect_eq = 0 },\n]\n";
    let report = run_cases_with(
        cases,
        RunOptions {
            isolate: Some(IsolationTarget {
                test: "t".to_string(),
                subcase: "t".to_string(),
            }),
            ..RunOptions::default()
        },
    );
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.failure, 1);
    assert!(report.failures()[0]
        .reason
        .as_deref()
        .unwrap_or("")
        .contains("-42"));
}

#[test]
fn run_level_env_frames_death_children_from_the_parent__F_T_05() {
    // A death test child runs its own env stack; the parent enters only
    // the run-level scopes (process, global). The global env here never
    // fails, so the crashed child's outcome is a clean pass and the
    // parent's global exit still runs once.
    let _guard = TICK_LOCK.lock().unwrap();
    let cases = "version = 1\n\n\
                 [env]\ninit = [\n\
                 \x20 { opfunc = \"Call_reset\", expect_eq = 0 },\n\
                 \x20 { opfunc = \"Call_tick\",  expect_eq = 0 },\n]\n\
                 exit = [{ opfunc = \"Call_tick\", expect_eq = 1 }]\n\n\
                 [[tests]]\nname = \"t_death\"\nshould_panic = true\ncmds = [\n\
                 \x20 { opfunc = \"Call_abort\", expect_eq = 0 },\n]\n";
    let report = run_death_cases(cases);
    assert_eq!(report.summary.success, 1);
    assert_eq!(report.summary.failure, 0);
}

/// Runs one end-to-end execution through the raw fixture for the JSON
/// reporter integration below.
fn report_of(cases_text: &str) -> RunReport {
    run_cases_with(cases_text, RunOptions::default())
}

#[test]
fn json_contract_end_to_end__F_R_06() {
    let report = report_of(
        "version = 1\n\n\
                 [[tests]]\nname = \"t\"\ncmds = [\n\
                 \x20 { opfunc = \"Call_ok\", expect_eq = 0, perf = true },\n]\n",
    );
    let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
    assert_eq!(json["schema"].as_u64(), Some(1));
    assert_eq!(json["ok"].as_bool(), Some(true));
    assert_eq!(json["summary"]["total"].as_u64(), Some(1));
    assert_eq!(json["cases"][0]["status"].as_str(), Some("passed"));
    assert!(json["perf"].as_array().is_some_and(|a| !a.is_empty()));
}
