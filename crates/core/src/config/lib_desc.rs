//! The library description model (requirement spec 7.2).
//!
//! A library description declares the dynamic libraries under test, their
//! exported `Call_<name>` functions, the ordered parameter names, and the
//! explicit `slot_roles` that drive the static def-use analysis (FR-C-10).
//! Slot semantics are never guessed from parameter names.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;
use toml::Spanned;

use crate::error::Location;

use super::diag::{codes, Diagnostic};
use super::source::SourceDoc;

/// The only library-description schema version understood by this release.
pub const SUPPORTED_VERSION: u64 = 1;

/// Root of a library description file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibDescription {
    /// Schema version; only [`SUPPORTED_VERSION`] is accepted.
    pub version: Spanned<u64>,
    /// Declared libraries; must contain at least one entry.
    pub libs: Vec<Spanned<LibEntry>>,
}

/// One dynamic library and the functions it exports.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibEntry {
    /// Dynamic library path, relative to the description file.
    pub path: Spanned<String>,
    /// Functions exported by this library.
    pub funcs: Vec<Spanned<FuncDecl>>,
}

/// One exported function in the unified ABI.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FuncDecl {
    /// Exported symbol name; globally unique across libraries (FR-A-01).
    pub name: Spanned<String>,
    /// Ordered parameter names; Cmd `args` must match name, count, order
    /// (FR-C-08).
    pub paras: Vec<Spanned<String>>,
    /// Explicit slot roles for parameters whose value is a param_page index.
    #[serde(default)]
    pub slot_roles: BTreeMap<String, SlotRole>,
}

/// What a wrapper does with the slot a parameter points at (requirement
/// spec 7.2 slot-role semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotRole {
    /// The wrapper reads the slot.
    Read,
    /// The wrapper writes the slot.
    Write,
    /// The wrapper reads and writes the slot.
    ReadWrite,
}

impl SlotRole {
    /// Whether the wrapper reads the slot (`read` or `read_write`).
    pub fn reads(self) -> bool {
        matches!(self, SlotRole::Read | SlotRole::ReadWrite)
    }

    /// Whether the wrapper writes the slot (`write` or `read_write`).
    pub fn writes(self) -> bool {
        matches!(self, SlotRole::Write | SlotRole::ReadWrite)
    }
}

impl LibDescription {
    /// Looks up a function declaration by exported name.
    pub fn func(&self, name: &str) -> Option<&FuncDecl> {
        self.libs
            .iter()
            .flat_map(|lib| lib.get_ref().funcs.iter())
            .map(Spanned::get_ref)
            .find(|func| func.name.get_ref() == name)
    }
}

/// Checks a parsed library description against the 7.2 constraints.
///
/// Findings accumulate: unsupported version, empty `libs`, duplicate
/// library paths, duplicate function names across libraries (FR-A-01),
/// duplicate parameter names within a function, and `slot_roles` keys the
/// function does not declare.
pub fn validate(desc: &LibDescription, doc: &SourceDoc) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let version = *desc.version.get_ref();
    if version != SUPPORTED_VERSION {
        out.push(Diagnostic::new(
            codes::UNSUPPORTED_VERSION,
            doc.locate(desc.version.span()),
            format!("unsupported schema version {version}, expected {SUPPORTED_VERSION}"),
        ));
    }
    if desc.libs.is_empty() {
        out.push(Diagnostic::new(
            codes::SCHEMA,
            Location {
                file: doc.path.clone(),
                line: 1,
                column: 1,
            },
            "`libs` must contain at least one library".to_string(),
        ));
    }

    let mut lib_paths: HashMap<&str, Location> = HashMap::new();
    let mut func_sites: HashMap<&str, (&str, Location)> = HashMap::new();
    for lib in &desc.libs {
        validate_lib_entry(
            lib.get_ref(),
            doc,
            &mut lib_paths,
            &mut func_sites,
            &mut out,
        );
    }
    out
}

fn validate_lib_entry<'a>(
    lib: &'a LibEntry,
    doc: &SourceDoc,
    lib_paths: &mut HashMap<&'a str, Location>,
    func_sites: &mut HashMap<&'a str, (&'a str, Location)>,
    out: &mut Vec<Diagnostic>,
) {
    let path = lib.path.get_ref().as_str();
    let path_loc = doc.locate(lib.path.span());
    if let Some(first) = lib_paths.get(path) {
        out.push(Diagnostic::new(
            codes::DUPLICATE_LIB_PATH,
            path_loc,
            format!("duplicate library path `{path}` (first declared at {first})"),
        ));
    } else {
        lib_paths.insert(path, path_loc);
    }

    for func in &lib.funcs {
        let func = func.get_ref();
        let name = func.name.get_ref().as_str();
        let name_loc = doc.locate(func.name.span());
        if let Some((first_lib, first_loc)) = func_sites.get(name) {
            out.push(Diagnostic::new(
                codes::DUPLICATE_FUNCTION,
                name_loc.clone(),
                format!(
                    "function `{name}` is declared in both library `{first_lib}` (at {first_loc}) \
                     and library `{path}`"
                ),
            ));
        } else {
            func_sites.insert(name, (path, name_loc.clone()));
        }
        validate_func_decl(func, name, name_loc, doc, out);
    }
}

