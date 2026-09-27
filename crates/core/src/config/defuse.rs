//! Static slot def-use analysis (FR-C-10, requirement spec 7.2).
//!
//! The library description marks, per function parameter, whether the value
//! is a `param_page` slot index and whether the wrapper reads or writes that
//! slot. This module replays every sub-case as one linear command sequence —
//! process env init, global env init, case env init, thread env init, the
//! sub-case's commands, then the exit commands in reverse layer order — and
//! tracks a single written-set:
//!
//! - `read` (and the read side of `read_write`) requires an earlier write,
//!   otherwise [`codes::SLOT_READ_BEFORE_WRITE`];
//! - every slot index must be an integer in `[0, PARAM_PAGE_SLOTS)`;
//! - slot semantics come only from `slot_roles`, never from parameter names.
//!
//! The written-set starts empty for each sub-case: pages are thread-local
//! (decision Q-06) and sub-cases spread over worker threads, so a slot
//! written by a *different* sub-case must not satisfy a read — the analysis
//! keeps every sub-case self-contained. Writes from the env prefix are
//! visible because the runtime pre-writes them into each worker's page
//! (decision Q-02, FR-E-04).
//!
//! Findings that recur in several sub-cases (a command's text is static, so
//! the same slot error usually repeats in all of them) are deduplicated by
//! (code, message) and annotated with the number of further sub-cases
//! affected. Env command *values* are resolved once per env, not per
//! sub-case: env commands apply to every sub-case, so only literal values
//! are allowed there — `$var` is rejected.

use std::collections::{HashMap, HashSet};

use toml::Spanned;

use crate::error::Location;

use super::cases::{CaseConfig, CmdDef, GlobalEnv, TestDef};
use super::diag::{codes, Diagnostic};
use super::expand::SubCase;
use super::lib_desc::{FuncDecl, LibDescription};
use super::source::SourceDoc;
use super::value::{parse_value, ConcreteValue, ScalarRaw, ValueKind};

/// Number of `u64` slots in one thread's `param_page` (FR-A-06).
///
/// Re-exported from the ffi crate so the static analysis and the
/// runtime page share one numeric truth with `ccaller.h`.
pub use ccaller_ffi::page::PARAM_PAGE_SLOTS;

/// Runs the def-use analysis over every expanded test.
///
/// `expansions` pairs each test with the sub-cases [`super::expand_test`]
/// produced for it, in declaration order. Env-level findings (non-literal
/// env values) go straight to `out`; per-sub-case findings are deduplicated
/// as described in the module documentation.
pub fn analyze_def_use(
    config: &CaseConfig,
    expansions: &[(&TestDef, Vec<SubCase>)],
    libs: &LibDescription,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) {
    let process = resolve_env_layer(config.process_env.as_ref(), "process", libs, doc, out);
    let global = resolve_env_layer(config.env.as_ref(), "global", libs, doc, out);
    let thread = resolve_env_layer(config.thread_env.as_ref(), "thread", libs, doc, out);

    let mut case_envs: Vec<ResolvedEnv> = Vec::new();
    for env in &config.envs {
        let env = env.get_ref();
        let scope = format!("env `{}`", env.name.get_ref());
        let init = resolve_env_cmds(&env.init, "init", &scope, libs, doc, out);
        let exit = resolve_env_cmds(&env.exit, "exit", &scope, libs, doc, out);
        let tests = env.tests.iter().map(|t| t.get_ref().as_str()).collect();
        case_envs.push(ResolvedEnv { tests, init, exit });
    }

    let mut agg = DefUseAggregator::default();
    for (test, subcases) in expansions {
        let test_name = test.name.get_ref().as_str();
        let case_env = case_envs.iter().find(|env| env.tests.contains(&test_name));

        // The prefix and suffix are the same for every sub-case of the
        // test; only the test commands vary (their `$var` bindings).
        // Entering order: process, global, case, thread (FR-E-02); the
        // exit order is the reverse.
        let mut prefix = Vec::new();
        let mut suffix = Vec::new();
        for layer in [&process, &global].into_iter().flatten() {
            extend_walk_items(&mut prefix, &layer.init, test_name);
            extend_walk_items(&mut suffix, &layer.exit, test_name);
        }
        if let Some(env) = case_env {
            extend_walk_items(&mut prefix, &env.init, test_name);
            extend_walk_items(&mut suffix, &env.exit, test_name);
        }
        if let Some(l) = &thread {
            extend_walk_items(&mut prefix, &l.init, test_name);
            extend_walk_items(&mut suffix, &l.exit, test_name);
        }
        // Layer exit order is the reverse of the entry order: the loop
        // above pushed exits global-first, so flip the whole sequence.
        suffix.reverse();

        let test_cmds: Vec<(String, Location, Option<&FuncDecl>)> = test
            .cmds
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                let cmd = cmd.get_ref();
                (
                    format!("test `{test_name}` command {i}"),
                    doc.locate(cmd.opfunc.span()),
                    libs.func(cmd.opfunc.get_ref()),
                )
            })
            .collect();

        for subcase in subcases {
            agg.begin_subcase();
            let mut written = [false; PARAM_PAGE_SLOTS];
            for item in &prefix {
                apply_cmd(
                    &item.site,
                    &item.loc,
                    item.args,
                    item.func,
                    &mut written,
                    &mut agg,
                );
            }
            for (cmd, (site, loc, func)) in subcase.cmds.iter().zip(&test_cmds) {
                apply_cmd(site, loc, &cmd.args, *func, &mut written, &mut agg);
            }
            for item in &suffix {
                apply_cmd(
                    &item.site,
                    &item.loc,
                    item.args,
                    item.func,
                    &mut written,
                    &mut agg,
                );
            }
        }
    }
    agg.finish(out);
}

