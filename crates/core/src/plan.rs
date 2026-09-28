//! The resolved run plan (execution scheduling model).
//!
//! [`build_plan`] turns two on-disk documents - a library description and
//! a case configuration - into a [`Plan`] the executor can walk without
//! touching the TOML again. It runs the same load-time pipeline as
//! `ccaller check` (parse, validate, expand, def-use) and refuses to build
//! a plan when any stage reports findings, so the executor never sees a
//! configuration that failed validation.
//!
//! Library paths are resolved here, relative to the library-description
//! file's directory, into the [`LibraryRequest`]s the ffi loader expects.
//! Env command values are resolved to literals here as well; the def-use
//! analysis has already rejected `$var` in env commands, so this stage
//! only ever reads literal values (a `$var` here is a defensive error).

use std::path::Path;

use toml::Spanned;

use ccaller_ffi::loader::LibraryRequest;

use crate::assertion::{lookup, Assertion};
use crate::config::diag::{codes, Diagnostic};
use crate::config::source::SourceDoc;
use crate::config::value::{parse_value, ConcreteValue, ScalarRaw, ValueKind};
use crate::config::{
    analyze_def_use, expand_test, validate_cases, validate_lib_description, CaseConfig, CmdDef,
    GlobalEnv, LibDescription, ResolvedCmd, ResolvedExpectation, SubCase, TestDef,
};
use crate::error::{CoreError, Location};

/// The fully-resolved execution plan for one `ccaller run`.
#[derive(Debug)]
pub struct Plan {
    /// Libraries to load, with paths resolved against the description file.
    pub libraries: Vec<LibraryRequest>,
    /// The global env layer, if declared.
    pub global_env: Option<EnvLayer>,
    /// The process env layer, if declared.
    pub process_env: Option<EnvLayer>,
    /// The thread env layer, if declared.
    pub thread_env: Option<EnvLayer>,
    /// Case-level envs, in declaration order.
    pub case_envs: Vec<CaseEnvPlan>,
    /// Tests in declaration order, each with its expanded sub-cases.
    pub tests: Vec<TestPlan>,
    /// Concurrency groups in declaration order (FR-T-03); a referenced
    /// test runs only inside its group.
    pub concurrency_groups: Vec<ConcurrencyGroupPlan>,
    /// The configuration's `debug_test` names (FR-T-08); they take
    /// priority over the CLI `-d` selection at run time.
    pub debug_tests: Vec<String>,
    /// Default `serial` value for tests that do not declare one
    /// (requirement spec 7.3, FR-T-09).
    pub default_serial: bool,
}

/// One concurrency group: the member tests run in parallel (FR-T-03).
#[derive(Debug)]
pub struct ConcurrencyGroupPlan {
    /// The group's name; prefixes its members' display names in reports.
    pub name: String,
    /// Names of the member tests, in declaration order.
    pub tests: Vec<String>,
}

/// One name-less env layer's resolved init and exit commands.
#[derive(Debug)]
pub struct EnvLayer {
    /// Commands run when entering the layer.
    pub init: Vec<ResolvedCmd>,
    /// Commands run when leaving the layer (reverse order at run time).
    pub exit: Vec<ResolvedCmd>,
}

/// A case-level env with its commands resolved and its test membership.
#[derive(Debug)]
pub struct CaseEnvPlan {
    /// The env's unique name.
    pub name: String,
    /// Commands run before the env's tests.
    pub init: Vec<ResolvedCmd>,
    /// Commands run after the env's tests.
    pub exit: Vec<ResolvedCmd>,
    /// Names of the tests this env claims.
    pub tests: Vec<String>,
}

