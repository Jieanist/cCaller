//! The `migrate` pipeline: hitest-style configuration → cCaller
//! configuration (developer tooling).
//!
//! [`migrate`] parses two hitest-style documents (a library description
//! and a case configuration) with a **loose** model — unknown fields are
//! tolerated, because hitest configs predate cCaller's strict
//! `deny_unknown_fields` schemas — and re-renders them as cCaller
//! documents. The field-level mapping is the one pinned by
//! `examples/migrate/README.md`:
//!
//! - both documents gain `version = 1`;
//! - `debug_test` (single string) becomes a one-element list;
//! - a hitest negated assertion `expect_eq = "!X"` folds into
//!   `expect_ne = X` (and `expect_ne = "!X"` into `expect_eq = X`);
//! - input groups without a name — and groups carrying hitest's
//!   auto-name `default1`/`default2`/… — get the stable, position-based
//!   name `ipt1`, `ipt2`, …;
//! - a `Range` without `step` gains an explicit `step = 1`;
//! - string scalars that parse as integers become integer literals
//!   (`"888"` → `888`); everything else stays a string, carrying
//!   cCaller's value grammar (`$var`, `'quoted'`, `0x…`);
//! - group-level `should_panic` / `break_if_fail` map directly onto the
//!   InputGroup overrides;
//! - an `envs[]` entry with `tests = []` becomes the global `[env]`
//!   table; named entries stay `[[envs]]`;
//! - `concurrences` re-renders as an array of tables;
//! - `thread_num` (i64) is rejected below 1 instead of truncating.
//!
//! `slot_roles` — the one thing hitest never records — is inferred from
//! the wrapper source when given: `GET_INPUT_IDX`/`GET_INPUT_IDX_NZ`
//! mark the addressed parameter `read`, `SET_OUTPUT_IDX` marks it
//! `write`, both together `read_write`, and `GET_VALUE` (a plain value
//! parameter) marks nothing. Without a wrapper the functions migrate
//! without roles and a warning says so.
//!
//! What migration cannot do is reported as warnings, never errors:
//! missing `slot_roles`, hitest `ref_inputs` (no cCaller equivalent),
//! and — printed by the CLI — the wrapper-side recompile requirements
//! (signed `params`, the `CCaller_abi_version` handshake, and failure
//! codes that must converge into `[-127, -1]`).
//!
//! **Output purity**: the generated documents are indistinguishable
//! from hand-written cCaller configuration — like `gen`, migration
//! emits bare TOML with zero comments, and no hitest artifact (the
//! word itself, `ref_inputs`, negated assertions, `default{N}` group
//! names, string-typed shared inputs, single-name `debug_test`)
//! survives into them. hitest may be mentioned in diagnostics on
//! stderr — the tool's interface — never in the artifacts.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use super::fmt::line_column_of;
use super::gen::{classify_lines, splice_lines, strip_comments};
use super::value::ScalarRaw;

/// Which document a migration error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateSource {
    /// The hitest-style library description.
    Libs,
    /// The hitest-style case configuration.
    Cases,
    /// A generated document failed cCaller's own parse (an internal
    /// consistency check; unreachable in practice).
    Output,
}

impl fmt::Display for MigrateSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MigrateSource::Libs => "libs.toml",
            MigrateSource::Cases => "cases.toml",
            MigrateSource::Output => "output",
        })
    }
}

/// One migration failure: a message with its 1-based location and the
/// document it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateError {
    /// The document the error refers to.
    pub source: MigrateSource,
    /// The failure message, without location decoration.
    pub message: String,
    /// 1-based line of the offending construct.
    pub line: usize,
    /// 1-based column of the offending construct.
    pub column: usize,
}

impl MigrateError {
    /// Builds an error located at the start of the document.
    fn at(source: MigrateSource, message: String) -> Self {
        Self {
            source,
            message,
            line: 1,
            column: 1,
        }
    }
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.source, self.line, self.column, self.message
        )
    }
}

impl std::error::Error for MigrateError {}

/// The outcome of a successful migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutput {
    /// The rendered cCaller library description.
    pub libs_toml: String,
    /// The rendered cCaller case configuration.
    pub cases_toml: String,
    /// Number of tests carried over.
    pub test_count: usize,
    /// Number of functions carried over.
    pub func_count: usize,
    /// Non-fatal advisories, in emission order.
    pub warnings: Vec<String>,
}

// ---- hitest-side models (loose on purpose: no deny_unknown_fields) ----

/// A hitest library description.
#[derive(Debug, Deserialize)]
struct HitestLibs {
    #[serde(default)]
    libs: Vec<HitestLib>,
}

/// One hitest library.
#[derive(Debug, Deserialize)]
struct HitestLib {
    path: String,
    #[serde(default)]
    funcs: Vec<HitestFunc>,
}

/// One hitest function declaration.
#[derive(Debug, Deserialize)]
struct HitestFunc {
    name: String,
    #[serde(default)]
    paras: Vec<String>,
}

/// A hitest case configuration.
#[derive(Debug, Deserialize)]
struct HitestCases {
    #[serde(default)]
    debug_test: Option<HitestDebug>,
    #[serde(default)]
    default_serial: Option<bool>,
    #[serde(default)]
    concurrences: Vec<HitestConcurrence>,
    #[serde(default)]
    shared_inputs: BTreeMap<String, BTreeMap<String, HitestInputValue>>,
    #[serde(default)]
    envs: Vec<HitestEnv>,
    #[serde(default)]
    thread_env: Option<HitestGlobalEnv>,
    #[serde(default)]
    process_env: Option<HitestGlobalEnv>,
    #[serde(default)]
    tests: Vec<HitestTest>,
}

/// hitest `debug_test`: one name or a list of names.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HitestDebug {
    /// The documented hitest form: a single string.
    One(String),
    /// Tolerated: an explicit list.
    Many(Vec<String>),
}

/// One hitest concurrency group.
#[derive(Debug, Deserialize)]
struct HitestConcurrence {
    name: String,
    #[serde(default)]
    tests: Vec<String>,
}

/// A hitest env as it appears in `envs[]`.
#[derive(Debug, Deserialize)]
struct HitestEnv {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    init: Vec<HitestCmd>,
    #[serde(default)]
    exit: Vec<HitestCmd>,
    #[serde(default)]
    tests: Vec<String>,
}

/// A hitest `thread_env` / `process_env` table.
#[derive(Debug, Deserialize)]
struct HitestGlobalEnv {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    init: Vec<HitestCmd>,
    #[serde(default)]
    exit: Vec<HitestCmd>,
}

/// A hitest test.
#[derive(Debug, Deserialize)]
struct HitestTest {
    name: String,
    #[serde(default)]
    cmds: Vec<HitestCmd>,
    #[serde(default)]
    thread_num: Option<i64>,
    #[serde(default)]
    should_panic: Option<bool>,
    #[serde(default)]
    break_if_fail: Option<bool>,
    #[serde(default)]
    serial: Option<bool>,
    #[serde(default)]
    inputs: Vec<HitestGroup>,
    /// hitest-only; present means a warning, the value itself is not
    /// representable.
    #[serde(default)]
    ref_inputs: Option<serde::de::IgnoredAny>,
}

