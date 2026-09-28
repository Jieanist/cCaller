//! End-to-end checks of the ccaller binary surface (M0 skeleton).

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

use std::path::{Path, PathBuf};
use std::process::Command;

fn ccaller() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ccaller"))
}

/// The M4 expand acceptance corpus (commit c96ef4a): three case files
/// whose comments pin the exact expanded sub-case names.
fn expand_corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/expand")
        .join(name)
}

/// Runs `expand` over one corpus file and returns (exit code, stdout).
fn expand_corpus_output(file: &str, format: &[&str]) -> (Option<i32>, String) {
    let output = ccaller()
        .arg("-t")
        .arg(expand_corpus(file))
        .arg("-i")
        .arg(expand_corpus("libs.toml"))
        .arg("expand")
        .args(format)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn version_flag_prints_binary_name_and_exits_zero__F_G_03() {
    let output = ccaller().arg("--version").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("ccaller"), "stdout was: {stdout}");
}

#[test]
fn unknown_flag_exits_with_config_error_code__F_G_03() {
    // Usage mistakes share exit code 2 with load-time config errors.
    let output = ccaller().arg("--definitely-not-a-flag").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn invalid_log_level_exits_with_config_error_code__F_R_02() {
    let output = ccaller().args(["--log", "verbose"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid log level"), "stderr was: {stderr}");
}

#[test]
fn help_flag_prints_help_and_exits_zero__F_X_01() {
    // `run` is now the default subcommand, so the way to ask for help is
    // an explicit --help, not a bare invocation (FR-X-01).
    let output = ccaller().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: ccaller"), "stdout was: {stdout}");
}

#[test]
fn bare_invocation_requires_test_and_lib__F_X_01() {
    // The default `run` subcommand needs both --test and --lib; a bare
    // invocation is therefore a usage error (exit 2), not help (FR-X-01).
    let output = ccaller().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--test") && stderr.contains("--lib"),
        "stderr was: {stderr}"
    );
}

#[test]
fn run_subcommand_exposes_help__F_X_01() {
    // The ABI conformance kit probes `ccaller run --help` to detect that
    // the subcommand exists, so this must succeed without side effects.
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "stdout was: {stdout}");
    assert!(stdout.contains("run"), "stdout was: {stdout}");
}

#[test]
fn run_help_lists_serial_flag__F_T_09() {
    // The `--serial` override (FR-T-09) is part of the run surface, so
    // the plumbing is observable through the generated help.
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--serial"), "stdout was: {stdout}");
}

#[test]
fn help_lists_debug_and_max_thread_flags__F_T_08_F_T_04() {
    // The global -d/--debug (FR-T-08) and -m/--max-thread (FR-T-04)
    // options are part of the run surface and appear in the top-level
    // help.
    let output = ccaller().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--debug"), "stdout was: {stdout}");
    assert!(stdout.contains("--max-thread"), "stdout was: {stdout}");
    // The internal isolation flags are hidden from users (FR-T-05).
    assert!(!stdout.contains("--isolate-test"), "stdout was: {stdout}");
}

#[test]
fn max_thread_zero_is_a_usage_error__F_T_04() {
    // A zero cap would silently disable parallelism; clap's range
    // parser rejects it with the usage exit code.
    let output = ccaller().args(["--max-thread", "0"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value"), "stderr was: {stderr}");
}

#[test]
fn run_help_lists_the_format_flag__F_R_06() {
    let output = ccaller().arg("run").arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--format"), "stdout was: {stdout}");
}

#[test]
fn unknown_debug_name_exits_two_before_any_execution__F_T_08() {
    // The selection is resolved before libraries load, so a typo'd -d
    // name is reported as the user mistake it is - the library file
    // never needs to exist, but its declaration must validate.
    let dir = std::env::temp_dir().join("ccaller-cli-debug");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = dir.join("cases.toml");
    let libs = dir.join("libs.toml");
    std::fs::write(
        &cases,
        "version = 1\n\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_ok\", expect_eq = 0 }]\n",
    )
    .unwrap();
    std::fs::write(
        &libs,
        "version = 1\n\n[[libs]]\npath = \"missing.so\"\nfuncs = [{ name = \"Call_ok\", paras = [] }]\n",
    )
    .unwrap();
    let output = ccaller()
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&libs)
        .arg("-d")
        .arg("ghost")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ghost"), "stderr was: {stderr}");
}

#[test]
fn isolate_flags_come_in_pairs__F_T_05() {
    // The hidden isolation flags form one protocol; one without the
    // other is a usage error rather than a half-run. The config files
    // are never touched (the pair check fires first).
    let dummy = std::env::temp_dir().join("ccaller-cli-nothing.toml");
    let output = ccaller()
        .arg("-t")
        .arg(&dummy)
        .arg("-i")
        .arg(&dummy)
        .args(["run", "--isolate-test", "t"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("together"), "stderr was: {stderr}");
}

#[test]
fn relaunch_protocol_reaches_the_child_and_exits_normally__F_T_05() {
    // The launcher's re-execution protocol, exercised against the real
    // binary: the child of a death test runs exactly one sub-case and
    // exits through the normal run codes. A missing config file makes
    // the child exit 2 (an environment error) - a normal exit, which is
    // the "survived" answer the parent expects when nothing crashed.
    let dir = std::env::temp_dir().join("ccaller-cli-relaunch");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("missing.toml");
    let output = ccaller()
        .arg("-t")
        .arg(&missing)
        .arg("-i")
        .arg(&missing)
        .arg("run")
        .arg("--isolate-test")
        .arg("t")
        .arg("--isolate-subcase")
        .arg("t")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"), "stderr was: {stderr}");
}

#[test]
fn expand_lists_refs_and_ranges_exactly__F_C_03_F_C_06() {
    // The acceptance corpus pins the expected names: a refs group with
    // a list value, and an own-args range whose bounds reference
    // single-valued shared parameters (which also appear as dimensions).
    let (code, stdout) = expand_corpus_output("refs_list.toml", &[]);
    assert_eq!(code, Some(0), "stdout was: {stdout}");
    let expected = "\
t_refs: 2 subcase(s)
  t_refs/ipt#0[val=888]  val=888
  t_refs/ipt#1[val=999]  val=999
t_own_var_range: 3 subcase(s)
  t_own_var_range/ipt#0[hi=6,lo=0,off=0,st=3]  hi=6 lo=0 off=0 st=3
  t_own_var_range/ipt#1[hi=6,lo=0,off=3,st=3]  hi=6 lo=0 off=3 st=3
  t_own_var_range/ipt#2[hi=6,lo=0,off=6,st=3]  hi=6 lo=0 off=6 st=3
";
    assert_eq!(stdout, expected);
}

#[test]
fn expand_lists_the_cartesian_product_in_sorted_order__F_C_05() {
    // FR-C-05: within one group the first sorted parameter varies
    // slowest, so x=1 walks every y before x=2 starts.
    let (code, stdout) = expand_corpus_output("range_cartesian.toml", &[]);
    assert_eq!(code, Some(0), "stdout was: {stdout}");
    let expected = "\
t_cartesian: 6 subcase(s)
  t_cartesian/ipt#0[x=1,y=0]  x=1 y=0
  t_cartesian/ipt#1[x=1,y=2]  x=1 y=2
  t_cartesian/ipt#2[x=1,y=4]  x=1 y=4
  t_cartesian/ipt#3[x=2,y=0]  x=2 y=0
  t_cartesian/ipt#4[x=2,y=2]  x=2 y=2
  t_cartesian/ipt#5[x=2,y=4]  x=2 y=4
";
    assert_eq!(stdout, expected);
}

#[test]
fn expand_lists_groups_independently_in_declaration_order__F_C_03() {
    // Multiple input groups expand independently (no cross-group
    // cartesian product) and list in declaration order.
    let (code, stdout) = expand_corpus_output("multi_group.toml", &[]);
    assert_eq!(code, Some(0), "stdout was: {stdout}");
    let expected = "\
t_multi: 3 subcase(s)
  t_multi/alpha#0[p=10]  p=10
  t_multi/beta#0[p=20]  p=20
  t_multi/beta#1[p=21]  p=21
";
    assert_eq!(stdout, expected);
}

#[test]
fn expand_json_matches_the_check_contract_style__F_R_06() {
    let (code, stdout) = expand_corpus_output("multi_group.toml", &["--format", "json"]);
    assert_eq!(code, Some(0), "stdout was: {stdout}");
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema"].as_u64(), Some(1));
    assert_eq!(json["ok"].as_bool(), Some(true));
    assert_eq!(json["errors"].as_array().map(Vec::len), Some(0));
    assert_eq!(json["tests"][0]["name"].as_str(), Some("t_multi"));
    let subcases = json["tests"][0]["subcases"].as_array().unwrap();
    assert_eq!(subcases.len(), 3);
    assert_eq!(subcases[0]["name"].as_str(), Some("t_multi/alpha#0[p=10]"));
    assert_eq!(subcases[0]["bindings"]["p"].as_i64(), Some(10));
}

#[test]
fn expand_reports_findings_and_exits_two__F_C_08() {
    let dir = std::env::temp_dir().join("ccaller-cli-expand-bad");
    std::fs::create_dir_all(&dir).unwrap();
    let cases = dir.join("cases.toml");
    let libs = dir.join("libs.toml");
    std::fs::write(
        &cases,
        "version = 1\n\n[[tests]]\nname = \"t\"\ncmds = [{ opfunc = \"Call_nope\", expect_eq = 0 }]\n",
    )
    .unwrap();
    std::fs::write(
        &libs,
        "version = 1\n\n[[libs]]\npath = \"missing.so\"\nfuncs = [{ name = \"Call_ok\", paras = [] }]\n",
    )
    .unwrap();
    let output = ccaller()
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&libs)
        .arg("expand")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown_opfunc"), "stderr was: {stderr}");
}

#[test]
fn expand_requires_both_files__F_X_02() {
    let output = ccaller().arg("expand").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--test"), "stderr was: {stderr}");
}

#[test]
fn init_scaffolds_a_checkable_project__F_X_04() {
    let dir = std::env::temp_dir().join(format!("ccaller-cli-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let output = ccaller()
        .arg("init")
        .arg(&dir)
        .arg("--build-sh")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "output: {output:?}");
    assert!(dir.join("libs.toml").exists());
    assert!(dir.join("cases.toml").exists());
    assert!(dir.join("build.sh").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("created"), "stdout was: {stdout}");

    // The scaffold is immediately checkable: same pair, same verdict.
    let check = ccaller()
        .arg("-t")
        .arg(dir.join("cases.toml"))
        .arg("-i")
        .arg(dir.join("libs.toml"))
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(0), "check output: {check:?}");
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(
        stdout.contains("ok: 1 tests, 2 subcases"),
        "stdout was: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_refuses_to_silently_overwrite__F_X_04() {
    let dir = std::env::temp_dir().join(format!("ccaller-cli-init-exists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libs.toml"), "precious").unwrap();
    let output = ccaller().arg("init").arg(&dir).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already exists") && stderr.contains("libs.toml"),
        "stderr was: {stderr}"
    );
    // Nothing was overwritten and nothing else was written.
    assert_eq!(
        std::fs::read_to_string(dir.join("libs.toml")).unwrap(),
        "precious"
    );
    assert!(!dir.join("cases.toml").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_defaults_to_the_current_directory__F_X_04() {
    let dir = std::env::temp_dir().join(format!("ccaller-cli-init-dot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ccaller"))
        .current_dir(&dir)
        .arg("init")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "output: {output:?}");
    assert!(dir.join("cases.toml").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fmt_prints_the_canonical_form_to_stdout__F_X_04() {
    let dir = std::env::temp_dir().join("ccaller-cli-fmt");
    std::fs::create_dir_all(&dir).unwrap();
    let messy = dir.join("messy.toml");
    std::fs::write(
        &messy,
        "zz = \"last\"\nversion = 1\n\n[shared_inputs.common]\nval = [\"888\", \"999\"]\n",
    )
    .unwrap();
    let output = ccaller().arg("fmt").arg(&messy).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Keys sorted, tables laid out uniformly.
    assert!(
        stdout.starts_with("version = 1\nzz = \"last\"\n"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("[shared_inputs.common]\nval = [\"888\", \"999\"]\n"));
    // The original file is untouched without --in-place.
    assert!(std::fs::read_to_string(&messy).unwrap().starts_with("zz ="));
}

#[test]
fn fmt_in_place_rewrites_the_file_idempotently__F_X_04() {
    let dir = std::env::temp_dir().join("ccaller-cli-fmt-in-place");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cases.toml");
    std::fs::copy(expand_corpus("multi_group.toml"), &file).unwrap();
    let first = ccaller()
        .arg("fmt")
        .arg(&file)
        .arg("--in-place")
        .output()
        .unwrap();
    assert_eq!(first.status.code(), Some(0), "output: {first:?}");
    let once = std::fs::read_to_string(&file).unwrap();
    // A second pass is a no-op: the canonical form is a fixed point.
    let second = ccaller()
        .arg("fmt")
        .arg(&file)
        .arg("--in-place")
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(0));
    let twice = std::fs::read_to_string(&file).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn fmt_preserves_check_semantics__F_X_04() {
    // The normalization contract: after fmt, the same configuration
    // passes check with the same stats.
    let dir = std::env::temp_dir().join("ccaller-cli-fmt-semantics");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cases.toml");
    std::fs::copy(expand_corpus("refs_list.toml"), &file).unwrap();
    let libs = dir.join("libs.toml");
    std::fs::copy(expand_corpus("libs.toml"), &libs).unwrap();
    let before = ccaller()
        .arg("-t")
        .arg(&file)
        .arg("-i")
        .arg(&libs)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(before.status.code(), Some(0));
    let fmt_run = ccaller()
        .arg("fmt")
        .arg(&file)
        .arg("--in-place")
        .output()
        .unwrap();
    assert_eq!(fmt_run.status.code(), Some(0));
    let after = ccaller()
        .arg("-t")
        .arg(&file)
        .arg("-i")
        .arg(&libs)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(after.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&before.stdout),
        String::from_utf8_lossy(&after.stdout),
        "check must report the same result before and after fmt"
    );
}

#[test]
fn fmt_reports_invalid_toml_with_a_location__F_X_04() {
    let dir = std::env::temp_dir().join("ccaller-cli-fmt-bad");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "version = 1\n[env\ninit = []\n").unwrap();
    let output = ccaller().arg("fmt").arg(&bad).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unclosed table"), "stderr was: {stderr}");
    // The 1-based line of the offending bracket.
    assert!(stderr.contains(":2:"), "stderr was: {stderr}");
}

#[test]
fn fmt_requires_a_file_argument__F_X_04() {
    let output = ccaller().arg("fmt").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("FILE"), "stderr was: {stderr}");
}

#[test]
fn fmt_defaults_to_the_test_flag_value__F_X_04() {
    // `ccaller -t FILE fmt` normalizes the --test file; the positional
    // is a convenience, not a requirement.
    let dir = std::env::temp_dir().join("ccaller-cli-fmt-default");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cases.toml");
    std::fs::write(&file, "zzz = 1\nversion = 1\n").unwrap();
    let output = ccaller().arg("-t").arg(&file).arg("fmt").output().unwrap();
    assert_eq!(output.status.code(), Some(0), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("version = 1\n"), "stdout was: {stdout}");
}

/// A wrapper written the `gen` way: the 22 functions of the committed
/// examples/libc_wrapper description, declared through CCALLER_FUNC,
/// plus the decoys (comments, strings, foreign-platform blocks) the
/// scanner must skip.
const GEN_SAMPLE: &str = r#"#include <stdlib.h>

#include "ccaller_gen.h"

int64_t CCaller_abi_version(void) {
    return CCALLER_ABI_VERSION;
}

/* ---- memory management ---- */

CCALLER_FUNC(malloc, len, mem_idx:write)
{
    (void)param_page; (void)params; (void)param_len;
    return 0;
}

CCALLER_FUNC(free, mem_idx:read)
{
    return 0;
}

// A comment mentioning CCALLER_FUNC(hidden, ghost:read) must not count.

/* A block comment with CCALLER_FUNC(also_hidden, x:read) inside. */

static const char *kNeedle = "CCALLER_FUNC(in_string, x)";

CCALLER_FUNC(memcpy, dst_idx:read, src_idx:read, len)
{
    return 0;
}

CCALLER_FUNC(memset, dst_idx:read, val, len)
{
    return 0;
}

CCALLER_FUNC(memcmp, dst_idx:read, dst_off, src_idx:read, src_off, len)
{
    return 0;
}

CCALLER_FUNC(read8, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read16, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read32, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read64, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(write8, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write16, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write32, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write64, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(strlen, str)
{
    return 0;
}

CCALLER_FUNC(atoi, str)
{
    return 0;
}

CCALLER_FUNC(strcmp, str1, str2)
{
    return 0;
}

CCALLER_FUNC(strncpy, dst_idx:read, str, len)
{
    return 0;
}

CCALLER_FUNC(add, a, b)
{
    return 0;
}

CCALLER_FUNC(store, slot_idx:write, val)
{
    return 0;
}

CCALLER_FUNC(fetch_add, slot_idx:read_write, delta)
{
    return 0;
}

CCALLER_FUNC(skip)
{
    return CCALLER_ERR_SKIP;
}

CCALLER_FUNC(abort)
{
    abort();
    return 0;
}

#ifdef __APPLE__
CCALLER_FUNC(apple_only, never:read)
{
    return 0;
}
#endif

#if defined(__linux__) || defined(_WIN32)
CCALLER_FUNC(unix_or_windows, p:read_write)
{
    return 0;
}
#endif

#if 0
CCALLER_FUNC(dead_code, x)
{
    return 0;
}
#endif
"#;

/// Writes the sample into `dir` and returns its path.
fn write_gen_sample(dir: &Path) -> PathBuf {
    let source = dir.join("libc_wrapper.c");
    std::fs::write(&source, GEN_SAMPLE).unwrap();
    source
}

#[test]
fn gen_writes_libs_toml_from_wrapper_source__F_X_05() {
    let dir = std::env::temp_dir().join("ccaller-cli-gen");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = write_gen_sample(&dir);
    let output = Command::new(env!("CARGO_BIN_EXE_ccaller"))
        .current_dir(&dir)
        .arg("gen")
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("generated 23 function(s)"),
        "stdout was: {stdout}"
    );
    let libs = dir.join("libs.toml");
    let text = std::fs::read_to_string(&libs).unwrap();
    assert!(text.contains("path = \"libc_wrapper.so\""), "text: {text}");
    for name in ["Call_malloc", "Call_read32", "Call_fetch_add", "Call_abort"] {
        assert!(text.contains(&format!("name = \"{name}\"")), "text: {text}");
    }
    assert!(
        text.contains("paras = [\"len\", \"mem_idx\"], slot_roles = { mem_idx = \"write\" }"),
        "text: {text}"
    );
    assert!(
        text.contains("slot_roles = { slot_idx = \"read_write\" }"),
        "text: {text}"
    );
    // The decoys never make it into the description.
    for name in [
        "Call_hidden",
        "Call_also_hidden",
        "Call_in_string",
        "Call_apple_only",
        "Call_dead_code",
    ] {
        assert!(text.find(name).is_none(), "{name} leaked into: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gen_honors_the_output_flag_and_reports_errors__F_X_05() {
    let dir = std::env::temp_dir().join("ccaller-cli-gen-flags");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = write_gen_sample(&dir);
    let custom = dir.join("elsewhere").join("my_libs.toml");
    std::fs::create_dir_all(custom.parent().unwrap()).unwrap();
    let output = ccaller()
        .arg("gen")
        .arg(&source)
        .arg("-o")
        .arg(&custom)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "output: {output:?}");
    assert!(custom.exists());
    assert!(!dir.join("libs.toml").exists());

    // Unreadable source.
    let output = ccaller()
        .arg("gen")
        .arg(dir.join("missing.c"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"), "stderr was: {stderr}");

    // Bad syntax with a source location.
    let bad = dir.join("bad.c");
    std::fs::write(&bad, "CCALLER_FUNC(f, a:reed)\n{\n}\n").unwrap();
    let output = ccaller().arg("gen").arg(&bad).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bad.c:1:19") && stderr.contains("unknown slot role `reed`"),
        "stderr was: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gen_output_passes_check_against_the_real_cases__F_X_05() {
    // Round-trip: the generated description is a drop-in replacement
    // for the committed hand-written one — the real cases.toml checks
    // clean against it.
    let dir = std::env::temp_dir().join("ccaller-cli-gen-roundtrip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = write_gen_sample(&dir);
    let libs = dir.join("libs.toml");
    let gen = ccaller()
        .arg("gen")
        .arg(&source)
        .arg("-o")
        .arg(&libs)
        .output()
        .unwrap();
    assert_eq!(gen.status.code(), Some(0), "gen output: {gen:?}");
    let cases =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/libc_wrapper/cases.toml");
    let check = ccaller()
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&libs)
        .arg("check")
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "check output: {check:?}\nstderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(
        stdout.contains("ok: 14 tests, 18 subcases, 77 commands"),
        "stdout was: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_gen_macro_header_compiles_as_c__F_X_05() {
    // CCALLER_FUNC must be real C, not just scanner bait: compile the
    // sample against ccaller_gen.h into a shared library. Skipped
    // where no C compiler is installed.
    let dir = std::env::temp_dir().join("ccaller-cli-gen-compile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = write_gen_sample(&dir);
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/ffi/include");
    let probe = Command::new("cc").arg("--version").output();
    if probe.map(|probe| !probe.status.success()).unwrap_or(true) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let out = dir.join("libc_wrapper.so");
    let compile = Command::new("cc")
        .arg("--shared")
        .arg("-fPIC")
        .arg("-I")
        .arg(&include)
        .arg(&source)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        compile.status.code(),
        Some(0),
        "cc failed: {}\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(out.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
