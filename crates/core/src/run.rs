//! The executor that walks a [`Plan`] and invokes wrappers.
//!
//! [`execute`] is the public entry point: it builds the plan, loads the
//! wrapper libraries through the ffi loader, then drives a [`Runner`]
//! through every sub-case in declaration order. Execution is serial for
//! this milestone — `thread_num`, `concurrences`, and `max-threads`
//! parallelism land later — so a worker is the calling thread itself.
//!
//! Env lifecycle (FR-E-02, decision Q-13): the process and global envs
//! frame the whole run, entered once before any test and exited once
//! after the last test; the case and thread envs frame one test, entered
//! once before that test's sub-cases and exited once after them. The
//! entry order is process, global, case, thread with exits reversed, and
//! the order is shared with the slot def-use analysis through
//! [`crate::config::layers::ENTRY_ORDER`] so the analyzer describes the
//! sequence the runtime actually performs. A case/global init failure
//! stops the remaining init and fails every sub-case that scope framed;
//! exit failures are attributed the same way (FR-E-03).
//!
//! The [`Runner`] is the explicit execution context (architecture rule
//! AR-03): it owns the loaded libraries, the per-thread `param_page`, the
//! plan, and the reporter. Nothing here touches global mutable state; the
//! assertion registry is a read-only table consulted through
//! [`crate::assertion`].

use std::path::Path;

use ccaller_ffi::abi::{classify_return_value, ReturnClass};
use ccaller_ffi::call;
use ccaller_ffi::loader::{LoadError, LoadedFunctions};
use ccaller_ffi::page::ParamPage;
use log::warn;

use crate::config::diag::Diagnostic;
use crate::config::layers::{EnvScope, ENTRY_ORDER};
use crate::config::ResolvedCmd;
use crate::error::CoreError;
use crate::plan::{self, Plan, PlanOutcome, TestPlan};
use crate::report::{FailedCase, Reporter, RunReport, RunSummary};
use crate::runtime::marshal_args;

/// Run-time options that shape one execution.
///
/// Today this only carries the serial override (FR-T-09); `max-threads`
/// (FR-T-04) joins it when parallel scheduling lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// Force every test's sub-cases to run serially (FR-T-09).
    ///
    /// A test's own `serial` wins over this flag, and this flag wins over
    /// the configuration's `default_serial` (FR-T-09/T-10).
    pub serial: bool,
}

