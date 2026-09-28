//! The executor that walks a [`Plan`] and invokes wrappers.
//!
//! [`execute`] is the public entry point: it builds the plan, resolves
//! which tests the run selects (debug filter, FR-T-08), loads the
//! wrapper libraries through the ffi loader, then drives a [`Runner`]
//! through every scheduled execution.
//!
//! # Scheduling (M3)
//!
//! The unit of work is one **execution**: a sub-case replicated onto
//! one worker. A test with `thread_num = N` duplicates each of its
//! sub-cases `N` times (FR-T-02), and non-serial tests spread their
//! executions over `min(thread_num, executions)` worker threads,
//! capped by `-m/--max-thread` (FR-T-04). A `concurrences` group runs
//! its member tests in parallel, one thread per member (FR-T-03); a
//! member test never runs standalone. Worker threads own private
//! `param_page`s (decision Q-06): a page is never shared across
//! threads, so the executor itself stays lock-free.
//!
//! # Page state (Q-06)
//!
//! The run keeps one evolving **run page**: the process and global env
//! inits write it, serial tests run on it directly, and after a
//! parallel test the first worker's final page is absorbed back. Each
//! worker starts from a copy of the page it inherits, so every
//! execution observes the analyzer's per-sub-case prefix (process,
//! global, case, thread inits — Q-13) and no execution can observe
//! another thread's writes.
//!
//! Death tests (`should_panic`, FR-T-05) execute in isolated child
//! processes through the injected [`crate::death::DeathLauncher`]; the
//! child frames its own env stack, so the parent side does not enter
//! the test-level scopes for them. Without a launcher the death test
//! is skipped with a warning and never judged a failure.
//!
//! # Env lifecycle (FR-E-02, decision Q-13)
//!
//! The process and global envs frame the whole run; the case and
//! thread envs frame one worker's executions of a test (a serial test
//! has exactly one worker). A case/global init failure stops the
//! remaining init and fails every execution that scope framed; exit
//! failures are attributed the same way (FR-E-03).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ccaller_ffi::abi::{classify_return_value, ReturnClass};
use ccaller_ffi::call;
use ccaller_ffi::loader::{LoadError, LoadedFunctions};
use ccaller_ffi::page::ParamPage;
use log::{debug, info, warn};

use crate::config::diag::Diagnostic;
use crate::config::layers::{EnvScope, ENTRY_ORDER};
use crate::config::{ResolvedCmd, SubCase};
use crate::death::{
    isolate_child, DeathIsolation, DeathOutcome, DeathTestRequest, DEFAULT_DEATH_TIMEOUT,
};
use crate::error::CoreError;
use crate::plan::{self, ConcurrencyGroupPlan, Plan, PlanOutcome, TestPlan};
use crate::report::{CaseOutcome, CaseStatus, PerfSample, Reporter, RunReport, RunSummary};
use crate::runtime::marshal_args;

/// Run-time options that shape one execution.
#[derive(Default)]
pub struct RunOptions {
    /// Force every test's sub-cases to run serially (FR-T-09).
    ///
    /// A test's own `serial` wins over this flag, and this flag wins
    /// over the configuration's `default_serial` (FR-T-09/T-10).
    pub serial: bool,
    /// Cap on concurrent worker threads per parallel region
    /// (`-m/--max-thread`, FR-T-04).
    pub max_threads: Option<usize>,
    /// Run only the named test (`-d/--debug`, FR-T-08). The
    /// configuration's `debug_test` list wins over this flag.
    pub debug: Option<String>,
    /// Child-isolation mode (FR-T-05): run exactly one sub-case of one
    /// test as a normal test — `should_panic` is ignored so the child
    /// never spawns grandchildren. Set by death-test launchers, never
    /// by users.
    pub isolate: Option<IsolationTarget>,
    /// Wall-clock budget per isolated death-test child; defaults to
    /// [`DEFAULT_DEATH_TIMEOUT`].
    pub death_timeout: Option<Duration>,
    /// How death tests are isolated (FR-T-05); the default skips them.
    pub death: DeathIsolation,
}

impl std::fmt::Debug for RunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunOptions")
            .field("serial", &self.serial)
            .field("max_threads", &self.max_threads)
            .field("debug", &self.debug)
            .field("isolate", &self.isolate)
            .field("death_timeout", &self.death_timeout)
            .field("death", &self.death)
            .finish()
    }
}