/// A hitest input group.
#[derive(Debug, Deserialize)]
struct HitestGroup {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    refs: Vec<String>,
    #[serde(default)]
    args: BTreeMap<String, HitestInputValue>,
    #[serde(default)]
    should_panic: Option<bool>,
    #[serde(default)]
    break_if_fail: Option<bool>,
}

/// A hitest command with its assertion.
#[derive(Debug, Deserialize)]
struct HitestCmd {
    opfunc: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    perf: Option<bool>,
    #[serde(default)]
    expect_eq: Option<ScalarRaw>,
    #[serde(default)]
    expect_ne: Option<ScalarRaw>,
}

/// A hitest input value: a scalar, a list, or a closed range.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HitestInputValue {
    /// `{ start, end, step? }` — tried first so tables never fall
    /// through to the scalar arm.
    Range {
        start: i64,
        end: i64,
        step: Option<i64>,
    },
    /// An explicit list.
    List(Vec<ScalarRaw>),
    /// Exactly one value.
    Scalar(ScalarRaw),
}

// ---- entry point ----

/// Migrates hitest-style documents to cCaller documents.
///
/// `wrapper_text`, when given, drives the `slot_roles` inference (see
/// the module docs); without it the functions migrate without roles and
/// a warning is emitted.
///
/// # Errors
/// Returns [`MigrateError`] for input that cannot be migrated
/// mechanically: TOML that does not parse, both `expect_eq` and
/// `expect_ne` on one command, an empty negated assertion, `thread_num`
/// below 1, a `Range` step below 1, a `Range` in `shared_inputs`, more
/// than one global env, or a generated document that fails cCaller's
/// own parse (the internal self-check).
pub fn migrate(
    libs_text: &str,
    cases_text: &str,
    wrapper_text: Option<&str>,
) -> Result<MigrationOutput, MigrateError> {
    let libs: HitestLibs = parse_hitest(libs_text, MigrateSource::Libs)?;
    let cases: HitestCases = parse_hitest(cases_text, MigrateSource::Cases)?;

    let mut warnings = Vec::new();
    let roles = wrapper_text.map(scan_wrapper);
    if roles.is_none() {
        warnings.push(
            "slot_roles cannot be inferred without --wrapper; fill them in by hand (FR-C-10)"
                .to_owned(),
        );
    }
    for test in &cases.tests {
        if test.ref_inputs.is_some() {
            warnings.push(format!(
                "test `{}` uses ref_inputs, which has no cCaller equivalent; express it \
                 through env layers instead",
                test.name
            ));
        }
    }

    let libs_toml = render_libs(&libs, roles.as_ref());
    let (cases_toml, test_count) = render_cases(&cases)?;

    // Self-check: what we just rendered must load as cCaller config.
    check_output::<super::lib_desc::LibDescription>(&libs_toml)?;
    check_output::<super::cases::CaseConfig>(&cases_toml)?;

    let func_count = libs.libs.iter().map(|lib| lib.funcs.len()).sum();
    Ok(MigrationOutput {
        libs_toml,
        cases_toml,
        test_count,
        func_count,
        warnings,
    })
}

/// Parses one hitest document, mapping parse failures to locations.
fn parse_hitest<'a, T: Deserialize<'a>>(
    text: &'a str,
    source: MigrateSource,
) -> Result<T, MigrateError> {
    toml::from_str(text).map_err(|error| {
        let message = error.message().to_string();
        let (line, column) = error
            .span()
            .map(|span| line_column_of(text, span.start))
            .unwrap_or((1, 1));
        MigrateError {
            source,
            message,
            line,
            column,
        }
    })
}

/// The internal self-check: a rendered document must parse as the
/// cCaller model it claims to be.
fn check_output<T: serde::de::DeserializeOwned>(text: &str) -> Result<(), MigrateError> {
    let doc = super::source::SourceDoc {
        path: "output".to_owned(),
        text: text.to_owned(),
    };
    doc.parse::<T>()
        .map(|_| ())
        .map_err(|diagnostic| MigrateError {
            source: MigrateSource::Output,
            message: format!(
                "generated document is not valid cCaller TOML: {}",
                diagnostic.message
            ),
            line: diagnostic.location.line,
            column: diagnostic.location.column,
        })
}

// ---- scalar and assertion mapping ----

/// Integer-ifies a string scalar: `"888"` becomes the literal `888`,
/// anything else stays a string (carrying cCaller's value grammar).
fn map_scalar(value: &ScalarRaw) -> ScalarRaw {
    match value {
        ScalarRaw::Int(number) => ScalarRaw::Int(*number),
        ScalarRaw::Str(text) => match text.parse::<i64>() {
            Ok(number) => ScalarRaw::Int(number),
            Err(_) => ScalarRaw::Str(text.clone()),
        },
    }
}

/// Maps one hitest command's assertion to the cCaller field name and
/// value, folding hitest's negated spelling.
fn map_assertion(cmd: &HitestCmd) -> Result<Option<(String, ScalarRaw)>, MigrateError> {
    let folded =
        |field: &str, value: &ScalarRaw| -> Result<Option<(String, ScalarRaw)>, MigrateError> {
            let (name, value) = match value {
                ScalarRaw::Str(text) if text.starts_with('!') => {
                    let stripped = &text[1..];
                    if stripped.is_empty() {
                        return Err(MigrateError::at(
                            MigrateSource::Cases,
                            format!("empty negated value in `{field}`"),
                        ));
                    }
                    let flipped = if field == "expect_eq" {
                        "expect_ne"
                    } else {
                        "expect_eq"
                    };
                    let value = match stripped.parse::<i64>() {
                        Ok(number) => ScalarRaw::Int(number),
                        Err(_) => ScalarRaw::Str(stripped.to_owned()),
                    };
                    (flipped.to_owned(), value)
                }
                other => (field.to_owned(), other.clone()),
            };
            Ok(Some((name, value)))
        };
    match (&cmd.expect_eq, &cmd.expect_ne) {
        (Some(_), Some(_)) => Err(MigrateError::at(
            MigrateSource::Cases,
            "a command carries both expect_eq and expect_ne".to_owned(),
        )),
        (Some(value), None) => folded("expect_eq", value),
        (None, Some(value)) => folded("expect_ne", value),
        (None, None) => Ok(None),
    }
}

// ---- rendering ----

/// Renders a cCaller string literal with minimal escaping.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Renders a table key: bare when it is a legal bare TOML key, quoted
/// otherwise.
fn render_key(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if bare {
        name.to_owned()
    } else {
        quote(name)
    }
}

