//! Cross-reference and semantic validation of a parsed case configuration
//! (requirement FR-C-08).
//!
//! Runs once both documents deserialize cleanly and before input
//! expansion: everything checked here is static, it needs no concrete
//! argument values. Findings accumulate so one `ccaller check` run reports
//! every fixable problem at once.
//!
//! `$var` references are NOT resolved here: whether a variable exists
//! depends on the expanded input group, so unresolved-variable errors are
//! produced during expansion instead (FR-C-06).

use std::collections::{BTreeMap, HashMap};

use toml::Spanned;

use crate::error::Location;

use super::cases::{CaseConfig, CmdDef, GlobalEnv, InputGroup, InputValue, RangeSpec, TestDef};
use super::diag::{codes, Diagnostic};
use super::lib_desc::{LibDescription, SUPPORTED_VERSION};
use super::source::SourceDoc;
use super::value::{parse_value, ScalarRaw, ValueKind};

/// Validates a parsed case configuration against the library description.
///
/// Everything reported here is a cross-reference or semantic rule of
/// FR-C-08 plus the input-group rules that are checkable without
/// expansion. Returns all findings; an empty vector means the
/// configuration is statically sound.
pub fn validate_cases(
    config: &CaseConfig,
    doc: &SourceDoc,
    libs: &LibDescription,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    if *config.version.get_ref() != SUPPORTED_VERSION {
        out.push(Diagnostic::new(
            codes::UNSUPPORTED_VERSION,
            doc.locate(config.version.span()),
            format!(
                "unsupported schema version {}, expected {SUPPORTED_VERSION}",
                config.version.get_ref()
            ),
        ));
    }

    // The index serves both the duplicate check and the reference checks.
    let mut test_sites: HashMap<&str, Location> = HashMap::new();
    for test in &config.tests {
        let name = test.get_ref().name.get_ref().as_str();
        if let Some(first) = test_sites.get(name) {
            out.push(Diagnostic::new(
                codes::DUPLICATE_TEST_NAME,
                doc.locate(test.get_ref().name.span()),
                format!("test name `{name}` is declared twice (first at {first})"),
            ));
        } else {
            test_sites.insert(name, doc.locate(test.get_ref().name.span()));
        }
    }

    validate_envs(config, doc, libs, &test_sites, &mut out);
    validate_concurrences(config, doc, &test_sites, &mut out);
    validate_debug_tests(config, doc, &test_sites, &mut out);
    validate_global_env(config.env.as_ref(), "global", doc, libs, &mut out);
    validate_global_env(config.thread_env.as_ref(), "thread", doc, libs, &mut out);
    validate_global_env(config.process_env.as_ref(), "process", doc, libs, &mut out);

    for test in &config.tests {
        validate_test(test, doc, libs, &config.shared_inputs, &mut out);
    }
    out
}

/// Checks `envs`: unique names, existing test references, one-env-per-test
/// membership, and the init/exit commands (assertions optional there).
fn validate_envs(
    config: &CaseConfig,
    doc: &SourceDoc,
    libs: &LibDescription,
    test_sites: &HashMap<&str, Location>,
    out: &mut Vec<Diagnostic>,
) {
    let mut env_names: HashMap<&str, Location> = HashMap::new();
    // test name -> (owning env name, reference location)
    let mut owner: HashMap<&str, (&str, Location)> = HashMap::new();

    for env in &config.envs {
        let env = env.get_ref();
        let name = env.name.get_ref().as_str();
        if let Some(first) = env_names.get(name) {
            out.push(Diagnostic::new(
                codes::DUPLICATE_ENV_NAME,
                doc.locate(env.name.span()),
                format!("env name `{name}` is declared twice (first at {first})"),
            ));
        } else {
            env_names.insert(name, doc.locate(env.name.span()));
        }

        validate_cmds(
            &env.init,
            "init",
            &format!("env `{name}`"),
            doc,
            libs,
            false,
            out,
        );
        validate_cmds(
            &env.exit,
            "exit",
            &format!("env `{name}`"),
            doc,
            libs,
            false,
            out,
        );

        for test in &env.tests {
            let test_name = test.get_ref().as_str();
            match test_sites.get(test_name) {
                None => out.push(Diagnostic::new(
                    codes::UNKNOWN_TEST_REF,
                    doc.locate(test.span()),
                    format!("env `{name}` references unknown test `{test_name}`"),
                )),
                Some(_) => {
                    let ref_loc = doc.locate(test.span());
                    if let Some((first_env, first_loc)) = owner.get(test_name) {
                        out.push(Diagnostic::new(
                            codes::DUPLICATE_ENV_MEMBERSHIP,
                            ref_loc,
                            format!(
                                "test `{test_name}` belongs to both env `{first_env}` \
                                 (at {first_loc}) and env `{name}`; at most one non-global env \
                                 is allowed"
                            ),
                        ));
                    } else {
                        owner.insert(test_name, (name, ref_loc));
                    }
                }
            }
        }
    }
}

