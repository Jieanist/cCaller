//! Wrapper ABI contract constants and return-code classification
//! (requirement spec 7.1; features F-A-04 and F-A-05).
//!
//! The C-facing source of truth is `include/ccaller.h`: every
//! `#define CCALLER_*` contract constant there has a mirror in this
//! module, and a unit test keeps the two sides from drifting apart
//! (style guide 8.4). Any ABI change must update the header, this
//! module, and the requirement specification together, bumping
//! [`CCALLER_ABI_VERSION`].

/// ABI version advertised by this framework (`CCALLER_ABI_VERSION`).
///
/// A wrapper must export `int64_t CCaller_abi_version(void)` returning
/// exactly this value; a mismatch rejects the library at load time with
/// both version numbers in the message (FR-A-04).
pub const CCALLER_ABI_VERSION: i64 = 1;

/// Return value meaning success (`CCALLER_OK`).
pub const CCALLER_OK: i64 = 0;

/// Return value meaning "skip this Cmd" (`CCALLER_ERR_SKIP`).
///
/// Cmd-level skip (decision Q-01): the framework continues with the
/// next Cmd, counts the skip in the statistics, shows it in the
/// report, and does not treat it as a failure for the exit code.
pub const CCALLER_ERR_SKIP: i64 = -255;

/// Inclusive lower bound of the wrapper-custom failure range
/// `[-127, -1]` (requirement spec 7.1 error-code table).
pub const CCALLER_ERR_WRAPPER_CUSTOM_MIN: i64 = -127;

/// Inclusive upper bound of the wrapper-custom failure range
/// `[-127, -1]`.
pub const CCALLER_ERR_WRAPPER_CUSTOM_MAX: i64 = -1;

/// Inclusive lower bound of the framework-reserved range
/// `[-255, -128]`; wrappers must never return values inside it.
pub const CCALLER_ERR_FRAMEWORK_RESERVED_MIN: i64 = -255;

/// Inclusive upper bound of the framework-reserved range
/// `[-255, -128]`.
pub const CCALLER_ERR_FRAMEWORK_RESERVED_MAX: i64 = -128;

/// How the framework interprets a value returned by a `Call_<name>`
/// function, per the numeric table of requirement spec 7.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnClass {
    /// The call succeeded (`CCALLER_OK`).
    Ok,
    /// The Cmd is skipped (`CCALLER_ERR_SKIP`, decision Q-01); not a
    /// failure.
    Skip,
    /// A wrapper-custom code in `[-127, -1]`: the Cmd failed; the
    /// numeric value is passed through to the report verbatim, without
    /// interpretation.
    WrapperFailure(i64),
    /// A positive code: reserved and undefined in this version; the
    /// Cmd failed and the value is passed through.
    PositiveFailure(i64),
    /// A code wrappers must never return: the reserved interior
    /// `[-254, -128]`, or anything below `-255`. The Cmd failed and
    /// the value is reported as a contract violation.
    ContractViolation(i64),
}

impl ReturnClass {
    /// Whether this class counts as a Cmd failure.
    ///
    /// Only [`ReturnClass::Ok`] and [`ReturnClass::Skip`] are not
    /// failures; a skip is visible in the report and the statistics
    /// but never affects the exit code (FR-T-07, Q-01).
    pub fn is_failure(&self) -> bool {
        !matches!(self, ReturnClass::Ok | ReturnClass::Skip)
    }
}