/// One test of the run plan: its name, interruption policy, and sub-cases.
#[derive(Debug)]
pub struct TestPlan {
    /// The test's globally unique name.
    pub name: String,
    /// Whether a failed Cmd stops the remaining Cmds (FR-T-01).
    pub break_if_fail: bool,
    /// Index into [`Plan::case_envs`] of the env owning this test, if any.
    pub case_env: Option<usize>,
    /// The test's concrete sub-cases.
    pub subcases: Vec<SubCase>,
    /// The test's declared `serial`, if any; `None` inherits the CLI flag
    /// or [`Plan::default_serial`] (FR-T-09/T-10).
    pub serial: Option<bool>,
    /// Whether this is a death test (FR-T-05).
    pub should_panic: bool,
    /// Worker threads for this test; `>= 1` (FR-T-02). Each worker owns
    /// one `param_page` (Q-06) and the sub-cases are duplicated once per
    /// replica, exactly as the thread-per-test stress mode of hitest.
    pub thread_num: usize,
}

/// Computes the effective serial flag for one test (FR-T-09/T-10).
///
/// Precedence, highest first: the test's own `serial`, then the CLI
/// `--serial`, then the configuration's `default_serial`. Execution is
/// serial in this milestone regardless, but the resolved value is what
/// parallel scheduling will honour later and is logged for observability.
pub(crate) fn effective_serial(
    test_serial: Option<bool>,
    cli_serial: bool,
    default_serial: bool,
) -> bool {
    test_serial.unwrap_or(if cli_serial { true } else { default_serial })
}

/// The result of [`build_plan`]: a ready plan or the load-time findings.
///
/// The variants differ in size because [`Plan`] grew with the M3
/// scheduling fields; it stays unboxed deliberately - a plan is built
/// once per run and is the executor's hot value, while the small
/// `Invalid` variant is a cold error path.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum PlanOutcome {
    /// The plan is complete and executable.
    Ready(Plan),
    /// Load-time validation, expansion, or def-use found problems.
    Invalid(Vec<Diagnostic>),
}

/// Builds the run plan from the two configuration files.
///
/// # Errors
/// Returns [`CoreError::Io`] when a file cannot be read. Load-time
/// findings are reported through [`PlanOutcome::Invalid`] instead, because
/// they are configuration problems, not environment problems.
pub fn build_plan(lib_path: &Path, cases_path: &Path) -> Result<PlanOutcome, CoreError> {
    let lib_doc = SourceDoc::load(lib_path)?;
    let cases_doc = SourceDoc::load(cases_path)?;
    Ok(build_plan_docs(&lib_doc, &cases_doc, lib_path))
}

/// The plan pipeline over already-loaded documents.
fn build_plan_docs(lib_doc: &SourceDoc, cases_doc: &SourceDoc, lib_path: &Path) -> PlanOutcome {
    let mut errors = Vec::new();

    let libs = match lib_doc.parse::<LibDescription>() {
        Ok(libs) => Some(libs),
        Err(diag) => {
            errors.push(diag);
            None
        }
    };
    let cases = match cases_doc.parse::<CaseConfig>() {
        Ok(cases) => Some(cases),
        Err(diag) => {
            errors.push(diag);
            None
        }
    };

    if let Some(libs) = &libs {
        errors.extend(validate_lib_description(libs, lib_doc));
    }
    let mut expansions: Vec<(&TestDef, Vec<SubCase>)> = Vec::new();
    if let (Some(libs), Some(cases)) = (&libs, &cases) {
        errors.extend(validate_cases(cases, cases_doc, libs));
        if errors.is_empty() {
            for test in &cases.tests {
                let subcases =
                    expand_test(test.get_ref(), &cases.shared_inputs, cases_doc, &mut errors);
                expansions.push((test.get_ref(), subcases));
            }
            analyze_def_use(cases, &expansions, libs, cases_doc, &mut errors);
        }
    }

    if errors.is_empty() {
        if let (Some(libs), Some(cases)) = (&libs, &cases) {
            match assemble(libs, cases, &expansions, cases_doc, lib_path) {
                Ok(plan) => return PlanOutcome::Ready(plan),
                Err(diag) => errors.push(diag),
            }
        }
    }
    PlanOutcome::Invalid(errors)
}

