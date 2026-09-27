//! Configuration loading and static analysis for cCaller.
//!
//! Owns everything between a TOML file on disk and a validated, expanded run
//! plan: [`source`] tracks file text and maps parser spans back to
//! line/column, [`diag`] defines the load-time diagnostic currency,
//! [`lib_desc`] models library descriptions (requirement spec 7.2), and
//! [`value`] defines the scalar grammar shared by command arguments and
//! expectation values (requirement spec 7.3).

pub mod diag;
pub mod lib_desc;
pub mod source;
pub mod value;

pub use diag::{codes, Diagnostic};
pub use lib_desc::{
    validate as validate_lib_description, FuncDecl, LibDescription, LibEntry, SlotRole,
    SUPPORTED_VERSION,
};
pub use source::SourceDoc;
pub use value::{
    parse_value, ConcreteValue, ParsedValue, ResolveError, ScalarRaw, ValueKind, ValueSyntaxError,
};
