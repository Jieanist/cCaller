//! The test case configuration model (requirement spec 7.3).
//!
//! Strict deserialization only: unknown fields are rejected (FR-C-09) and
//! spans are kept on every referenceable element so the cross-reference
//! validator can point at exact positions. Semantic checks live in
//! [`super::validate`]; this module never rejects a well-formed document.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use toml::Spanned;

use super::value::ScalarRaw;

/// Root of a test case configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseConfig {
    /// Schema version; only `1` is accepted (checked in [`super::validate`]).
    pub version: Spanned<u64>,
    /// Global env, at most one (the singular TOML key enforces this
    /// structurally); must not declare `tests`.
    #[serde(default)]
    pub env: Option<Spanned<GlobalEnv>>,
    /// Case-level envs; a test belongs to one via `envs[].tests`.
    #[serde(default)]
    pub envs: Vec<Spanned<CaseEnv>>,
    /// The tests to run.
    pub tests: Vec<Spanned<TestDef>>,
    /// Concurrency groups; referenced tests no longer run standalone.
    #[serde(default)]
    pub concurrences: Vec<Spanned<ConcurrencyGroup>>,
    /// Named parameter groups shared through InputGroup `refs`.
    #[serde(default)]
    pub shared_inputs: BTreeMap<String, Spanned<BTreeMap<String, Spanned<InputValue>>>>,
    /// Thread-level env, at most one; applied per worker thread.
    #[serde(default)]
    pub thread_env: Option<Spanned<GlobalEnv>>,
    /// Process-level env, at most one; its slots are pre-written into every
    /// worker's param_page (FR-E-04).
    #[serde(default)]
    pub process_env: Option<Spanned<GlobalEnv>>,
    /// Debug test names; takes priority over CLI `-d` (FR-T-08, M2).
    #[serde(default)]
    pub debug_test: Vec<Spanned<String>>,
    /// Default `serial` for tests that do not declare it.
    #[serde(default)]
    pub default_serial: bool,
}

/// An env without test ownership: the global `env`, `thread_env`, and
/// `process_env` (requirement spec 7.3 env field table).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalEnv {
    /// Optional label; useful in reports.
    #[serde(default)]
    pub name: Option<Spanned<String>>,
    /// Commands run on scope entry.
    #[serde(default)]
    pub init: Vec<Spanned<CmdDef>>,
    /// Commands run on scope exit.
    #[serde(default)]
    pub exit: Vec<Spanned<CmdDef>>,
}

/// A case-level env from `envs[]`: owns the tests it references.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEnv {
    /// Unique name within `envs`.
    pub name: Spanned<String>,
    /// Commands run before the owned tests.
    #[serde(default)]
    pub init: Vec<Spanned<CmdDef>>,
    /// Commands run after the owned tests.
    #[serde(default)]
    pub exit: Vec<Spanned<CmdDef>>,
    /// Tests owned by this env; references must exist (FR-C-08).
    #[serde(default)]
    pub tests: Vec<Spanned<String>>,
}

/// One test: an ordered command list plus execution attributes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestDef {
    /// Globally unique test name.
    pub name: Spanned<String>,
    /// Ordered commands; must be non-empty (checked in [`super::validate`]).
    pub cmds: Vec<Spanned<CmdDef>>,
    /// Worker threads for this test; `>= 1`.
    #[serde(default = "default_thread_num")]
    pub thread_num: Spanned<u64>,
    /// Whether this is a death test (isolated subprocess, FR-T-05, M3).
    #[serde(default)]
    pub should_panic: bool,
    /// Whether a failed command stops the remaining ones (FR-T-01).
    #[serde(default = "default_true")]
    pub break_if_fail: bool,
    /// Input groups; each expands into subcases (FR-C-03).
    #[serde(default)]
    pub inputs: Vec<Spanned<InputGroup>>,
    /// `None` inherits `default_serial`.
    #[serde(default)]
    pub serial: Option<bool>,
}