/// Classify a wrapper return value according to the numeric table of
/// requirement spec 7.1.
///
/// ```
/// use ccaller_ffi::abi::{classify_return_value, ReturnClass};
///
/// assert_eq!(classify_return_value(0), ReturnClass::Ok);
/// assert_eq!(classify_return_value(-255), ReturnClass::Skip);
/// assert_eq!(classify_return_value(-42), ReturnClass::WrapperFailure(-42));
/// assert_eq!(classify_return_value(7), ReturnClass::PositiveFailure(7));
/// ```
pub fn classify_return_value(code: i64) -> ReturnClass {
    match code {
        CCALLER_OK => ReturnClass::Ok,
        CCALLER_ERR_SKIP => ReturnClass::Skip,
        CCALLER_ERR_WRAPPER_CUSTOM_MIN..=CCALLER_ERR_WRAPPER_CUSTOM_MAX => {
            ReturnClass::WrapperFailure(code)
        }
        1.. => ReturnClass::PositiveFailure(code),
        // Everything else: the framework-reserved interior [-254, -128]
        // and values below -255, which wrappers must never return.
        _ => ReturnClass::ContractViolation(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wrapper-facing header; the constants below must mirror it.
    const HEADER: &str = include_str!("../include/ccaller.h");

    /// Every mirrored contract constant: header name and Rust value.
    const MIRRORED: &[(&str, i64)] = &[
        ("CCALLER_ABI_VERSION", CCALLER_ABI_VERSION),
        ("CCALLER_OK", CCALLER_OK),
        ("CCALLER_ERR_SKIP", CCALLER_ERR_SKIP),
        (
            "CCALLER_ERR_WRAPPER_CUSTOM_MIN",
            CCALLER_ERR_WRAPPER_CUSTOM_MIN,
        ),
        (
            "CCALLER_ERR_WRAPPER_CUSTOM_MAX",
            CCALLER_ERR_WRAPPER_CUSTOM_MAX,
        ),
        (
            "CCALLER_ERR_FRAMEWORK_RESERVED_MIN",
            CCALLER_ERR_FRAMEWORK_RESERVED_MIN,
        ),
        (
            "CCALLER_ERR_FRAMEWORK_RESERVED_MAX",
            CCALLER_ERR_FRAMEWORK_RESERVED_MAX,
        ),
    ];

    /// Numeric value of `#define <name> ...` in the header.
    ///
    /// Negative values are parenthesized in the header so they stay
    /// safe inside C expressions; the parentheses are stripped here.
    fn header_value(name: &str) -> i64 {
        let prefix = format!("#define {name} ");
        let line = HEADER
            .lines()
            .find(|l| l.starts_with(&prefix))
            .unwrap_or_else(|| panic!("ccaller.h has no `#define {name}`"));
        let token = line[prefix.len()..].split_whitespace().next().unwrap();
        let token = token
            .strip_prefix('(')
            .and_then(|t| t.strip_suffix(')'))
            .unwrap_or(token);
        token.parse().unwrap()
    }

    #[test]
    fn header_defines_match_rust_mirrors__F_A_05() {
        for (name, value) in MIRRORED {
            assert_eq!(
                header_value(name),
                *value,
                "ccaller.h `{name}` drifted from its Rust mirror"
            );
        }
    }

    #[test]
    fn every_header_contract_define_has_a_rust_mirror__F_A_05() {
        let mut defined: Vec<String> = HEADER
            .lines()
            .filter_map(|l| l.strip_prefix("#define CCALLER_"))
            .filter_map(|rest| rest.split_whitespace().next())
            .map(|suffix| format!("CCALLER_{suffix}"))
            // `CCALLER_H` is the include guard, not a contract constant.
            .filter(|name| name != "CCALLER_H")
            .collect();
        defined.sort();
        let mut mirrored: Vec<String> = MIRRORED.iter().map(|(name, _)| (*name).into()).collect();
        mirrored.sort();
        assert_eq!(
            defined, mirrored,
            "every `#define CCALLER_*` in ccaller.h must have a Rust mirror here and vice versa"
        );
    }

    #[test]
    fn classification_partitions_every_return_code__F_A_05() {
        assert_eq!(classify_return_value(0), ReturnClass::Ok);
        assert_eq!(classify_return_value(CCALLER_ERR_SKIP), ReturnClass::Skip);
        assert_eq!(
            classify_return_value(CCALLER_ERR_FRAMEWORK_RESERVED_MIN + 1),
            ReturnClass::ContractViolation(-254)
        );
        assert_eq!(
            classify_return_value(CCALLER_ERR_FRAMEWORK_RESERVED_MAX),
            ReturnClass::ContractViolation(-128)
        );
        assert_eq!(
            classify_return_value(CCALLER_ERR_WRAPPER_CUSTOM_MIN),
            ReturnClass::WrapperFailure(-127)
        );
        assert_eq!(
            classify_return_value(CCALLER_ERR_WRAPPER_CUSTOM_MAX),
            ReturnClass::WrapperFailure(-1)
        );
        assert_eq!(classify_return_value(1), ReturnClass::PositiveFailure(1));
        assert_eq!(
            classify_return_value(i64::MAX),
            ReturnClass::PositiveFailure(i64::MAX)
        );
        assert_eq!(
            classify_return_value(CCALLER_ERR_FRAMEWORK_RESERVED_MIN - 1),
            ReturnClass::ContractViolation(-256)
        );
        assert_eq!(
            classify_return_value(i64::MIN),
            ReturnClass::ContractViolation(i64::MIN)
        );
    }

    #[test]
    fn skip_and_ok_are_not_failures__F_A_05() {
        assert!(!ReturnClass::Ok.is_failure());
        assert!(!ReturnClass::Skip.is_failure());
        assert!(ReturnClass::WrapperFailure(-3).is_failure());
        assert!(ReturnClass::PositiveFailure(5).is_failure());
        assert!(ReturnClass::ContractViolation(-200).is_failure());
    }
}
