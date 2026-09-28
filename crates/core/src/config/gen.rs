//! The `gen` pipeline: macro-annotated wrapper source → library
//! description (developer tooling).
//!
//! [`generate_lib_description`] scans a C source file for
//! `CCALLER_FUNC(name, param, ..., param:role, ...)` call sites — the
//! declaration macro from `crates/ffi/include/ccaller_gen.h` — and
//! renders a library description (requirement spec 7.2) for every
//! function found. This ports the hitest `export_function.h` +
//! `generate_config.py` workflow into cCaller, with the addition that
//! the macro tail also carries each parameter's slot role
//! (read / write / read_write), so the generated `slot_roles` drive the
//! static def-use analysis exactly like a hand-written description.
//!
//! What the scanner skips, in translation-phase order:
//!
//! - backslash-newline splicing (phase 2) happens before everything,
//!   so a spliced `//` comment or `#define` covers its continuations;
//! - comments and string/char literal *contents* (phase 3) are blanked
//!   to spaces at identical byte offsets, keeping line/column correct;
//! - `#define` lines never contain call sites (the macro definition
//!   itself is skipped like any directive);
//! - conditional blocks of other platforms are skipped: `#if(def)`
//!   conditions are evaluated over the platform macros `_WIN32`,
//!   `__linux__`, and `__APPLE__` only; a condition that mentions none
//!   of them (e.g. `#ifdef __cplusplus`) or fails to parse is treated
//!   as active, and the `#else` arm of such an unknown condition is
//!   skipped (the unknown arm already contributed).
//!
//! Errors carry the 1-based line and column of the offending construct
//! in the source file.

use std::fmt;

use super::fmt::line_column_of;

/// One generation failure: a scan or parse error with its source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenError {
    /// The failure message, without location decoration.
    pub message: String,
    /// 1-based line of the offending construct.
    pub line: usize,
    /// 1-based column of the offending construct.
    pub column: usize,
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for GenError {}

/// The outcome of a successful generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenReport {
    /// The rendered library description, ready to write as libs.toml.
    pub toml: String,
    /// Number of `CCALLER_FUNC` call sites that contributed.
    pub func_count: usize,
}

/// One function parsed from a `CCALLER_FUNC` call site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GenFunc {
    /// Exported symbol name, `Call_<name>`.
    name: String,
    /// Declared parameter names, in call order.
    paras: Vec<String>,
    /// Slot role per slot-carrying parameter, in declaration order.
    roles: Vec<(String, String)>,
}

/// The macro the scanner looks for.
const MACRO: &str = "CCALLER_FUNC(";

/// The slot roles a parameter suffix may name.
const ROLES: [&str; 3] = ["read", "write", "read_write"];

/// Scans `source` and renders its library description.
///
/// `lib_path` is the library file name written into the description;
/// the caller derives it from the source file (see
/// [`default_lib_filename`]).
///
/// # Errors
/// Returns [`GenError`] for anything unscannable or ambiguous: no
/// `CCALLER_FUNC` call site at all, malformed names, parameters, or
/// roles, unterminated calls, and duplicate functions or parameters.
/// Every error carries a 1-based source location.
pub fn generate_lib_description(source: &str, lib_path: &str) -> Result<GenReport, GenError> {
    let stripped = strip_comments(&splice_lines(source));
    let funcs = scan_ccaller_funcs(&stripped)?;
    if funcs.is_empty() {
        return Err(GenError {
            message: format!("no {MACRO}...) declarations found"),
            line: 1,
            column: 1,
        });
    }
    let func_count = funcs.len();
    Ok(GenReport {
        toml: render(&funcs, lib_path),
        func_count,
    })
}

/// The library file name a source file is expected to build into.
///
/// The stem keeps the source file's name and the extension follows the
/// `init` scaffold convention: `.dll` on Windows, `.so` elsewhere.
pub fn default_lib_filename(source_stem: &str) -> String {
    if cfg!(windows) {
        format!("{source_stem}.dll")
    } else {
        format!("{source_stem}.so")
    }
}

/// Renders the description in the layout `fmt` produces for one.
fn render(funcs: &[GenFunc], lib_path: &str) -> String {
    let mut out = String::from("version = 1\n\n[[libs]]\n");
    out.push_str(&format!("path = \"{lib_path}\"\n\nfuncs = [\n"));
    for func in funcs {
        out.push_str("  { name = \"");
        out.push_str(&func.name);
        out.push_str("\", paras = [");
        for (index, para) in func.paras.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("\"{para}\""));
        }
        out.push(']');
        if !func.roles.is_empty() {
            out.push_str(", slot_roles = { ");
            for (index, (param, role)) in func.roles.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{param} = \"{role}\""));
            }
            out.push_str(" }");
        }
        out.push_str(" },\n");
    }
    out.push_str("]\n");
    out
}

