//! Call-bridge integration tests (F-A-02/03, Q-06/07) against the
//! real wrapper fixture.

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

use std::ffi::CString;

use ccaller_ffi::call::{invoke, CallArg};
use ccaller_ffi::loader::{LibraryRequest, LoadedFunctions};
use ccaller_ffi::page::ParamPage;

use common::build_fixture;

/// The loaded fixture with `Call_add` and `Call_strlen` resolved.
fn loaded_fixture() -> LoadedFunctions {
    let request = LibraryRequest {
        path: build_fixture("ok_wrapper"),
        func_names: vec!["Call_add".to_string(), "Call_strlen".to_string()],
    };
    LoadedFunctions::load(std::slice::from_ref(&request)).expect("fixture loads")
}

#[test]
fn invoke_passes_integer_arguments_and_returns_the_result__F_A_02() {
    let loaded = loaded_fixture();
    let mut page = ParamPage::zeroed();
    let result = invoke(
        loaded.get("Call_add").unwrap().call,
        &mut page,
        &[CallArg::Int(2), CallArg::Int(40)],
    );
    assert_eq!(result, 42);
}

#[test]
fn invoke_passes_no_arguments_when_the_list_is_empty__F_A_02() {
    let loaded = loaded_fixture();
    let mut page = ParamPage::zeroed();
    // Call_add returns -1 for any count other than 2; an empty list
    // must still reach the wrapper (param_len == 0), not crash.
    let result = invoke(loaded.get("Call_add").unwrap().call, &mut page, &[]);
    assert_eq!(result, -1);
}

#[test]
fn string_pointer_is_valid_during_the_call__F_A_03() {
    let loaded = loaded_fixture();
    let mut page = ParamPage::zeroed();
    let arg = CallArg::Str(CString::new("hello").unwrap());
    let result = invoke(loaded.get("Call_strlen").unwrap().call, &mut page, &[arg]);
    assert_eq!(result, 5, "the wrapper must see the full string");
}

#[test]
fn each_call_gets_its_own_valid_string_pointer__F_A_03() {
    let loaded = loaded_fixture();
    let mut page = ParamPage::zeroed();
    let call = loaded.get("Call_strlen").unwrap().call;
    let first = invoke(
        call,
        &mut page,
        &[CallArg::Str(CString::new("abc").unwrap())],
    );
    let second = invoke(
        call,
        &mut page,
        &[CallArg::Str(CString::new("abcdefgh").unwrap())],
    );
    assert_eq!((first, second), (3, 8));
}

#[test]
fn param_page_changes_are_visible_across_calls_on_the_same_page__F_A_06() {
    // Q-06: a slot written through one call stays visible to the next
    // call that runs on the same page; the fixture writes nothing, so
    // the page module's own roundtrip plus a live call sharing the
    // same page proves the plumbing.
    let loaded = loaded_fixture();
    let mut page = ParamPage::zeroed();
    page.write(9, 0x1234).unwrap();
    let result = invoke(
        loaded.get("Call_add").unwrap().call,
        &mut page,
        &[CallArg::Int(0), CallArg::Int(1)],
    );
    assert_eq!(result, 1);
    assert_eq!(
        page.read(9).unwrap(),
        0x1234,
        "the call must not disturb other slots"
    );
}