/// Checks that every `concurrences` test reference exists.
fn validate_concurrences(
    config: &CaseConfig,
    doc: &SourceDoc,
    test_sites: &HashMap<&str, Location>,
    out: &mut Vec<Diagnostic>,
) {
    for group in &config.concurrences {
        let group = group.get_ref();
        for test in &group.tests {
            if !test_sites.contains_key(test.get_ref().as_str()) {
                out.push(Diagnostic::new(
                    codes::UNKNOWN_TEST_REF,
                    doc.locate(test.span()),
                    format!(
                        "concurrency group `{}` references unknown test `{}`",
                        group.name.get_ref(),
                        test.get_ref()
                    ),
                ));
            }
        }
    }
}

/// Checks that every `debug_test` entry names a declared test (FR-T-08).
///
/// A stale debug name would otherwise select zero tests and turn the run
/// into a confusing empty failure (decision Q-08); load time is where the
/// configuration is checked, so the typo is reported there.
fn validate_debug_tests(
    config: &CaseConfig,
    doc: &SourceDoc,
    test_sites: &HashMap<&str, Location>,
    out: &mut Vec<Diagnostic>,
) {
    for entry in &config.debug_test {
        let name = entry.get_ref();
        if !test_sites.contains_key(name.as_str()) {
            out.push(Diagnostic::new(
                codes::UNKNOWN_TEST_REF,
                doc.locate(entry.span()),
                format!("debug_test references unknown test `{name}`"),
            ));
        }
    }
}

/// Checks the init/exit commands of a name-less global-scope env.
///
/// Assertions stay optional in env commands (they are setup/teardown, not
/// behavior under test), but exclusivity is still enforced.
fn validate_global_env(
    env: Option<&Spanned<GlobalEnv>>,
    kind: &str,
    doc: &SourceDoc,
    libs: &LibDescription,
    out: &mut Vec<Diagnostic>,
) {
    let Some(env) = env else { return };
    let env = env.get_ref();
    let scope = match &env.name {
        Some(name) => format!("{kind} env `{}`", name.get_ref()),
        None => format!("{kind} env"),
    };
    validate_cmds(&env.init, "init", &scope, doc, libs, false, out);
    validate_cmds(&env.exit, "exit", &scope, doc, libs, false, out);
}

/// Checks one test: non-empty cmds, thread_num, input groups, and the
/// commands (an assertion is mandatory there, FR-C-08).
fn validate_test(
    test: &Spanned<TestDef>,
    doc: &SourceDoc,
    libs: &LibDescription,
    shared: &BTreeMap<String, Spanned<BTreeMap<String, Spanned<InputValue>>>>,
    out: &mut Vec<Diagnostic>,
) {
    let test = test.get_ref();
    let name = test.name.get_ref().as_str();
    let scope = format!("test `{name}`");

    if test.cmds.is_empty() {
        out.push(Diagnostic::new(
            codes::EMPTY_CMDS,
            doc.locate(test.name.span()),
            format!("{scope} has no commands"),
        ));
    }
    if *test.thread_num.get_ref() < 1 {
        out.push(Diagnostic::new(
            codes::INVALID_THREAD_NUM,
            doc.locate(test.thread_num.span()),
            format!(
                "{scope}: thread_num must be >= 1, got {}",
                test.thread_num.get_ref()
            ),
        ));
    }

    let mut group_names: HashMap<&str, Location> = HashMap::new();
    for group in &test.inputs {
        let group_name = group.get_ref().name.get_ref().as_str();
        if let Some(first) = group_names.get(group_name) {
            out.push(Diagnostic::new(
                codes::DUPLICATE_INPUT_NAME,
                doc.locate(group.get_ref().name.span()),
                format!(
                    "{scope}: input group name `{group_name}` is declared twice \
                     (first at {first})"
                ),
            ));
        } else {
            group_names.insert(group_name, doc.locate(group.get_ref().name.span()));
        }
        validate_input_group(name, group, shared, doc, out);
    }

    validate_cmds(&test.cmds, "command", &scope, doc, libs, true, out);
}