/// The single sub-case a death-test child executes.
#[derive(Debug, Clone)]
pub struct IsolationTarget {
    /// Name of the test owning the sub-case.
    pub test: String,
    /// Display name of the sub-case (Q-05 format).
    pub subcase: String,
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
    /// The debug or isolation selection named something that does not
    /// exist (FR-T-08): a user mistake, not a library problem.
    #[error("no test or sub-case named `{0}` exists in the configuration")]
    TestNotFound(String),
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
/// for load-time findings (the same gate `check` enforces),
/// [`RunError::TestNotFound`] when the debug/isolate selection names
/// something unknown, and [`RunError::Load`] for library-load
/// failures. A run that executes and finds failing cases still returns
/// [`Ok`] - the failure signal lives in the report's summary, not in
/// this result.
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
    // Resolve the selection before any library is loaded: a typo'd
    // filter is a user mistake (exit 2), not a load failure.
    let filter = resolve_selection(&plan, &options)?;
    let loaded = LoadedFunctions::load(&plan.libraries)?;
    let mut runner = Runner::new(
        lib_path, cases_path, plan, loaded, options, filter, reporter,
    );
    runner.run()
}

/// The explicit execution context (AR-03): everything one run needs.
pub struct Runner {
    lib_path: PathBuf,
    cases_path: PathBuf,
    plan: Plan,
    loaded: LoadedFunctions,
    options: RunOptions,
    filter: Option<Vec<String>>,
    page: ParamPage,
    reporter: Box<dyn Reporter>,
}

impl Runner {
    /// Constructs a runner from an assembled plan, loaded libraries, run
    /// options, the resolved selection, and a reporter. The
    /// `param_page` starts zeroed (FR-A-06).
    pub fn new(
        lib_path: &Path,
        cases_path: &Path,
        plan: Plan,
        loaded: LoadedFunctions,
        options: RunOptions,
        filter: Option<Vec<String>>,
        reporter: Box<dyn Reporter>,
    ) -> Self {
        Self {
            lib_path: lib_path.to_path_buf(),
            cases_path: cases_path.to_path_buf(),
            plan,
            loaded,
            options,
            filter,
            page: ParamPage::zeroed(),
            reporter,
        }
    }

    /// Runs every selected execution and renders the report.
    ///
    /// # Errors
    /// Returns [`RunError::TestNotFound`] defensively (the selection
    /// was resolved by [`execute`]) and the reporter's I/O error
    /// wrapped in [`RunError::Report`] when rendering fails.
    pub fn run(&mut self) -> Result<RunReport, RunError> {
        let Runner {
            lib_path,
            cases_path,
            plan,
            loaded,
            options,
            filter,
            page,
            reporter,
        } = self;

        let ctx = RunContext {
            lib_path,
            cases_path,
            plan,
            loaded,
            options,
        };

        let mut summary = RunSummary::default();
        let mut cases: Vec<CaseOutcome> = Vec::new();
        let mut perf: Vec<PerfSample> = Vec::new();

        // Enter the run-level scopes (process, global) in entry order.
        // An init failure stops the remaining scopes' init; the exits
        // of the scopes that did enter still run below (FR-E-03).
        let mut run_init_failures: Vec<String> = Vec::new();
        let mut entered_run: Vec<EnvScope> = Vec::new();
        for &scope in run_scopes() {
            let phase = run_env_cmds(
                ctx.loaded,
                page,
                scope_init(ctx.plan, None, scope),
                scope_label(scope),
                "init",
                true,
                &mut perf,
            );
            summary.skipped += phase.skipped;
            if phase.failures.is_empty() {
                entered_run.push(scope);
            } else {
                run_init_failures.extend(phase.failures);
                break;
            }
        }

        // Which tests this run executes, and which of them belong to a
        // concurrency group. A debug or isolation selection disables
        // the grouping entirely: the selected test runs standalone even
        // when a group references it (FR-T-08).
        let selected: Vec<&TestPlan> = plan
            .tests
            .iter()
            .filter(|test| match filter.as_deref() {
                Some(names) => names.iter().any(|name| name == &test.name),
                None => true,
            })
            .collect();
        let groups_active = filter.is_none() && !plan.concurrency_groups.is_empty();
        let grouped: HashSet<&str> = if groups_active {
            plan.concurrency_groups
                .iter()
                .flat_map(|group| group.tests.iter().map(String::as_str))
                .collect()
        } else {
            HashSet::new()
        };

        let mut skipped = 0usize;
        if run_init_failures.is_empty() {
            if groups_active {
                for group in &plan.concurrency_groups {
                    run_group(&ctx, group, page, &mut cases, &mut perf, &mut skipped);
                }
            }
            for test in &selected {
                if filter.is_none() && grouped.contains(test.name.as_str()) {
                    // A grouped test runs only inside its group (FR-T-03).
                    continue;
                }
                run_test(&ctx, test, None, page, &mut cases, &mut perf, &mut skipped);
            }
        } else {
            // The run-level setup failed: no test can run, so every
            // selected execution inherits that failure (FR-E-03).
            for test in &selected {
                for execution in executions_of(options, test, None) {
                    run_init_failures_accumulate(
                        &mut cases,
                        &execution.display,
                        &run_init_failures,
                    );
                }
            }
        }

        // Leave the run-level scopes in reverse entry order.
        let mut run_exit_failures: Vec<String> = Vec::new();
        for scope in entered_run.iter().rev() {
            let phase = run_env_cmds(
                ctx.loaded,
                page,
                scope_exit(ctx.plan, None, *scope),
                scope_label(*scope),
                "exit",
                false,
                &mut perf,
            );
            summary.skipped += phase.skipped;
            run_exit_failures.extend(phase.failures);
        }

        // Account every execution. A run-level failure frames the whole
        // run, so it is attributed even to an execution that was
        // skipped for lack of isolation (FR-E-03, NFR-02).
        summary.skipped += skipped;
        let accounted: Vec<CaseOutcome> = cases
            .drain(..)
            .map(|mut case| {
                if !run_init_failures.is_empty() || !run_exit_failures.is_empty() {
                    let mut reasons = run_init_failures.clone();
                    if case.status == CaseStatus::Failed {
                        if let Some(reason) = case.reason.take() {
                            reasons.push(reason);
                        }
                    }
                    reasons.extend(run_exit_failures.iter().cloned());
                    case.reason = Some(reasons.join("; "));
                    case.status = CaseStatus::Failed;
                }
                summary.total += 1;
                match case.status {
                    CaseStatus::Passed => summary.success += 1,
                    CaseStatus::Failed => summary.failure += 1,
                    CaseStatus::Skipped => summary.skipped += 1,
                }
                case
            })
            .collect();
        cases.extend(accounted);

        let report = RunReport {
            summary,
            cases,
            perf,
        };
        reporter.report(&report).map_err(RunError::Report)?;
        Ok(report)
    }
}

