//! Runtime marshalling between the domain value model and the ffi
//! call bridge (FR-A-03).
//!
//! [`ResolvedCmd`] arguments are [`ConcreteValue`]s; the unified ABI
//! takes an `i64` array. [`marshal_args`] performs that translation:
//! integers pass through, strings become NUL-terminated [`CString`]s
//! whose pointers the bridge keeps alive for exactly one call
//! (decision Q-07).

use std::ffi::CString;

use ccaller_ffi::call::CallArg;

use crate::config::value::ConcreteValue;
use crate::error::CoreError;

/// Translate one Cmd's arguments into the unified-ABI argument list.
///
/// # Errors
/// Returns [`CoreError::Marshal`] when a value is not representable -
/// in practice only a string with an interior NUL byte, which the
/// value grammar already rejects; this is a runtime defense that
/// keeps a defect visible instead of silently truncating the string.
pub fn marshal_args(args: &[(String, ConcreteValue)]) -> Result<Vec<CallArg>, CoreError> {
    let mut out = Vec::with_capacity(args.len());
    for (name, value) in args {
        let arg = match value {
            ConcreteValue::Int(v) => CallArg::Int(*v),
            ConcreteValue::Str(s) => {
                let bytes = CString::new(s.as_str()).map_err(|_| CoreError::Marshal {
                    param: name.clone(),
                    reason: "string contains an interior NUL byte".to_string(),
                })?;
                CallArg::Str(bytes)
            }
        };
        out.push(arg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_pass_through_unchanged__F_A_03() {
        let args = vec![
            ("len".to_string(), ConcreteValue::Int(7)),
            ("mask".to_string(), ConcreteValue::Int(-1)),
        ];
        let marshalled = marshal_args(&args).unwrap();
        assert_eq!(marshalled, vec![CallArg::Int(7), CallArg::Int(-1)]);
    }

    #[test]
    fn strings_become_nul_terminated__F_A_03() {
        let args = vec![("path".to_string(), ConcreteValue::Str("abc".to_string()))];
        let marshalled = marshal_args(&args).unwrap();
        let CallArg::Str(bytes) = &marshalled[0] else {
            panic!("expected a string argument");
        };
        assert_eq!(bytes.to_bytes(), b"abc");
        assert_eq!(bytes.to_bytes_with_nul(), b"abc\0");
    }

    #[test]
    fn interior_nul_is_rejected_visibly__F_A_03() {
        let args = vec![("path".to_string(), ConcreteValue::Str("a\0b".to_string()))];
        let error = marshal_args(&args).unwrap_err();
        let CoreError::Marshal { param, reason } = &error else {
            panic!("expected a marshal error, got: {error}");
        };
        assert_eq!(param, "path");
        assert!(reason.contains("NUL"), "{reason}");
        assert!(
            error.to_string().contains("path"),
            "names the parameter: {error}"
        );
    }

    #[test]
    fn empty_argument_lists_marshal_to_empty__F_A_03() {
        let marshalled = marshal_args(&[]).unwrap();
        assert!(marshalled.is_empty());
    }
}
