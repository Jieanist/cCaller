//! The `check` pipeline: every load-time stage glued into one report
//! (requirement spec 7.6, FR-X-03).
//!
//! [`run_check`] loads and parses both documents, validates them
//! (FR-C-08/09), and — only when validation is clean — expands every
//! input group (FR-C-03~06) and runs the slot def-use analysis
//! (FR-C-10). The gate keeps a malformed entry from being reported
//! twice: a test whose `opfunc` is unknown, say, would also fail
//! expansion and def-use with no new information.
//!
//! [`CheckReport`] is the in-memory form of the 7.6 JSON contract;
//! [`CheckReport::to_json`] renders it.

use std::path::Path;

use serde_json::json;

use crate::error::CoreError;

use super::cases::{CaseConfig, TestDef};
use super::defuse::analyze_def_use;
use super::diag::Diagnostic;
use super::expand::{expand_test, SubCase};
use super::lib_desc::{self, LibDescription};
use super::source::SourceDoc;
use super::validate::validate_cases;

/// JSON contract version of the check report (requirement spec 7.6).
///
/// The contract version evolves together with the configuration schema
/// version; both are `1` today.
pub const REPORT_SCHEMA: u64 = 1;

/// Result of one `ccaller check` run.
///
/// The in-memory form of the 7.6 JSON contract: findings accumulate
/// from every stage, and the stats describe the run the configuration
/// would produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// JSON contract version; always [`REPORT_SCHEMA`] today.
    pub schema: u64,
    /// Every finding, in discovery order.
    pub errors: Vec<Diagnostic>,
    /// Size of the run the configuration describes.
    pub stats: CheckStats,
}

impl CheckReport {
    /// Whether the configuration passed every load-time check.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Renders the machine-readable JSON of requirement spec 7.6.
    ///
    /// The report is built through [`serde_json::Value`], whose
    /// `Display` cannot fail — the shape is plain scalars and strings,
    /// so rendering needs no unwrap. Object key order is not part of
    /// the contract; consumers must parse the JSON, not grep it.
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
        json!({
            "schema": self.schema,
            "ok": self.ok(),
            "errors": errors,
            "stats": {
                "tests": self.stats.tests,
                "subcases": self.stats.subcases,
                "cmds": self.stats.cmds,
            },
        })
        .to_string()
    }
}

/// Run-size counters of a [`CheckReport`] (requirement spec 7.6 `stats`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckStats {
    /// Number of declared tests.
    pub tests: usize,
    /// Number of expanded sub-cases; stays 0 while earlier findings
    /// block expansion.
    pub subcases: usize,
    /// Test commands across all sub-cases. Env commands are excluded:
    /// they frame the run, they are not part of any test.
    pub cmds: usize,
}

/// Runs the whole `ccaller check` pipeline over two files.
///
/// Stages: load → parse → validate (FR-C-08/09) →, when validation is
/// clean, expand every input group (FR-C-03~06) and analyze slot
/// def-use (FR-C-10). Findings from all stages accumulate into one
/// report, so a single run reports everything fixable at once.
///
/// # Errors
/// Returns [`CoreError::Io`] when a file cannot be read; an unreadable
/// file is an environment problem, not a config finding, so it aborts
/// instead of degrading the report.
pub fn run_check(lib_path: &Path, cases_path: &Path) -> Result<CheckReport, CoreError> {
    let lib_doc = SourceDoc::load(lib_path)?;
    let cases_doc = SourceDoc::load(cases_path)?;
    Ok(check_docs(&lib_doc, &cases_doc))
}

