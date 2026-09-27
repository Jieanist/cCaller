//! The call bridge: marshals one Cmd's arguments into the unified ABI
//! and invokes the wrapper (FR-A-03, decision Q-07).
//!
//! String arguments are passed as pointers that stay valid for
//! exactly the duration of one call: [`invoke`] borrows the
//! [`CallArg`] slice, and every pointer it hands out therefore dies
//! when the call returns. A wrapper that wants the bytes later must
//! copy them (Q-07) - the framework never lets a pointer dangle
//! *during* a call.

use std::ffi::CString;

use crate::loader::CallFn;
use crate::page::ParamPage;

/// One argument of a `Call_<name>` invocation in the unified ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallArg {
    /// A 64-bit integer argument (decimal or `0x` hexadecimal in the
    /// configuration; FR-A-03).
    Int(i64),
    /// A NUL-terminated string passed by pointer; the pointer is
    /// valid only for the duration of this call (Q-07).
    Str(CString),
}

/// Invoke one wrapper function with `args` on `page`.
///
/// This is the single place a `Call_<name>` pointer is dereferenced.
/// The argument array is rebuilt per call; string pointers borrow from
/// `args`, which the caller keeps alive across the call, so the Q-07
/// one-call lifetime holds mechanically.
pub fn invoke(func: CallFn, page: &mut ParamPage, args: &[CallArg]) -> i64 {
    let slots: Vec<i64> = args
        .iter()
        .map(|arg| match arg {
            CallArg::Int(value) => *value,
            CallArg::Str(bytes) => bytes.as_ptr() as i64,
        })
        .collect();
    // SAFETY: `func` was resolved by the loader from a library the
    // caller still holds open; `page` is a live 512-slot page and
    // `slots` is the complete argument array of this call; string
    // pointers inside `slots` borrow from `args`, which outlives the
    // call, satisfying the Q-07 one-call lifetime contract.
    unsafe { func(page.as_mut_ptr(), slots.as_ptr(), slots.len() as i64) }
}