fn validate_func_decl(
    func: &FuncDecl,
    name: &str,
    name_loc: Location,
    doc: &SourceDoc,
    out: &mut Vec<Diagnostic>,
) {
    let mut seen_paras: HashSet<&str> = HashSet::new();
    for para_spanned in &func.paras {
        let para = para_spanned.get_ref().as_str();
        if !seen_paras.insert(para) {
            out.push(Diagnostic::new(
                codes::DUPLICATE_PARAM,
                doc.locate(para_spanned.span()),
                format!("parameter `{para}` is declared twice in function `{name}`"),
            ));
        }
    }

    // `slot_roles` keys must reference declared parameters (7.2). BTreeMap
    // iteration keeps the diagnostic order deterministic.
    for key in func.slot_roles.keys() {
        if !seen_paras.contains(key.as_str()) {
            out.push(Diagnostic::new(
                codes::SLOT_ROLE_UNKNOWN_PARAM,
                name_loc.clone(),
                format!(
                    "slot role for unknown parameter `{key}` in function `{name}` \
                     (not declared in paras)"
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (SourceDoc, LibDescription) {
        let doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: text.to_string(),
        };
        let desc = doc.parse().unwrap();
        (doc, desc)
    }

    fn codes_of(diags: &[Diagnostic]) -> Vec<&'static str> {
        diags.iter().map(|d| d.code).collect()
    }

    #[test]
    fn valid_description_validates_clean__F_C_02() {
        let (doc, desc) = parse(
            "version = 1\n\
             [[libs]]\n\
             path = \"a.so\"\n\
             funcs = [\n\
             \x20 { name = \"Call_x\", paras = [\"p\"], slot_roles = { p = \"read\" } },\n\
             \x20 { name = \"Call_y\", paras = [] },\n\
             ]\n",
        );
        assert!(validate(&desc, &doc).is_empty());
    }

    #[test]
    fn missing_version_is_rejected_at_parse__F_C_02() {
        let doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: "[[libs]]\npath = \"a.so\"\nfuncs = []\n".to_string(),
        };
        let err = doc.parse::<LibDescription>().unwrap_err();
        assert_eq!(err.code, "missing_field");
        assert!(err.message.contains("version"));
    }

    #[test]
    fn unsupported_version_is_reported__F_C_02() {
        let (doc, desc) = parse("version = 2\n[[libs]]\npath = \"a.so\"\nfuncs = []\n");
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["unsupported_version"]);
        assert!(diags[0].message.contains('1'));
        assert!(diags[0].location.line >= 1);
    }

    #[test]
    fn duplicate_function_across_libs_is_reported__F_A_01() {
        let (doc, desc) = parse(
            "version = 1\n\
             [[libs]]\n\
             path = \"a.so\"\n\
             funcs = [{ name = \"Call_dup\", paras = [] }]\n\
             [[libs]]\n\
             path = \"b.so\"\n\
             funcs = [{ name = \"Call_dup\", paras = [] }]\n",
        );
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["duplicate_function"]);
        // FR-A-01: the conflict must name both library files.
        assert!(diags[0].message.contains("a.so") && diags[0].message.contains("b.so"));
        // The location points at the second declaration (line 7).
        assert_eq!(diags[0].location.line, 7);
    }

    #[test]
    fn duplicate_lib_path_is_reported__F_C_07() {
        let (doc, desc) = parse(
            "version = 1\n\
             [[libs]]\n\
             path = \"a.so\"\n\
             funcs = [{ name = \"Call_x\", paras = [] }]\n\
             [[libs]]\n\
             path = \"a.so\"\n\
             funcs = [{ name = \"Call_y\", paras = [] }]\n",
        );
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["duplicate_lib_path"]);
    }

    #[test]
    fn duplicate_param_within_function_is_reported__F_C_07() {
        let (doc, desc) = parse(
            "version = 1\n[[libs]]\npath = \"a.so\"\n\
             funcs = [{ name = \"Call_x\", paras = [\"p\", \"p\"] }]\n",
        );
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["duplicate_param"]);
        assert!(diags[0].message.contains("`p`"));
    }

    #[test]
    fn slot_role_for_undeclared_param_is_reported__F_C_07() {
        let (doc, desc) = parse(
            "version = 1\n[[libs]]\npath = \"a.so\"\n\
             funcs = [{ name = \"Call_x\", paras = [\"p\"], slot_roles = { q = \"read\" } }]\n",
        );
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["slot_role_unknown_param"]);
        assert!(diags[0].message.contains("`q`"));
    }

    #[test]
    fn invalid_slot_role_value_is_a_type_mismatch__F_C_07() {
        let doc = SourceDoc {
            path: "libs.toml".to_string(),
            text: "version = 1\n[[libs]]\npath = \"a.so\"\n\
                   funcs = [{ name = \"Call_x\", paras = [\"p\"], slot_roles = { p = \"sometimes\" } }]\n"
                .to_string(),
        };
        let err = doc.parse::<LibDescription>().unwrap_err();
        assert_eq!(err.code, "type_mismatch");
        assert!(err.message.contains("sometimes"));
    }

    #[test]
    fn empty_libs_is_reported__F_C_07() {
        let (doc, desc) = parse("version = 1\nlibs = []\n");
        let diags = validate(&desc, &doc);
        assert_eq!(codes_of(&diags), vec!["schema"]);
    }

    #[test]
    fn slot_role_predicates_cover_read_write__F_C_10() {
        assert!(SlotRole::Read.reads() && !SlotRole::Read.writes());
        assert!(!SlotRole::Write.reads() && SlotRole::Write.writes());
        assert!(SlotRole::ReadWrite.reads() && SlotRole::ReadWrite.writes());
    }

    #[test]
    fn func_lookup_finds_declared_functions__F_C_07() {
        let (_, desc) = parse(
            "version = 1\n[[libs]]\npath = \"a.so\"\n\
             funcs = [{ name = \"Call_x\", paras = [\"p\"] }]\n",
        );
        assert!(desc.func("Call_x").is_some());
        assert!(desc.func("Call_nope").is_none());
    }
}