/// One case-level env with its commands resolved.
struct ResolvedEnv<'a> {
    /// Test names this env claims (membership from `envs[].tests`).
    tests: Vec<&'a str>,
    init: Vec<EnvCmd<'a>>,
    exit: Vec<EnvCmd<'a>>,
}

/// The init/exit commands of one name-less global-scope env, resolved.
struct EnvLayer<'a> {
    init: Vec<EnvCmd<'a>>,
    exit: Vec<EnvCmd<'a>>,
}

/// An env command with every value resolved as a literal.
struct EnvCmd<'a> {
    /// Site without test context, e.g. ``env `with_socket` init command 0``.
    site: String,
    /// Location of the `opfunc` for diagnostics.
    loc: Location,
    /// Resolved literal argument values in declared order.
    args: Vec<(String, ConcreteValue)>,
    /// The declared function, if `opfunc` resolved (validation reports
    /// unknown names separately).
    func: Option<&'a FuncDecl>,
}

/// One env command placed in a test's walk sequence.
struct WalkItem<'a> {
    /// Site including test context, e.g. ``test `t` env `e` exit command 0``.
    site: String,
    loc: Location,
    args: &'a [(String, ConcreteValue)],
    func: Option<&'a FuncDecl>,
}

/// Places an env command into a test's walk, prefixing the site with the
/// test name so findings name the context they belong to.
fn extend_walk_items<'a, 'b>(walk: &mut Vec<WalkItem<'a>>, cmds: &'b [EnvCmd<'a>], test: &str)
where
    'b: 'a,
{
    for cmd in cmds {
        walk.push(WalkItem {
            site: format!("test `{test}` {}", cmd.site),
            loc: cmd.loc.clone(),
            args: &cmd.args,
            func: cmd.func,
        });
    }
}

/// Applies one command to the written-set, reporting slot findings.
fn apply_cmd(
    site: &str,
    loc: &Location,
    args: &[(String, ConcreteValue)],
    func: Option<&FuncDecl>,
    written: &mut [bool; PARAM_PAGE_SLOTS],
    agg: &mut DefUseAggregator,
) {
    let Some(func) = func else {
        // Unknown opfunc was already reported by validation.
        return;
    };
    for (name, value) in args {
        let Some(&role) = func.slot_roles.get(name) else {
            continue;
        };
        let slot = match value {
            ConcreteValue::Int(i) => *i,
            ConcreteValue::Str(s) => {
                agg.push(Diagnostic::new(
                    codes::SLOT_NOT_INTEGER,
                    loc.clone(),
                    format!(
                        "{site}: slot parameter `{name}` needs an integer index, \
                             got string `{s}`"
                    ),
                ));
                continue;
            }
        };
        let index = match usize::try_from(slot) {
            Ok(i) if i < PARAM_PAGE_SLOTS => i,
            _ => {
                agg.push(Diagnostic::new(
                    codes::SLOT_OUT_OF_RANGE,
                    loc.clone(),
                    format!(
                        "{site}: slot index {slot} (parameter `{name}`) is out of \
                             range [0, {PARAM_PAGE_SLOTS})"
                    ),
                ));
                continue;
            }
        };
        if role.reads() && !written[index] {
            agg.push(Diagnostic::new(
                codes::SLOT_READ_BEFORE_WRITE,
                loc.clone(),
                format!("{site}: slot {slot} is read (parameter `{name}`) but never written"),
            ));
        }
        if role.writes() {
            written[index] = true;
        }
    }
}

/// Resolves the commands of one name-less global-scope env layer.
fn resolve_env_layer<'a>(
    env: Option<&Spanned<GlobalEnv>>,
    kind: &str,
    libs: &'a LibDescription,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) -> Option<EnvLayer<'a>> {
    let env = env?.get_ref();
    let scope = match &env.name {
        Some(name) => format!("{kind} env `{}`", name.get_ref()),
        None => format!("{kind} env"),
    };
    Some(EnvLayer {
        init: resolve_env_cmds(&env.init, "init", &scope, libs, doc, out),
        exit: resolve_env_cmds(&env.exit, "exit", &scope, libs, doc, out),
    })
}

/// Resolves every command of one env phase to literal values.
///
/// `$var` has no scope in env commands — they run once, outside any
/// sub-case — so variables are rejected here (FR-C-06, FR-C-08).
fn resolve_env_cmds<'a>(
    cmds: &[Spanned<CmdDef>],
    phase: &str,
    scope: &str,
    libs: &'a LibDescription,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) -> Vec<EnvCmd<'a>> {
    let mut resolved = Vec::with_capacity(cmds.len());
    for (i, cmd) in cmds.iter().enumerate() {
        let cmd = cmd.get_ref();
        let site = format!("{scope} {phase} command {i}");
        let loc = doc.locate(cmd.opfunc.span());
        let func = libs.func(cmd.opfunc.get_ref());

        let mut args = Vec::with_capacity(cmd.args.len());
        for entry in &cmd.args {
            let entry = entry.get_ref();
            let Some((name, value_text)) = entry.split_once('=') else {
                // Validation reports the malformed entry; this mirrors it
                // so a standalone call cannot pass silently.
                out.push(Diagnostic::new(
                    codes::INVALID_VALUE,
                    loc.clone(),
                    format!("{site}: argument `{entry}` is not in `name=value` form"),
                ));
                continue;
            };
            match resolve_env_value(value_text) {
                Ok(value) => args.push((name.to_string(), value)),
                Err((code, message)) => {
                    out.push(Diagnostic::new(
                        code,
                        loc.clone(),
                        format!("{site}: {message}"),
                    ));
                }
            }
        }

        // Expectations are optional in env commands, but a `$var` there is
        // just as unresolvable.
        for raw in cmd.expectations.values() {
            check_env_expectation(raw.get_ref(), &site, &loc, out);
        }

        resolved.push(EnvCmd {
            site,
            loc,
            args,
            func,
        });
    }
    resolved
}

/// Resolves one env command argument; only literals are allowed.
fn resolve_env_value(text: &str) -> Result<ConcreteValue, (&'static str, String)> {
    let parsed = parse_value(text)
        .map_err(|e| (codes::INVALID_VALUE, format!("invalid value `{text}`: {e}")))?;
    if parsed.negated {
        return Err((
            codes::INVALID_VALUE,
            "negation `!` is only valid in expectation values".to_string(),
        ));
    }
    match parsed.kind {
        ValueKind::Int(i) => Ok(ConcreteValue::Int(i)),
        ValueKind::Str(s) => Ok(ConcreteValue::Str(s)),
        ValueKind::Var(name) => Err((codes::UNRESOLVED_VAR, env_var_message(&name))),
    }
}

/// Checks an env expectation value: literals (with optional negation) are
/// fine, `$var` is not. The folded kind is irrelevant to def-use.
fn check_env_expectation(raw: &ScalarRaw, site: &str, loc: &Location, out: &mut Vec<Diagnostic>) {
    let ScalarRaw::Str(s) = raw else {
        return; // TOML integers are already concrete.
    };
    let parsed = match parse_value(s) {
        Ok(p) => p,
        Err(e) => {
            out.push(Diagnostic::new(
                codes::INVALID_VALUE,
                loc.clone(),
                format!("{site}: invalid value `{s}`: {e}"),
            ));
            return;
        }
    };
    if let ValueKind::Var(name) = parsed.kind {
        out.push(Diagnostic::new(
            codes::UNRESOLVED_VAR,
            loc.clone(),
            format!("{site}: {}", env_var_message(&name)),
        ));
    }
}

/// The shared message for `$var` in env commands.
fn env_var_message(name: &str) -> String {
    format!(
        "`${name}` cannot be used in env commands (they apply to every \
         sub-case); use a literal value"
    )
}

/// Collects def-use findings, counting affected sub-cases.
///
/// The same (code, message) usually recurs in every sub-case of a test —
/// a command's text is static — so the aggregator keeps the first
/// diagnostic and appends the number of further sub-cases instead of
/// repeating it.
#[derive(Default)]
struct DefUseAggregator {
    /// (code, message) -> index into `entries`/`counts`.
    index: HashMap<(&'static str, String), usize>,
    entries: Vec<Diagnostic>,
    counts: Vec<usize>,
    /// Keys already counted for the current sub-case.
    local: HashSet<(&'static str, String)>,
}

impl DefUseAggregator {
    /// Starts a new sub-case; identical errors within one sub-case count
    /// once.
    fn begin_subcase(&mut self) {
        self.local.clear();
    }

    /// Records one finding, or counts another affected sub-case.
    fn push(&mut self, diag: Diagnostic) {
        let key = (diag.code, diag.message.clone());
        if !self.local.insert(key.clone()) {
            return;
        }
        match self.index.get(&key) {
            Some(&i) => self.counts[i] += 1,
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push(diag);
                self.counts.push(1);
            }
        }
    }

    /// Emits the deduplicated findings into `out`.
    fn finish(self, out: &mut Vec<Diagnostic>) {
        for (mut diag, subcases) in self.entries.into_iter().zip(self.counts) {
            if subcases > 1 {
                diag.message
                    .push_str(&format!(" (also in {} more sub-cases)", subcases - 1));
            }
            out.push(diag);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::cases::CaseConfig;
    use super::super::expand::expand_test;
    use super::super::lib_desc::validate as validate_libs;
    use super::super::lib_desc::LibDescription;
    use super::super::validate::validate_cases;
    use super::*;

    /// One library with every slot shape the analysis distinguishes:
    /// write, read, read_write, and a plain non-slot parameter.
    const LIBS: &str = r#"
version = 1
[[libs]]
path = "wrapper.so"
funcs = [
  { name = "Call_ctx_new",  paras = ["out_idx"],  slot_roles = { out_idx = "write" } },
  { name = "Call_ctx_free", paras = ["in_idx"],   slot_roles = { in_idx = "read" } },
  { name = "Call_swap",     paras = ["both_idx"], slot_roles = { both_idx = "read_write" } },
  { name = "Call_mode",     paras = ["mode"] },
]
"#;

    /// Runs the full M1 analysis pipeline (validate, expand, def-use) over
    /// a case configuration against [`LIBS`].
    fn findings(cases: &str) -> Vec<Diagnostic> {
        let case_doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: cases.to_string(),
        };
        let config: CaseConfig = case_doc.parse().expect("cases config parses");
        let lib_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: LIBS.to_string(),
        };
        let libs: LibDescription = lib_doc.parse().expect("lib description parses");

        let mut out = Vec::new();
        out.extend(validate_libs(&libs, &lib_doc));
        out.extend(validate_cases(&config, &case_doc, &libs));
        let mut expansions = Vec::new();
        for test in &config.tests {
            let subcases = expand_test(test.get_ref(), &config.shared_inputs, &case_doc, &mut out);
            expansions.push((test.get_ref(), subcases));
        }
        analyze_def_use(&config, &expansions, &libs, &case_doc, &mut out);
        out
    }

    fn codes_of(diags: &[Diagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.code).collect()
    }

    #[test]
    fn write_then_read_passes__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_ctx_new",  expect_eq = 0, args = ["out_idx=3"] },
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=3"] },
]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn read_before_write_is_reported_with_slot_and_command__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=3"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(
            diags[0].message.contains("slot 3") && diags[0].message.contains("command 0"),
            "message names the slot and the command: {}",
            diags[0].message
        );
    }

    #[test]
    fn later_write_does_not_satisfy_earlier_read__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=5"] },
  { opfunc = "Call_ctx_new",  expect_eq = 0, args = ["out_idx=5"] },
]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(diags[0].message.contains("command 0"));
    }

    #[test]
    fn read_write_requires_prior_write_then_writes__F_C_10() {
        // The read side of `read_write` fails on an unwritten slot; its
        // write side then marks it, so the following plain read passes.
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_swap",     expect_eq = 0, args = ["both_idx=7"] },
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=7"] },
]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(diags[0].message.contains("command 0"));
    }

    #[test]
    fn env_init_write_satisfies_test_read__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[envs]]