/// Records one execution that inherited a run-level init failure.
fn run_init_failures_accumulate(cases: &mut Vec<CaseOutcome>, display: &str, failures: &[String]) {
    cases.push(CaseOutcome {
        name: display.to_string(),
        status: CaseStatus::Failed,
        reason: Some(failures.join("; ")),
    });
}

/// The immutable context every scheduler level shares: the plan, the
/// loaded functions, the options, and the two configuration paths a
/// death-test launcher needs.
struct RunContext<'a> {
    lib_path: &'a Path,
    cases_path: &'a Path,
    plan: &'a Plan,
    loaded: &'a LoadedFunctions,
    options: &'a RunOptions,
}

/// One schedulable execution: a sub-case replicated onto one worker.
#[derive(Clone)]
struct Execution<'a> {
    /// The sub-case whose commands run.
    subcase: &'a SubCase,
    /// Display name: the sub-case name, with a `@replica` suffix when
    /// the test runs more than one worker and a `group/` prefix inside
    /// a concurrency group.
    display: String,
}

/// Computes the executions one test schedules (FR-T-02, FR-T-08).
///
/// Every sub-case is duplicated once per `thread_num` replica; a child
/// isolation target schedules exactly its own sub-case once. The
/// display name keeps the bare sub-case name for the serial single
/// worker so reports stay identical to the pre-concurrency format.
fn executions_of<'a>(
    options: &RunOptions,
    test: &'a TestPlan,
    group: Option<&str>,
) -> Vec<Execution<'a>> {
    if let Some(target) = &options.isolate {
        let subcase = test
            .subcases
            .iter()
            .find(|subcase| subcase.name == target.subcase);
        return subcase
            .into_iter()
            .map(|subcase| Execution {
                subcase,
                display: subcase.name.clone(),
            })
            .collect();
    }
    let thread_num = test.thread_num;
    let mut out = Vec::with_capacity(test.subcases.len() * thread_num);
    for subcase in &test.subcases {
        for replica in 0..thread_num {
            let mut display = subcase.name.clone();
            if thread_num > 1 {
                display = format!("{display}@{replica}");
            }
            if let Some(group) = group {
                display = format!("{group}/{display}");
            }
            out.push(Execution { subcase, display });
        }
    }
    out
}

/// Resolves the debug/isolate selection of one run (FR-T-08).
///
/// Precedence: the isolation target (a death-test child must win, or a
/// configuration-level `debug_test` could filter the death test out of
/// its own child), then the configuration's `debug_test`, then the
/// CLI `-d`. An empty list selects everything.
fn selection_filter(plan: &Plan, options: &RunOptions) -> Option<Vec<String>> {
    if let Some(target) = &options.isolate {
        Some(vec![target.test.clone()])
    } else if plan.debug_tests.is_empty() {
        options.debug.clone().map(|name| vec![name])
    } else {
        Some(plan.debug_tests.clone())
    }
}

