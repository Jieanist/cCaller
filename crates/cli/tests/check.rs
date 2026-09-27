//! End-to-end checks of the `check` subcommand (FR-X-03, FR-X-02).

// Integration tests are separate crates, so the crate-level test
// exemptions in main.rs do not reach here; style guide section 6.3
// allows unwrap/expect/panic freely in test code, and the __F_xx_nn
// suffixes from verification plan section 9.1 are upper-case on purpose.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    non_snake_case
)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const LIBS_TOML: &str = r#"version = 1

[[libs]]
path = "wrapper.dll"
funcs = [
  { name = "Call_malloc", paras = ["len", "mem_idx"], slot_roles = { mem_idx = "write" } },
  { name = "Call_read32", paras = ["addr_idx"],       slot_roles = { addr_idx = "read" } },
]
"#;

/// Two tests: `t_in` expands to 2 sub-cases via refs, `t_plain` to 1;
/// every test has 2 commands → stats {tests: 2, subcases: 3, cmds: 6}.
const CASES_TOML: &str = r#"version = 1

[shared_inputs.common]
val = ["7", "8"]

[[tests]]
name = "t_in"
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0, args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = 7, args = ["addr_idx=1"] },
]
[[tests.inputs]]
name = "ipt1"
refs = ["common"]

[[tests]]
name = "t_plain"
cmds = [
  { opfunc = "Call_malloc", expect_eq = 0, args = ["len=100", "mem_idx=1"] },
  { opfunc = "Call_read32", expect_eq = 7, args = ["addr_idx=1"] },
]
"#;

fn ccaller() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ccaller"))
}

/// Writes `text` to a uniquely named file under the test temp dir.
fn config(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("ccaller-cli-check");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn configs(name: &str) -> (PathBuf, PathBuf) {
    (
        config(&format!("{name}-libs.toml"), LIBS_TOML),
        config(&format!("{name}-cases.toml"), CASES_TOML),
    )
}

fn missing(name: &str) -> PathBuf {
    std::env::temp_dir().join("ccaller-cli-check").join(name)
}

#[test]
fn check_clean_config_prints_summary_and_exits_zero__F_X_03() {
    let (lib, cases) = configs("clean");
    let output = ccaller()
        .arg("--test")
        .arg(&cases)
        .arg("--lib")
        .arg(&lib)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ok: 2 tests, 3 subcases, 6 commands"),
        "stdout was: {stdout}"
    );
}

#[test]
fn check_json_output_matches_the_76_contract__F_X_03() {
    let (lib, cases) = configs("json");
    let output = ccaller()
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg("--test")
        .arg(&cases)
        .arg("--lib")
        .arg(&lib)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(json["schema"].as_u64(), Some(1));
    assert_eq!(json["ok"].as_bool(), Some(true));
    assert_eq!(json["errors"].as_array().map(Vec::len), Some(0));
    assert_eq!(json["stats"]["tests"].as_u64(), Some(2));
    assert_eq!(json["stats"]["subcases"].as_u64(), Some(3));
    assert_eq!(json["stats"]["cmds"].as_u64(), Some(6));
}

#[test]
fn check_reports_findings_in_text_and_exits_two__F_X_02() {
    let (lib, _) = configs("bad-text");
    let cases = config(
        "bad-text-cases.toml",
        "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", expect_eq = 0, \
         args = [] }]\n",
    );
    let output = ccaller()
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&lib)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("error[unknown_opfunc]:"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("error: 1 finding(s)"),
        "stdout was: {stdout}"
    );
}

#[test]
fn check_json_reports_findings_and_exits_two__F_X_02() {
    let (lib, _) = configs("bad-json");
    let cases = config(
        "bad-json-cases.toml",
        "version = 1\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", expect_eq = 0, \
         args = [] }]\n",
    );
    let output = ccaller()
        .arg("check")
        .arg("--format")
        .arg("json")
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&lib)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let json: Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(json["ok"].as_bool(), Some(false));
    let error = &json["errors"][0];
    assert_eq!(error["code"].as_str(), Some("unknown_opfunc"));
    assert_eq!(error["file"].as_str(), Some(cases.to_str().unwrap()));
    assert!(error["line"].as_u64().is_some());
}

#[test]
fn check_without_test_and_lib_exits_two__F_X_01() {
    let output = ccaller().arg("check").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--test") && stderr.contains("--lib"),
        "stderr was: {stderr}"
    );
}

#[test]
fn check_accepts_legacy_long_aliases__F_X_01() {
    // --test_case/--input must keep working (FR-X-01 legacy aliases).
    let (lib, cases) = configs("legacy");
    let output = ccaller()
        .arg("--test_case")
        .arg(&cases)
        .arg("--input")
        .arg(&lib)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_missing_file_exits_two__F_X_02() {
    let (lib, _) = configs("missing-file");
    let ghost = missing("no-such-cases.toml");
    let output = ccaller()
        .arg("-t")
        .arg(&ghost)
        .arg("-i")
        .arg(&lib)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"), "stderr was: {stderr}");
}