/// Whether a group name is hitest's auto-generated `default1/default2…`
/// (README §7): such names are hitest artifacts, not choices, and fold
/// into the position-based `ipt{N}` scheme so the generated document
/// stays pure cCaller.
fn is_hitest_default_name(name: &str) -> bool {
    let digits = name.strip_prefix("default").unwrap_or("");
    !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
}

/// Renders a scalar: integers bare, strings quoted.
fn render_scalar(value: &ScalarRaw) -> String {
    match value {
        ScalarRaw::Int(number) => number.to_string(),
        ScalarRaw::Str(text) => quote(text),
    }
}

/// Renders one mapped input value (always carries an explicit step for
/// ranges).
fn render_input_value(value: &MappedValue) -> String {
    match value {
        MappedValue::Scalar(scalar) => render_scalar(scalar),
        MappedValue::List(items) => {
            let parts: Vec<String> = items.iter().map(render_scalar).collect();
            format!("[{}]", parts.join(", "))
        }
        MappedValue::Range { start, end, step } => {
            format!("{{ start = {start}, end = {end}, step = {step} }}")
        }
    }
}

/// A hitest input value after mapping.
enum MappedValue {
    /// One value.
    Scalar(ScalarRaw),
    /// A list of values.
    List(Vec<ScalarRaw>),
    /// A closed range with its (explicit) step.
    Range {
        /// First value.
        start: i64,
        /// Last value, inclusive.
        end: i64,
        /// Stride, always `> 0`.
        step: i64,
    },
}

/// Maps one input value: integer-ifies scalars, defaults the range step
/// to 1.
fn map_input_value(value: &HitestInputValue) -> Result<MappedValue, MigrateError> {
    match value {
        HitestInputValue::Scalar(scalar) => Ok(MappedValue::Scalar(map_scalar(scalar))),
        HitestInputValue::List(items) => {
            Ok(MappedValue::List(items.iter().map(map_scalar).collect()))
        }
        HitestInputValue::Range { start, end, step } => {
            let step = step.unwrap_or(1);
            if step <= 0 {
                return Err(MigrateError::at(
                    MigrateSource::Cases,
                    format!("Range step {step} is not positive; cCaller requires step > 0"),
                ));
            }
            Ok(MappedValue::Range {
                start: *start,
                end: *end,
                step,
            })
        }
    }
}

/// Renders one command as a cCaller inline table.
fn render_cmd(cmd: &HitestCmd) -> Result<String, MigrateError> {
    let mut out = format!("{{ opfunc = {}", quote(&cmd.opfunc));
    if let Some((field, value)) = map_assertion(cmd)? {
        out.push_str(&format!(", {field} = {}", render_scalar(&value)));
    }
    if cmd.perf == Some(true) {
        out.push_str(", perf = true");
    }
    if !cmd.args.is_empty() {
        let args: Vec<String> = cmd.args.iter().map(|arg| quote(arg)).collect();
        out.push_str(&format!(", args = [{}]", args.join(", ")));
    }
    out.push_str(" }");
    Ok(out)
}

/// Renders a command list (env init/exit or test cmds).
fn render_cmds(cmds: &[HitestCmd], indent: &str) -> Result<String, MigrateError> {
    let mut out = String::new();
    for cmd in cmds {
        out.push_str(indent);
        out.push_str(&render_cmd(cmd)?);
        out.push_str(",\n");
    }
    Ok(out)
}