/// Errors raised while preparing or running one execution.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RunError {
    /// A configuration file could not be read from disk.
    #[error("failed to read `{path}`: {source}")]
    Io {
        /// Path of the unreadable file.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// Load-time validation, expansion, or def-use found problems.
    #[error("the configuration has load-time findings; see the reported diagnostics")]
    Config(Vec<Diagnostic>),
    /// A wrapper library failed to load or resolve (FR-A-01/02/04).
    #[error("failed to load library: {0}")]
    Load(#[from] LoadError),
    /// A defensive path that should be unreachable for a validated plan.
    #[error("internal framework error: {0}")]
    Internal(String),
    /// The reporter failed to render the result.
    #[error("failed to write the run report: {0}")]
    Report(#[source] std::io::Error),
}

/// Executes one configuration end to end and renders the report.
///
/// # Errors
/// Returns [`RunError::Io`] for unreadable files, [`RunError::Config`]
/// for load-time findings (the same gate `check` enforces), and
/// [`RunError::Load`] for library-load failures. A run that executes and
/// finds failing cases still returns [`Ok`] - the failure signal lives in
/// the report's summary, not in this result.
pub fn execute(
    lib_path: &Path,
    cases_path: &Path,
    options: RunOptions,
    reporter: Box<dyn Reporter>,
) -> Result<RunReport, RunError> {
    let plan = match plan::build_plan(lib_path, cases_path) {
        Ok(PlanOutcome::Ready(plan)) => plan,
        Ok(PlanOutcome::Invalid(errors)) => return Err(RunError::Config(errors)),
        Err(CoreError::Io { path, source }) => return Err(RunError::Io { path, source }),
        // build_plan promises Io only; any other CoreError is a framework
        // defect, surfaced as an internal error instead of a panic.
        Err(other) => return Err(RunError::Internal(other.to_string())),
    };
    let loaded = LoadedFunctions::load(&plan.libraries)?;
    let mut runner = Runner::new(plan, loaded, options, reporter);
    runner.run().map_err(RunError::Report)
}

/// The explicit execution context (AR-03): everything one run needs.
pub struct Runner {
    plan: Plan,
    loaded: LoadedFunctions,
    options: RunOptions,
    page: ParamPage,
    reporter: Box<dyn Reporter>,
}

impl Runner {
    /// Constructs a runner from an assembled plan, loaded libraries, run
    /// options, and a reporter. The `param_page` starts zeroed (FR-A-06).
    pub fn new(
        plan: Plan,
        loaded: LoadedFunctions,
        options: RunOptions,
        reporter: Box<dyn Reporter>,
    ) -> Self {
        Self {
            plan,
            loaded,
            options,
            page: ParamPage::zeroed(),
            reporter,
        }
    }

    /// Runs every sub-case in declaration order and renders the report.
    ///
    /// # Errors
    /// Returns the reporter's I/O error when rendering fails.
    pub fn run(&mut self) -> std::io::Result<RunReport> {
        let mut summary = RunSummary::default();
        let mut failures = Vec::new();
        let serial = self.options.serial;

        {
            let plan = &self.plan;
            let loaded = &self.loaded;
            let page = &mut self.page;

            // Enter the run-level scopes (process, global) in entry order.
            // An init failure stops the remaining scopes' init; the exits
            // of the scopes that did enter still run below (FR-E-03).
            let mut run_init_failures: Vec<String> = Vec::new();
            let mut entered_run: Vec<EnvScope> = Vec::new();
            for &scope in run_scopes() {
                let phase = run_env_cmds(loaded, page, scope_init(plan, None, scope), "init", true);
                summary.skipped += phase.skipped;
                if phase.failures.is_empty() {
                    entered_run.push(scope);
                } else {
                    run_init_failures.extend(phase.failures);
                    break;
                }
            }

            // Run every test. When the run-level setup failed, no test can
            // run, so every sub-case inherits that failure (FR-E-03).
            let mut outcomes: Vec<SubCaseOutcome> = Vec::new();
            for test in &plan.tests {
                if run_init_failures.is_empty() {
                    run_test(
                        loaded,
                        page,
                        plan,
                        test,
                        serial,
                        &mut summary,
                        &mut outcomes,
                    );
                } else {
                    for subcase in &test.subcases {
                        outcomes.push(SubCaseOutcome {
                            name: subcase.name.clone(),
                            skipped: false,
                            failures: run_init_failures.clone(),
                        });
                    }
                }
            }

            // Leave the run-level scopes in reverse entry order.
            let mut run_exit_failures: Vec<String> = Vec::new();
            for scope in entered_run.iter().rev() {
                let phase =
                    run_env_cmds(loaded, page, scope_exit(plan, None, *scope), "exit", false);
                summary.skipped += phase.skipped;
                run_exit_failures.extend(phase.failures);
            }

            // Account every sub-case. A run-level exit failure frames the
            // whole run, so it is attributed even to a sub-case that was
            // skipped for lack of isolation (FR-E-03, NFR-02).
            for outcome in outcomes {
                summary.total += 1;
                let mut all = outcome.failures;
                all.extend(run_exit_failures.iter().cloned());
                if all.is_empty() {
                    if outcome.skipped {
                        summary.skipped += 1;
                    } else {
                        summary.success += 1;
                    }
                } else {
                    summary.failure += 1;
                    failures.push(FailedCase {
                        name: outcome.name,
                        reason: all.join("; "),
                    });
                }
            }
        }

        let report = RunReport { summary, failures };
        self.reporter.report(&report)?;
        Ok(report)
    }
}

/// The per-test outcome accumulated before run-level failures are folded
/// in by [`Runner::run`].
struct SubCaseOutcome {
    /// Display name of the sub-case (Q-05 format).
    name: String,
    /// The whole sub-case was skipped (a death test without isolation).
    skipped: bool,
    /// Failures from the test-level env and the sub-case's own Cmds.
    failures: Vec<String>,
}

/// The outcome of a single Cmd invocation.
enum CmdResult {
    /// The Cmd succeeded (return code OK and assertion held).
    Passed,
    /// The Cmd returned `CCALLER_ERR_SKIP` (Q-01); not a failure.
    Skipped,
    /// The Cmd failed; carries a human-readable reason.
    Failed(String),
}

/// Run-level env scopes in entry order: the first two of [`ENTRY_ORDER`].
fn run_scopes() -> &'static [EnvScope] {
    &ENTRY_ORDER[..2]
}

/// Test-level env scopes in entry order: the last two of [`ENTRY_ORDER`].
fn test_scopes() -> &'static [EnvScope] {
    &ENTRY_ORDER[2..]
}