/// Validates a list of commands under one scope label.
///
/// `require_assertion` distinguishes test commands (mandatory expectation,
/// FR-C-08) from env init/exit commands (optional).
fn validate_cmds(
    cmds: &[Spanned<CmdDef>],
    phase: &str,
    scope: &str,
    doc: &SourceDoc,
    libs: &LibDescription,
    require_assertion: bool,
    out: &mut Vec<Diagnostic>,
) {
    for (i, cmd) in cmds.iter().enumerate() {
        let cmd_scope = format!("{scope} {phase} {i}");
        out.extend(validate_cmd(cmd, doc, libs, &cmd_scope, require_assertion));
    }
}

/// Checks one command: assertion presence/exclusivity, `opfunc` existence,
/// and `args` names against the declared `paras`.
fn validate_cmd(
    cmd: &Spanned<CmdDef>,
    doc: &SourceDoc,
    libs: &LibDescription,
    scope: &str,
    require_assertion: bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let cmd = cmd.get_ref();
    let opfunc = cmd.opfunc.get_ref();
    let loc = doc.locate(cmd.opfunc.span());

    // An unregistered `expect_*` name and every non-`expect_` stray field
    // are unknown fields (FR-C-09); reporting here keeps the code and
    // location contract intact while the assertion namespace stays open.
    for (key, value) in &cmd.expectations {
        if crate::assertion::lookup(key).is_none() {
            out.push(Diagnostic::new(
                codes::UNKNOWN_FIELD,
                doc.locate(value.span()),
                format!("unknown field `{key}`"),
            ));
        }
    }
    for (key, span) in &cmd.stray_fields {
        out.push(Diagnostic::new(
            codes::UNKNOWN_FIELD,
            doc.locate(span.clone()),
            format!("unknown field `{key}`"),
        ));
    }

    let assertions = cmd.registered_assertions();
    if assertions.len() > 1 {
        let names: Vec<&'static str> = assertions
            .iter()
            .map(|(assertion, _)| assertion.field_name())
            .collect();
        out.push(Diagnostic::new(
            codes::ASSERTION_CONFLICT,
            loc.clone(),
            format!(
                "{scope}: {} are mutually exclusive",
                crate::assertion::backticked(&names)
            ),
        ));
    } else if assertions.is_empty() && require_assertion {
        let expected: Vec<String> = crate::assertion::field_names()
            .map(|name| format!("`{name}`"))
            .collect();
        out.push(Diagnostic::new(
            codes::ASSERTION_MISSING,
            loc.clone(),
            format!(
                "{scope}: `{opfunc}` has no assertion (expected one of {})",
                expected.join(", ")
            ),
        ));
    }

    let Some(func) = libs.func(opfunc) else {
        // Verification plan 3.1 (opfunc-not-found): the message names the
        // function and the library paths it was searched in.
        let paths: Vec<&str> = libs
            .libs
            .iter()
            .map(|lib| lib.get_ref().path.get_ref().as_str())
            .collect();
        out.push(Diagnostic::new(
            codes::UNKNOWN_OPFUNC,
            loc,
            format!(
                "{scope}: unknown function `{opfunc}` (not declared in any \
                 of: {})",
                paths.join(", ")
            ),
        ));
        return out;
    };

    let expected: Vec<&str> = func.paras.iter().map(|p| p.get_ref().as_str()).collect();
    let mut actual: Vec<&str> = Vec::with_capacity(cmd.args.len());
    let mut malformed = false;
    for arg in &cmd.args {
        let entry = arg.get_ref();
        match entry.split_once('=') {
            Some((arg_name, _)) if !arg_name.is_empty() => actual.push(arg_name),
            _ => {
                malformed = true;
                out.push(Diagnostic::new(
                    codes::ARGS_MISMATCH,
                    doc.locate(arg.span()),
                    format!("{scope}: argument `{entry}` is not in `name=value` form"),
                ));
            }
        }
    }
    // Skip the comparison when entries were malformed: the mismatch would
    // just repeat what the per-entry diagnostics already said.
    if !malformed && actual != expected {
        out.push(Diagnostic::new(
            codes::ARGS_MISMATCH,
            doc.locate(cmd.opfunc.span()),
            format!(
                "{scope}: args of `{opfunc}` do not match paras: expected {}, got {} \
                 (name, count, and order must match)",
                bracketed(&expected),
                bracketed(&actual),
            ),
        ));
    }
    out
}