/// One interface call: which function, which arguments, what to expect.
///
/// The `expect_*` namespace is open-ended (FR-V-02): every `expect_*` field
/// is captured by name with its value span, and every non-`expect_` stray
/// field is captured by name with its value span so [`super::validate`] can
/// reject it as an unknown field (FR-C-09). Deserialization is hand-written
/// because `deny_unknown_fields` cannot coexist with the open assertion
/// namespace, and `#[serde(flatten)]` would drop the TOML spans the
/// validator needs.
#[derive(Debug, Clone)]
pub struct CmdDef {
    /// Function name; must exist in the library description (FR-C-08).
    pub opfunc: Spanned<String>,
    /// `name=value` strings; names must match the library `paras` in name,
    /// count, and order (FR-C-08).
    pub args: Vec<Spanned<String>>,
    /// Record the duration of this call (FR-P-01, M3).
    pub perf: bool,
    /// Every `expect_*` field's value, keyed by its TOML name and carrying
    /// the value span. Registered names route to the assertion registry;
    /// unregistered `expect_*` names are unknown fields.
    pub expectations: BTreeMap<String, Spanned<ScalarRaw>>,
    /// Non-`expect_*` stray fields: name and value span for unknown-field
    /// reporting (FR-C-09).
    pub stray_fields: BTreeMap<String, Range<usize>>,
}

impl CmdDef {
    /// The registered assertion fields present on this command, in registry
    /// order.
    ///
    /// Unknown `expect_*` names and non-`expect_` stray fields are excluded:
    /// validation reports those separately as `unknown_field`.
    pub fn registered_assertions(
        &self,
    ) -> Vec<(
        &'static dyn crate::assertion::Assertion,
        &Spanned<ScalarRaw>,
    )> {
        let mut found = Vec::new();
        for (name, assertion) in crate::assertion::iter() {
            if let Some(value) = self.expectations.get(name) {
                found.push((assertion, value));
            }
        }
        found
    }
}

impl<'de> Deserialize<'de> for CmdDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CmdVisitor)
    }
}

/// Deserializes one command table, routing `expect_*` keys to the assertion
/// map and everything else to either a known field or the stray-field map.
struct CmdVisitor;

impl<'de> Visitor<'de> for CmdVisitor {
    type Value = CmdDef;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a command table with `opfunc`")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut opfunc = None;
        let mut args = Vec::new();
        let mut perf = false;
        let mut expectations = BTreeMap::new();
        let mut stray_fields = BTreeMap::new();

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "opfunc" => opfunc = Some(map.next_value::<Spanned<String>>()?),
                "args" => args = map.next_value::<Vec<Spanned<String>>>()?,
                "perf" => perf = map.next_value::<bool>()?,
                expect_key if expect_key.starts_with("expect_") => {
                    let value = map.next_value::<Spanned<ScalarRaw>>()?;
                    expectations.insert(expect_key.to_string(), value);
                }
                _ => {
                    // Capture the value span, then drop the value: the
                    // validator reports the name and location.
                    let value = map.next_value::<Spanned<de::IgnoredAny>>()?;
                    stray_fields.insert(key, value.span());
                }
            }
        }

        let opfunc = opfunc.ok_or_else(|| de::Error::missing_field("opfunc"))?;
        Ok(CmdDef {
            opfunc,
            args,
            perf,
            expectations,
            stray_fields,
        })
    }
}

/// A concurrency group: its member tests run in parallel (FR-T-03, M2).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcurrencyGroup {
    /// Group name; appears in reports.
    pub name: Spanned<String>,
    /// Member test names; references must exist (FR-C-08).
    pub tests: Vec<Spanned<String>>,
}

/// One input group of a test (requirement spec 7.3).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputGroup {
    /// Unique within the test; becomes part of every subcase name.
    pub name: Spanned<String>,
    /// `shared_inputs` keys to merge into this group (FR-C-06).
    #[serde(default)]
    pub refs: Vec<Spanned<String>>,
    /// Own parameters: single value, list, or closed range.
    #[serde(default)]
    pub args: BTreeMap<String, Spanned<InputValue>>,
}

/// The three value forms an input parameter can take (FR-C-04).
///
/// Ranges are closed intervals `{start, end, step}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    /// Exactly one value.
    Single(ScalarRaw),
    /// An explicit list of values.
    List(Vec<ScalarRaw>),
    /// A closed interval expanded with `step`.
    Range(RangeSpec),
}

/// Bounds of an [`InputValue::Range`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RangeSpec {
    /// First value of the closed interval.
    pub start: ScalarRaw,
    /// Last value of the closed interval (inclusive).
    pub end: ScalarRaw,
    /// Stride between values; `> 0`.
    pub step: ScalarRaw,
}

