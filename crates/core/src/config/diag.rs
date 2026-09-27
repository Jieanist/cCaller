//! Load-time diagnostics: the single error currency of `ccaller check`.
//!
//! Every load-time finding — TOML/schema problems, cross-reference
//! violations, expansion failures, slot def-use analysis — is a
//! [`Diagnostic`] carrying a machine-readable code, a source location, and a
//! human-readable message (requirement spec 7.6). Diagnostics accumulate;
//! nothing here aborts a run by itself.

use crate::error::Location;

/// Machine-readable diagnostic codes (requirement spec 7.6
/// `errors[].code`).
///
/// Kept as `&'static str` constants: they are a wire contract for the JSON
/// output, not a type-safety boundary.
pub mod codes {
    /// TOML/serde: a field the schema does not know (FR-C-09).
    pub const UNKNOWN_FIELD: &str = "unknown_field";
    /// TOML/serde: a required field is absent.
    pub const MISSING_FIELD: &str = "missing_field";
    /// TOML/serde: a value has the wrong type or is not a valid variant.
    pub const TYPE_MISMATCH: &str = "type_mismatch";
    /// TOML/serde: anything else, mostly raw TOML syntax errors.
    pub const SCHEMA: &str = "schema";
    /// Schema version is not `1` (requirement spec 7.2/7.3, FR-C-02).
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    /// The same function name is declared in two libraries (FR-A-01).
    pub const DUPLICATE_FUNCTION: &str = "duplicate_function";
    /// The same library path is declared twice (requirement spec 7.2).
    pub const DUPLICATE_LIB_PATH: &str = "duplicate_lib_path";
    /// The same parameter name appears twice in one function's `paras`.
    pub const DUPLICATE_PARAM: &str = "duplicate_param";
    /// `slot_roles` names a parameter the function does not declare.
    pub const SLOT_ROLE_UNKNOWN_PARAM: &str = "slot_role_unknown_param";
}

/// One load-time finding as reported by `ccaller check`.
///
/// Mirrors the JSON contract of requirement spec 7.6: a code, a source
/// location, and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Machine-readable code; see [`codes`].
    pub code: &'static str,
    /// Where the problem was found.
    pub location: Location,
    /// Human-readable description of the violated rule.
    pub message: String,
}

impl Diagnostic {
    /// Builds a diagnostic from its parts.
    pub fn new(code: &'static str, location: Location, message: String) -> Self {
        Self {
            code,
            location,
            message,
        }
    }
}

/// Maps a serde/TOML error message to a diagnostic code.
///
/// The toml crate does not expose error kinds on stable, so the code is
/// sniffed from the message text. Unknown shapes fall back to `schema`.
pub fn sniff_code(message: &str) -> &'static str {
    if message.contains("unknown field") {
        codes::UNKNOWN_FIELD
    } else if message.contains("missing field") {
        codes::MISSING_FIELD
    } else if message.contains("unknown variant")
        || message.contains("invalid type")
        || message.contains("invalid value")
        || message.contains("data did not match")
    {
        codes::TYPE_MISMATCH
    } else {
        codes::SCHEMA
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_maps_serde_messages_to_codes__F_C_09() {
        assert_eq!(
            sniff_code("unknown field `threads`, expected one of `version`"),
            codes::UNKNOWN_FIELD
        );
        assert_eq!(sniff_code("missing field `funcs`"), codes::MISSING_FIELD);
        assert_eq!(
            sniff_code("unknown variant `sometimes`, expected one of `read`"),
            codes::TYPE_MISMATCH
        );
        assert_eq!(
            sniff_code("invalid type: string, expected u64"),
            codes::TYPE_MISMATCH
        );
        assert_eq!(sniff_code("unclosed bracket"), codes::SCHEMA);
    }
}