/// Checks one input group: `refs` must be `shared_inputs` keys and range
/// values must be well-formed (FR-C-08, FR-C-06).
fn validate_input_group(
    test: &str,
    group: &Spanned<InputGroup>,
    shared: &BTreeMap<String, Spanned<BTreeMap<String, Spanned<InputValue>>>>,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) {
    let group = group.get_ref();
    let group_name = group.name.get_ref().as_str();

    for r in &group.refs {
        let key = r.get_ref();
        if !shared.contains_key(key) {
            out.push(Diagnostic::new(
                codes::UNKNOWN_SHARED_INPUT,
                doc.locate(r.span()),
                format!(
                    "test `{test}` input `{group_name}`: refs entry `{key}` \
                     is not a shared_inputs key"
                ),
            ));
        }
    }

    for (param, value) in &group.args {
        if let InputValue::Range(spec) = value.get_ref() {
            validate_range(test, group_name, param, spec, value.span(), doc, out);
        }
    }
}

/// Checks a closed range: bounds must be integers or `$var` references,
/// `step > 0`, and `start <= end` whenever both sides are known.
///
/// Bounds given as `$var` are left to expansion, which resolves them and
/// re-checks the invariants on the concrete values.
fn validate_range(
    test: &str,
    group: &str,
    param: &str,
    spec: &RangeSpec,
    value_span: std::ops::Range<usize>,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) {
    let start = parse_bound(&spec.start);
    let end = parse_bound(&spec.end);
    let step = parse_bound(&spec.step);
    let loc = doc.locate(value_span);

    for (which, bound) in [("start", &start), ("end", &end), ("step", &step)] {
        if let Bound::Invalid(reason) = bound {
            out.push(Diagnostic::new(
                codes::INVALID_RANGE,
                loc.clone(),
                format!("test `{test}` input `{group}`: range `{param}` {which} {reason}"),
            ));
        }
    }
    if let (Bound::Int(s), Bound::Int(e)) = (&start, &end) {
        if s > e {
            out.push(Diagnostic::new(
                codes::INVALID_RANGE,
                loc.clone(),
                format!("test `{test}` input `{group}`: range `{param}` has start {s} > end {e}"),
            ));
        }
    }
    if let Bound::Int(st) = &step {
        if *st <= 0 {
            out.push(Diagnostic::new(
                codes::INVALID_RANGE,
                loc,
                format!(
                    "test `{test}` input `{group}`: range `{param}` has step {st}, must be > 0"
                ),
            ));
        }
    }
}

/// A range bound after parsing: a known integer, a `$var` reference left
/// to expansion, or a reason why it cannot be a bound at all.
enum Bound {
    /// A concrete integer literal.
    Int(i64),
    /// A `$var` reference; only expansion knows its value.
    Var,
    /// Not usable as a range bound; carries the reason.
    Invalid(String),
}

fn parse_bound(raw: &ScalarRaw) -> Bound {
    match raw {
        ScalarRaw::Int(i) => Bound::Int(*i),
        ScalarRaw::Str(s) => match parse_value(s) {
            Err(e) => Bound::Invalid(format!("bound `{s}` is invalid: {e}")),
            Ok(parsed) => match (parsed.kind, parsed.negated) {
                (ValueKind::Int(i), false) => Bound::Int(i),
                (ValueKind::Var(_), false) => Bound::Var,
                _ => Bound::Invalid(format!(
                    "bound `{s}` must be an integer or a `$var` reference"
                )),
            },
        },
    }
}

