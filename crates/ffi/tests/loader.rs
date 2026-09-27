//! Loader integration tests (F-A-01/02/04/08) against real wrapper
//! fixtures compiled on the fly.

// Integration tests are separate crates, so the crate-level test
// exemptions in lib.rs do not reach here; style guide section 6.3
// allows unwrap/expect/panic freely in test code, and the __F_xx_nn
// suffixes from verification plan section 9.1 are upper-case on purpose.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    non_snake_case
)]

mod common;

use std::path::PathBuf;

use ccaller_ffi::loader::{LibraryRequest, LoadError, LoadedFunctions};

use common::build_fixture;

/// A request for `funcs` from the fixture named `fixture`.
fn fixture_request(fixture: &str, funcs: &[&str]) -> LibraryRequest {
    LibraryRequest {
        path: build_fixture(fixture),
        func_names: funcs.iter().map(|f| (*f).to_string()).collect(),
    }
}

#[test]
fn ok_wrapper_handshakes_and_resolves_functions__F_A_04() {
    let request = fixture_request("ok_wrapper", &["Call_add", "Call_dup"]);
    let loaded = LoadedFunctions::load(std::slice::from_ref(&request))
        .expect("the well-formed fixture loads");
    let add = loaded.get("Call_add").expect("Call_add resolved");
    assert_eq!(add.lib_path, request.path);
}

#[test]
fn missing_version_symbol_is_rejected__F_A_04() {
    let request = fixture_request("nover_wrapper", &["Call_add"]);
    let error = LoadedFunctions::load(std::slice::from_ref(&request))
        .expect_err("a wrapper without the handshake symbol must be rejected");
    assert!(matches!(error, LoadError::MissingVersion { .. }), "{error}");
    assert!(
        error.to_string().contains("CCaller_abi_version"),
        "names the missing symbol: {error}"
    );
    assert!(
        error
            .to_string()
            .contains(request.path.to_string_lossy().as_ref()),
        "names the library: {error}"
    );
}

#[test]
fn version_mismatch_reports_both_versions__F_A_04() {
    let request = fixture_request("wrongver_wrapper", &["Call_add"]);
    let error = LoadedFunctions::load(&[request])
        .expect_err("a wrapper reporting a different ABI version must be rejected");
    let LoadError::VersionMismatch {
        found, expected, ..
    } = error
    else {
        panic!("expected VersionMismatch, got: {error}");
    };
    assert_eq!(found, 2);
    assert_eq!(expected, 1);
}

#[test]
fn missing_function_reports_name_and_library__F_A_02() {
    let request = fixture_request("ok_wrapper", &["Call_nope"]);
    let error = LoadedFunctions::load(std::slice::from_ref(&request))
        .expect_err("a missing declared symbol must be rejected");
    assert!(
        matches!(error, LoadError::MissingFunc { .. }),
        "expected MissingFunc, got: {error}"
    );
    let text = error.to_string();
    assert!(text.contains("Call_nope"), "names the function: {text}");
    assert!(
        text.contains(request.path.to_string_lossy().as_ref()),
        "names the library: {text}"
    );
}

#[test]
fn duplicate_function_across_libraries_names_both__F_A_01() {
    let first = fixture_request("ok_wrapper", &["Call_dup"]);
    let second = fixture_request("dup_wrapper", &["Call_dup"]);
    let error = LoadedFunctions::load(&[first.clone(), second.clone()])
        .expect_err("the same symbol declared by two libraries must be rejected");
    assert!(
        matches!(error, LoadError::DuplicateFunc { .. }),
        "expected DuplicateFunc, got: {error}"
    );
    let text = error.to_string();
    assert!(
        text.contains(first.path.to_string_lossy().as_ref()),
        "names the first library: {text}"
    );
    assert!(
        text.contains(second.path.to_string_lossy().as_ref()),
        "names the second library: {text}"
    );
}

#[test]
fn unopenable_library_hints_the_search_env__F_A_08() {
    let request = LibraryRequest {
        path: PathBuf::from("no/such/wrapper.library"),
        func_names: vec!["Call_add".to_string()],
    };
    let error =
        LoadedFunctions::load(&[request]).expect_err("a nonexistent library must be rejected");
    assert!(
        matches!(error, LoadError::LibraryOpen { .. }),
        "expected LibraryOpen, got: {error}"
    );
    let expected_env = if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    assert!(
        error.to_string().contains(expected_env),
        "hints the dependency search env: {error}"
    );
}