/// Splices backslash-newline pairs (translation phase 2).
///
/// Both bytes become spaces so every later offset stays valid; after
/// splicing, each physical line is a logical line, and a `//` comment
/// or directive covers its continuations naturally.
fn splice_lines(source: &str) -> String {
    let mut out = source.as_bytes().to_vec();
    let mut index = 0;
    while index < out.len() {
        if out[index] == b'\\' {
            if out.get(index + 1) == Some(&b'\n') {
                out[index] = b' ';
                out[index + 1] = b' ';
                index += 2;
                continue;
            }
            if out.get(index + 1) == Some(&b'\r') && out.get(index + 2) == Some(&b'\n') {
                out[index] = b' ';
                out[index + 1] = b' ';
                out[index + 2] = b' ';
                index += 3;
                continue;
            }
        }
        index += 1;
    }
    // Only blanks were written over blanks, so the text stays valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| source.to_owned())
}

/// Blanks comments and string/char literal contents to spaces (phase 3).
///
/// Newlines always survive so the line structure is preserved; every
/// replaced byte becomes exactly one space so byte offsets — and with
/// them line/column positions — stay valid. A multi-byte character
/// inside a comment is blanked byte-for-byte, which can shift the
/// reported column of constructs after it by a few columns; that is
/// accepted for diagnostics only.
fn strip_comments(source: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        LineComment,
        BlockComment,
        Str,
        Char,
    }
    let mut out = source.as_bytes().to_vec();
    let mut state = State::Normal;
    let mut index = 0;
    while index < out.len() {
        let byte = out[index];
        match state {
            State::Normal => match byte {
                b'/' if out.get(index + 1) == Some(&b'/') => {
                    out[index] = b' ';
                    out[index + 1] = b' ';
                    state = State::LineComment;
                    index += 2;
                    continue;
                }
                b'/' if out.get(index + 1) == Some(&b'*') => {
                    out[index] = b' ';
                    out[index + 1] = b' ';
                    state = State::BlockComment;
                    index += 2;
                    continue;
                }
                b'"' => {
                    out[index] = b' ';
                    state = State::Str;
                }
                b'\'' => {
                    out[index] = b' ';
                    state = State::Char;
                }
                _ => {}
            },
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Normal;
                } else {
                    out[index] = b' ';
                }
            }
            State::BlockComment => {
                if byte == b'*' && out.get(index + 1) == Some(&b'/') {
                    out[index] = b' ';
                    out[index + 1] = b' ';
                    state = State::Normal;
                    index += 2;
                    continue;
                }
                if byte != b'\n' {
                    out[index] = b' ';
                }
            }
            State::Str | State::Char => {
                let quote = if state == State::Str { b'"' } else { b'\'' };
                if byte == b'\\' {
                    out[index] = b' ';
                    if let Some(next) = out.get(index + 1) {
                        if *next != b'\n' {
                            out[index + 1] = b' ';
                        }
                    }
                    index += 2;
                    continue;
                }
                if byte == quote || byte == b'\n' {
                    // Close, or forgive a literal left open at end of line.
                    state = State::Normal;
                }
                if byte != b'\n' {
                    out[index] = b' ';
                }
            }
        }
        index += 1;
    }
    // Only blanks were written over blanks, so the text stays valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| source.to_owned())
}

/// One `#if` chain on the conditional stack.
struct CondArm {
    /// Whether the enclosing chain contributed.
    parent_active: bool,
    /// Whether an arm of this chain already contributed.
    seen_true: bool,
    /// Whether the current arm contributes.
    active: bool,
}

/// A three-valued preprocessor condition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cond {
    /// Definitely true on this platform.
    True,
    /// Definitely false on this platform.
    False,
    /// Mentions unknown identifiers; treated as true, `#else` skipped.
    Unknown,
}

impl Cond {
    /// Negation; unknown stays unknown.
    fn not(self) -> Cond {
        match self {
            Cond::True => Cond::False,
            Cond::False => Cond::True,
            Cond::Unknown => Cond::Unknown,
        }
    }

    /// Logical and over three values.
    fn and(self, other: Cond) -> Cond {
        match (self, other) {
            (Cond::False, _) | (_, Cond::False) => Cond::False,
            (Cond::Unknown, _) | (_, Cond::Unknown) => Cond::Unknown,
            _ => Cond::True,
        }
    }

