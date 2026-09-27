//! The serial executor that walks a [`Plan`] and invokes wrappers.
//!
//! [`execute`] is the public entry point: it builds the plan, loads the
//! wrapper libraries through the ffi loader, then drives a [`Runner`]
//! through every sub-case in declaration order. Execution is deliberately
//! serial for this milestone - `thread_num`, `concurrences`, and
//! `max-threads` parallelism land later.
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

use crate::config::diag::Diagnostic;
use crate::config::{ResolvedCmd, SubCase};
use crate::error::CoreError;
use crate::plan::{self, Plan, PlanOutcome, TestPlan};
use crate::report::{FailedCase, Reporter, RunReport, RunSummary};
use crate::runtime::marshal_args;

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
    let mut runner = Runner::new(plan, loaded, reporter);
    runner.run().map_err(RunError::Report)
}

/// The explicit execution context (AR-03): everything one run needs.
pub struct Runner {
    plan: Plan,
    loaded: LoadedFunctions,
    page: ParamPage,
    reporter: Box<dyn Reporter>,
}

impl Runner {
    /// Constructs a runner from an assembled plan, loaded libraries, and a
    /// reporter. The `param_page` starts zeroed (FR-A-06).
    pub fn new(plan: Plan, loaded: LoadedFunctions, reporter: Box<dyn Reporter>) -> Self {
        Self {
            plan,
            loaded,
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

        {
            let plan = &self.plan;
            let loaded = &self.loaded;
            let page = &mut self.page;
            for test in &plan.tests {
                for subcase in &test.subcases {
                    let result = run_subcase(plan, loaded, page, test, subcase);
                    summary.total += 1;
                    summary.skipped += result.skipped;
                    if result.failures.is_empty() {
                        summary.success += 1;
                    } else {
                        summary.failure += 1;
                        failures.push(FailedCase {
                            name: subcase.name.clone(),
                            reason: result.failures.join("; "),
                        });
                    }
                }
            }
        }

        let report = RunReport { summary, failures };
        self.reporter.report(&report)?;
        Ok(report)
    }
}

/// The outcome of one sub-case's command walk.
struct SubCaseRun {
    /// Cmds skipped by `CCALLER_ERR_SKIP`.
    skipped: usize,
    /// Human-readable reasons for every failure observed.
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

/// Runs one sub-case: env init, the test Cmds, then env exit.
///
/// Env layers apply in the order the specification defines - global,
/// process, case, thread - with exits reversed (FR-E-02, Q-13). An init
/// failure skips the test Cmds but still runs the exit layers, so cleanup
/// is attempted even after a broken setup (FR-E-03, FR-E-07).
fn run_subcase(
    plan: &Plan,
    loaded: &LoadedFunctions,
    page: &mut ParamPage,
    test: &TestPlan,
    subcase: &SubCase,
) -> SubCaseRun {
    let mut skipped = 0;
    let mut failures: Vec<String> = Vec::new();

    for cmd in init_sequence(plan, test) {
        match run_cmd(loaded, page, cmd) {
            CmdResult::Passed => {}
            CmdResult::Skipped => skipped += 1,
            CmdResult::Failed(reason) => {
                failures.push(format!("env init failed: {reason}"));
                break;
            }
        }
    }

    if failures.is_empty() {
        for cmd in &subcase.cmds {
            match run_cmd(loaded, page, cmd) {
                CmdResult::Passed => {}
                CmdResult::Skipped => skipped += 1,
                CmdResult::Failed(reason) => {
                    failures.push(reason);
                    // FR-T-01: a failure interrupts the remaining Cmds only
                    // when break_if_fail is true (the default).
                    if test.break_if_fail {
                        break;
                    }
                }
            }
        }
    }

    for cmd in exit_sequence(plan, test) {
        match run_cmd(loaded, page, cmd) {
            CmdResult::Passed => {}
            CmdResult::Skipped => skipped += 1,
            CmdResult::Failed(reason) => failures.push(format!("env exit failed: {reason}")),
        }
    }

    SubCaseRun { skipped, failures }
}

/// Env init commands in entry order: global, process, case, thread (Q-13).
fn init_sequence<'a>(plan: &'a Plan, test: &TestPlan) -> Vec<&'a ResolvedCmd> {
    let mut seq = Vec::new();
    if let Some(layer) = &plan.global_env {
        seq.extend(layer.init.iter());
    }
    if let Some(layer) = &plan.process_env {
        seq.extend(layer.init.iter());
    }
    if let Some(idx) = test.case_env {
        if let Some(env) = plan.case_envs.get(idx) {
            seq.extend(env.init.iter());
        }
    }
    if let Some(layer) = &plan.thread_env {
        seq.extend(layer.init.iter());
    }
    seq
}

/// Env exit commands in reverse entry order: thread, case, process, global.
fn exit_sequence<'a>(plan: &'a Plan, test: &TestPlan) -> Vec<&'a ResolvedCmd> {
    let mut seq = Vec::new();
    if let Some(layer) = &plan.thread_env {
        seq.extend(layer.exit.iter());
    }
    if let Some(idx) = test.case_env {
        if let Some(env) = plan.case_envs.get(idx) {
            seq.extend(env.exit.iter());
        }
    }
    if let Some(layer) = &plan.process_env {
        seq.extend(layer.exit.iter());
    }
    if let Some(layer) = &plan.global_env {
        seq.extend(layer.exit.iter());
    }
    seq
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
