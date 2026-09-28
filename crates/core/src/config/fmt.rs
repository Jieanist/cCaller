//! The `fmt` pipeline: canonical re-layout of a TOML configuration file
//! (developer tooling).
//!
//! [`normalize_toml`] parses the document into a generic [`toml::Value`]
//! and re-renders it through the toml writer, so key order, indentation,
//! and table layout become uniform. The transformation is semantic
//! identity: the re-rendered document parses back to the same value tree
//! (`fmt` then `check` reports the same findings), and it is idempotent.
//!
//! What normalization intentionally does not preserve: comments and the
//! original key order. Both are presentation, not configuration; the
//! canonical form sorts keys and lays arrays of tables out as sections.

use std::fmt;

/// One normalization failure: a parse error with its source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmtError {
    /// The parse failure message, without location decoration.
    pub message: String,
    /// 1-based line of the offending byte.
    pub line: usize,
    /// 1-based column of the offending byte.
    pub column: usize,
}

impl fmt::Display for FmtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for FmtError {}

/// Re-renders `text` in the canonical TOML layout.
///
/// # Errors
/// Returns [`FmtError`] when `text` is not valid TOML; the error carries
/// the 1-based line and column of the parse failure.
pub fn normalize_toml(text: &str) -> Result<String, FmtError> {
    let value: toml::Value = toml::from_str(text).map_err(|error| {
        let message = error.message().to_string();
        let (line, column) = error
            .span()
            .map(|span| line_column_of(text, span.start))
            .unwrap_or((1, 1));
        FmtError {
            message,
            line,
            column,
        }
    })?;
    toml::to_string(&value).map_err(|error| FmtError {
        message: error.to_string(),
        line: 1,
        column: 1,
    })
}

/// Converts a byte offset into a 1-based (line, column) pair.
///
/// A multi-byte character counts as one column; the offset is clamped
/// to the document length so a parser pointing one byte past the end
/// (e.g. an unclosed table at EOF) still reports a real position.
pub(super) fn line_column_of(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, ch) in text.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_reorders_keys_and_is_idempotent__F_X_04() {
        let messy =
            "zz = 1\nversion = 1\n\n[env]\nexit = [{ opfunc = \"Call_teardown\", args = [] }]\n";
        let once = normalize_toml(messy).unwrap();
        assert!(
            once.starts_with("version = 1\n"),
            "canonical form sorts keys: {once}"
        );
        // Idempotent: the canonical form is a fixed point.
        let twice = normalize_toml(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn normalization_preserves_the_value_tree__F_X_04() {
        let input = "version = 1\n\
                     [shared_inputs.common]\n\
                     val = [\"888\", \"999\"]\n\
                     [[tests]]\n\
                     name = \"t\"\n\
                     break_if_fail = true\n\
                     cmds = [\n\
                     \x20 { opfunc = \"Call_malloc\", expect_eq = 0, args = [\"len=100\"] },\n\
                     ]\n\
                     [[tests.inputs]]\n\
                     name = \"ipt1\"\n\
                     refs = [\"common\"]\n\
                     args = { a = { start = 0, end = 8, step = 4 } }\n";
        let before: toml::Value = toml::from_str(input).unwrap();
        let normalized = normalize_toml(input).unwrap();
        let after: toml::Value = toml::from_str(&normalized).unwrap();
        assert_eq!(
            before, after,
            "normalization must not change the document's meaning"
        );
    }

    #[test]
    fn a_parse_failure_reports_line_and_column__F_X_04() {
        let error = normalize_toml("version = 1\n[env\ninit = []\n").unwrap_err();
        assert_eq!(error.line, 2);
        assert!(error.column >= 1);
        assert!(error.message.contains("unclosed table"), "{error}");
    }

    #[test]
    fn a_document_with_no_tables_stays_flat__F_X_04() {
        let flat = normalize_toml("version = 1\n").unwrap();
        assert_eq!(flat, "version = 1\n");
    }
}
