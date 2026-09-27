//! The scalar value grammar shared by Cmd `args` and `expect_*` values.
//!
//! Requirement specification 7.3 defines one grammar for every scalar written
//! in a configuration: decimal and `0x` hexadecimal integers, single-quoted C
//! string literals, `$name` substitution, and negation (`!value` or `$!name`),
//! which flips `expect_eq` into `expect_ne` (FR-C-07). Parsing is pure syntax;
//! variable resolution happens later, per subcase, against an input scope.
//!
//! [`ScalarRaw`] mirrors what serde produces for a TOML scalar before the
//! grammar runs: integers are already concrete, strings still need parsing.

use std::fmt;

/// A scalar exactly as it appears in the TOML file, before grammar parsing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(untagged)]
pub enum ScalarRaw {
    /// A TOML integer literal, already a concrete value.
    Int(i64),
    /// A TOML string literal that still carries the value grammar.
    Str(String),
}

/// The syntactic shape of a value after grammar parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// A decimal or `0x` hexadecimal integer literal.
    Int(i64),
    /// A single-quoted C string literal with `\\` and `\'` escapes.
    Str(String),
    /// A `$name` substitution, resolved per subcase later.
    Var(String),
}

/// A parsed value together with the negation flag (FR-C-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedValue {
    /// The base value.
    pub kind: ValueKind,
    /// `!value` or `$!name`: flips `expect_eq` into `expect_ne`. Only
    /// meaningful for expectation values; argument positions reject it.
    pub negated: bool,
}

/// A fully resolved value with all substitutions applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcreteValue {
    /// A 64-bit signed integer.
    Int(i64),
    /// An owned C string; the framework passes a pointer valid for one call.
    Str(String),
}

impl fmt::Display for ConcreteValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConcreteValue::Int(n) => write!(f, "{n}"),
            // Quoted so subcase names and reports stay unambiguous.
            ConcreteValue::Str(s) => write!(f, "'{s}'"),
        }
    }
}

/// Errors from parsing the textual value grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueSyntaxError {
    /// The value is empty.
    #[error("empty value")]
    Empty,
    /// `!` was not followed by a value.
    #[error("missing value after `!`")]
    MissingValueAfterNegation,
    /// `!` and `$!` (or two `!`) cannot combine.
    #[error("double negation in `{0}`")]
    DoubleNegation(String),
    /// Not a valid signed 64-bit decimal integer.
    #[error("invalid integer `{0}`")]
    InvalidInteger(String),
    /// Not a valid hexadecimal integer.
    #[error("invalid hexadecimal integer `{0}`")]
    InvalidHex(String),
    /// The closing `'` is missing.
    #[error("unterminated string literal")]
    UnterminatedString,
    /// Only `\\` and `\'` escapes exist.
    #[error("invalid escape `\\{0}` in string literal")]
    InvalidEscape(char),
    /// Extra characters after the closing `'`.
    #[error("unexpected characters after closing quote")]
    TrailingAfterQuote,
    /// `$` must be followed by an identifier.
    #[error("invalid variable name `{0}`")]
    InvalidVarName(String),
}

/// Errors from resolving a [`ParsedValue`] against a variable scope.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// The variable is not defined in the current scope.
    #[error("undefined variable `${0}`")]
    UndefinedVar(String),
}

/// Parses the textual value grammar of requirement specification 7.3.
///
/// Supported forms: decimal integers (`100`, `-3`), `0x` hexadecimal decoded
/// as `u64` so the bit pattern survives (`0xFFFFFFFFFFFFFFFF` is `-1`),
/// `'…'` C string literals with `\\` and `\'` escapes, `$name` substitution,
/// and negation — `!value` or `$!name` — which flips `expect_eq` into
/// `expect_ne` (FR-C-07).
///
/// # Errors
/// Returns [`ValueSyntaxError`] describing the first syntax problem found.
pub fn parse_value(raw: &str) -> Result<ParsedValue, ValueSyntaxError> {
    if let Some(rest) = raw.strip_prefix('!') {
        if rest.is_empty() {
            return Err(ValueSyntaxError::MissingValueAfterNegation);
        }
        if rest.starts_with('!') {
            return Err(ValueSyntaxError::DoubleNegation(raw.to_string()));
        }
        let mut parsed = parse_base(rest)?;
        if parsed.negated {
            // `!$!name`: the `!` prefix met the `$!name` negation.
            return Err(ValueSyntaxError::DoubleNegation(raw.to_string()));
        }
        parsed.negated = true;
        return Ok(parsed);
    }
    parse_base(raw)
}