    /// Logical or over three values.
    fn or(self, other: Cond) -> Cond {
        match (self, other) {
            (Cond::True, _) | (_, Cond::True) => Cond::True,
            (Cond::Unknown, _) | (_, Cond::Unknown) => Cond::Unknown,
            _ => Cond::False,
        }
    }

    /// Whether the condition is definitely false here.
    fn is_false(self) -> bool {
        matches!(self, Cond::False)
    }
}

/// Truth of a platform macro on the current platform, if recognized.
fn platform_cond(ident: &str) -> Option<Cond> {
    let truth = match ident {
        "_WIN32" => cfg!(windows),
        "__linux__" => cfg!(target_os = "linux"),
        "__APPLE__" => cfg!(target_os = "macos"),
        _ => return None,
    };
    Some(if truth { Cond::True } else { Cond::False })
}

/// Truth of `defined(ident)`.
fn defined_cond(ident: &str) -> Cond {
    platform_cond(ident).unwrap_or(Cond::Unknown)
}

/// One condition token.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// `!`
    Not,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `defined`
    Defined,
    /// An identifier.
    Ident(String),
    /// An integer literal.
    Int(u64),
}

/// Splits a condition into tokens; `None` on an illegal character.
fn tokenize(text: &str) -> Option<Vec<Tok>> {
    let bytes = text.as_bytes();
    let mut toks = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b' ' | b'\t' | b'\r' => index += 1,
            b'!' => {
                toks.push(Tok::Not);
                index += 1;
            }
            b'&' if bytes.get(index + 1) == Some(&b'&') => {
                toks.push(Tok::And);
                index += 2;
            }
            b'|' if bytes.get(index + 1) == Some(&b'|') => {
                toks.push(Tok::Or);
                index += 2;
            }
            b'(' => {
                toks.push(Tok::LParen);
                index += 1;
            }
            b')' => {
                toks.push(Tok::RParen);
                index += 1;
            }
            b'0'..=b'9' => {
                let start = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                let digits = text.get(start..index)?;
                toks.push(Tok::Int(digits.parse().ok()?));
            }
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                let ident = text.get(start..index)?;
                if ident == "defined" {
                    toks.push(Tok::Defined);
                } else {
                    toks.push(Tok::Ident(ident.to_owned()));
                }
            }
            _ => return None,
        }
    }
    Some(toks)
}

/// Evaluates a `#if`/`#elif` expression; unparseable → [`Cond::Unknown`].
fn eval_expr(text: &str) -> Cond {
    let toks = match tokenize(text) {
        Some(toks) => toks,
        None => return Cond::Unknown,
    };
    let mut pos = 0;
    let value = parse_or(&toks, &mut pos);
    match value {
        Some(value) if pos == toks.len() => value,
        _ => Cond::Unknown,
    }
}

/// Parses and evaluates `a || b || ...`.
fn parse_or(toks: &[Tok], pos: &mut usize) -> Option<Cond> {
    let mut value = parse_and(toks, pos)?;
    while toks.get(*pos) == Some(&Tok::Or) {
        *pos += 1;
        let right = parse_and(toks, pos)?;
        value = value.or(right);
    }
    Some(value)
}

/// Parses and evaluates `a && b && ...`.
fn parse_and(toks: &[Tok], pos: &mut usize) -> Option<Cond> {
    let mut value = parse_unary(toks, pos)?;
    while toks.get(*pos) == Some(&Tok::And) {
        *pos += 1;
        let right = parse_unary(toks, pos)?;
        value = value.and(right);
    }
    Some(value)
}

/// Parses and evaluates `!x`, `(x)`, `defined(x)`, `defined x`, an
/// identifier, or an integer.
fn parse_unary(toks: &[Tok], pos: &mut usize) -> Option<Cond> {
    match toks.get(*pos)? {
        Tok::Not => {
            *pos += 1;
            Some(parse_unary(toks, pos)?.not())
        }
        Tok::LParen => {
            *pos += 1;
            let value = parse_or(toks, pos)?;
            if toks.get(*pos) != Some(&Tok::RParen) {
                return None;
            }
            *pos += 1;
            Some(value)
        }
        Tok::Defined => {
            *pos += 1;
            let ident = match toks.get(*pos)? {
                Tok::Ident(ident) => {
                    *pos += 1;
                    ident.clone()
                }
                Tok::LParen => {
                    *pos += 1;
                    let ident = match toks.get(*pos)? {
                        Tok::Ident(ident) => ident.clone(),
                        _ => return None,
                    };
                    *pos += 1;
                    if toks.get(*pos) != Some(&Tok::RParen) {
                        return None;
                    }
                    *pos += 1;
                    ident
                }
                _ => return None,
            };
            Some(defined_cond(&ident))
        }
        Tok::Ident(ident) => {
            *pos += 1;
            Some(defined_cond(ident))
        }
        Tok::Int(value) => {
            *pos += 1;
            Some(if *value == 0 { Cond::False } else { Cond::True })
        }
        Tok::And | Tok::Or | Tok::RParen => None,
    }
}