/// Init commands of one env scope for `test`.
///
/// Run-level scopes (process, global) ignore `test`; the case scope reads
/// the test's env membership. A scope without a declared layer yields an
/// empty slice, so callers can iterate every scope uniformly.
fn scope_init<'a>(plan: &'a Plan, test: Option<&TestPlan>, scope: EnvScope) -> &'a [ResolvedCmd] {
    match scope {
        EnvScope::Process => plan
            .process_env
            .as_ref()
            .map(|layer| layer.init.as_slice())
            .unwrap_or(&[]),
        EnvScope::Global => plan
            .global_env
            .as_ref()
            .map(|layer| layer.init.as_slice())
            .unwrap_or(&[]),
        EnvScope::Case => test
            .and_then(|test| test.case_env)
            .and_then(|index| plan.case_envs.get(index))
            .map(|env| env.init.as_slice())
            .unwrap_or(&[]),
        EnvScope::Thread => plan
            .thread_env
            .as_ref()
            .map(|layer| layer.init.as_slice())
            .unwrap_or(&[]),
    }
}

/// Exit commands of one env scope for `test`; see [`scope_init`].
fn scope_exit<'a>(plan: &'a Plan, test: Option<&TestPlan>, scope: EnvScope) -> &'a [ResolvedCmd] {
    match scope {
        EnvScope::Process => plan
            .process_env
            .as_ref()
            .map(|layer| layer.exit.as_slice())
            .unwrap_or(&[]),
        EnvScope::Global => plan
            .global_env
            .as_ref()
            .map(|layer| layer.exit.as_slice())
            .unwrap_or(&[]),
        EnvScope::Case => test
            .and_then(|test| test.case_env)
            .and_then(|index| plan.case_envs.get(index))
            .map(|env| env.exit.as_slice())
            .unwrap_or(&[]),
        EnvScope::Thread => plan
            .thread_env
            .as_ref()
            .map(|layer| layer.exit.as_slice())
            .unwrap_or(&[]),
    }
}

/// Runs the commands of one env phase, returning failures and skip count.
///
/// `stop_on_failure` makes init stop at the first failed command (so a
/// broken setup does not keep configuring), while exit keeps going so
/// teardown is attempted even when one step fails (FR-E-03).
fn run_env_cmds(
    loaded: &LoadedFunctions,
    page: &mut ParamPage,
    cmds: &[ResolvedCmd],
    phase: &str,
    stop_on_failure: bool,
) -> EnvPhaseRun {
    let mut result = EnvPhaseRun {
        failures: Vec::new(),
        skipped: 0,
    };
    for cmd in cmds {
        match run_cmd(loaded, page, cmd) {
            CmdResult::Passed => {}
            CmdResult::Skipped => result.skipped += 1,
            CmdResult::Failed(reason) => {
                result
                    .failures
                    .push(format!("env {phase} failed: {reason}"));
                if stop_on_failure {
                    break;
                }
            }
        }
    }
    result
}

/// The failures and skip count of one env phase's command walk.
struct EnvPhaseRun {
    /// Human-readable reasons for every failed command.
    failures: Vec<String>,
    /// Cmds skipped by `CCALLER_ERR_SKIP` (Q-01).
    skipped: usize,
}