/// Assembles the executable plan from validated, expanded inputs.
///
/// # Errors
/// Returns a diagnostic only in a defensive case (an env `$var` or bad
/// value) that the def-use stage has already rejected; reaching it means
/// an internal ordering mistake, surfaced instead of panicking.
fn assemble(
    libs: &LibDescription,
    cases: &CaseConfig,
    expansions: &[(&TestDef, Vec<SubCase>)],
    doc: &SourceDoc,
    lib_path: &Path,
) -> Result<Plan, Diagnostic> {
    let global_env = env_layer(cases.env.as_ref(), doc)?;
    let process_env = env_layer(cases.process_env.as_ref(), doc)?;
    let thread_env = env_layer(cases.thread_env.as_ref(), doc)?;

    let mut case_envs = Vec::with_capacity(cases.envs.len());
    for env in &cases.envs {
        let env = env.get_ref();
        case_envs.push(CaseEnvPlan {
            name: env.name.get_ref().clone(),
            init: resolve_env_cmds(&env.init, doc)?,
            exit: resolve_env_cmds(&env.exit, doc)?,
            tests: env.tests.iter().map(|t| t.get_ref().clone()).collect(),
        });
    }

    let tests = expansions
        .iter()
        .map(|(test, subcases)| {
            let name = test.name.get_ref().as_str();
            let case_env = case_envs
                .iter()
                .position(|env| env.tests.iter().any(|t| t == name));
            TestPlan {
                name: name.to_string(),
                break_if_fail: test.break_if_fail,
                case_env,
                subcases: subcases.clone(),
                serial: test.serial,
                should_panic: test.should_panic,
                thread_num: (*test.thread_num.get_ref() as usize).max(1),
            }
        })
        .collect();

    let concurrency_groups = cases
        .concurrences
        .iter()
        .map(|group| {
            let group = group.get_ref();
            ConcurrencyGroupPlan {
                name: group.name.get_ref().clone(),
                tests: group
                    .tests
                    .iter()
                    .map(|test| test.get_ref().clone())
                    .collect(),
            }
        })
        .collect();

    Ok(Plan {
        libraries: library_requests(libs, lib_path),
        global_env,
        process_env,
        thread_env,
        case_envs,
        tests,
        concurrency_groups,
        debug_tests: cases
            .debug_test
            .iter()
            .map(|name| name.get_ref().clone())
            .collect(),
        default_serial: cases.default_serial,
    })
}

/// Translates the parsed library description into loader requests, with
/// each path resolved against the description file's directory.
fn library_requests(libs: &LibDescription, lib_path: &Path) -> Vec<LibraryRequest> {
    // A bare `libs.toml` has the empty path as its parent; joining onto
    // "" keeps the library path slash-free, and dlopen then resolves it
    // through the OS library search instead of opening it directly. Dot
    // keeps the joined path relative-but-direct.
    let base = lib_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    libs.libs
        .iter()
        .map(|lib| {
            let lib = lib.get_ref();
            LibraryRequest {
                path: base.join(lib.path.get_ref().as_str()),
                func_names: lib
                    .funcs
                    .iter()
                    .map(|func| func.get_ref().name.get_ref().clone())
                    .collect(),
            }
        })
        .collect()
}

/// Resolves one name-less global-scope env layer into executable commands.
fn env_layer(
    env: Option<&Spanned<GlobalEnv>>,
    doc: &SourceDoc,
) -> Result<Option<EnvLayer>, Diagnostic> {
    let Some(env) = env else {
        return Ok(None);
    };
    let env = env.get_ref();
    Ok(Some(EnvLayer {
        init: resolve_env_cmds(&env.init, doc)?,
        exit: resolve_env_cmds(&env.exit, doc)?,
    }))
}