fn default_thread_num() -> Spanned<u64> {
    // Dummy span: the default value is always valid, so it is never located.
    Spanned::new(0..0, 1)
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::super::source::SourceDoc;
    use super::*;

    fn parse(text: &str) -> Result<CaseConfig, super::super::diag::Diagnostic> {
        let doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: text.to_string(),
        };
        doc.parse::<CaseConfig>()
    }

    /// The complete field example of requirement spec 7.3, verbatim.
    const FULL_EXAMPLE: &str = r#"
version = 1
default_serial = false
debug_test = []                       # equivalent of CLI -d list (config wins, FR-T-08)

[env]                                 # global env (at most one): init first, exit last
init = [{ opfunc = "Call_setup",    args = ["mode=1"] }]
exit = [{ opfunc = "Call_teardown", args = [] }]

[process_env]                         # process env: slots visible to workers (FR-E-04)
init = [{ opfunc = "Call_ctx_new",  args = ["out_idx=0"] }]
exit = [{ opfunc = "Call_ctx_free", args = ["in_idx=0"] }]

[thread_env]                          # thread env: applied per worker thread
init = [{ opfunc = "Call_thr_init", args = [] }]
exit = [{ opfunc = "Call_thr_fini", args = [] }]

[shared_inputs.common]                # named parameter group for refs
val = ["888", "999"]

[[envs]]                              # case env: ownership declared by tests
name = "with_socket"
init = [{ opfunc = "Call_sock_open",  args = ["out_fd_idx=2"] }]
exit = [{ opfunc = "Call_sock_close", args = ["in_fd_idx=2"] }]
tests = ["test_sock"]

[[tests]]
name = "test_rw_u32"                  # not referenced by any envs entry
thread_num = 2
break_if_fail = true
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0,      args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=1"], perf = true },
]
[[tests.inputs]]
name = "ipt1"
refs  = ["common"]                    # reuse shared_inputs.common -> val in {888, 999}

[[tests]]
name = "test_sock"
cmds = [
  { opfunc = "Call_write8", expect_eq = 0,      args = ["out_idx=3", "byte=0x5A"] },
  { opfunc = "Call_read8",  expect_ne = "0x00", args = ["in_idx=3"] },
]

[[tests]]
name = "test_range"                   # range expansion: addr in {0, 4, 8} (closed)
serial = true
cmds = [
  { opfunc = "Call_write8", expect_eq = 0,      args = ["out_idx=$addr", "byte=0x2A"] },
  { opfunc = "Call_read8",  expect_ne = "0x00", args = ["in_idx=$addr"] },
]
[[tests.inputs]]
name = "ipt1"
args = { addr = { start = 0, end = 8, step = 4 } }