/// Validates the selection against the plan: every filtered name and
/// the isolation target must exist.
fn resolve_selection(plan: &Plan, options: &RunOptions) -> Result<Option<Vec<String>>, RunError> {
    let filter = selection_filter(plan, options);
    if let Some(names) = &filter {
        for name in names {
            if !plan.tests.iter().any(|test| &test.name == name) {
                return Err(RunError::TestNotFound(name.clone()));
            }
        }
    }
    if let Some(target) = &options.isolate {
        match plan.tests.iter().find(|test| test.name == target.test) {
            None => return Err(RunError::TestNotFound(target.test.clone())),
            Some(test) => {
                if !test
                    .subcases
                    .iter()
                    .any(|subcase| subcase.name == target.subcase)
                {
                    return Err(RunError::TestNotFound(target.subcase.clone()));
                }
            }
        }
    }
    Ok(filter)
}

/// Worker threads for one parallel test: `thread_num` bounded by the
/// execution count and the `-m` cap (FR-T-02/T-04). Never below one.
fn worker_count(executions: usize, thread_num: usize, max_threads: Option<usize>) -> usize {
    let mut workers = thread_num.min(executions).max(1);
    if let Some(cap) = max_threads {
        workers = workers.min(cap).max(1);
    }
    workers
}

/// Everything one worker thread produces for one test or group chunk.
struct WorkerResult {
    /// Case outcomes in the worker's execution order.
    outcomes: Vec<CaseOutcome>,
    /// Perf samples collected on the worker.
    perf: Vec<PerfSample>,
    /// Skip count: env-phase and command `CCALLER_ERR_SKIP`s (Q-01).
    skipped: usize,
    /// The worker's final page; the first worker's page is absorbed
    /// back into the owning thread's page (see the module docs).
    page: ParamPage,
}

impl WorkerResult {
    /// The defensive outcome of a worker that panicked: every execution
    /// it owned fails visibly instead of vanishing.
    fn poisoned(displays: &[String]) -> Self {
        Self {
            outcomes: displays
                .iter()
                .map(|display| CaseOutcome {
                    name: display.clone(),
                    status: CaseStatus::Failed,
                    reason: Some("internal error: the worker thread panicked".to_string()),
                })
                .collect(),
            perf: Vec::new(),
            skipped: 0,
            page: ParamPage::zeroed(),
        }
    }
}

/// Folds one worker's results into the run accumulators.
fn absorb_worker(
    result: WorkerResult,
    cases: &mut Vec<CaseOutcome>,
    perf: &mut Vec<PerfSample>,
    skipped: &mut usize,
) {
    let WorkerResult {
        outcomes,
        perf: samples,
        skipped: worker_skipped,
        page: _,
    } = result;
    cases.extend(outcomes);
    perf.extend(samples);
    *skipped += worker_skipped;
}

/// Resolves a sub-case's death-test flag (FR-T-05): the input group's
/// override, else the test-level default.
fn effective_should_panic(test: &TestPlan, subcase: &SubCase) -> bool {
    subcase.should_panic.unwrap_or(test.should_panic)
}

/// Resolves a sub-case's failure handling (FR-T-01): the input group's
/// override, else the test-level default.
fn effective_break_if_fail(test: &TestPlan, subcase: &SubCase) -> bool {
    subcase.break_if_fail.unwrap_or(test.break_if_fail)
}

