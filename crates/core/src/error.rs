//! Error types shared by the cCaller core domain.
//!
//! Load-time validation errors (requirements FR-C-08/09) must carry the
//! source location (file, line, column) of the offending configuration
//! element so users can fix their TOML without re-reading the schema.

use std::fmt;

/// Position of a configuration element inside its source file.
///
/// Lines and columns are 1-based, matching what editors display for a
/// TOML document. Every load-time diagnostic must embed one of these so
/// the message is actionable (requirement FR-C-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Path of the configuration file as given by the user.
    pub file: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// Errors raised by the core domain model and its validators.
///
/// The M0 skeleton defines the taxonomy root only; milestone M1 grows
/// dedicated variants (slot def-use, input expansion, unresolved
/// references) on top of this type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A configuration document violates the schema or cross-reference rules.
    #[error("config error at {location}: {message}")]
    Config {
        /// Where the offending element lives in the source file.
        location: Location,
        /// Human-readable description of the violated rule.
        message: String,
    },
    /// A configuration file could not be read from disk.
    #[error("failed to read `{path}`: {source}")]
    Io {
        /// Path of the file that could not be read.
        path: String,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// A resolved value cannot cross the wrapper ABI (FR-A-03); a
    /// runtime defense below the load-time grammar.
    #[error("argument `{param}` cannot cross the ABI: {reason}")]
    Marshal {
        /// Name of the offending parameter.
        param: String,
        /// Why the value is not representable in the unified ABI.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_display_uses_file_line_column__F_C_10() {
        let location = Location {
            file: "cases.toml".to_string(),
            line: 12,
            column: 3,
        };
        assert_eq!(location.to_string(), "cases.toml:12:3");
    }

    #[test]
    fn config_error_display_includes_location_and_message__F_C_10() {
        let error = CoreError::Config {
            location: Location {
                file: "cases.toml".to_string(),
                line: 4,
                column: 1,
            },
            message: "unknown field `threads`".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "config error at cases.toml:4:1: unknown field `threads`"
        );
    }
}