[[concurrences]]
name = "mixed_io"
tests = ["test_sock"]                 # referenced tests no longer run standalone
"#;

    #[test]
    fn full_field_example_parses__F_C_01() {
        let config = parse(FULL_EXAMPLE).unwrap();
        assert_eq!(*config.version.get_ref(), 1);
        assert_eq!(config.tests.len(), 3);
        assert_eq!(config.envs.len(), 1);
        assert_eq!(config.concurrences.len(), 1);
        assert!(config.env.is_some());
        assert!(config.thread_env.is_some());
        assert!(config.process_env.is_some());
        assert!(config.shared_inputs.contains_key("common"));
    }

    #[test]
    fn minimal_config_parses_with_defaults__F_C_01() {
        let config = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_x\" }]\n",
        )
        .unwrap();
        let test = config.tests[0].get_ref();
        assert_eq!(*test.thread_num.get_ref(), 1);
        assert!(test.break_if_fail);
        assert!(test.serial.is_none());
        assert!(test.inputs.is_empty());
        assert!(!config.default_serial);
    }

    #[test]
    fn unknown_top_level_field_is_rejected__F_C_09() {
        let err = parse("version = 1\nthreads = 4\n").unwrap_err();
        assert_eq!(err.code, "unknown_field");
        assert!(err.message.contains("threads"));
        // FR-C-09: the location must be inside the file, not a default.
        assert!(err.location.line >= 1);
    }

    #[test]
    fn typoed_field_is_not_silently_ignored__F_C_09() {
        let err = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             thread_nu = 4\n\
             cmds = [{ opfunc = \"Call_x\" }]\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "unknown_field");
        assert!(err.message.contains("thread_nu"));
    }

    #[test]
    fn missing_required_fields_are_rejected__F_C_09() {
        let err = parse("version = 1\n").unwrap_err();
        assert_eq!(err.code, "missing_field");
        assert!(err.message.contains("tests"));

        let err =
            parse("version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ args = [] }]\n").unwrap_err();
        assert_eq!(err.code, "missing_field");
        assert!(err.message.contains("opfunc"));
    }

    #[test]
    fn global_env_cannot_declare_tests__F_C_01() {
        let err = parse(
            "version = 1\n\
             [env]\n\
             init = []\n\
             tests = [\"t\"]\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_x\" }]\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "unknown_field");
        assert!(err.message.contains("tests"));
    }

    #[test]
    fn duplicate_env_table_is_rejected_by_toml__F_C_01() {
        // The singular-key rule ("at most one global env") is enforced by
        // TOML itself: a second [env] table is a duplicate key.
        let err = parse(
            "version = 1\n\
             [env]\n\
             init = []\n\
             [env]\n\
             init = []\n",
        )
        .unwrap_err();
        // The core message is terse; the span points at the duplicate table.
        assert!(err.message.contains("duplicate key"));
    }

    #[test]
    fn expect_values_accept_int_or_string__F_C_01() {
        let config = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [\n\
             \x20 { opfunc = \"Call_a\", expect_eq = 0 },\n\
             \x20 { opfunc = \"Call_b\", expect_eq = \"0x10\" },\n\
             \x20 { opfunc = \"Call_c\", expect_ne = \"$val\" },\n\
             \x20 { opfunc = \"Call_d\", expect_eq = \"!7\" },\n\
             ]\n",
        )
        .unwrap();
        let cmds = &config.tests[0].get_ref().cmds;
        assert_eq!(
            *cmds[0].get_ref().expectations["expect_eq"].get_ref(),
            ScalarRaw::Int(0)
        );
        assert_eq!(
            *cmds[1].get_ref().expectations["expect_eq"].get_ref(),
            ScalarRaw::Str("0x10".to_string())
        );
        assert!(cmds[3].get_ref().expectations.contains_key("expect_eq"));
    }

    #[test]
    fn expect_fields_and_strays_are_captured_with_spans__F_V_02() {
        // The open-ended `expect_*` namespace and stray fields both land in
        // `expectations`, carrying spans so validation can locate them.
        let config = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [\n\
             \x20 { opfunc = \"Call_a\", expect_ge = 5, expect_gt = 1, stray = 9 },\n\
             ]\n",
        )
        .unwrap();
        let cmd = config.tests[0].get_ref().cmds[0].get_ref();
        assert_eq!(*cmd.expectations["expect_ge"].get_ref(), ScalarRaw::Int(5));
        assert!(cmd.expectations.contains_key("expect_gt"));
        assert!(cmd.stray_fields.contains_key("stray"));
        // The value sits on line 5; its span must map there, not to the
        // 0..0 default.
        assert!(cmd.expectations["expect_ge"].span().start > 0);
        assert!(cmd.stray_fields["stray"].start > 0);
    }

    #[test]
    fn input_values_parse_in_all_three_forms__F_C_03() {
        let config = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_x\" }]\n\
             [[tests.inputs]]\n\
             name = \"g\"\n\
             args = { a = 5, b = [\"1\", \"2\"], c = { start = 0, end = 8, step = 4 } }\n",
        )
        .unwrap();
        let args = &config.tests[0].get_ref().inputs[0].get_ref().args;
        assert_eq!(*args["a"].get_ref(), InputValue::Single(ScalarRaw::Int(5)));
        assert_eq!(
            *args["b"].get_ref(),
            InputValue::List(vec![
                ScalarRaw::Str("1".to_string()),
                ScalarRaw::Str("2".to_string())
            ])
        );
        assert_eq!(
            *args["c"].get_ref(),
            InputValue::Range(RangeSpec {
                start: ScalarRaw::Int(0),
                end: ScalarRaw::Int(8),
                step: ScalarRaw::Int(4),
            })
        );
    }

    #[test]
    fn input_value_with_unknown_shape_is_rejected__F_C_09() {
        let err = parse(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_x\" }]\n\
             [[tests.inputs]]\n\
             name = \"g\"\n\
             args = { a = { start = 1 } }\n",
        )
        .unwrap_err();
        assert_eq!(err.code, "type_mismatch");
    }

    #[test]
    fn shared_inputs_parse_as_groups__F_C_05() {
        let config = parse(
            "version = 1\n\
             [shared_inputs.common]\n\
             val = [\"888\", \"999\"]\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_x\" }]\n",
        )
        .unwrap();
        let group = config.shared_inputs["common"].get_ref();
        assert!(group.contains_key("val"));
    }
}