/// Formats `[a, b]` for messages; `Debug` on `&str` would add quotes.
fn bracketed(names: &[&str]) -> String {
    format!("[{}]", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Library fixture: multi-param, single-param, and zero-param functions,
    /// the shapes the args checks need.
    const LIBS_TEXT: &str = r#"
version = 1
[[libs]]
path = "libfake.so"
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"] },
  { name = "Call_read32", paras = ["addr_idx"] },
  { name = "Call_ping", paras = [] },
]
"#;

    fn check(text: &str) -> Vec<Diagnostic> {
        let libs_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: LIBS_TEXT.to_string(),
        };
        let libs: LibDescription = libs_doc.parse().unwrap();
        let doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: text.to_string(),
        };
        let config: CaseConfig = doc.parse().unwrap();
        validate_cases(&config, &doc, &libs)
    }

    fn codes_of(diags: &[Diagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.code).collect()
    }

    #[test]
    fn valid_config_validates_clean__F_C_08() {
        // Env commands without assertions are legal (they are setup); test
        // commands carry one expectation each.
        let diags = check(
            r#"
version = 1
[env]
init = [{ opfunc = "Call_malloc", args = ["len=100", "mem_idx=1"] }]
exit = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[envs]]
name = "e1"
init = [{ opfunc = "Call_ping", expect_eq = 0 }]
exit = []
tests = ["t1"]
[[tests]]
name = "t1"
thread_num = 2
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0, args = ["len=100", "mem_idx=2"] },
  { opfunc = "Call_read32", expect_ne = 5, args = ["addr_idx=2"] },
]
[[tests.inputs]]
name = "g1"
args = { a = { start = 0, end = 8, step = 4 } }
[[tests]]
name = "t2"
cmds = [{ opfunc = "Call_ping", expect_eq = 1 }]
[[concurrences]]
name = "cg"
tests = ["t2"]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn duplicate_test_name_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["duplicate_test_name"]);
        assert!(diags[0].message.contains("first at cases.toml"));
    }

    #[test]
    fn duplicate_env_name_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[envs]]
name = "e"
init = []
tests = []
[[envs]]
name = "e"
init = []
tests = []
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["duplicate_env_name"]);
    }

    #[test]
    fn env_reference_to_unknown_test_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[envs]]
name = "e"
init = []
tests = ["ghost"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_test_ref"]);
        assert!(diags[0].message.contains("ghost"));
    }

    #[test]
    fn concurrency_reference_to_unknown_test_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[concurrences]]
name = "cg"
tests = ["ghost"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_test_ref"]);
        assert!(diags[0].message.contains("concurrency group `cg`"));
    }

    #[test]
    fn debug_test_unknown_name_is_reported__F_T_08() {
        // A stale debug name would silently select zero tests; load time
        // is where the typo is caught, with the entry's own location.
        let diags = check(
            r#"
version = 1
debug_test = ["ghost"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_test_ref"]);
        assert!(diags[0].message.contains("debug_test"));
        assert!(diags[0].message.contains("ghost"));
        assert!(diags[0].location.line >= 1);
    }

    #[test]
    fn debug_test_known_names_validate_clean__F_T_08() {
        let diags = check(
            r#"
version = 1
debug_test = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn test_in_two_envs_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[envs]]
name = "e1"
init = []
tests = ["t"]
[[envs]]
name = "e2"
init = []
tests = ["t"]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["duplicate_env_membership"]);
        assert!(
            diags[0].message.contains("`e1`") && diags[0].message.contains("`e2`"),
            "message names both envs: {}",
            diags[0].message
        );
    }

    #[test]
    fn empty_cmds_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = []
"#,
        );
        assert_eq!(codes_of(&diags), vec!["empty_cmds"]);
    }

    #[test]
    fn thread_num_zero_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