name = "e"
init = [{ opfunc = "Call_ctx_new", args = ["out_idx=3"] }]
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=3"] }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn env_exit_read_sees_test_write__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[envs]]
name = "e"
exit = [{ opfunc = "Call_ctx_free", args = ["in_idx=4"] }]
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_new", expect_eq = 0, args = ["out_idx=4"] }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn env_exit_read_without_any_write_is_reported__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[envs]]
name = "e"
exit = [{ opfunc = "Call_ctx_free", args = ["in_idx=9"] }]
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_mode", expect_eq = 0, args = ["mode=1"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(
            diags[0].message.contains("env `e` exit command 0"),
            "message names the env exit command: {}",
            diags[0].message
        );
    }

    #[test]
    fn slot_out_of_range_is_reported_with_bounds__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_new", expect_eq = 0, args = ["out_idx=999"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_out_of_range"]);
        assert!(
            diags[0].message.contains("999") && diags[0].message.contains("[0, 512)"),
            "message names the index and the bounds: {}",
            diags[0].message
        );
    }

    #[test]
    fn negative_slot_index_is_out_of_range__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_new", expect_eq = 0, args = ["out_idx=-1"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_out_of_range"]);
    }

    #[test]
    fn slot_parameter_with_string_value_is_reported__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_new", expect_eq = 0, args = ["out_idx='x'"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_not_integer"]);
    }

    #[test]
    fn non_slot_parameters_do_not_track_state__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_mode", expect_eq = 7, args = ["mode=7"] }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn var_in_env_cmd_args_is_rejected_once__F_C_06() {
        // Env commands apply to every sub-case, so `$var` has no scope
        // there; the finding is env-level and not multiplied per sub-case.
        let diags = findings(
            r#"
version = 1
[[envs]]
name = "e"
init = [{ opfunc = "Call_ctx_new", args = ["out_idx=$base"] }]
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_mode", expect_eq = 0, args = ["mode=1"] }]
[[tests.inputs]]
name = "g"
args = { a = [1, 2] }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unresolved_var"]);
        assert!(
            diags[0].message.contains("$base")
                && diags[0].message.contains("env `e` init command 0")
                && diags[0].message.contains("literal"),
            "message names the variable, the site, and the remedy: {}",
            diags[0].message
        );
    }

    #[test]
    fn var_in_env_expectation_is_rejected__F_C_06() {
        let diags = findings(
            r#"
version = 1
[[envs]]
name = "e"
init = [{ opfunc = "Call_mode", args = ["mode=1"], expect_eq = "$n" }]
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_mode", expect_eq = 0, args = ["mode=2"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unresolved_var"]);
        assert!(
            diags[0].message.contains("$n") && diags[0].message.contains("env `e`"),
            "message names the variable and the env: {}",
            diags[0].message
        );
    }

    #[test]
    fn repeated_findings_across_subcases_are_counted__F_C_10() {
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=7"] }]
[[tests.inputs]]
name = "g"
args = { a = [1, 2, 3] }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(
            diags[0].message.contains("(also in 2 more sub-cases)"),
            "message counts the further sub-cases: {}",
            diags[0].message
        );
    }

    #[test]
    fn decision_q02_process_env_worker_readable__F_C_10() {
        // Q-02 / FR-E-04: process-env init runs once and the framework
        // pre-writes its slots into every worker's page, so a worker-side
        // read is satisfied — the static model must agree.
        let diags = findings(
            r#"
version = 1
[process_env]
init = [{ opfunc = "Call_ctx_new", args = ["out_idx=3"] }]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=3"] }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn decision_q06_param_page_thread_local__F_C_10() {
        // Q-06: pages are thread-local and sub-cases spread over workers,
        // so each sub-case is analyzed with a fresh written-set. Sub-case
        // i=2 writes slot 2 itself and passes; that write must NOT satisfy
        // sub-case i=1's read of slot 2.
        let diags = findings(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_ctx_new",  expect_eq = 0, args = ["out_idx=$i"] },
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=2"] },
]
[[tests.inputs]]
name = "g"
args = { i = [1, 2] }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["slot_read_before_write"]);
        assert!(
            diags[0].message.contains("slot 2") && !diags[0].message.contains("also in"),
            "exactly one sub-case is affected: {}",
            diags[0].message
        );
    }

    #[test]
    fn thread_and_global_env_layers_are_walked__F_C_10() {
        let diags = findings(
            r#"
version = 1
[env]
init = [{ opfunc = "Call_ctx_new", args = ["out_idx=1"] }]
[thread_env]
init = [{ opfunc = "Call_ctx_new", args = ["out_idx=2"] }]
[[tests]]
name = "t"
cmds = [
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=1"] },
  { opfunc = "Call_ctx_free", expect_eq = 0, args = ["in_idx=2"] },
]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }
}