/// Resolves env commands into executable commands.
///
/// Env commands apply to every sub-case, so their values must be literals;
/// the def-use stage rejects `$var` first, making this function's own
/// rejection a defensive re-check.
fn resolve_env_cmds(
    cmds: &[Spanned<CmdDef>],
    doc: &SourceDoc,
) -> Result<Vec<ResolvedCmd>, Diagnostic> {
    let mut resolved = Vec::with_capacity(cmds.len());
    for cmd in cmds {
        let cmd = cmd.get_ref();
        let loc = doc.locate(cmd.opfunc.span());

        let mut args = Vec::with_capacity(cmd.args.len());
        for arg in &cmd.args {
            let entry = arg.get_ref();
            let Some((name, text)) = entry.split_once('=') else {
                return Err(Diagnostic::new(
                    codes::INVALID_VALUE,
                    loc.clone(),
                    format!("argument `{entry}` is not in `name=value` form"),
                ));
            };
            args.push((name.to_string(), resolve_env_value(text, &loc)?));
        }

        let expect = match cmd.registered_assertions().as_slice() {
            [(assertion, raw)] => Some(resolve_env_expectation(*assertion, raw.get_ref(), &loc)?),
            // Zero assertions is legal for env commands; more than one is a
            // validation finding, so both shapes fall through to `None`.
            _ => None,
        };

        resolved.push(ResolvedCmd {
            opfunc: cmd.opfunc.get_ref().clone(),
            args,
            expect,
            perf: cmd.perf,
        });
    }
    Ok(resolved)
}

/// Resolves one env argument; only literals are allowed (no `$var`, no `!`).
fn resolve_env_value(text: &str, loc: &Location) -> Result<ConcreteValue, Diagnostic> {
    let parsed = parse_value(text).map_err(|e| {
        Diagnostic::new(
            codes::INVALID_VALUE,
            loc.clone(),
            format!("invalid value `{text}`: {e}"),
        )
    })?;
    if parsed.negated {
        return Err(Diagnostic::new(
            codes::INVALID_VALUE,
            loc.clone(),
            "negation `!` is only valid in expectation values".to_string(),
        ));
    }
    match parsed.kind {
        ValueKind::Int(i) => Ok(ConcreteValue::Int(i)),
        ValueKind::Str(s) => Ok(ConcreteValue::Str(s)),
        ValueKind::Var(name) => Err(Diagnostic::new(
            codes::UNRESOLVED_VAR,
            loc.clone(),
            format!(
                "`${name}` cannot be used in env commands (they apply to every \
                 sub-case); use a literal value"
            ),
        )),
    }
}