/// Runs one test: its case/thread envs, then each sub-case's Cmds.
///
/// A death test (`should_panic`) is skipped wholesale until the isolation
/// strategy lands (FR-T-05): its sub-cases count as skipped and none of
/// its Cmds or case/thread envs run. Otherwise the case env init runs
/// once before all of the test's sub-cases and its exit once after, and
/// the thread env does the same for the single serial worker (FR-E-02).
fn run_test(
    loaded: &LoadedFunctions,
    page: &mut ParamPage,
    plan: &Plan,
    test: &TestPlan,
    cli_serial: bool,
    summary: &mut RunSummary,
    outcomes: &mut Vec<SubCaseOutcome>,
) {
    if test.should_panic {
        // FR-T-05: a platform that cannot isolate must skip the death
        // test with a warning and must not judge it a failure. The
        // failure path below stays dormant until isolation lands.
        warn!(
            "skipping death test `{}`: no isolation strategy is available on this platform",
            test.name
        );
        for subcase in &test.subcases {
            outcomes.push(SubCaseOutcome {
                name: subcase.name.clone(),
                skipped: true,
                failures: Vec::new(),
            });
        }
        return;
    }

    let serial = plan::effective_serial(test.serial, cli_serial, plan.default_serial);
    log::debug!(
        "test `{}`: serial = {serial} (declared {:?}, cli {cli_serial}, default {})",
        test.name,
        test.serial,
        plan.default_serial
    );

    // Enter the test-level scopes (case, thread) in entry order.
    let mut init_failures: Vec<String> = Vec::new();
    let mut entered: Vec<EnvScope> = Vec::new();
    for &scope in test_scopes() {
        let phase = run_env_cmds(
            loaded,
            page,
            scope_init(plan, Some(test), scope),
            "init",
            true,
        );
        summary.skipped += phase.skipped;
        if phase.failures.is_empty() {
            entered.push(scope);
        } else {
            init_failures.extend(phase.failures);
            break;
        }
    }
    let init_ok = init_failures.is_empty();

    // Run each sub-case's own Cmds. An init failure means the sub-case's
    // setup never completed, so the sub-case inherits the failure instead
    // of running (FR-E-03).
    let mut test_outcomes: Vec<SubCaseOutcome> = Vec::with_capacity(test.subcases.len());
    for subcase in &test.subcases {
        let mut failures = init_failures.clone();
        if init_ok {
            for cmd in &subcase.cmds {
                match run_cmd(loaded, page, cmd) {
                    CmdResult::Passed => {}
                    CmdResult::Skipped => summary.skipped += 1,
                    CmdResult::Failed(reason) => {
                        failures.push(reason);
                        // FR-T-01: a failure interrupts the remaining
                        // Cmds only when break_if_fail is true.
                        if test.break_if_fail {
                            break;
                        }
                    }
                }
            }
        }
        test_outcomes.push(SubCaseOutcome {
            name: subcase.name.clone(),
            skipped: false,
            failures,
        });
    }

    // Leave the test-level scopes in reverse entry order; exit failures
    // belong to every sub-case the test framed.
    let mut exit_failures: Vec<String> = Vec::new();
    for scope in entered.iter().rev() {
        let phase = run_env_cmds(
            loaded,
            page,
            scope_exit(plan, Some(test), *scope),
            "exit",
            false,
        );
        summary.skipped += phase.skipped;
        exit_failures.extend(phase.failures);
    }
    for outcome in &mut test_outcomes {
        outcome.failures.extend(exit_failures.iter().cloned());
    }
    outcomes.extend(test_outcomes);
}

/// Marshals one Cmd's arguments, invokes it, and classifies the result.
fn run_cmd(loaded: &LoadedFunctions, page: &mut ParamPage, cmd: &ResolvedCmd) -> CmdResult {
    // Validation plus a successful load guarantee the function resolves;
    // reaching the missing branch is a defensive internal error, surfaced
    // as a visible failure instead of a panic.
    let Some(func) = loaded.get(&cmd.opfunc) else {
        return CmdResult::Failed(format!(
            "internal error: function `{}` was not loaded",
            cmd.opfunc
        ));
    };
    let args = match marshal_args(&cmd.args) {
        Ok(args) => args,
        Err(error) => return CmdResult::Failed(format!("argument error: {error}")),
    };
    let code = call::invoke(func.call, page, &args);
    let class = classify_return_value(code);
    match class {
        ReturnClass::Skip => CmdResult::Skipped,
        ReturnClass::ContractViolation(value) => CmdResult::Failed(format!(
            "contract violation: return value {value} is inside the framework-reserved range"
        )),
        _ => {
            if let Some(expect) = &cmd.expect {
                let outcome = expect.evaluate(code);
                if outcome.passed {
                    CmdResult::Passed
                } else {
                    CmdResult::Failed(format!("{} (actual {code})", outcome.expectation))
                }
            } else if class.is_failure() {
                CmdResult::Failed(format!("returned {code}"))
            } else {
                CmdResult::Passed
            }
        }
    }
}
