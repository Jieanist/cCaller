//! Source text of a configuration file plus span-to-location mapping.
//!
//! The TOML parser reports byte spans; users need line/column. A
//! [`SourceDoc`] keeps the file text next to its path so every span can be
//! mapped back (FR-C-09, FR-C-10).

use std::fs;
use std::ops::Range;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::error::{CoreError, Location};

use super::diag::{self, Diagnostic};

/// A configuration file loaded into memory.
///
/// Cheap to clone; the check pipeline keeps one per file so diagnostics can
/// be located lazily.
#[derive(Debug, Clone)]
pub struct SourceDoc {
    /// Path as given by the user; used verbatim in diagnostics.
    pub path: String,
    /// Full file text.
    pub text: String,
}

impl SourceDoc {
    /// Reads a file from disk.
    ///
    /// # Errors
    /// Returns [`CoreError::Io`] when the file cannot be read.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = fs::read_to_string(path).map_err(|source| CoreError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self {
            path: path.display().to_string(),
            text,
        })
    }

    /// Deserializes the document into `T` with strict field checking.
    ///
    /// The serde message is kept — it already names the fields and expected
    /// types — but wrapped in a [`Diagnostic`] with the span mapped to a
    /// line/column location, so no bare serde string ever reaches the user
    /// (FR-C-09).
    ///
    /// # Errors
    /// Returns a [`Diagnostic`] describing the first TOML or schema problem.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T, Diagnostic> {
        toml::from_str(&self.text).map_err(|err| {
            let location = match err.span() {
                Some(span) => self.locate(span),
                None => Location {
                    file: self.path.clone(),
                    line: 1,
                    column: 1,
                },
            };
            Diagnostic::new(
                diag::sniff_code(err.message()),
                location,
                err.message().to_string(),
            )
        })
    }

    /// Maps a byte span to a 1-based line/column location.
    ///
    /// Out-of-range or mid-character offsets — never produced by the TOML
    /// parser, but cheap to guard — are clamped instead of panicking.
    pub fn locate(&self, span: Range<usize>) -> Location {
        let start = char_boundary(&self.text, span.start.min(self.text.len()));
        let prefix = &self.text[..start];
        let line = prefix.matches('\n').count() + 1;
        let line_start = prefix.rfind('\n').map_or(0, |nl| nl + 1);
        let column = self.text[line_start..start].chars().count() + 1;
        Location {
            file: self.path.clone(),
            line,
            column,
        }
    }
}

/// Advances `i` to the next UTF-8 character boundary in `text`.
fn char_boundary(text: &str, mut i: usize) -> usize {
    while i < text.len() && !text.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Mini {
        a: u64,
    }

    fn loc(path: &str, line: usize, column: usize) -> Location {
        Location {
            file: path.to_string(),
            line,
            column,
        }
    }

    #[test]
    fn locate_maps_offset_to_line_column__F_C_10() {
        let doc = SourceDoc {
            path: "x.toml".to_string(),
            text: "a = 1\nbb = 2\n".to_string(),
        };
        // `bb` starts at byte offset 6 → line 2, column 1.
        assert_eq!(doc.locate(6..8), loc("x.toml", 2, 1));
        // The `2` of `bb = 2` sits at byte offset 11 → line 2, column 6.
        assert_eq!(doc.locate(11..12), loc("x.toml", 2, 6));
    }

    #[test]
    fn locate_handles_first_line_and_out_of_range__F_C_10() {
        let doc = SourceDoc {
            path: "x.toml".to_string(),
            text: "ab".to_string(),
        };
        assert_eq!(doc.locate(0..1), loc("x.toml", 1, 1));
        // Clamped beyond EOF instead of panicking.
        assert_eq!(doc.locate(99..100), loc("x.toml", 1, 3));
    }

    #[test]
    fn parse_reports_unknown_field_with_location__F_C_09() {
        let doc = SourceDoc {
            path: "m.toml".to_string(),
            text: "a = 1\nb = 2\n".to_string(),
        };
        let err = doc.parse::<Mini>().unwrap_err();
        assert_eq!(err.code, "unknown_field");
        assert_eq!(err.location.line, 2);
        assert!(err.message.contains("unknown field `b`"));
    }

    #[test]
    fn parse_reports_missing_field__F_C_09() {
        let doc = SourceDoc {
            path: "m.toml".to_string(),
            text: String::new(),
        };
        let err = doc.parse::<Mini>().unwrap_err();
        assert_eq!(err.code, "missing_field");
        assert!(err.message.contains("missing field `a`"));
    }

    #[test]
    fn parse_reports_raw_toml_syntax_as_schema__F_C_09() {
        let doc = SourceDoc {
            path: "m.toml".to_string(),
            text: "a = [unclosed\n".to_string(),
        };
        let err = doc.parse::<Mini>().unwrap_err();
        assert_eq!(err.code, "schema");
    }

    #[test]
    fn parse_returns_the_deserialized_value__F_C_09() {
        let doc = SourceDoc {
            path: "m.toml".to_string(),
            text: "a = 5\n".to_string(),
        };
        assert_eq!(doc.parse::<Mini>().unwrap().a, 5);
    }
}