/// Resolves an optional env expectation, folding FR-C-07 negation through
/// the assertion registry the same way [`crate::config::expand`] does.
fn resolve_env_expectation(
    declared: &'static dyn Assertion,
    raw: &ScalarRaw,
    loc: &Location,
) -> Result<ResolvedExpectation, Diagnostic> {
    let (value, negated) = match raw {
        ScalarRaw::Int(i) => (ConcreteValue::Int(*i), false),
        ScalarRaw::Str(s) => {
            let parsed = parse_value(s).map_err(|e| {
                Diagnostic::new(
                    codes::INVALID_VALUE,
                    loc.clone(),
                    format!("invalid value `{s}`: {e}"),
                )
            })?;
            let value = match parsed.kind {
                ValueKind::Int(i) => ConcreteValue::Int(i),
                ValueKind::Str(inner) => ConcreteValue::Str(inner),
                ValueKind::Var(name) => {
                    return Err(Diagnostic::new(
                        codes::UNRESOLVED_VAR,
                        loc.clone(),
                        format!(
                            "`${name}` cannot be used in env commands (they apply to every \
                             sub-case); use a literal value"
                        ),
                    ));
                }
            };
            (value, parsed.negated)
        }
    };

    let assertion = if negated {
        let Some(name) = declared.negated_field_name() else {
            return Err(Diagnostic::new(
                codes::INVALID_VALUE,
                loc.clone(),
                format!(
                    "negation `!` is not supported for `{}`",
                    declared.field_name()
                ),
            ));
        };
        lookup(name).ok_or_else(|| {
            Diagnostic::new(
                codes::INVALID_VALUE,
                loc.clone(),
                format!(
                    "negation `!` on `{}` folds into `{name}`, which is not registered",
                    declared.field_name()
                ),
            )
        })?
    } else {
        declared
    };

    Ok(ResolvedExpectation {
        kind: assertion.field_name(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::value::ConcreteValue;

    fn plan(lib_text: &str, case_text: &str) -> Plan {
        let lib_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: lib_text.to_string(),
        };
        let case_doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: case_text.to_string(),
        };
        match build_plan_docs(&lib_doc, &case_doc, Path::new("libs.toml")) {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::Invalid(diags) => panic!("unexpected findings: {diags:?}"),
        }
    }

    const LIBS: &str = "version = 1\n[[libs]]\npath = \"wrapper.so\"\nfuncs = [\n\
                        { name = \"Call_setup\", paras = [\"mode\"] },\n\
                        { name = \"Call_ping\", paras = [] },\n\
                        { name = \"Call_malloc\", paras = [\"len\", \"mem_idx\"], \
                        slot_roles = { mem_idx = \"write\" } },\n\
                        { name = \"Call_read\", paras = [\"mem_idx\"], \
                        slot_roles = { mem_idx = \"read\" } },\n]\n";

    #[test]
    fn library_paths_resolve_against_the_description_file__F_A_01() {
        let plan = plan(
            "version = 1\n[[libs]]\npath = \"lib/wrapper.so\"\nfuncs = [{ name = \"Call_ping\", paras = [] }]\n",
            "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n",
        );
        assert_eq!(plan.libraries.len(), 1);
        // The description file is `libs.toml`; a bare filename has the
        // empty parent, so the base normalizes to `.` and the joined
        // path stays description-relative (and slash-bearing, which
        // dlopen requires to open a file directly).
        assert_eq!(
            plan.libraries[0].path,
            std::path::PathBuf::from("./lib/wrapper.so")
        );
        assert_eq!(plan.libraries[0].func_names, vec!["Call_ping".to_string()]);
    }

    #[test]
    fn env_layers_assemble_with_case_binding__F_E_02() {
        let cases = "version = 1\n\
                     [env]\ninit = [{ opfunc = \"Call_ping\" }]\n\
                     [process_env]\ninit = [{ opfunc = \"Call_ping\" }]\n\
                     [thread_env]\ninit = [{ opfunc = \"Call_ping\" }]\n\
                     [[envs]]\nname = \"e\"\ninit = [{ opfunc = \"Call_ping\" }]\ntests = [\"t\"]\n\
                     [[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n";
        let plan = plan(LIBS, cases);
        assert!(plan.global_env.is_some());
        assert!(plan.process_env.is_some());
        assert!(plan.thread_env.is_some());
        assert_eq!(plan.case_envs.len(), 1);
        assert_eq!(plan.case_envs[0].tests, vec!["t".to_string()]);
        // The test belongs to case env index 0.
        assert_eq!(plan.tests[0].case_env, Some(0));
        assert!(plan.tests[0].break_if_fail);
    }

    #[test]
    fn env_init_and_exit_values_resolve_to_literals__F_E_01() {
        let cases = "version = 1\n\
                     [env]\ninit = [{ opfunc = \"Call_setup\", args = [\"mode=1\"] }]\n\
                     exit = [{ opfunc = \"Call_ping\" }]\n\
                     [[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n";
        let plan = plan(LIBS, cases);
        let global = plan.global_env.as_ref().unwrap();
        assert_eq!(global.init.len(), 1);
        assert_eq!(
            global.init[0].args,
            vec![("mode".to_string(), ConcreteValue::Int(1))]
        );
        assert!(global.init[0].expect.is_none());
        assert_eq!(global.exit.len(), 1);
    }

    #[test]
    fn config_with_findings_never_becomes_a_plan__F_C_08() {
        let lib_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: LIBS.to_string(),
        };
        let case_doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", \
                   expect_eq = 0 }]\n"
                .to_string(),
        };
        let outcome = build_plan_docs(&lib_doc, &case_doc, Path::new("libs.toml"));
        let PlanOutcome::Invalid(diags) = outcome else {
            panic!("expected an invalid plan");
        };
        assert!(diags.iter().any(|d| d.code == codes::UNKNOWN_OPFUNC));
    }

    #[test]
    fn serial_precedence_test_then_cli_then_default__F_T_09() {
        // A test's own `serial` wins over the CLI flag; the CLI flag wins
        // over `default_serial` (FR-T-09/T-10).
        assert!(effective_serial(Some(true), false, false));
        assert!(!effective_serial(Some(false), true, true));
        assert!(effective_serial(None, true, false));
        assert!(effective_serial(None, true, true));
        assert!(!effective_serial(None, false, false));
        assert!(effective_serial(None, false, true));
    }

    #[test]
    fn concurrency_and_thread_plumbing_assembles__F_T_02_F_T_03() {
        // FR-T-02: thread_num rides on the test plan; FR-T-03: concurrency
        // groups ride on the run plan with their member names; FR-T-08:
        // the configuration's debug names ride along for run-time filtering.
        let cases = "version = 1\n\
                     debug_test = []\n\
                     [[tests]]\nname = \"t1\"\nthread_num = 4\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n\
                     [[tests]]\nname = \"t2\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n\
                     [[concurrences]]\nname = \"g\"\ntests = [\"t2\"]\n";
        let plan = plan(LIBS, cases);
        assert_eq!(plan.tests[0].thread_num, 4);
        assert_eq!(plan.tests[1].thread_num, 1);
        assert_eq!(plan.concurrency_groups.len(), 1);
        assert_eq!(plan.concurrency_groups[0].name, "g");
        assert_eq!(plan.concurrency_groups[0].tests, vec!["t2".to_string()]);
        assert!(plan.debug_tests.is_empty());
    }

    #[test]
    fn debug_tests_assemble_for_run_time_filtering__F_T_08() {
        let cases = "version = 1\n\
                     debug_test = [\"t2\"]\n\
                     [[tests]]\nname = \"t1\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n\
                     [[tests]]\nname = \"t2\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n";
        let plan = plan(LIBS, cases);
        assert_eq!(plan.debug_tests, vec!["t2".to_string()]);
    }

    #[test]
    fn perf_flag_survives_resolution__F_P_01() {
        // CmdDef.perf must reach the executable ResolvedCmd (FR-P-01) —
        // for test commands (expansion) and env commands (plan assembly).
        let cases = "version = 1\n\
                     [env]\ninit = [{ opfunc = \"Call_ping\", perf = true }]\n\
                     [[tests]]\nname = \"t\"\ncmds = [\n\
                     \x20 { opfunc = \"Call_ping\", expect_eq = 0, perf = true },\n\
                     \x20 { opfunc = \"Call_ping\", expect_eq = 0 },\n]\n";
        let plan = plan(LIBS, cases);
        assert!(plan.global_env.as_ref().unwrap().init[0].perf);
        assert!(plan.tests[0].subcases[0].cmds[0].perf);
        assert!(!plan.tests[0].subcases[0].cmds[1].perf);
    }

    #[test]
    fn bare_relative_description_yields_a_directly_openable_path__F_A_02() {
        // A description given as a bare `libs.toml` must resolve library
        // paths against `.`, not against the empty path: dlopen treats a
        // slash-free name as a library-search name, silently ignoring the
        // description's directory (found while exercising M3 end to end).
        let plan = plan(
            LIBS,
            "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ping\", expect_eq = 0 }]\n",
        );
        assert_eq!(
            plan.libraries[0].path,
            Path::new("./wrapper.so"),
            "paths must carry a slash so dlopen opens them directly"
        );
    }
}
