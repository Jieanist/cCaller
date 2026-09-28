//! The `expand` pipeline: the sub-case listing a configuration would run
//! (FR-C-03, developer tooling).
//!
//! [`run_expand`] loads and parses both documents and validates them
//! exactly like [`super::check::run_check`], but reports the expansion
//! itself instead of def-use statistics: every test with its concrete
//! sub-cases — name and parameter bindings. Nothing is executed.
//!
//! The gate mirrors check's: a configuration whose validation fails has
//! no trustworthy expansion, so its findings are reported and the
//! listing stays empty. Expansion-stage findings (an unresolved `$var`)
//! accumulate the same way; the affected sub-case is dropped from the
//! listing while the other tests still expand.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::error::CoreError;

use super::cases::CaseConfig;
use super::diag::Diagnostic;
use super::expand::expand_test;
use super::lib_desc::{self, LibDescription};
use super::source::SourceDoc;
use super::validate::validate_cases;
use super::value::ConcreteValue;

/// JSON contract version of the expand report; aligned with the check
/// report's schema versioning (requirement spec 7.6).
pub const EXPAND_SCHEMA: u64 = 1;

/// Result of one `ccaller expand` run: the sub-case listing, or the
/// load-time findings that prevented it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpandReport {
    /// JSON contract version; always [`EXPAND_SCHEMA`] today.
    pub schema: u64,
    /// Every finding, in discovery order.
    pub errors: Vec<Diagnostic>,
    /// The expansion listing, in declaration order; empty while earlier
    /// findings block expansion.
    pub tests: Vec<ExpandedTest>,
}

impl ExpandReport {
    /// Whether the configuration expanded cleanly.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Renders the machine-readable form, in the style of the check
    /// report's 7.6 JSON contract.
    ///
    /// Built through [`serde_json::Value`], whose `Display` cannot fail.
    /// Object key order is not part of the contract; consumers must
    /// parse the JSON, not grep it.
    pub fn to_json(&self) -> String {
        let errors: Vec<_> = self
            .errors
            .iter()
            .map(|d| {
                json!({
                    "code": d.code,
                    "file": d.location.file.as_str(),
                    "line": d.location.line,
                    "column": d.location.column,
                    "message": d.message.as_str(),
                })
            })
            .collect();
        let tests: Vec<_> = self
            .tests
            .iter()
            .map(|test| {
                json!({
                    "name": test.name.as_str(),
                    "subcases": test.subcases.iter().map(|subcase| {
                        json!({
                            "name": subcase.name.as_str(),
                            "bindings": bindings_json(&subcase.bindings),
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "schema": self.schema,
            "ok": self.ok(),
            "errors": errors,
            "tests": tests,
        })
        .to_string()
    }
}

/// One test's expansion: its name and concrete sub-cases.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpandedTest {
    /// The test's globally unique name.
    pub name: String,
    /// The sub-cases its input groups expand into, in expansion order.
    pub subcases: Vec<ExpandedSubCase>,
}

/// One listed sub-case: display name and parameter bindings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpandedSubCase {
    /// Display name, Q-05 format `{test}/{input}#{index}[k=v,...]`.
    pub name: String,
    /// The concrete parameter bindings of this sub-case.
    pub bindings: BTreeMap<String, ConcreteValue>,
}

/// Renders bindings as a JSON object; integers as numbers, strings as
/// strings, keys in sorted order (the map is ordered).
fn bindings_json(bindings: &BTreeMap<String, ConcreteValue>) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = bindings
        .iter()
        .map(|(name, value)| {
            let value = match value {
                ConcreteValue::Int(n) => json!(n),
                ConcreteValue::Str(s) => json!(s),
            };
            (name.clone(), value)
        })
        .collect();
    serde_json::Value::Object(map)
}

/// Runs the `ccaller expand` pipeline over two files.
///
/// Stages: load → parse → validate (FR-C-08/09) →, when validation is
/// clean, expand every input group (FR-C-03~06). Def-use analysis is not
/// part of the listing; `check` owns that gate.
///
/// # Errors
/// Returns [`CoreError::Io`] when a file cannot be read; an unreadable
/// file is an environment problem, not a config finding, so it aborts
/// instead of degrading the report.
pub fn run_expand(lib_path: &Path, cases_path: &Path) -> Result<ExpandReport, CoreError> {
    let lib_doc = SourceDoc::load(lib_path)?;
    let cases_doc = SourceDoc::load(cases_path)?;
    Ok(expand_docs(&lib_doc, &cases_doc))
}

/// The pipeline over already-loaded documents.
fn expand_docs(lib_doc: &SourceDoc, cases_doc: &SourceDoc) -> ExpandReport {
    let mut errors = Vec::new();
    let mut tests = Vec::new();

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
        errors.extend(lib_desc::validate(libs, lib_doc));
    }
    if let (Some(libs), Some(cases)) = (&libs, &cases) {
        errors.extend(validate_cases(cases, cases_doc, libs));
        if errors.is_empty() {
            for test in &cases.tests {
                let test = test.get_ref();
                let subcases = expand_test(test, &cases.shared_inputs, cases_doc, &mut errors);
                tests.push(ExpandedTest {
                    name: test.name.get_ref().clone(),
                    subcases: subcases
                        .iter()
                        .map(|subcase| ExpandedSubCase {
                            name: subcase.name.clone(),
                            bindings: subcase.bindings.clone(),
                        })
                        .collect(),
                });
            }
        }
    }

    ExpandReport {
        schema: EXPAND_SCHEMA,
        errors,
        tests,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::diag::codes;

    use serde_json::Value;

    /// The library description of requirement spec 7.2, verbatim.
    const LIBS_TOML: &str = r#"version = 1

[[libs]]
path = "libc_wrapper.so"
funcs = [
  { name = "Call_setup",      paras = ["mode"] },
  { name = "Call_teardown",   paras = [] },
  { name = "Call_write8",     paras = ["out_idx", "byte"], slot_roles = { out_idx = "write" } },
  { name = "Call_read8",      paras = ["in_idx"],          slot_roles = { in_idx = "read" } },
]
"#;

    /// Runs the pipeline over in-memory documents.
    fn report(case_text: &str) -> ExpandReport {
        let lib_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: LIBS_TOML.to_string(),
        };
        let case_doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: case_text.to_string(),
        };
        expand_docs(&lib_doc, &case_doc)
    }

    #[test]
    fn expansion_lists_subcases_with_bindings__F_C_03() {
        let report = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_teardown\", expect_eq = 0 }]\n\
             [[tests.inputs]]\n\
             name = \"ipt1\"\n\
             args = { a = [1, 2], b = [\"'x'\"] }\n",
        );
        assert!(report.errors.is_empty(), "findings: {report:?}");
        assert!(report.ok());
        assert_eq!(report.tests.len(), 1);
        assert_eq!(report.tests[0].name, "t");
        let names: Vec<&str> = report.tests[0]
            .subcases
            .iter()
            .map(|subcase| subcase.name.as_str())
            .collect();
        // Sorted-name order: `a` varies slowest (FR-C-05).
        assert_eq!(names, vec!["t/ipt1#0[a=1,b='x']", "t/ipt1#1[a=2,b='x']"]);
        let bindings = &report.tests[0].subcases[0].bindings;
        assert_eq!(
            bindings.get("a"),
            Some(&ConcreteValue::Int(1)),
            "bindings must carry the concrete values"
        );
        assert_eq!(
            bindings.get("b"),
            Some(&ConcreteValue::Str("x".to_string())),
            "string parameters keep their quoted form in bindings"
        );
    }