fn parse_base(raw: &str) -> Result<ParsedValue, ValueSyntaxError> {
    if raw.is_empty() {
        return Err(ValueSyntaxError::Empty);
    }
    if let Some(rest) = raw.strip_prefix('$') {
        let (name, negated) = match rest.strip_prefix('!') {
            Some(name) => (name, true),
            None => (rest, false),
        };
        return Ok(ParsedValue {
            kind: ValueKind::Var(var_name(name)?),
            negated,
        });
    }
    if let Some(hex) = raw.strip_prefix("0x") {
        // Decode as u64 so every bit pattern is representable; the i64 view
        // preserves the bits (requirement spec 7.3 value syntax).
        let bits = u64::from_str_radix(hex, 16)
            .map_err(|_| ValueSyntaxError::InvalidHex(raw.to_string()))?;
        return Ok(ParsedValue {
            kind: ValueKind::Int(bits as i64),
            negated: false,
        });
    }
    if let Some(body) = raw.strip_prefix('\'') {
        return Ok(ParsedValue {
            kind: ValueKind::Str(parse_string_body(body)?),
            negated: false,
        });
    }
    let n: i64 = raw
        .parse()
        .map_err(|_| ValueSyntaxError::InvalidInteger(raw.to_string()))?;
    Ok(ParsedValue {
        kind: ValueKind::Int(n),
        negated: false,
    })
}

fn var_name(raw: &str) -> Result<String, ValueSyntaxError> {
    let valid = !raw.is_empty() && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(raw.to_string())
    } else {
        Err(ValueSyntaxError::InvalidVarName(raw.to_string()))
    }
}

/// Parses the body following the opening `'` of a string literal.
///
/// Only `\\` and `\'` escapes exist; a bare `'` closes the literal and
/// nothing may follow it.
///
/// # Errors
/// Returns [`ValueSyntaxError`] on a missing closing quote, an unknown
/// escape, or trailing characters.
fn parse_string_body(body: &str) -> Result<String, ValueSyntaxError> {
    let mut out = String::new();
    let mut chars = body.chars();
    loop {
        match chars.next() {
            None => return Err(ValueSyntaxError::UnterminatedString),
            Some('\\') => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some(c) => return Err(ValueSyntaxError::InvalidEscape(c)),
                None => return Err(ValueSyntaxError::UnterminatedString),
            },
            Some('\'') => {
                return if chars.next().is_some() {
                    Err(ValueSyntaxError::TrailingAfterQuote)
                } else {
                    Ok(out)
                };
            }
            Some(c) => out.push(c),
        }
    }
}

impl ScalarRaw {
    /// Parses the scalar through the value grammar.
    ///
    /// TOML integers are already concrete and bypass the grammar; TOML
    /// strings are parsed (they may carry `0x…`, `'…'`, `$name`, or `!`).
    ///
    /// # Errors
    /// Returns [`ValueSyntaxError`] when a string violates the grammar.
    pub fn to_parsed(&self) -> Result<ParsedValue, ValueSyntaxError> {
        match self {
            ScalarRaw::Int(n) => Ok(ParsedValue {
                kind: ValueKind::Int(*n),
                negated: false,
            }),
            ScalarRaw::Str(s) => parse_value(s),
        }
    }
}

