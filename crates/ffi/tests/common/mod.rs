//! Shared helpers for the ffi integration tests: compile a wrapper
//! fixture and hand back the path of its dynamic library.
//!
//! Each fixture under `tests/fixtures/` is its own workspace (empty
//! `[workspace]` table) so the root workspace never picks it up and
//! parallel fixture builds cannot contend on one target directory.

// Test-support code: the unwrap/expect exemptions mirror the ones the
// integration-test crates carry (style guide section 6.3).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

/// Build `crates/ffi/tests/fixtures/<name>` and return the path of
/// the produced cdylib.
///
/// Fixtures are tiny Rust cdylibs, so no external toolchain is needed
/// (requirement spec 7.7 platform matrix keeps the test suite
/// self-contained on Windows and Linux alike).
pub fn build_fixture(name: &str) -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .expect("cargo must be available to build wrapper fixtures");
    assert!(
        output.status.success(),
        "fixture `{name}` failed to build:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = if cfg!(windows) {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    };
    let path = manifest
        .parent()
        .expect("the manifest path always has a parent")
        .join("target")
        .join("debug")
        .join(file);
    assert!(
        path.exists(),
        "fixture artifact not found at {}",
        path.display()
    );
    path
}