    #[test]
    fn a_test_without_inputs_lists_one_bare_subcase__F_C_03() {
        let report = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"bare\"\n\
             cmds = [{ opfunc = \"Call_teardown\", expect_eq = 0 }]\n",
        );
        assert!(report.ok());
        assert_eq!(report.tests[0].subcases.len(), 1);
        assert_eq!(report.tests[0].subcases[0].name, "bare");
        assert!(report.tests[0].subcases[0].bindings.is_empty());
    }

    #[test]
    fn validation_findings_block_the_listing__F_C_08() {
        let report = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_nope\", expect_eq = 0 }]\n",
        );
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|d| d.code == codes::UNKNOWN_OPFUNC));
        assert!(report.tests.is_empty());
    }

    #[test]
    fn expansion_findings_keep_other_tests_listed__F_C_06() {
        // The bad test's sub-case is dropped but the good one still
        // lists; the finding names the cause.
        let report = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"t_bad\"\n\
             cmds = [{ opfunc = \"Call_teardown\", expect_eq = \"$ghost\" }]\n\
             [[tests]]\n\
             name = \"t_ok\"\n\
             cmds = [{ opfunc = \"Call_teardown\", expect_eq = 0 }]\n",
        );
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|d| d.code == codes::UNRESOLVED_VAR));
        assert_eq!(report.tests.len(), 2);
        assert!(report.tests[0].subcases.is_empty());
        assert_eq!(report.tests[1].subcases.len(), 1);
    }

    #[test]
    fn to_json_renders_the_contract_shape__F_C_03() {
        let listing = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_teardown\", expect_eq = 0 }]\n\
             [[tests.inputs]]\n\
             name = \"ipt1\"\n\
             args = { a = [1, 2] }\n",
        );
        let json: Value = serde_json::from_str(&listing.to_json()).unwrap();
        assert_eq!(json["schema"].as_u64(), Some(1));
        assert_eq!(json["ok"].as_bool(), Some(true));
        assert_eq!(json["errors"].as_array().map(Vec::len), Some(0));
        assert_eq!(json["tests"][0]["name"].as_str(), Some("t"));
        let subcase = &json["tests"][0]["subcases"][0];
        assert_eq!(subcase["name"].as_str(), Some("t/ipt1#0[a=1]"));
        assert_eq!(subcase["bindings"]["a"].as_i64(), Some(1));

        // The failing side mirrors check's error objects.
        let failing = report(
            "version = 1\n\
             [[tests]]\n\
             name = \"t\"\n\
             cmds = [{ opfunc = \"Call_nope\", expect_eq = 0 }]\n",
        );
        let json: Value = serde_json::from_str(&failing.to_json()).unwrap();
        assert_eq!(json["ok"].as_bool(), Some(false));
        assert_eq!(json["errors"][0]["code"].as_str(), Some("unknown_opfunc"));
        assert!(json["errors"][0]["line"].as_u64().is_some());
    }

    #[test]
    fn unreadable_file_aborts_with_io_error__F_C_09() {
        let missing = std::env::temp_dir().join("ccaller/no-such-dir/cases.toml");
        let result = run_expand(&missing, &missing);
        assert!(matches!(result, Err(CoreError::Io { .. })));
    }
}