/// Half-open byte ranges of lines, `(start, end)`.
type LineRanges = Vec<(usize, usize)>;

/// Walks the (spliced, stripped) text line by line and classifies each
/// line as directive, inactive, or scannable.
///
/// Returns the byte ranges of inactive lines (inside conditional arms
/// of other platforms) and of directive lines (anything starting with
/// `#`).
fn classify_lines(text: &str) -> (LineRanges, LineRanges) {
    let mut inactive = Vec::new();
    let mut directives = Vec::new();
    let mut stack: Vec<CondArm> = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    for index in 0..=bytes.len() {
        let at_end = index == bytes.len();
        if !at_end && bytes[index] != b'\n' {
            continue;
        }
        let range = (start, index);
        start = index + 1;
        if range.0 >= range.1 {
            continue;
        }
        let line = text[range.0..range.1].trim_start();
        if line.starts_with('#') {
            directives.push(range);
            handle_directive(line, &mut stack);
        } else if stack.iter().any(|arm| !arm.active) {
            inactive.push(range);
        }
    }
    (inactive, directives)
}

/// Updates the conditional stack for one directive line.
fn handle_directive(line: &str, stack: &mut Vec<CondArm>) {
    let body = line[1..].trim_start();
    let (keyword, rest) = match body.find(char::is_whitespace) {
        Some(split) => (&body[..split], body[split..].trim()),
        None => (body, ""),
    };
    match keyword {
        "if" => push_arm(stack, eval_expr(rest)),
        "ifdef" => push_arm(
            stack,
            single_ident(rest).map_or(Cond::Unknown, defined_cond),
        ),
        "ifndef" => push_arm(
            stack,
            single_ident(rest).map_or(Cond::Unknown, |ident| defined_cond(ident).not()),
        ),
        "elif" => {
            if let Some(top) = stack.last_mut() {
                let keep = !eval_expr(rest).is_false() && !top.seen_true;
                top.seen_true |= keep;
                top.active = top.parent_active && keep;
            }
        }
        "else" => {
            if let Some(top) = stack.last_mut() {
                let keep = !top.seen_true;
                top.seen_true = true;
                top.active = top.parent_active && keep;
            }
        }
        "endif" => {
            stack.pop();
        }
        // Unknown directives (#include, #define, #pragma, ...) never
        // affect the stack; their lines are skipped as directives.
        _ => {}
    }
}

/// The single identifier an `#ifdef`/`#ifndef` names, if that is all
/// there is.
fn single_ident(rest: &str) -> Option<&str> {
    let trimmed = rest.trim();
    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !trimmed.is_empty()
        && !trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
    {
        Some(trimmed)
    } else {
        None
    }
}

/// Pushes one `#if` arm for a condition.
fn push_arm(stack: &mut Vec<CondArm>, cond: Cond) {
    let parent_active = stack.iter().all(|arm| arm.active);
    // Unknown conditions are kept; their `#else` is not.
    let keep = !cond.is_false();
    stack.push(CondArm {
        parent_active,
        seen_true: keep,
        active: parent_active && keep,
    });
}

/// Whether `offset` falls inside any of the ranges.
fn in_ranges(offset: usize, ranges: &[(usize, usize)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| offset >= *start && offset < *end)
}

/// Scans the prepared text for `CCALLER_FUNC` call sites.
fn scan_ccaller_funcs(text: &str) -> Result<Vec<GenFunc>, GenError> {
    let (inactive, directives) = classify_lines(text);
    let mut funcs = Vec::new();
    let mut sites: Vec<(String, usize)> = Vec::new();
    let mut search = 0usize;
    while let Some(found) = text[search..].find(MACRO) {
        let open = search + found;
        search = open + MACRO.len();
        // Not part of a longer identifier (e.g. `XCCALLER_FUNC(`).
        let boundary_ok = open == 0 || !is_ident_byte(text.as_bytes()[open - 1]);
        if !boundary_ok || in_ranges(open, &inactive) || in_ranges(open, &directives) {
            continue;
        }
        let func = parse_call(text, open + MACRO.len() - 1)?;
        if let Some((_, first)) = sites.iter().find(|(name, _)| *name == func.name) {
            let (line, column) = line_column_of(text, open);
            let (first_line, first_column) = line_column_of(text, *first);
            return Err(GenError {
                message: format!(
                    "duplicate function `{}` (first declared at {first_line}:{first_column})",
                    func.name
                ),
                line,
                column,
            });
        }
        sites.push((func.name.clone(), open));
        funcs.push(func);
    }
    Ok(funcs)
}

