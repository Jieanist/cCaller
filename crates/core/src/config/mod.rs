//! Configuration loading and static analysis for cCaller.
//!
//! Owns everything between a TOML file on disk and a validated, expanded run
//! plan: [`source`] tracks file text and maps parser spans back to
//! line/column, [`diag`] defines the load-time diagnostic currency,
//! [`lib_desc`] models library descriptions (requirement spec 7.2),
//! [`cases`] models test case configurations (requirement spec 7.3),
//! [`validate`] runs the cross-reference rules of FR-C-08, [`expand`]
//! turns input groups into concrete sub-cases (FR-C-03~06), and [`value`]
//! defines the scalar grammar shared by command arguments and expectation
//! values.

pub mod cases;
pub mod diag;
pub mod expand;
pub mod lib_desc;
pub mod source;
pub mod validate;
pub mod value;

pub use cases::{
    CaseConfig, CaseEnv, CmdDef, ConcurrencyGroup, GlobalEnv, InputGroup, InputValue, RangeSpec,
    TestDef,
};
pub use diag::{codes, Diagnostic};
pub use expand::{
    expand_test, Expectation, ResolvedCmd, SubCase, MAX_COMBINATIONS_PER_INPUT_GROUP,
};
pub use lib_desc::{
    validate as validate_lib_description, FuncDecl, LibDescription, LibEntry, SlotRole,
    SUPPORTED_VERSION,
};
pub use source::SourceDoc;
pub use validate::validate_cases;
pub use value::{
    parse_value, ConcreteValue, ParsedValue, ResolveError, ScalarRaw, ValueKind, ValueSyntaxError,
};