/// Renders the library description, joining the inferred roles.
fn render_libs(libs: &HitestLibs, roles: Option<&WrapperRoles>) -> String {
    let mut out = String::from("version = 1\n");
    for lib in &libs.libs {
        out.push_str(&format!("\n[[libs]]\npath = {}\n", quote(&lib.path)));
        if lib.funcs.is_empty() {
            continue;
        }
        out.push_str("\nfuncs = [\n");
        for func in &lib.funcs {
            out.push_str(&format!(
                "  {{ name = {}, paras = [{}]",
                quote(&func.name),
                func.paras
                    .iter()
                    .map(|para| quote(para))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let inferred = roles.and_then(|roles| roles.roles_of(&func.name, &func.paras));
            if let Some(roles) = inferred {
                let parts: Vec<String> = roles
                    .iter()
                    .map(|(param, role)| format!("{param} = {}", quote(role)))
                    .collect();
                out.push_str(&format!(", slot_roles = {{ {} }}", parts.join(", ")));
            }
            out.push_str(" },\n");
        }
        out.push_str("]\n");
    }
    out
}

/// Renders the case configuration; returns the text and test count.
fn render_cases(cases: &HitestCases) -> Result<(String, usize), MigrateError> {
    let mut out = String::from("version = 1\n");

    if let Some(debug) = &cases.debug_test {
        let names: Vec<String> = match debug {
            HitestDebug::One(name) => vec![quote(name)],
            HitestDebug::Many(names) => names.iter().map(|name| quote(name)).collect(),
        };
        out.push_str(&format!("\ndebug_test = [{}]\n", names.join(", ")));
    }
    if let Some(serial) = cases.default_serial {
        out.push_str(&format!("\ndefault_serial = {serial}\n"));
    }

    for (group, params) in &cases.shared_inputs {
        out.push_str(&format!("\n[shared_inputs.{}]\n", render_key(group)));
        for (param, value) in params {
            let mapped = match value {
                HitestInputValue::Scalar(scalar) => MappedValue::List(vec![map_scalar(scalar)]),
                HitestInputValue::List(items) => {
                    MappedValue::List(items.iter().map(map_scalar).collect())
                }
                HitestInputValue::Range { .. } => {
                    return Err(MigrateError::at(
                        MigrateSource::Cases,
                        format!(
                            "shared_inputs.{group}.{param} is a Range; cCaller shared values \
                             must be literals — convert it to an explicit list"
                        ),
                    ));
                }
            };
            out.push_str(&format!(
                "{} = {}\n",
                render_key(param),
                render_input_value(&mapped)
            ));
        }
    }

    // Split the envs into one global [env] and the named [[envs]].
    let mut global_env: Option<&HitestEnv> = None;
    let mut named_envs: Vec<&HitestEnv> = Vec::new();
    for env in &cases.envs {
        if env.tests.is_empty() {
            if global_env.is_some() {
                return Err(MigrateError::at(
                    MigrateSource::Cases,
                    "multiple global envs (tests = []); cCaller allows at most one [env]"
                        .to_owned(),
                ));
            }
            global_env = Some(env);
        } else {
            named_envs.push(env);
        }
    }
    let render_env_tables = |name: Option<&str>,
                             init: &[HitestCmd],
                             exit: &[HitestCmd],
                             header: &str,
                             out: &mut String|
     -> Result<(), MigrateError> {
        out.push_str(&format!("\n{header}\n"));
        if let Some(name) = name {
            out.push_str(&format!("name = {}\n", quote(name)));
        }
        if !init.is_empty() {
            out.push_str("init = [\n");
            out.push_str(&render_cmds(init, "  ")?);
            out.push_str("]\n");
        }
        if !exit.is_empty() {
            out.push_str("exit = [\n");
            out.push_str(&render_cmds(exit, "  ")?);
            out.push_str("]\n");
        }
        Ok(())
    };
    if let Some(env) = global_env {
        render_env_tables(env.name.as_deref(), &env.init, &env.exit, "[env]", &mut out)?;
    }
    for env in &named_envs {
        let Some(name) = &env.name else {
            return Err(MigrateError::at(
                MigrateSource::Cases,
                "a named env (tests != []) without a name cannot become [[envs]]".to_owned(),
            ));
        };
        let mut block = format!("\n[[envs]]\nname = {}\n", quote(name));
        if !env.init.is_empty() {
            block.push_str("init = [\n");
            block.push_str(&render_cmds(&env.init, "  ")?);
            block.push_str("]\n");
        }
        if !env.exit.is_empty() {
            block.push_str("exit = [\n");
            block.push_str(&render_cmds(&env.exit, "  ")?);
            block.push_str("]\n");
        }
        let tests: Vec<String> = env.tests.iter().map(|test| quote(test)).collect();
        block.push_str(&format!("tests = [{}]\n", tests.join(", ")));
        out.push_str(&block);
    }
    if let Some(env) = &cases.thread_env {
        render_env_tables(
            env.name.as_deref(),
            &env.init,
            &env.exit,
            "[thread_env]",
            &mut out,
        )?;
    }
    if let Some(env) = &cases.process_env {
        render_env_tables(
            env.name.as_deref(),
            &env.init,
            &env.exit,
            "[process_env]",
            &mut out,
        )?;
    }

    for group in &cases.concurrences {
        out.push_str(&format!(
            "\n[[concurrences]]\nname = {}\n",
            quote(&group.name)
        ));
        let tests: Vec<String> = group.tests.iter().map(|test| quote(test)).collect();
        out.push_str(&format!("tests = [{}]\n", tests.join(", ")));
    }

    for test in &cases.tests {
        out.push_str(&format!("\n[[tests]]\nname = {}\n", quote(&test.name)));
        if let Some(thread_num) = test.thread_num {
            if thread_num < 1 {
                return Err(MigrateError::at(
                    MigrateSource::Cases,
                    format!("thread_num {thread_num} cannot become a cCaller u64 (must be >= 1)"),
                ));
            }
            out.push_str(&format!("thread_num = {thread_num}\n"));
        }
        if let Some(should_panic) = test.should_panic {
            out.push_str(&format!("should_panic = {should_panic}\n"));
        }
        if let Some(break_if_fail) = test.break_if_fail {
            out.push_str(&format!("break_if_fail = {break_if_fail}\n"));
        }
        if let Some(serial) = test.serial {
            out.push_str(&format!("serial = {serial}\n"));
        }
        out.push_str("cmds = [\n");
        out.push_str(&render_cmds(&test.cmds, "  ")?);
        out.push_str("]\n");

        for (position, group) in test.inputs.iter().enumerate() {
            // Unnamed groups and groups carrying hitest's auto-name
            // (`default1/default2…`, see README §7) both get the stable
            // position-based cCaller name.
            let name = match group.name.as_deref() {
                Some(name) if !is_hitest_default_name(name) => name.to_owned(),
                _ => format!("ipt{}", position + 1),
            };
            out.push_str("\n[[tests.inputs]]\n");
            out.push_str(&format!("name = {}\n", quote(&name)));
            if !group.refs.is_empty() {
                let refs: Vec<String> = group.refs.iter().map(|r| quote(r)).collect();
                out.push_str(&format!("refs = [{}]\n", refs.join(", ")));
            }
            if let Some(should_panic) = group.should_panic {
                out.push_str(&format!("should_panic = {should_panic}\n"));
            }
            if let Some(break_if_fail) = group.break_if_fail {
                out.push_str(&format!("break_if_fail = {break_if_fail}\n"));
            }
            if !group.args.is_empty() {
                let parts: Vec<String> = group
                    .args
                    .iter()
                    .map(|(param, value)| {
                        Ok(format!(
                            "{} = {}",
                            render_key(param),
                            render_input_value(&map_input_value(value)?)
                        ))
                    })
                    .collect::<Result<Vec<_>, MigrateError>>()?;
                out.push_str(&format!("args = {{ {} }}\n", parts.join(", ")));
            }
        }
    }

    Ok((out, cases.tests.len()))
}

// ---- wrapper scan (slot_roles inference) ----

/// The read bit of a role accumulator.
const READ: u8 = 1;
/// The write bit of a role accumulator.
const WRITE: u8 = 2;

/// Per-function role bits keyed by parameter index, as scanned from a
/// hitest wrapper.
#[derive(Debug, Default)]
struct WrapperRoles {
    functions: BTreeMap<String, BTreeMap<usize, u8>>,
}

impl WrapperRoles {
    /// The `(parameter, role)` pairs for one declared function, in
    /// parameter order; parameters the wrapper never touches are left
    /// out.
    fn roles_of(&self, name: &str, paras: &[String]) -> Option<Vec<(String, String)>> {
        let bits = self.functions.get(name)?;
        let mut out = Vec::new();
        for (index, param) in paras.iter().enumerate() {
            let Some(bits) = bits.get(&index) else {
                continue;
            };
            let role = match *bits {
                READ => "read",
                WRITE => "write",
                _ => "read_write",
            };
            out.push((param.clone(), role.to_owned()));
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// Scans a hitest wrapper for the slot-access macros.
///
/// Function bodies are located by their definitions — `EXPORT_FUNC(name,
/// …)` macro calls or direct `Call_<name>(…)` declarations — and each
/// body is searched for `GET_INPUT_IDX` / `GET_INPUT_IDX_NZ` (read) and
/// `SET_OUTPUT_IDX` (write); `GET_VALUE` contributes nothing (a value
/// parameter). Indices must be integer literals referring to the
/// declared parameter order. Comments, strings, `#define` lines, and
/// conditional blocks of other platforms are skipped with the same
/// machinery `gen` uses.
fn scan_wrapper(wrapper: &str) -> WrapperRoles {
    let stripped = strip_comments(&splice_lines(wrapper));
    let (inactive, directives) = classify_lines(&stripped);
    let mut roles = WrapperRoles::default();

    let mut sites: Vec<(String, usize)> = Vec::new(); // symbol, macro end
    collect_definition_sites(&stripped, &inactive, &directives, &mut sites);

    for (name, macro_end) in sites {
        let Some(body) = body_after(&stripped, macro_end) else {
            continue;
        };
        let entry = roles.functions.entry(name).or_default();
        scan_body(&stripped, body, &inactive, &directives, entry);
    }
    roles
}

/// Collects `(symbol, macro_end)` definition sites in the stripped
/// text: `EXPORT_FUNC(name, …)` calls and direct `Call_<name>(…)`
/// declarations whose parameter list is followed by a body brace.
fn collect_definition_sites(
    text: &str,
    inactive: &[(usize, usize)],
    directives: &[(usize, usize)],
    sites: &mut Vec<(String, usize)>,
) {
    let mut search = 0usize;
    while let Some(found) = text[search..].find("EXPORT_FUNC(") {
        let open = search + found;
        search = open + "EXPORT_FUNC(".len();
        if preceded_by_ident(text, open) || skipped(open, inactive, directives) {
            continue;
        }
        let Some(close) = matching_paren(text, open + "EXPORT_FUNC(".len() - 1) else {
            continue;
        };
        let args = &text[open + "EXPORT_FUNC(".len()..close];
        let Some(name) = args.split(',').next() else {
            continue;
        };
        let name = name.trim();
        if !is_c_ident(name) {
            continue;
        }
        sites.push((format!("Call_{name}"), close + 1));
    }

    let mut search = 0usize;
    while let Some(found) = text[search..].find("Call_") {
        let open = search + found;
        search = open + "Call_".len();
        if preceded_by_ident(text, open) || skipped(open, inactive, directives) {
            continue;
        }
        let ident_end = open + ident_len(&text[open..]);
        let name = &text[open..ident_end];
        if name.is_empty() || !text[ident_end..].starts_with('(') {
            continue;
        }
        // Only definitions count: the parameter list must be followed
        // by a body brace, not a semicolon or an operator.
        let Some(close) = matching_paren(text, ident_end) else {
            continue;
        };
        if body_after(text, close + 1).is_some() {
            sites.push((name.to_owned(), close + 1));
        }
    }
}

/// Scans one function body for the slot-access macros.
fn scan_body(
    text: &str,
    body: (usize, usize),
    inactive: &[(usize, usize)],
    directives: &[(usize, usize)],
    entry: &mut BTreeMap<usize, u8>,
) {
    let needles: [(&str, usize, u8); 3] = [
        ("GET_INPUT_IDX(", 2, READ),
        ("GET_INPUT_IDX_NZ(", 2, READ),
        ("SET_OUTPUT_IDX(", 0, WRITE),
    ];
    for (needle, index_arg, bit) in needles {
        let mut search = body.0;
        while let Some(found) = text[search..body.1].find(needle) {
            let open = search + found;
            search = open + needle.len();
            if preceded_by_ident(text, open) || skipped(open, inactive, directives) {
                continue;
            }
            let Some(close) = matching_paren(text, open + needle.len() - 1) else {
                continue;
            };
            let args = &text[open + needle.len()..close];
            let Some(raw) = args.split(',').nth(index_arg) else {
                continue;
            };
            if let Ok(index) = raw.trim().parse::<usize>() {
                *entry.entry(index).or_insert(0) |= bit;
            }
            // Non-literal indices (constants, expressions) cannot be
            // resolved; the parameter keeps whatever other uses say.
        }
    }
}

/// Length of the C identifier starting at `text`.
fn ident_len(text: &str) -> usize {
    text.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(text.len())
}

/// Whether the byte before `offset` continues an identifier.
fn preceded_by_ident(text: &str, offset: usize) -> bool {
    offset > 0
        && text
            .as_bytes()
            .get(offset - 1)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Whether an offset sits on a skipped line (directive or inactive).
fn skipped(offset: usize, inactive: &[(usize, usize)], directives: &[(usize, usize)]) -> bool {
    let in_ranges =
        |ranges: &[(usize, usize)]| ranges.iter().any(|(s, e)| offset >= *s && offset < *e);
    in_ranges(inactive) || in_ranges(directives)
}

/// Whether `text` is a C identifier.
fn is_c_ident(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Index of the `)` matching the `(` at `open`, if balanced.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// The `{ … }` block starting at the first non-whitespace byte from
/// `from`, as a byte range.
fn body_after(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    let mut depth = 1usize;
    let mut scan = index + 1;
    while scan < bytes.len() {
        match bytes[scan] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((index + 1, scan));
                }
            }
            _ => {}
        }
        scan += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed hitest-style input sample (examples/migrate/hitest).
    const HITEST_LIBS: &str = r#"
[[libs]]
path = "libmalloc.so"
funcs = [
    { name = "Call_malloc",  paras = ["len", "mem_idx"] },
    { name = "Call_write32", paras = ["addr_idx", "off", "val"] },
    { name = "Call_read32",  paras = ["addr_idx", "off"] },
    { name = "Call_free",    paras = ["mem_idx"] },
]
"#;

    /// The committed hitest-style case sample, verbatim.
    const HITEST_CASES: &str = r#"
debug_test = "test_rw_u32"

[shared_inputs]
w888 = { val = "888" }

[[tests]]
name = "test_rw_u32"
thread_num = 2
cmds = [
  { opfunc = "Call_malloc",  expect_eq = 0, args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_write32", expect_eq = 0, perf = true, args = ["addr_idx=1", "off=0", "val=$write_val"] },
  { opfunc = "Call_read32",  expect_eq = "$write_val", args = ["addr_idx=1", "off=0"] },
  { opfunc = "Call_read32",  expect_eq = "!0", args = ["addr_idx=1", "off=0"] },
  { opfunc = "Call_free",    expect_eq = 0, args = ["mem_idx=1"] },
]
inputs = [
  { args = { write_val = "888" } },
  { name = "ipt2", args = { write_val = "999" }, break_if_fail = false },
  { name = "ipt3", args = { write_val = "555" }, should_panic = true },
]
"#;

    /// A hitest-style wrapper with the macro semantics of
    /// hitest/sample/export_function.h, covering every inference rule.
    const WRAPPER: &str = r#"
#include <stdint.h>
#include <stdlib.h>

#define EXPORT_FUNC(func_name, ...) \
    int64_t Call_##func_name(uint64_t *param_page, const uint64_t *params, int64_t params_len)

#define GET_INPUT_IDX(type, name, param_idx) type name = (type)param_page[params[param_idx]]
#define GET_INPUT_IDX_NZ(type, name, param_idx) \
    type name; do { name = (type)param_page[params[param_idx]]; if (!name) return -14; } while (0)
#define GET_VALUE(type, name, param_idx) type name = (type)params[param_idx]
#define SET_OUTPUT_IDX(param_idx, val) param_page[param_idx] = (uint64_t)(val)

EXPORT_FUNC(malloc, len, mem_idx)
{
    GET_VALUE(int64_t, len, 0);
    void *ptr = malloc((size_t)len);
    if (!ptr) { return -4; }
    SET_OUTPUT_IDX(1, (uint64_t)(uintptr_t)ptr);
    return 0;
}

EXPORT_FUNC(write32, addr_idx, off, val)
{
    GET_INPUT_IDX(uint8_t *, addr, 0);
    GET_VALUE(int64_t, off, 1);
    GET_VALUE(uint32_t, val, 2);
    *(uint32_t *)(addr + off) = val;
    return 0;
}

EXPORT_FUNC(read32, addr_idx, off)
{
    GET_INPUT_IDX(const uint8_t *, addr, 0);
    GET_VALUE(int64_t, off, 1);
    return (int64_t)*(const uint32_t *)(addr + off);
}

EXPORT_FUNC(free, mem_idx)
{
    GET_INPUT_IDX_NZ(void *, mem, 0);
    free(mem);
    return 0;
}

/* read + write on both parameters folds to read_write */
EXPORT_FUNC(exchange, a_idx, b_idx)
{
    GET_INPUT_IDX(uint64_t, a, 0);
    GET_INPUT_IDX(uint64_t, b, 1);
    SET_OUTPUT_IDX(0, b);
    SET_OUTPUT_IDX(1, a);
    return 0;
}

/* an out-of-range index cannot be mapped back to a parameter */
EXPORT_FUNC(mixed, a, b)
{
    GET_INPUT_IDX(uint64_t, x, 0);
    SET_OUTPUT_IDX(3, 0);
    return 0;
}

// A comment mentioning SET_OUTPUT_IDX(0, ghost) must not count.
static const char *kNeedle = "GET_INPUT_IDX(u64, x, 0)";

#ifdef __APPLE__
EXPORT_FUNC(apple_only, x)
{
    GET_INPUT_IDX(uint64_t, v, 0);
    return 0;
}
#endif
"#;

    /// Migrates the committed sample with the wrapper fixture.
    fn migrate_sample() -> MigrationOutput {
        migrate(HITEST_LIBS, HITEST_CASES, Some(WRAPPER)).unwrap()
    }

    /// Parses generated text as a cCaller case configuration.
    fn parse_cases(text: &str) -> super::super::cases::CaseConfig {
        let doc = super::super::source::SourceDoc {
            path: "cases.toml".to_owned(),
            text: text.to_owned(),
        };
        doc.parse().unwrap()
    }

    /// Parses generated text as a cCaller library description.
    fn parse_libs(text: &str) -> super::super::lib_desc::LibDescription {
        let doc = super::super::source::SourceDoc {
            path: "libs.toml".to_owned(),
            text: text.to_owned(),
        };
        doc.parse().unwrap()
    }

    #[test]
    fn both_documents_gain_a_version__F_X_06() {
        let output = migrate_sample();
        assert!(output.libs_toml.starts_with("version = 1\n"));
        assert!(output.cases_toml.starts_with("version = 1\n"));
        // Both outputs load as cCaller documents at all.
        parse_libs(&output.libs_toml);
        parse_cases(&output.cases_toml);
    }

    #[test]
    fn debug_test_becomes_a_list__F_X_06() {
        let output = migrate_sample();
        let config = parse_cases(&output.cases_toml);
        let names: Vec<&str> = config
            .debug_test
            .iter()
            .map(|name| name.get_ref().as_str())
            .collect();
        assert_eq!(names, vec!["test_rw_u32"]);
    }

    #[test]
    fn negated_expectations_fold__F_X_06() {
        let output = migrate_sample();
        let config = parse_cases(&output.cases_toml);
        let folded = config.tests[0]
            .get_ref()
            .cmds
            .iter()
            .map(|cmd| cmd.get_ref())
            .find(|cmd| cmd.expectations.contains_key("expect_ne"))
            .unwrap()
            .expectations
            .get("expect_ne")
            .unwrap();
        assert_eq!(folded.get_ref(), &ScalarRaw::Int(0));
    }

    #[test]
    fn assertion_folding_rules__F_X_06() {
        let make = |raw: Option<ScalarRaw>| HitestCmd {
            opfunc: "f".to_owned(),
            args: vec![],
            perf: None,
            expect_eq: raw,
            expect_ne: None,
        };
        // "!$v" flips to expect_ne with the variable kept.
        let folded = map_assertion(&make(Some(ScalarRaw::Str("!$v".to_owned())))).unwrap();
        assert_eq!(
            folded,
            Some(("expect_ne".to_owned(), ScalarRaw::Str("$v".to_owned())))
        );
        // A negated expect_ne flips back to expect_eq.
        let flipped = HitestCmd {
            opfunc: "f".to_owned(),
            args: vec![],
            perf: None,
            expect_eq: None,
            expect_ne: Some(ScalarRaw::Str("!5".to_owned())),
        };
        let folded = map_assertion(&flipped).unwrap();
        assert_eq!(folded, Some(("expect_eq".to_owned(), ScalarRaw::Int(5))));
        // Plain values pass through untouched.
        let folded = map_assertion(&make(Some(ScalarRaw::Str("$write_val".to_owned())))).unwrap();
        assert_eq!(
            folded,
            Some((
                "expect_eq".to_owned(),
                ScalarRaw::Str("$write_val".to_owned())
            ))
        );
        // Empty negation and double assertion are errors.
        let error = map_assertion(&make(Some(ScalarRaw::Str("!".to_owned())))).unwrap_err();
        assert!(error.message.contains("empty negated"), "{error}");
        let both = HitestCmd {
            opfunc: "f".to_owned(),
            args: vec![],
            perf: None,
            expect_eq: Some(ScalarRaw::Int(0)),
            expect_ne: Some(ScalarRaw::Int(1)),
        };
        let error = map_assertion(&both).unwrap_err();
        assert!(
            error.message.contains("both expect_eq and expect_ne"),
            "{error}"
        );
    }

    #[test]
    fn unnamed_groups_get_position_names__F_X_06() {
        let output = migrate_sample();
        let config = parse_cases(&output.cases_toml);
        let inputs = &config.tests[0].get_ref().inputs;
        let names: Vec<&str> = inputs
            .iter()
            .map(|group| group.get_ref().name.get_ref().as_str())
            .collect();
        assert_eq!(names, vec!["ipt1", "ipt2", "ipt3"]);
    }

    #[test]
    fn group_level_flags_map_directly__F_X_06() {
        let output = migrate_sample();
        let config = parse_cases(&output.cases_toml);
        let inputs = &config.tests[0].get_ref().inputs;
        assert_eq!(inputs[0].get_ref().should_panic, None);
        assert_eq!(inputs[0].get_ref().break_if_fail, None);
        assert_eq!(inputs[1].get_ref().should_panic, None);
        assert_eq!(inputs[1].get_ref().break_if_fail, Some(false));
        assert_eq!(inputs[2].get_ref().should_panic, Some(true));
        assert_eq!(inputs[2].get_ref().break_if_fail, None);
    }

    #[test]
    fn string_scalars_become_int_literals__F_X_06() {
        let output = migrate_sample();
        let config = parse_cases(&output.cases_toml);
        // shared_inputs: "888" → the literal list [888].
        let shared = &config.shared_inputs["w888"];
        assert_eq!(
            shared.get_ref()["val"].get_ref(),
            &super::super::cases::InputValue::List(vec![ScalarRaw::Int(888)])
        );
        // Input args: "888"/"999"/"555" → integer literals.
        let inputs = &config.tests[0].get_ref().inputs;
        for (group, expected) in [("ipt1", 888), ("ipt2", 999), ("ipt3", 555)] {
            let value = inputs
                .iter()
                .find(|input| input.get_ref().name.get_ref() == group)
                .unwrap()
                .get_ref()
                .args
                .get("write_val")
                .unwrap()
                .get_ref();
            assert_eq!(
                value,
                &super::super::cases::InputValue::Single(ScalarRaw::Int(expected))
            );
        }
    }

    #[test]
    fn non_numeric_strings_stay_strings__F_X_06() {
        let cases = r#"
[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
inputs = [ { args = { s = "abc", h = "0x10" } } ]
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        let config = parse_cases(&output.cases_toml);
        let args = &config.tests[0].get_ref().inputs[0].get_ref().args;
        assert_eq!(
            args["s"].get_ref(),
            &super::super::cases::InputValue::Single(ScalarRaw::Str("abc".to_owned()))
        );
        assert_eq!(
            args["h"].get_ref(),
            &super::super::cases::InputValue::Single(ScalarRaw::Str("0x10".to_owned()))
        );
    }

    #[test]
    fn range_step_defaults_to_one__F_X_06() {
        let cases = r#"
[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
inputs = [ { args = { off = { start = 0, end = 8 } } } ]
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        assert!(
            output
                .cases_toml
                .contains("off = { start = 0, end = 8, step = 1 }"),
            "cases: {}",
            output.cases_toml
        );
        // An explicit non-positive step is rejected instead of truncated.
        let cases = r#"
[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
inputs = [ { args = { off = { start = 0, end = 8, step = 0 } } } ]
"#;
        let error = migrate(HITEST_LIBS, cases, None).unwrap_err();
        assert!(error.message.contains("step 0"), "{error}");
    }

    #[test]
    fn envs_split_into_global_and_named__F_X_06() {
        let cases = r#"
[[envs]]
init = [ { opfunc = "Call_free", args = [] } ]
tests = []

[[envs]]
name = "sock"
exit = [ { opfunc = "Call_free", args = ["mem_idx=0"] } ]
tests = ["t"]

[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        let config = parse_cases(&output.cases_toml);
        let env = config.env.as_ref().unwrap().get_ref();
        assert_eq!(env.init.len(), 1);
        assert!(env.exit.is_empty());
        assert_eq!(config.envs.len(), 1);
        let named = config.envs[0].get_ref();
        assert_eq!(named.name.get_ref(), "sock");
        assert_eq!(named.tests.len(), 1);

        // Two global envs cannot both become [env].
        let cases = r#"
[[envs]]
tests = []
[[envs]]
tests = []

[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
"#;
        let error = migrate(HITEST_LIBS, cases, None).unwrap_err();
        assert!(error.message.contains("multiple global envs"), "{error}");
    }

    #[test]
    fn concurrences_render_as_tables__F_X_06() {
        let cases = r#"
concurrences = [ { name = "cg", tests = ["t"] } ]

[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        assert!(
            output
                .cases_toml
                .contains("[[concurrences]]\nname = \"cg\"\ntests = [\"t\"]"),
            "cases: {}",
            output.cases_toml
        );
        let config = parse_cases(&output.cases_toml);
        assert_eq!(config.concurrences.len(), 1);
    }

    #[test]
    fn thread_num_below_one_is_rejected__F_X_06() {
        let cases = r#"
[[tests]]
name = "t"
thread_num = 0
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
"#;
        let error = migrate(HITEST_LIBS, cases, None).unwrap_err();
        assert!(error.message.contains("thread_num 0"), "{error}");
    }

    #[test]
    fn slot_roles_are_inferred_from_the_wrapper__F_X_06() {
        let output = migrate_sample();
        let desc = parse_libs(&output.libs_toml);
        for (name, roles) in [
            ("Call_malloc", vec![("mem_idx", "write")]),
            ("Call_write32", vec![("addr_idx", "read")]),
            ("Call_read32", vec![("addr_idx", "read")]),
            ("Call_free", vec![("mem_idx", "read")]),
        ] {
            let func = desc.func(name).unwrap();
            let expected: std::collections::BTreeMap<String, super::super::lib_desc::SlotRole> =
                roles
                    .into_iter()
                    .map(|(param, role)| {
                        let role = match role {
                            "read" => super::super::lib_desc::SlotRole::Read,
                            "write" => super::super::lib_desc::SlotRole::Write,
                            _ => super::super::lib_desc::SlotRole::ReadWrite,
                        };
                        (param.to_owned(), role)
                    })
                    .collect();
            assert_eq!(func.slot_roles, expected, "roles of {name}");
        }
        // The path is carried over untouched (the committed expected
        // file points at the runnable library by hand).
        assert_eq!(desc.libs[0].get_ref().path.get_ref(), "libmalloc.so");
    }

    #[test]
    fn inference_folds_read_write_and_skips_decoys__F_X_06() {
        let libs = r#"
[[libs]]
path = "w.so"
funcs = [
  { name = "Call_exchange", paras = ["a_idx", "b_idx"] },
  { name = "Call_mixed", paras = ["a", "b"] },
  { name = "Call_apple_only", paras = ["x"] },
]
"#;
        let output = migrate(libs, "[[tests]]\nname = \"t\"\ncmds = []\n", Some(WRAPPER)).unwrap();
        let desc = parse_libs(&output.libs_toml);
        // read + write on both parameters → read_write.
        let exchange = desc.func("Call_exchange").unwrap();
        assert_eq!(exchange.slot_roles.len(), 2);
        assert!(matches!(
            exchange.slot_roles.get("a_idx"),
            Some(super::super::lib_desc::SlotRole::ReadWrite)
        ));
        // An out-of-range index is dropped; the mapped one stays.
        let mixed = desc.func("Call_mixed").unwrap();
        assert_eq!(mixed.slot_roles.len(), 1);
        assert!(matches!(
            mixed.slot_roles.get("a"),
            Some(super::super::lib_desc::SlotRole::Read)
        ));
        // The __APPLE__ arm is foreign on Linux and Windows CI.
        assert!(desc.func("Call_apple_only").unwrap().slot_roles.is_empty());
    }

    #[test]
    fn without_a_wrapper_roles_are_missing_with_a_warning__F_X_06() {
        let output = migrate(HITEST_LIBS, HITEST_CASES, None).unwrap();
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| warning.contains("slot_roles cannot be inferred")),
            "warnings: {:?}",
            output.warnings
        );
        let desc = parse_libs(&output.libs_toml);
        assert!(desc.func("Call_malloc").unwrap().slot_roles.is_empty());
    }

    #[test]
    fn ref_inputs_warns__F_X_06() {
        let cases = r#"
[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
ref_inputs = { init = [], cleanup = [] }
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| warning.contains("ref_inputs")),
            "warnings: {:?}",
            output.warnings
        );
    }

    #[test]
    fn a_range_in_shared_inputs_is_rejected__F_X_06() {
        let cases = r#"
[shared_inputs.g]
v = { start = 0, end = 8 }

[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
"#;
        let error = migrate(HITEST_LIBS, cases, None).unwrap_err();
        assert!(error.message.contains("Range"), "{error}");
    }

    #[test]
    fn unparseable_hitest_toml_reports_a_location__F_X_06() {
        let error = migrate("[[libs\n", HITEST_CASES, None).unwrap_err();
        assert_eq!(error.source, MigrateSource::Libs);
        assert!(error.line >= 1 && error.column >= 1);

        let error = migrate(HITEST_LIBS, "debug_test = \n", None).unwrap_err();
        assert_eq!(error.source, MigrateSource::Cases);
        assert!(error.line >= 1);
    }

    #[test]
    fn generated_toml_is_pure_ccaller__F_X_06() {
        let output = migrate_sample();
        for (label, text) in [
            ("libs.toml", &output.libs_toml),
            ("cases.toml", &output.cases_toml),
        ] {
            // No "hitest" mention — not in comments (there are none:
            // like `gen`, migration emits bare TOML), not in field
            // names, not in values.
            assert!(
                !text.to_lowercase().contains("hitest"),
                "{label} mentions hitest: {text}"
            );
            // Zero comments, like the `gen` output.
            assert!(
                !text.lines().any(|line| line.trim_start().starts_with('#')),
                "{label} contains comments: {text}"
            );
            // No hitest-specific field or negated assertion survives.
            assert!(!text.contains("ref_inputs"), "{label}: {text}");
            assert!(
                !text
                    .lines()
                    .any(|line| line.contains("= \"!") || line.contains("= '!'")),
                "negated assertion survived in {label}: {text}"
            );
        }
        // debug_test is a list, shared inputs are literal (not strings),
        // groups are explicitly named.
        assert!(output.cases_toml.contains("debug_test = ["));
        assert!(output.cases_toml.contains("val = [888]"));
        assert!(output.cases_toml.contains("name = \"ipt1\""));
    }

    #[test]
    fn hitest_default_group_names_fold_into_ipt_names__F_X_06() {
        let cases = r#"
[[tests]]
name = "t"
cmds = [ { opfunc = "Call_malloc", expect_eq = 0, args = ["len=1", "mem_idx=0"] } ]
inputs = [
  { name = "default1", args = { a = 1 } },
  { args = { a = 2 } },
  { name = "default7", args = { a = 3 } },
  { name = "defaulted", args = { a = 4 } },
]
"#;
        let output = migrate(HITEST_LIBS, cases, None).unwrap();
        let config = parse_cases(&output.cases_toml);
        let names: Vec<&str> = config.tests[0]
            .get_ref()
            .inputs
            .iter()
            .map(|group| group.get_ref().name.get_ref().as_str())
            .collect();
        // `default{N}` auto-names and unnamed groups both take the
        // position-based scheme; a merely similar name is a real choice
        // and stays.
        assert_eq!(names, vec!["ipt1", "ipt2", "ipt3", "defaulted"]);
    }

    #[test]
    fn migrated_sample_matches_the_committed_expected_output__F_X_06() {
        let output = migrate_sample();

        // libs: every field but the (hand-adjusted) path matches.
        let expected_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/migrate/ccaller/libs.toml");
        let expected = parse_libs(&std::fs::read_to_string(&expected_path).unwrap());
        let migrated = parse_libs(&output.libs_toml);
        assert_eq!(*migrated.version.get_ref(), *expected.version.get_ref());
        assert_eq!(migrated.libs.len(), expected.libs.len());
        let migrated_lib = migrated.libs[0].get_ref();
        let expected_lib = expected.libs[0].get_ref();
        assert_eq!(migrated_lib.funcs.len(), expected_lib.funcs.len());
        for (migrated_func, expected_func) in
            migrated_lib.funcs.iter().zip(expected_lib.funcs.iter())
        {
            let migrated_func = migrated_func.get_ref();
            let expected_func = expected_func.get_ref();
            assert_eq!(migrated_func.name.get_ref(), expected_func.name.get_ref());
            assert_eq!(
                migrated_func
                    .paras
                    .iter()
                    .map(|p| p.get_ref())
                    .collect::<Vec<_>>(),
                expected_func
                    .paras
                    .iter()
                    .map(|p| p.get_ref())
                    .collect::<Vec<_>>()
            );
            assert_eq!(migrated_func.slot_roles, expected_func.slot_roles);
        }

        // cases: identical modulo the group flags the committed file
        // deliberately omits to keep `run` green.
        let expected_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/migrate/ccaller/cases.toml");
        let expected = parse_cases(&std::fs::read_to_string(&expected_path).unwrap());
        let migrated = parse_cases(&output.cases_toml);
        assert_eq!(
            migrated
                .debug_test
                .iter()
                .map(|name| name.get_ref().clone())
                .collect::<Vec<_>>(),
            expected
                .debug_test
                .iter()
                .map(|name| name.get_ref().clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(migrated.shared_inputs.len(), expected.shared_inputs.len());
        assert_eq!(
            migrated.shared_inputs["w888"].get_ref()["val"].get_ref(),
            expected.shared_inputs["w888"].get_ref()["val"].get_ref()
        );
        assert_eq!(migrated.tests.len(), expected.tests.len());
        let migrated_test = migrated.tests[0].get_ref();
        let expected_test = expected.tests[0].get_ref();
        assert_eq!(migrated_test.name.get_ref(), expected_test.name.get_ref());
        assert_eq!(
            *migrated_test.thread_num.get_ref(),
            *expected_test.thread_num.get_ref()
        );
        assert_eq!(migrated_test.cmds.len(), expected_test.cmds.len());
        for (migrated_cmd, expected_cmd) in migrated_test.cmds.iter().zip(expected_test.cmds.iter())
        {
            let migrated_cmd = migrated_cmd.get_ref();
            let expected_cmd = expected_cmd.get_ref();
            assert_eq!(migrated_cmd.opfunc.get_ref(), expected_cmd.opfunc.get_ref());
            assert_eq!(
                migrated_cmd
                    .args
                    .iter()
                    .map(|arg| arg.get_ref())
                    .collect::<Vec<_>>(),
                expected_cmd
                    .args
                    .iter()
                    .map(|arg| arg.get_ref())
                    .collect::<Vec<_>>()
            );
            assert_eq!(migrated_cmd.perf, expected_cmd.perf);
            assert_eq!(migrated_cmd.expectations, expected_cmd.expectations);
        }
        // The migrated groups carry the hitest flags the committed
        // file omits; names and args line up.
        assert_eq!(migrated_test.inputs.len(), expected_test.inputs.len());
        for (migrated_group, expected_group) in
            migrated_test.inputs.iter().zip(expected_test.inputs.iter())
        {
            let migrated_group = migrated_group.get_ref();
            let expected_group = expected_group.get_ref();
            assert_eq!(migrated_group.name.get_ref(), expected_group.name.get_ref());
            assert_eq!(migrated_group.args, expected_group.args);
        }
    }
}