/// Whether `byte` can appear in a C identifier.
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `text` is a valid C identifier.
fn is_ident(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Parses one call site; `paren` is the index of the opening paren.
fn parse_call(text: &str, paren: usize) -> Result<GenFunc, GenError> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut args: Vec<(usize, usize)> = Vec::new();
    let mut start = paren + 1;
    let mut index = paren + 1;
    let mut closed = false;
    while index < bytes.len() {
        match bytes[index] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    args.push((start, index));
                    closed = true;
                    break;
                }
            }
            b',' if depth == 1 => {
                args.push((start, index));
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if !closed {
        let (line, column) = line_column_of(text, paren);
        return Err(GenError {
            message: format!("unterminated {MACRO}...) call"),
            line,
            column,
        });
    }

    let mut name: Option<String> = None;
    let mut paras = Vec::new();
    let mut roles = Vec::new();
    for (position, (arg_start, arg_end)) in args.iter().enumerate() {
        let raw = &text[*arg_start..*arg_end];
        let trimmed = raw.trim();
        let base = arg_start + (raw.len() - raw.trim_start().len());
        if trimmed.is_empty() {
            let (line, column) = line_column_of(text, base);
            return Err(GenError {
                message: if position == 0 {
                    "missing function name".to_owned()
                } else {
                    "empty parameter".to_owned()
                },
                line,
                column,
            });
        }
        if position == 0 {
            if !is_ident(trimmed) {
                let (line, column) = line_column_of(text, base);
                return Err(GenError {
                    message: format!("invalid function name `{trimmed}`"),
                    line,
                    column,
                });
            }
            name = Some(format!("Call_{trimmed}"));
            continue;
        }
        let colon = match trimmed.find(':') {
            Some(colon) => colon,
            None => {
                if !is_ident(trimmed) {
                    let (line, column) = line_column_of(text, base);
                    return Err(GenError {
                        message: format!("invalid parameter name `{trimmed}`"),
                        line,
                        column,
                    });
                }
                if paras.iter().any(|para: &String| para == trimmed) {
                    let (line, column) = line_column_of(text, base);
                    return Err(GenError {
                        message: format!("duplicate parameter `{trimmed}`"),
                        line,
                        column,
                    });
                }
                paras.push(trimmed.to_owned());
                continue;
            }
        };
        let param = trimmed[..colon].trim_end();
        let role_raw = &trimmed[colon + 1..];
        let role = role_raw.trim();
        let role_offset = base + colon + 1 + (role_raw.len() - role_raw.trim_start().len());
        if !is_ident(param) || role_raw.contains(':') {
            let (line, column) = line_column_of(text, base);
            return Err(GenError {
                message: format!("invalid parameter `{trimmed}` (expected `name` or `name:role`)"),
                line,
                column,
            });
        }
        if !ROLES.contains(&role) {
            let (line, column) = line_column_of(text, role_offset);
            return Err(GenError {
                message: format!(
                    "unknown slot role `{role}` (expected `read`, `write`, or `read_write`)"
                ),
                line,
                column,
            });
        }
        if paras.iter().any(|para: &String| para == param) {
            let (line, column) = line_column_of(text, base);
            return Err(GenError {
                message: format!("duplicate parameter `{param}`"),
                line,
                column,
            });
        }
        paras.push(param.to_owned());
        roles.push((param.to_owned(), role.to_owned()));
    }
    let name = name.ok_or_else(|| {
        // Unreachable: an empty call reports "missing function name" above.
        GenError {
            message: "missing function name".to_owned(),
            line: 1,
            column: 1,
        }
    })?;
    Ok(GenFunc { name, paras, roles })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 22 functions of examples/libc_wrapper, as wrapper source.
    ///
    /// The comments and decoys exercise what the scanner must skip;
    /// the kept declarations must match the committed libs.toml
    /// exactly (see the round-trip test below).
    const LIBC_SAMPLE: &str = r#"
#include "ccaller_gen.h"

int64_t CCaller_abi_version(void) {
    return CCALLER_ABI_VERSION;
}

/* ---- memory management ---- */

CCALLER_FUNC(malloc, len, mem_idx:write)
{
    (void)param_page; (void)params; (void)param_len;
    return 0;
}

CCALLER_FUNC(free, mem_idx:read)
{
    return 0;
}

// A comment mentioning CCALLER_FUNC(hidden, ghost:read) must not count.

/* A block comment with CCALLER_FUNC(also_hidden, x:read) inside. */

static const char *kNeedle = "CCALLER_FUNC(in_string, x)";

CCALLER_FUNC(memcpy, dst_idx:read, src_idx:read, len)
{
    return 0;
}

CCALLER_FUNC(memset, dst_idx:read, val, len)
{
    return 0;
}

CCALLER_FUNC(memcmp, dst_idx:read, dst_off, src_idx:read, src_off, len)
{
    return 0;
}

CCALLER_FUNC(read8, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read16, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read32, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(read64, addr_idx:read, off)
{
    return 0;
}

CCALLER_FUNC(write8, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write16, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write32, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(write64, addr_idx:read, off, val)
{
    return 0;
}

CCALLER_FUNC(strlen, str)
{
    return 0;
}

CCALLER_FUNC(atoi, str)
{
    return 0;
}

CCALLER_FUNC(strcmp, str1, str2)
{
    return 0;
}

CCALLER_FUNC(strncpy, dst_idx:read, str, len)
{
    return 0;
}

CCALLER_FUNC(add, a, b)
{
    return 0;
}

CCALLER_FUNC(store, slot_idx:write, val)
{
    return 0;
}

CCALLER_FUNC(fetch_add, slot_idx:read_write, delta)
{
    return 0;
}

CCALLER_FUNC(skip)
{
    return CCALLER_ERR_SKIP;
}

CCALLER_FUNC(abort)
{
    abort();
    return 0;
}

#ifdef __APPLE__
CCALLER_FUNC(apple_only, never:read)
#endif

#if defined(__linux__) || defined(_WIN32)
CCALLER_FUNC(unix_or_windows, p:read_write)
#endif

#if 0
CCALLER_FUNC(dead_code, x)
#endif
"#;

    /// One function of a generated description, in declaration order.
    fn func_of(report: &GenReport, name: &str) -> (Vec<String>, Vec<(String, String)>) {
        let doc = crate::config::source::SourceDoc {
            path: "gen".to_owned(),
            text: report.toml.clone(),
        };
        let desc: crate::config::lib_desc::LibDescription = doc.parse().unwrap();
        let func = desc.func(name).unwrap();
        let paras = func
            .paras
            .iter()
            .map(|para| para.get_ref().clone())
            .collect();
        let roles = func
            .slot_roles
            .iter()
            .map(|(param, role)| {
                let role = match role {
                    crate::config::lib_desc::SlotRole::Read => "read",
                    crate::config::lib_desc::SlotRole::Write => "write",
                    crate::config::lib_desc::SlotRole::ReadWrite => "read_write",
                };
                (param.clone(), role.to_owned())
            })
            .collect();
        (paras, roles)
    }

    #[test]
    fn names_paras_and_roles_are_extracted__F_X_05() {
        let report = generate_lib_description(LIBC_SAMPLE, "libc_wrapper.so").unwrap();
        // 22 libc functions + unix_or_windows; apple_only and dead_code
        // are skipped.
        assert_eq!(report.func_count, 23);

        let (paras, roles) = func_of(&report, "Call_malloc");
        assert_eq!(paras, vec!["len".to_owned(), "mem_idx".to_owned()]);
        assert_eq!(roles, vec![("mem_idx".to_owned(), "write".to_owned())]);

        let (paras, roles) = func_of(&report, "Call_fetch_add");
        assert_eq!(paras, vec!["slot_idx".to_owned(), "delta".to_owned()]);
        assert_eq!(
            roles,
            vec![("slot_idx".to_owned(), "read_write".to_owned())]
        );

        let (paras, roles) = func_of(&report, "Call_add");
        assert_eq!(paras, vec!["a".to_owned(), "b".to_owned()]);
        assert!(roles.is_empty());

        let (paras, _) = func_of(&report, "Call_skip");
        assert!(paras.is_empty());
    }

    #[test]
    fn comments_strings_and_macro_defines_are_skipped__F_X_05() {
        let report = generate_lib_description(LIBC_SAMPLE, "libc_wrapper.so").unwrap();
        for name in ["Call_hidden", "Call_also_hidden", "Call_in_string"] {
            assert!(
                report.toml.find(name).is_none(),
                "{name} must not be generated"
            );
        }
        // The `#define CCALLER_FUNC(name, ...) \` definition shape itself.
        let with_define = "#define CCALLER_FUNC(name, ...) \\\n    int64_t Call_##name(void)\n\
                           \nCCALLER_FUNC(real, x)\n{\n    return 0;\n}\n";
        let report = generate_lib_description(with_define, "w.so").unwrap();
        assert_eq!(report.func_count, 1);
        assert_eq!(func_of(&report, "Call_real").0, vec!["x".to_owned()]);
    }

    #[test]
    fn foreign_platform_blocks_are_skipped__F_X_05() {
        // __APPLE__ is foreign on both Linux and Windows CI, and the
        // `__linux__ || _WIN32` disjunction holds on both; `#if 0` is
        // dead on every platform.
        let report = generate_lib_description(LIBC_SAMPLE, "libc_wrapper.so").unwrap();
        assert!(
            report.toml.find("Call_apple_only").is_none(),
            "the __APPLE__ arm must be skipped here"
        );
        assert!(
            report.toml.find("Call_dead_code").is_none(),
            "#if 0 must be skipped"
        );
        assert!(
            report.toml.contains("Call_unix_or_windows"),
            "the linux-or-windows arm must be kept here"
        );

        // The defined()/negation spellings of the same platform logic.
        let source = "#if defined(__APPLE__)\nCCALLER_FUNC(apple, x)\n#endif\n\
                      #if !defined(__APPLE__)\nCCALLER_FUNC(not_apple, y:read)\n#endif\n";
        let report = generate_lib_description(source, "w.so").unwrap();
        assert!(report.toml.find("Call_apple").is_none());
        assert!(report.toml.contains("Call_not_apple"));
    }

    #[test]
    fn else_arms_of_foreign_blocks_are_kept__F_X_05() {
        let source = "#ifdef __APPLE__\nCCALLER_FUNC(mac_only, x)\n#else\n\
                      CCALLER_FUNC(kept_elsewhere, y:write)\n#endif\n";
        let report = generate_lib_description(source, "w.so").unwrap();
        assert!(report.toml.find("Call_mac_only").is_none());
        assert!(report.toml.contains("Call_kept_elsewhere"));
        let (_, roles) = func_of(&report, "Call_kept_elsewhere");
        assert_eq!(roles, vec![("y".to_owned(), "write".to_owned())]);
    }

    #[test]
    fn unknown_conditions_keep_their_arm_and_skip_the_else__F_X_05() {
        // Documented policy: an unrecognized condition is treated as
        // active, so its #else does not contribute.
        let source = "#ifdef __cplusplus\nCCALLER_FUNC(cpp, x)\n#else\n\
                      CCALLER_FUNC(not_cpp, y)\n#endif\n";
        let report = generate_lib_description(source, "w.so").unwrap();
        assert!(report.toml.contains("Call_cpp"));
        assert!(report.toml.find("Call_not_cpp").is_none());
    }

    #[test]
    fn whitespace_and_multi_line_calls_are_tolerated__F_X_05() {
        let source = "CCALLER_FUNC( spaced , a , b : read )\n{\n}\n\
                      CCALLER_FUNC(multi,\n    dst:read_write,\n    len)\n{\n}\n";
        let report = generate_lib_description(source, "w.so").unwrap();
        assert_eq!(
            func_of(&report, "Call_spaced").0,
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert_eq!(
            func_of(&report, "Call_multi"),
            (
                vec!["dst".to_owned(), "len".to_owned()],
                vec![("dst".to_owned(), "read_write".to_owned())]
            )
        );
    }

    #[test]
    fn bad_syntax_reports_line_and_column__F_X_05() {
        // Unknown role: points at the role text (`reed` starts at
        // column 19 of line 1).
        let error =
            generate_lib_description("CCALLER_FUNC(f, a:reed)\n{\n}\n", "w.so").unwrap_err();
        assert_eq!((error.line, error.column), (1, 19));
        assert!(
            error.message.contains("unknown slot role `reed`"),
            "{error}"
        );

        // Duplicate parameter.
        let error =
            generate_lib_description("CCALLER_FUNC(f, a, a:read)\n{\n}\n", "w.so").unwrap_err();
        assert!(error.message.contains("duplicate parameter `a`"), "{error}");
        assert_eq!(error.line, 1);

        // Duplicate function, message carries both sites.
        let error = generate_lib_description(
            "CCALLER_FUNC(f, a)\n{\n}\n\nCCALLER_FUNC(f, b)\n{\n}\n",
            "w.so",
        )
        .unwrap_err();
        assert!(
            error.message.contains("duplicate function `Call_f`"),
            "{error}"
        );
        assert!(error.message.contains("first declared at 1:"), "{error}");
        assert_eq!(error.line, 5);

        // Invalid function name.
        let error = generate_lib_description("CCALLER_FUNC(9bad, a)\n{\n}\n", "w.so").unwrap_err();
        assert!(
            error.message.contains("invalid function name `9bad`"),
            "{error}"
        );

        // Invalid parameter shape.
        let error = generate_lib_description("CCALLER_FUNC(f, a:b:c)\n{\n}\n", "w.so").unwrap_err();
        assert!(
            error.message.contains("invalid parameter `a:b:c`"),
            "{error}"
        );

        // Unterminated call.
        let error = generate_lib_description("CCALLER_FUNC(f, a\n", "w.so").unwrap_err();
        assert!(error.message.contains("unterminated"), "{error}");

        // Trailing comma → empty parameter on line 2 of the call.
        let error =
            generate_lib_description("CCALLER_FUNC(f,\n    a,)\n{\n}\n", "w.so").unwrap_err();
        assert!(error.message.contains("empty parameter"), "{error}");
        assert_eq!(error.line, 2);

        // Nothing to generate at all.
        let error = generate_lib_description("int main(void) { return 0; }\n", "w.so").unwrap_err();
        assert!(error.message.contains("no CCALLER_FUNC"), "{error}");
        assert_eq!((error.line, error.column), (1, 1));
    }

    #[test]
    fn generated_toml_matches_the_committed_libc_description__F_X_05() {
        let report = generate_lib_description(LIBC_SAMPLE, "libc_wrapper.so").unwrap();
        let doc = crate::config::source::SourceDoc {
            path: "gen".to_owned(),
            text: report.toml.clone(),
        };
        let generated: crate::config::lib_desc::LibDescription = doc.parse().unwrap();

        let example_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/libc_wrapper/libs.toml");
        let example_text = std::fs::read_to_string(&example_path).unwrap();
        let example_doc = crate::config::source::SourceDoc {
            path: "example".to_owned(),
            text: example_text,
        };
        let example: crate::config::lib_desc::LibDescription = example_doc.parse().unwrap();

        assert_eq!(*generated.version.get_ref(), *example.version.get_ref());
        assert_eq!(generated.libs.len(), example.libs.len());
        let generated_lib = generated.libs[0].get_ref();
        let example_lib = example.libs[0].get_ref();
        assert_eq!(generated_lib.path.get_ref(), example_lib.path.get_ref());

        // Every committed function must be generated identically.
        for example_func in example_lib.funcs.iter() {
            let example_func = example_func.get_ref();
            let name = example_func.name.get_ref();
            let generated_func = generated_lib
                .funcs
                .iter()
                .map(|func| func.get_ref())
                .find(|func| func.name.get_ref() == name)
                .unwrap_or_else(|| panic!("{name} missing from generated description"));
            assert_eq!(
                generated_func
                    .paras
                    .iter()
                    .map(|p| p.get_ref())
                    .collect::<Vec<_>>(),
                example_func
                    .paras
                    .iter()
                    .map(|p| p.get_ref())
                    .collect::<Vec<_>>(),
                "paras of {name}"
            );
            assert_eq!(
                generated_func.slot_roles, example_func.slot_roles,
                "roles of {name}"
            );
        }

        // The only functions beyond the committed set are the platform
        // decoys of the sample that stay active here.
        for generated_func in generated_lib.funcs.iter() {
            let name = generated_func.get_ref().name.get_ref().clone();
            let known = example_lib
                .funcs
                .iter()
                .any(|func| func.get_ref().name.get_ref() == &name)
                || name == "Call_unix_or_windows";
            assert!(known, "unexpected extra function {name}");
        }
    }

    #[test]
    fn the_sample_itself_lists_the_expected_functions__F_X_05() {
        // 22 libc functions + unix_or_windows (apple_only and dead_code
        // are skipped): the round-trip above pins the exact set.
        let report = generate_lib_description(LIBC_SAMPLE, "libc_wrapper.so").unwrap();
        assert_eq!(report.func_count, 23);
        for name in [
            "Call_malloc",
            "Call_free",
            "Call_memcpy",
            "Call_memset",
            "Call_memcmp",
            "Call_read8",
            "Call_read16",
            "Call_read32",
            "Call_read64",
            "Call_write8",
            "Call_write16",
            "Call_write32",
            "Call_write64",
            "Call_strlen",
            "Call_atoi",
            "Call_strcmp",
            "Call_strncpy",
            "Call_add",
            "Call_store",
            "Call_fetch_add",
            "Call_skip",
            "Call_abort",
        ] {
            assert!(
                report.toml.contains(&format!("name = \"{name}\"")),
                "{name}"
            );
        }
    }

    #[test]
    fn default_lib_filename_follows_the_platform_convention__F_X_05() {
        let name = default_lib_filename("libc_wrapper");
        if cfg!(windows) {
            assert_eq!(name, "libc_wrapper.dll");
        } else {
            assert_eq!(name, "libc_wrapper.so");
        }
    }
}