/// The pipeline over already-loaded documents.
fn check_docs(lib_doc: &SourceDoc, cases_doc: &SourceDoc) -> CheckReport {
    let mut errors = Vec::new();
    let mut stats = CheckStats::default();

    let libs = match lib_doc.parse::<LibDescription>() {
        Ok(libs) => Some(libs),
        Err(diag) => {
            errors.push(diag);
            None
        }
    };
    let cases = match cases_doc.parse::<CaseConfig>() {
        Ok(cases) => {
            stats.tests = cases.tests.len();
            Some(cases)
        }
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
            let expansions: Vec<(&TestDef, Vec<SubCase>)> = cases
                .tests
                .iter()
                .map(|test| {
                    let subcases =
                        expand_test(test.get_ref(), &cases.shared_inputs, cases_doc, &mut errors);
                    (test.get_ref(), subcases)
                })
                .collect();
            stats.subcases = expansions.iter().map(|(_, sub)| sub.len()).sum();
            stats.cmds = expansions
                .iter()
                .flat_map(|(_, sub)| sub.iter())
                .map(|sub| sub.cmds.len())
                .sum();
            analyze_def_use(cases, &expansions, libs, cases_doc, &mut errors);
        }
    }

    CheckReport {
        schema: REPORT_SCHEMA,
        errors,
        stats,
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
  { name = "Call_ctx_new",    paras = ["out_idx"],         slot_roles = { out_idx = "write" } },
  { name = "Call_ctx_free",   paras = ["in_idx"],          slot_roles = { in_idx = "read" } },
  { name = "Call_thr_init",   paras = [] },
  { name = "Call_thr_fini",   paras = [] },
  { name = "Call_sock_open",  paras = ["out_fd_idx"],      slot_roles = { out_fd_idx = "write" } },
  { name = "Call_sock_close", paras = ["in_fd_idx"],       slot_roles = { in_fd_idx = "read" } },
  { name = "Call_malloc",     paras = ["len", "mem_idx"],  slot_roles = { mem_idx = "write" } },
  { name = "Call_read32",     paras = ["addr_idx"],        slot_roles = { addr_idx = "read" } },
  { name = "Call_write8",     paras = ["out_idx", "byte"], slot_roles = { out_idx = "write" } },
  { name = "Call_read8",      paras = ["in_idx"],          slot_roles = { in_idx = "read" } },
]
"#;

    /// The case configuration of requirement spec 7.3, verbatim
    /// (comments dropped); the spec requires this pair to pass
    /// `ccaller check`.
    const CASES_TOML: &str = r#"version = 1
default_serial = false
debug_test = []

[env]
init = [{ opfunc = "Call_setup",    args = ["mode=1"] }]
exit = [{ opfunc = "Call_teardown", args = [] }]

[process_env]
init = [{ opfunc = "Call_ctx_new",  args = ["out_idx=0"] }]
exit = [{ opfunc = "Call_ctx_free", args = ["in_idx=0"] }]

[thread_env]
init = [{ opfunc = "Call_thr_init", args = [] }]
exit = [{ opfunc = "Call_thr_fini", args = [] }]

[shared_inputs.common]
val = ["888", "999"]

[[envs]]
name = "with_socket"
init = [{ opfunc = "Call_sock_open",  args = ["out_fd_idx=2"] }]
exit = [{ opfunc = "Call_sock_close", args = ["in_fd_idx=2"] }]
tests = ["test_sock"]

[[tests]]
name = "test_rw_u32"
thread_num = 2
break_if_fail = true
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0,      args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = "$val", args = ["addr_idx=1"], perf = true },
]
[[tests.inputs]]
name = "ipt1"
refs  = ["common"]

[[tests]]
name = "test_sock"
cmds = [
  { opfunc = "Call_write8", expect_eq = 0,      args = ["out_idx=3", "byte=0x5A"] },
  { opfunc = "Call_read8",  expect_ne = "0x00", args = ["in_idx=3"] },
]