/// Runs one test: its executions on this thread or on workers.
///
/// A death test (FR-T-05) executes in isolated children instead — see
/// [`run_death_test`]. Otherwise the effective serial flag decides: a
/// serial test runs inline on the owning thread's page (today's
/// behaviour), and a parallel test spreads its executions over
/// [`worker_count`] workers, each owning a private page copy.
///
/// A test whose input groups override `should_panic` (FR-T-05) can mix
/// death and normal executions; the death ones isolate in children and
/// the rest run through the normal scheduler.
fn run_test(
    ctx: &RunContext<'_>,
    test: &TestPlan,
    group: Option<&str>,
    page: &mut ParamPage,
    cases: &mut Vec<CaseOutcome>,
    perf: &mut Vec<PerfSample>,
    skipped: &mut usize,
) {
    let all = executions_of(ctx.options, test, group);
    if all.is_empty() {
        return;
    }

    let executions: Vec<Execution<'_>> = if ctx.options.isolate.is_some() {
        // A death-test child runs its target as a normal test; the
        // death dispatch below belongs to the parent only.
        all
    } else {
        let (death, normal): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|execution| effective_should_panic(test, execution.subcase));
        if !death.is_empty() {
            run_death_test(ctx, test, &death, cases);
        }
        if normal.is_empty() {
            return;
        }
        normal
    };

    let serial = ctx.options.isolate.is_some()
        || plan::effective_serial(test.serial, ctx.options.serial, ctx.plan.default_serial);
    debug!(
        "test `{}`: serial = {serial} (declared {:?}, cli {}, default {}), {} execution(s)",
        test.name,
        test.serial,
        ctx.options.serial,
        ctx.plan.default_serial,
        executions.len()
    );

    let workers = if serial || executions.len() == 1 {
        1
    } else {
        worker_count(executions.len(), test.thread_num, ctx.options.max_threads)
    };

    if workers <= 1 {
        let (outcomes, samples, worker_skipped) =
            run_framed_executions(ctx, test, page, &executions);
        cases.extend(outcomes);
        perf.extend(samples);
        *skipped += worker_skipped;
        return;
    }

    info!(
        "test `{}`: {} execution(s) on {workers} worker thread(s)",
        test.name,
        executions.len()
    );
    let chunk = executions.len().div_ceil(workers);
    let mut results: Vec<WorkerResult> = Vec::with_capacity(workers);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for worker_execs in executions.chunks(chunk) {
            let displays: Vec<String> = worker_execs
                .iter()
                .map(|execution| execution.display.clone())
                .collect();
            let mut worker_page = page.clone();
            handles.push((
                displays,
                scope.spawn(move || {
                    let (outcomes, samples, worker_skipped) =
                        run_framed_executions(ctx, test, &mut worker_page, worker_execs);
                    WorkerResult {
                        outcomes,
                        perf: samples,
                        skipped: worker_skipped,
                        page: worker_page,
                    }
                }),
            ));
        }
        for (displays, handle) in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => results.push(WorkerResult::poisoned(&displays)),
            }
        }
    });
    // Absorb the first worker's final page so the owning thread's page
    // keeps evolving through one sound sub-case state (module docs).
    if let Some(first) = results.first() {
        *page = first.page.clone();
    }
    for result in results {
        absorb_worker(result, cases, perf, skipped);
    }
}

/// Runs one concurrency group (FR-T-03): its member tests in parallel.
///
/// Member tests are chunked over `min(members, -m)` threads; every
/// chunk thread owns a page copy that its members evolve serially, and
/// the first chunk's final page is absorbed back into the caller's.
fn run_group(
    ctx: &RunContext<'_>,
    group: &ConcurrencyGroupPlan,
    page: &mut ParamPage,
    cases: &mut Vec<CaseOutcome>,
    perf: &mut Vec<PerfSample>,
    skipped: &mut usize,
) {
    let members: Vec<&TestPlan> = group
        .tests
        .iter()
        .filter_map(|name| ctx.plan.tests.iter().find(|test| &test.name == name))
        .collect();
    if members.is_empty() {
        return;
    }
    let threads = ctx
        .options
        .max_threads
        .map(|cap| cap.min(members.len()))
        .unwrap_or(members.len())
        .max(1);
    if threads <= 1 {
        for member in members {
            run_test(ctx, member, Some(&group.name), page, cases, perf, skipped);
        }
        return;
    }
    info!(
        "concurrency group `{}`: {} test(s) on {threads} thread(s)",
        group.name,
        members.len()
    );
    let chunk = members.len().div_ceil(threads);
    let mut results: Vec<WorkerResult> = Vec::with_capacity(threads);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for member_chunk in members.chunks(chunk) {
            let mut thread_page = page.clone();
            handles.push(scope.spawn(move || {
                let mut outcomes: Vec<CaseOutcome> = Vec::new();
                let mut samples: Vec<PerfSample> = Vec::new();
                let mut thread_skipped = 0usize;
                for member in member_chunk {
                    run_test(
                        ctx,
                        member,
                        Some(&group.name),
                        &mut thread_page,
                        &mut outcomes,
                        &mut samples,
                        &mut thread_skipped,
                    );
                }
                WorkerResult {
                    outcomes,
                    perf: samples,
                    skipped: thread_skipped,
                    page: thread_page,
                }
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => results.push(WorkerResult::poisoned(&[])),
            }
        }
    });
    if let Some(first) = results.first() {
        *page = first.page.clone();
    }
    for result in results {
        absorb_worker(result, cases, perf, skipped);
    }
}

