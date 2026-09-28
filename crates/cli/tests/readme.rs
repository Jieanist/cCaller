//! Pins the README example to reality: both TOML blocks must load and
//! pass `ccaller check` (F-Q-06, verification plan V-L5-05).

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
use std::process::Command;

/// Extracts every ```toml fenced block from `text`, in order.
fn toml_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(block) = current.as_mut() {
            if line.trim() == "```" {
                blocks.push(block.clone());
                current = None;
            } else {
                block.push_str(line);
                block.push('\n');
            }
        } else if line.trim_start().starts_with("```toml") {
            current = Some(String::new());
        }
    }
    blocks
}

#[test]
fn readme_example_passes_check__F_Q_06() {
    let readme = concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md");
    let text = fs::read_to_string(readme).unwrap();
    let blocks = toml_blocks(&text);
    assert_eq!(blocks.len(), 2, "README must carry exactly two TOML blocks");

    let dir = std::env::temp_dir().join("ccaller-cli-readme");
    fs::create_dir_all(&dir).unwrap();
    let libs = dir.join("libs.toml");
    let cases = dir.join("cases.toml");
    fs::write(&libs, &blocks[0]).unwrap();
    fs::write(&cases, &blocks[1]).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ccaller"))
        .arg("-t")
        .arg(&cases)
        .arg("-i")
        .arg(&libs)
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
        stdout.contains("ok: 1 tests, 1 subcases, 1 commands"),
        "stdout was: {stdout}"
    );
}