impl ParsedValue {
    /// Resolves `$name` substitutions through `lookup`.
    ///
    /// Negation is intentionally not applied here: it flips the comparison
    /// operator and is consumed while building expectations (FR-C-07).
    ///
    /// # Errors
    /// Returns [`ResolveError::UndefinedVar`] when a variable is not in
    /// scope.
    pub fn resolve(
        &self,
        lookup: &dyn Fn(&str) -> Option<ConcreteValue>,
    ) -> Result<ConcreteValue, ResolveError> {
        match &self.kind {
            ValueKind::Int(n) => Ok(ConcreteValue::Int(*n)),
            ValueKind::Str(s) => Ok(ConcreteValue::Str(s.clone())),
            ValueKind::Var(name) => {
                lookup(name).ok_or_else(|| ResolveError::UndefinedVar(name.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str) -> ParsedValue {
        parse_value(raw).unwrap_or_else(|e| panic!("parse `{raw}` failed: {e}"))
    }

    fn plain(kind: ValueKind) -> ParsedValue {
        ParsedValue {
            kind,
            negated: false,
        }
    }

    #[test]
    fn decimal_integers_parse__F_A_03() {
        assert_eq!(parsed("100"), plain(ValueKind::Int(100)));
        assert_eq!(parsed("-3"), plain(ValueKind::Int(-3)));
        assert_eq!(parsed("0"), plain(ValueKind::Int(0)));
    }

    #[test]
    fn hex_decodes_as_u64_bits__F_A_03() {
        assert_eq!(parsed("0xFF00"), plain(ValueKind::Int(0xFF00)));
        // The full u64 range survives as a bit pattern, not a signed parse.
        assert_eq!(parsed("0xFFFFFFFFFFFFFFFF"), plain(ValueKind::Int(-1)));
    }

    #[test]
    fn invalid_numbers_are_rejected__F_A_03() {
        assert!(matches!(parse_value(""), Err(ValueSyntaxError::Empty)));
        assert!(matches!(
            parse_value("12x"),
            Err(ValueSyntaxError::InvalidInteger(_))
        ));
        assert!(matches!(
            parse_value("0x"),
            Err(ValueSyntaxError::InvalidHex(_))
        ));
        assert!(matches!(
            parse_value("0xG1"),
            Err(ValueSyntaxError::InvalidHex(_))
        ));
        // Bare words are not strings: literals must be quoted.
        assert!(matches!(
            parse_value("hello"),
            Err(ValueSyntaxError::InvalidInteger(_))
        ));
    }

    #[test]
    fn string_literals_parse_escapes__F_A_03() {
        assert_eq!(
            parsed("'hello'"),
            plain(ValueKind::Str("hello".to_string()))
        );
        assert_eq!(parsed("''"), plain(ValueKind::Str(String::new())));
        assert_eq!(
            parsed("'it\\'s'"),
            plain(ValueKind::Str("it's".to_string()))
        );
        assert_eq!(
            parsed("'a\\\\b'"),
            plain(ValueKind::Str("a\\b".to_string()))
        );
    }

    #[test]
    fn string_literal_errors_are_specific__F_A_03() {
        assert!(matches!(
            parse_value("'abc"),
            Err(ValueSyntaxError::UnterminatedString)
        ));
        // A trailing escaped quote is still unterminated.
        assert!(matches!(
            parse_value("'ab\\'"),
            Err(ValueSyntaxError::UnterminatedString)
        ));
        assert!(matches!(
            parse_value("'ab'x"),
            Err(ValueSyntaxError::TrailingAfterQuote)
        ));
        assert!(matches!(
            parse_value("'a\\nb'"),
            Err(ValueSyntaxError::InvalidEscape('n'))
        ));
    }

    #[test]
    fn var_substitution_parses__F_C_05() {
        assert_eq!(parsed("$val"), plain(ValueKind::Var("val".to_string())));
        assert_eq!(
            parsed("$!val"),
            ParsedValue {
                kind: ValueKind::Var("val".to_string()),
                negated: true,
            }
        );
    }

    #[test]
    fn invalid_var_names_are_rejected__F_C_05() {
        assert!(matches!(
            parse_value("$"),
            Err(ValueSyntaxError::InvalidVarName(_))
        ));
        assert!(matches!(
            parse_value("$!"),
            Err(ValueSyntaxError::InvalidVarName(_))
        ));
        assert!(matches!(
            parse_value("$a-b"),
            Err(ValueSyntaxError::InvalidVarName(_))
        ));
    }

    #[test]
    fn expectation_negation_prefix_parses__F_C_06() {
        assert_eq!(
            parsed("!7"),
            ParsedValue {
                kind: ValueKind::Int(7),
                negated: true,
            }
        );
        // `!$name` and `$!name` mean the same thing.
        assert_eq!(
            parsed("!$val"),
            ParsedValue {
                kind: ValueKind::Var("val".to_string()),
                negated: true,
            }
        );
    }

    #[test]
    fn double_negation_is_rejected__F_C_06() {
        assert!(matches!(
            parse_value("!!7"),
            Err(ValueSyntaxError::DoubleNegation(_))
        ));
        assert!(matches!(
            parse_value("!$!val"),
            Err(ValueSyntaxError::DoubleNegation(_))
        ));
        assert!(matches!(
            parse_value("!"),
            Err(ValueSyntaxError::MissingValueAfterNegation)
        ));
    }

    #[test]
    fn scalar_raw_integers_bypass_the_grammar__F_A_03() {
        assert_eq!(
            ScalarRaw::Int(-5).to_parsed(),
            Ok(plain(ValueKind::Int(-5)))
        );
        assert_eq!(
            ScalarRaw::Str("0x10".to_string()).to_parsed(),
            Ok(plain(ValueKind::Int(0x10)))
        );
    }

    #[test]
    fn resolve_substitutes_variables__F_C_05() {
        let vars = |name: &str| (name == "val").then_some(ConcreteValue::Int(888));
        assert_eq!(
            plain(ValueKind::Var("val".to_string())).resolve(&vars),
            Ok(ConcreteValue::Int(888))
        );
        // Negation survives resolution untouched; expectations apply it.
        let negated = ParsedValue {
            kind: ValueKind::Var("val".to_string()),
            negated: true,
        };
        assert_eq!(negated.resolve(&vars), Ok(ConcreteValue::Int(888)));
    }

    #[test]
    fn resolve_reports_undefined_vars__F_C_05() {
        let empty = |_: &str| None;
        assert_eq!(
            plain(ValueKind::Var("missing".to_string())).resolve(&empty),
            Err(ResolveError::UndefinedVar("missing".to_string()))
        );
    }

    #[test]
    fn concrete_display_quotes_strings__F_C_04() {
        assert_eq!(ConcreteValue::Int(888).to_string(), "888");
        assert_eq!(ConcreteValue::Str("abc".to_string()).to_string(), "'abc'");
    }
}