/// Runs one death test (FR-T-05): every execution in an isolated child.
///
/// Children run one at a time on the calling thread; each child frames
/// its own env stack in its own process, so the parent enters no
/// test-level scopes here. A crash passes the execution; surviving,
/// timing out, or failing to spawn fails it.
fn run_death_test(
    ctx: &RunContext<'_>,
    test: &TestPlan,
    executions: &[Execution<'_>],
    cases: &mut Vec<CaseOutcome>,
) {
    let timeout = ctx.options.death_timeout.unwrap_or(DEFAULT_DEATH_TIMEOUT);
    match &ctx.options.death {
        DeathIsolation::Skip => {
            // FR-T-05: a platform that cannot isolate must skip the death
            // test with a warning and must not judge it a failure.
            warn!(
                "skipping death test `{}`: no isolation strategy is available",
                test.name
            );
            for execution in executions {
                cases.push(CaseOutcome {
                    name: execution.display.clone(),
                    status: CaseStatus::Skipped,
                    reason: None,
                });
            }
        }
        DeathIsolation::Child(launcher) => {
            for execution in executions {
                let request = DeathTestRequest {
                    lib_path: ctx.lib_path.to_path_buf(),
                    cases_path: ctx.cases_path.to_path_buf(),
                    test: test.name.clone(),
                    subcase: execution.subcase.name.clone(),
                };
                let verdict = match launcher.spawn(&request) {
                    Ok(child) => match isolate_child(child, timeout) {
                        Ok(DeathOutcome::Crashed(detail)) => {
                            info!(
                                "death test `{}` crashed as expected ({detail})",
                                execution.display
                            );
                            Ok(())
                        }
                        Ok(DeathOutcome::Survived(code)) => Err(format!(
                            "death test did not crash: the isolated child exited \
                             normally with code {}",
                            code.map(|c| c.to_string())
                                .unwrap_or_else(|| { "unknown".to_string() })
                        )),
                        Ok(DeathOutcome::TimedOut(budget)) => Err(format!(
                            "death test did not crash: the isolated child hung and \
                             was killed after {budget:?}"
                        )),
                        Err(error) => {
                            Err(format!("waiting for the isolated child failed: {error}"))
                        }
                    },
                    Err(error) => Err(format!("spawning the isolated child failed: {error}")),
                };
                cases.push(CaseOutcome {
                    name: execution.display.clone(),
                    status: if verdict.is_ok() {
                        CaseStatus::Passed
                    } else {
                        CaseStatus::Failed
                    },
                    reason: verdict.err(),
                });
            }
        }
    }
}

/// Runs `executions` inside the test-level env framing on `page`.
///
/// Mirrors the analyzer's per-sub-case replay: case env init, thread
/// env init, the commands, then the exits reversed (Q-13). An init
/// failure stops the remaining init and fails every execution the
/// framing covered; exit failures attribute the same way (FR-E-03).
fn run_framed_executions(
    ctx: &RunContext<'_>,
    test: &TestPlan,
    page: &mut ParamPage,
    executions: &[Execution<'_>],
) -> (Vec<CaseOutcome>, Vec<PerfSample>, usize) {
    let mut perf: Vec<PerfSample> = Vec::new();
    let mut skipped = 0usize;

    // Enter the test-level scopes (case, thread) in entry order.
    let mut init_failures: Vec<String> = Vec::new();
    let mut entered: Vec<EnvScope> = Vec::new();
    for &scope in test_scopes() {
        let phase = run_env_cmds(
            ctx.loaded,
            page,
            scope_init(ctx.plan, Some(test), scope),
            scope_label(scope),
            "init",
            true,
            &mut perf,
        );
        skipped += phase.skipped;
        if phase.failures.is_empty() {
            entered.push(scope);
        } else {
            init_failures.extend(phase.failures);
            break;
        }
    }
    let init_ok = init_failures.is_empty();

    // Run each execution's own Cmds. An init failure means the
    // execution's setup never completed, so it inherits the failure
    // instead of running (FR-E-03).
    let mut outcomes: Vec<CaseOutcome> = Vec::with_capacity(executions.len());
    for execution in executions {
        let mut failures = init_failures.clone();
        if init_ok {
            for (index, cmd) in execution.subcase.cmds.iter().enumerate() {
                match run_cmd(ctx.loaded, page, cmd, &execution.display, index, &mut perf) {
                    CmdResult::Passed => {}
                    CmdResult::Skipped => skipped += 1,
                    CmdResult::Failed(reason) => {
                        failures.push(reason);
                        // FR-T-01: a failure interrupts the remaining
                        // Cmds only when the (possibly group-overridden)
                        // break_if_fail is true.
                        if effective_break_if_fail(test, execution.subcase) {
                            break;
                        }
                    }
                }
            }
        }
        let status = if failures.is_empty() {
            CaseStatus::Passed
        } else {
            CaseStatus::Failed
        };
        outcomes.push(CaseOutcome {
            name: execution.display.clone(),
            status,
            reason: (!failures.is_empty()).then(|| failures.join("; ")),
        });
    }

    // Leave the test-level scopes in reverse entry order; exit failures
    // belong to every execution the test framed.
    let mut exit_failures: Vec<String> = Vec::new();
    for scope in entered.iter().rev() {
        let phase = run_env_cmds(
            ctx.loaded,
            page,
            scope_exit(ctx.plan, Some(test), *scope),
            scope_label(*scope),
            "exit",
            false,
            &mut perf,
        );
        skipped += phase.skipped;
        exit_failures.extend(phase.failures);
    }
    if !exit_failures.is_empty() {
        for outcome in outcomes.iter_mut() {
            let mut reasons = outcome.reason.take().into_iter().collect::<Vec<_>>();
            reasons.extend(exit_failures.iter().cloned());
            outcome.reason = Some(reasons.join("; "));
            outcome.status = CaseStatus::Failed;
        }
    }

    (outcomes, perf, skipped)
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

/// A short label for one env scope, used in perf sample contexts.
fn scope_label(scope: EnvScope) -> &'static str {
    match scope {
        EnvScope::Process => "process env",
        EnvScope::Global => "global env",
        EnvScope::Case => "case env",
        EnvScope::Thread => "thread env",
    }
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
    site: &str,
    phase: &str,
    stop_on_failure: bool,
    perf: &mut Vec<PerfSample>,
) -> EnvPhaseRun {
    let mut result = EnvPhaseRun {
        failures: Vec::new(),
        skipped: 0,
    };
    let display = format!("{site} {phase}");
    for (index, cmd) in cmds.iter().enumerate() {
        match run_cmd(loaded, page, cmd, &display, index, perf) {
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

/// Marshals one Cmd's arguments, invokes it, classifies the result, and
/// times the invocation when the Cmd declared `perf` (FR-P-01).
///
/// The timer wraps the call bridge only - argument marshalling and
/// assertion evaluation are framework work, not the wrapper's cost.
fn run_cmd(
    loaded: &LoadedFunctions,
    page: &mut ParamPage,
    cmd: &ResolvedCmd,
    display: &str,
    index: usize,
    perf: &mut Vec<PerfSample>,
) -> CmdResult {
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
    let started = Instant::now();
    let code = call::invoke(func.call, page, &args);
    let duration = started.elapsed();
    if cmd.perf {
        let context = format!("{display} cmd {index}");
        debug!("perf: {context} `{}` took {duration:?}", cmd.opfunc);
        perf.push(PerfSample {
            context,
            opfunc: cmd.opfunc.clone(),
            duration,
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_plan(name: &str, thread_num: usize, subcases: &[&str]) -> TestPlan {
        TestPlan {
            name: name.to_string(),
            break_if_fail: true,
            case_env: None,
            subcases: subcases
                .iter()
                .map(|subcase| SubCase {
                    name: (*subcase).to_string(),
                    bindings: BTreeMap::new(),
                    cmds: Vec::new(),
                    should_panic: None,
                    break_if_fail: None,
                })
                .collect(),
            serial: None,
            should_panic: false,
            thread_num,
        }
    }

    #[test]
    fn worker_count_is_bounded_by_executions_and_cap__F_T_02_F_T_04() {
        // thread_num alone: one worker per execution.
        assert_eq!(worker_count(8, 8, None), 8);
        // Fewer executions than thread_num: never more workers than work.
        assert_eq!(worker_count(3, 8, None), 3);
        // The -m cap wins over thread_num.
        assert_eq!(worker_count(8, 8, Some(4)), 4);
        assert_eq!(worker_count(8, 2, Some(4)), 2);
        // Never zero.
        assert_eq!(worker_count(8, 1, None), 1);
        assert_eq!(worker_count(1, 8, None), 1);
    }

    #[test]
    fn executions_duplicate_per_replica_with_display_tags__F_T_02() {
        let options = RunOptions::default();
        let test = test_plan("t", 2, &["t/a#0[x=1]", "t/a#1[x=2]"]);
        let executions = executions_of(&options, &test, None);
        assert_eq!(executions.len(), 4);
        // Sub-case major, replica minor; replicas carry @k tags.
        assert_eq!(executions[0].display, "t/a#0[x=1]@0");
        assert_eq!(executions[1].display, "t/a#0[x=1]@1");
        assert_eq!(executions[2].display, "t/a#1[x=2]@0");
        assert_eq!(executions[3].display, "t/a#1[x=2]@1");
    }

    #[test]
    fn single_worker_keeps_the_bare_subcase_name__F_T_02() {
        // thread_num = 1 (the default) must not change any report name.
        let options = RunOptions::default();
        let test = test_plan("t", 1, &["t/a#0[x=1]"]);
        let executions = executions_of(&options, &test, None);
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].display, "t/a#0[x=1]");
    }

    #[test]
    fn group_members_get_the_group_prefix__F_T_03() {
        let options = RunOptions::default();
        let test = test_plan("t", 1, &["t"]);
        let executions = executions_of(&options, &test, Some("mixed_io"));
        assert_eq!(executions[0].display, "mixed_io/t");
    }

    #[test]
    fn group_overrides_win_over_the_test_level_flags__F_T_05_F_T_01() {
        let mut test = test_plan("t", 1, &["t/a#0[x=1]", "t/a#1[x=2]"]);
        test.should_panic = true;
        test.break_if_fail = true;
        // Neither sub-case overrides: both inherit the death-test path.
        assert!(effective_should_panic(&test, &test.subcases[0]));
        assert!(effective_break_if_fail(&test, &test.subcases[0]));
        // One group opts out of both: its sub-case runs normally.
        let mut opted_out = test.subcases[1].clone();
        opted_out.should_panic = Some(false);
        opted_out.break_if_fail = Some(false);
        assert!(!effective_should_panic(&test, &opted_out));
        assert!(!effective_break_if_fail(&test, &opted_out));
        // A group can also force the death path onto an otherwise normal
        // test.
        let mut normal = test_plan("t2", 1, &["t2"]);
        normal.should_panic = false;
        let mut forced = normal.subcases[0].clone();
        forced.should_panic = Some(true);
        assert!(effective_should_panic(&normal, &forced));
    }

    #[test]
    fn isolation_target_schedules_exactly_its_subcase__F_T_05() {
        let options = RunOptions {
            isolate: Some(IsolationTarget {
                test: "t".to_string(),
                subcase: "t/a#1[x=2]".to_string(),
            }),
            ..RunOptions::default()
        };
        let test = test_plan("t", 4, &["t/a#0[x=1]", "t/a#1[x=2]"]);
        let executions = executions_of(&options, &test, None);
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].display, "t/a#1[x=2]");
    }

    #[test]
    fn selection_precedence_isolate_then_config_then_cli__F_T_08() {
        let mut plan = crate::plan::Plan {
            libraries: Vec::new(),
            global_env: None,
            process_env: None,
            thread_env: None,
            case_envs: Vec::new(),
            tests: vec![test_plan("t1", 1, &["t1"]), test_plan("t2", 1, &["t2"])],
            concurrency_groups: Vec::new(),
            debug_tests: vec!["t2".to_string()],
            default_serial: false,
        };

        // Configuration debug_test wins over the CLI flag.
        let options = RunOptions {
            debug: Some("t1".to_string()),
            ..RunOptions::default()
        };
        assert_eq!(
            selection_filter(&plan, &options),
            Some(vec!["t2".to_string()])
        );

        // Without a configuration list, the CLI flag applies.
        plan.debug_tests.clear();
        assert_eq!(
            selection_filter(&plan, &options),
            Some(vec!["t1".to_string()])
        );

        // The isolation target wins over everything.
        let options = RunOptions {
            debug: Some("t1".to_string()),
            isolate: Some(IsolationTarget {
                test: "t2".to_string(),
                subcase: "t2".to_string(),
            }),
            ..RunOptions::default()
        };
        assert_eq!(
            selection_filter(&plan, &options),
            Some(vec!["t2".to_string()])
        );

        // No selection anywhere: everything runs.
        assert_eq!(selection_filter(&plan, &RunOptions::default()), None);
    }

    #[test]
    fn resolve_selection_rejects_unknown_names__F_T_08() {
        let plan = crate::plan::Plan {
            libraries: Vec::new(),
            global_env: None,
            process_env: None,
            thread_env: None,
            case_envs: Vec::new(),
            tests: vec![test_plan("t1", 1, &["t1"])],
            concurrency_groups: Vec::new(),
            debug_tests: Vec::new(),
            default_serial: false,
        };
        let options = RunOptions {
            debug: Some("ghost".to_string()),
            ..RunOptions::default()
        };
        match resolve_selection(&plan, &options) {
            Err(RunError::TestNotFound(name)) => assert_eq!(name, "ghost"),
            other => panic!("expected TestNotFound, got {other:?}"),
        }

        // The isolation target must name an existing sub-case, not just
        // an existing test.
        let options = RunOptions {
            isolate: Some(IsolationTarget {
                test: "t1".to_string(),
                subcase: "no-such-subcase".to_string(),
            }),
            ..RunOptions::default()
        };
        match resolve_selection(&plan, &options) {
            Err(RunError::TestNotFound(name)) => assert_eq!(name, "no-such-subcase"),
            other => panic!("expected TestNotFound, got {other:?}"),
        }
    }
}