thread_num = 0
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_thread_num"]);
    }

    #[test]
    fn test_cmd_requires_assertion_env_cmd_does_not__F_C_08() {
        let diags = check(
            r#"
version = 1
[env]
init = [{ opfunc = "Call_ping" }]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping" }]
"#,
        );
        // Exactly one finding: the env command without an assertion is
        // legal, the test command is not.
        assert_eq!(codes_of(&diags), vec!["assertion_missing"]);
    }

    #[test]
    fn expect_eq_and_expect_ne_are_exclusive__F_C_08() {
        let diags = check(
            r#"
version = 1
[env]
init = [{ opfunc = "Call_ping", expect_eq = 0, expect_ne = 1 }]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0, expect_ne = 1 }]
"#,
        );
        // Exclusivity applies to env commands too (the missing-assertion
        // rule does not, the conflict rule does).
        assert_eq!(
            codes_of(&diags),
            vec!["assertion_conflict", "assertion_conflict"]
        );
    }

    #[test]
    fn unknown_cmd_field_is_rejected_with_location__F_C_09() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0, stray = 1 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_field"]);
        assert!(diags[0].message.contains("unknown field `stray`"));
        assert!(diags[0].location.line >= 1);
    }

    #[test]
    fn unknown_expect_field_is_rejected_as_unknown_field__F_C_09() {
        // An unregistered `expect_*` name is an unknown field, not a
        // silently ignored assertion (FR-C-09, FR-V-02).
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0, expect_gt = 5 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_field"]);
        assert!(diags[0].message.contains("unknown field `expect_gt`"));
    }

    #[test]
    fn registered_comparison_assertion_counts_as_assertion__F_V_03() {
        // `expect_ge` satisfies the "at least one assertion" rule, so no
        // assertion_missing is reported.
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_ge = 0 }]
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn eq_and_ge_conflict_is_reported__F_V_03() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0, expect_ge = 1 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["assertion_conflict"]);
        assert!(diags[0].message.contains("mutually exclusive"));
    }

    #[test]
    fn unknown_opfunc_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_nope", expect_eq = 0 }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_opfunc"]);
        // Verification plan 3.1 (opfunc-not-found): the message names the
        // function and the library paths it was searched in.
        assert!(
            diags[0].message.contains("Call_nope") && diags[0].message.contains("libfake.so"),
            "message names the function and the library path: {}",
            diags[0].message
        );
    }

    #[test]
    fn args_order_mismatch_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_malloc", expect_eq = 0, args = ["mem_idx=2", "len=100"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["args_mismatch"]);
        assert!(
            diags[0]
                .message
                .contains("expected [len, mem_idx], got [mem_idx, len]"),
            "message lists expected vs actual: {}",
            diags[0].message
        );
    }

    #[test]
    fn args_count_mismatch_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_malloc", expect_eq = 0, args = ["len=100"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["args_mismatch"]);
    }

    #[test]
    fn arg_without_equal_sign_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0, args = ["100"] }]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["args_mismatch"]);
        assert!(diags[0].message.contains("name=value"));
    }

    #[test]
    fn range_step_zero_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = 0, end = 8, step = 0 } }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_range"]);
        assert!(diags[0].message.contains("step"));
    }

    #[test]
    fn range_start_after_end_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = 4, end = 2, step = 1 } }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_range"]);
        assert!(diags[0].message.contains("start 4 > end 2"));
    }

    #[test]
    fn range_bound_must_be_integer_or_var__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = "'x'", end = 8, step = 4 } }
"#,
        );
        assert_eq!(codes_of(&diags), vec!["invalid_range"]);
        assert!(diags[0].message.contains("start bound"));
    }

    #[test]
    fn range_bound_with_var_defers_to_expansion__F_C_08() {
        // `$var` bounds are statically legal; whether they resolve is an
        // expansion-time question (FR-C-06), not a validation one.
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = { a = { start = "$base", end = 8, step = 4 } }
"#,
        );
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    }

    #[test]
    fn duplicate_input_group_name_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
args = {}
[[tests.inputs]]
name = "g"
args = {}
"#,
        );
        assert_eq!(codes_of(&diags), vec!["duplicate_input_name"]);
    }

    #[test]
    fn ref_to_unknown_shared_input_is_reported__F_C_08() {
        let diags = check(
            r#"
version = 1
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
[[tests.inputs]]
name = "g"
refs = ["common"]
"#,
        );
        assert_eq!(codes_of(&diags), vec!["unknown_shared_input"]);
        assert!(diags[0].message.contains("common"));
    }

    #[test]
    fn multiple_problems_are_reported_together__F_C_08() {
        // The load-time contract is "report everything once"; three
        // independent problems yield three findings in one run.
        let diags = check(
            r#"
version = 2
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_nope", expect_eq = 0 }]
[[tests]]
name = "t"
cmds = [{ opfunc = "Call_ping", expect_eq = 0 }]
"#,
        );
        assert_eq!(
            codes_of(&diags),
            vec![
                "unsupported_version",
                "duplicate_test_name",
                "unknown_opfunc"
            ]
        );
    }
}