[[tests]]
name = "test_range"
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
tests = ["test_sock"]
"#;

    /// Runs the pipeline over in-memory documents.
    fn report(lib_text: &str, case_text: &str) -> CheckReport {
        let lib_doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: lib_text.to_string(),
        };
        let case_doc = SourceDoc {
            path: "cases.toml".to_string(),
            text: case_text.to_string(),
        };
        check_docs(&lib_doc, &case_doc)
    }

    #[test]
    fn spec_example_passes_check_with_exact_stats__F_X_03() {
        let report = report(LIBS_TOML, CASES_TOML);
        assert!(report.errors.is_empty(), "unexpected findings: {report:?}");
        assert!(report.ok());
        // rw_u32: 2 sub-cases (val), sock: 1, range: 3 (addr 0/4/8);
        // every test has 2 commands; the 8 env commands are excluded.
        assert_eq!(
            report.stats,
            CheckStats {
                tests: 3,
                subcases: 6,
                cmds: 12
            }
        );
    }

    #[test]
    fn validation_findings_block_expansion__F_C_08() {
        let cases = "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", \
                     expect_eq = 0, args = [] }]\n";
        let report = report(LIBS_TOML, cases);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|d| d.code == codes::UNKNOWN_OPFUNC));
        assert_eq!(
            report.stats,
            CheckStats {
                tests: 1,
                subcases: 0,
                cmds: 0
            }
        );
    }

    #[test]
    fn expansion_failure_drops_only_the_affected_test__F_C_06() {
        let cases = "version = 1\n[[tests]]\nname = \"t_ok\"\ncmds = [{ opfunc = \"Call_setup\", \
                     expect_eq = 0, args = [\"mode=1\"] }]\n[[tests]]\nname = \"t_bad\"\ncmds = [{ \
                     opfunc = \"Call_setup\", expect_eq = 0, args = [\"mode=$ghost\"] }]\n";
        let report = report(LIBS_TOML, cases);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|d| d.code == codes::UNRESOLVED_VAR));
        // t_bad's only sub-case was dropped; t_ok still counts.
        assert_eq!(
            report.stats,
            CheckStats {
                tests: 2,
                subcases: 1,
                cmds: 1
            }
        );
    }

    #[test]
    fn def_use_findings_keep_the_stats__F_C_10() {
        let cases = "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_read8\", \
                     expect_ne = \"0x00\", args = [\"in_idx=3\"] }]\n";
        let report = report(LIBS_TOML, cases);
        assert!(!report.ok());
        assert!(report
            .errors
            .iter()
            .any(|d| d.code == codes::SLOT_READ_BEFORE_WRITE));
        // Expansion already ran; the def-use finding does not zero it.
        assert_eq!(
            report.stats,
            CheckStats {
                tests: 1,
                subcases: 1,
                cmds: 1
            }
        );
    }

    #[test]
    fn lib_parse_failure_blocks_cross_validation__F_C_09() {
        // The lib entry misses `path` → a parse finding. The cases file
        // also references an unknown function, but validate_cases
        // needs a parsed lib model, so that finding must not appear.
        let bad_libs = "version = 1\n[[libs]]\nfuncs = []\n";
        let cases = "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", \
                     expect_eq = 0, args = [] }]\n";
        let report = report(bad_libs, cases);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].code, codes::MISSING_FIELD);
        assert_eq!(
            report.stats,
            CheckStats {
                tests: 1,
                subcases: 0,
                cmds: 0
            }
        );
    }

    #[test]
    fn to_json_renders_the_76_contract__F_X_03() {
        // Failing side: one def-use finding at a known place.
        let cases = "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_read8\", \
                     expect_ne = \"0x00\", args = [\"in_idx=3\"] }]\n";
        let failing = report(LIBS_TOML, cases);
        let json: Value = serde_json::from_str(&failing.to_json()).unwrap();
        assert_eq!(json["schema"].as_u64(), Some(1));
        assert_eq!(json["ok"].as_bool(), Some(false));
        let error = &json["errors"][0];
        assert_eq!(error["code"].as_str(), Some("slot_read_before_write"));
        assert_eq!(error["file"].as_str(), Some("cases.toml"));
        assert_eq!(error["line"].as_u64(), Some(4));
        assert!(error["column"].as_u64().is_some());
        assert!(error["message"].as_str().unwrap().contains("slot 3"));
        assert_eq!(json["stats"]["tests"].as_u64(), Some(1));
        assert_eq!(json["stats"]["subcases"].as_u64(), Some(1));
        assert_eq!(json["stats"]["cmds"].as_u64(), Some(1));

        // Passing side: no findings, empty array, stats intact.
        let passing = report(LIBS_TOML, CASES_TOML);
        let json: Value = serde_json::from_str(&passing.to_json()).unwrap();
        assert_eq!(json["ok"].as_bool(), Some(true));
        assert_eq!(json["errors"].as_array().map(Vec::len), Some(0));
        assert_eq!(json["stats"]["subcases"].as_u64(), Some(6));
    }

    #[test]
    fn unreadable_file_aborts_with_io_error__F_C_09() {
        let missing = std::env::temp_dir().join("ccaller/no-such-dir/cases.toml");
        let result = run_check(&missing, &missing);
        assert!(matches!(result, Err(CoreError::Io { .. })));
    }
}
