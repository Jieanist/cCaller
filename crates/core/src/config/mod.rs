//! Configuration loading and static analysis for cCaller.
//!
//! Owns everything between a TOML file on disk and a validated, expanded run
//! plan: [`source`] tracks file text and maps parser spans back to
//! line/column, [`diag`] defines the load-time diagnostic currency,
//! [`lib_desc`] models library descriptions (requirement spec 7.2),
//! [`cases`] models test case configurations (requirement spec 7.3),
//! [`validate`] runs the cross-reference rules of FR-C-08, [`expand`]
//! turns input groups into concrete sub-cases (FR-C-03~06), [`defuse`]
//! runs the slot def-use analysis over the expanded sub-cases (FR-C-10),
//! [`check`] glues every stage into the 7.6 report, [`expand_report`]
//! renders the sub-case listing of the `expand` tool, [`fmt`]
//! normalizes a configuration file's TOML layout, [`gen`] derives a
//! library description from macro-annotated wrapper source, and
//! [`value`] defines the scalar grammar shared by command arguments and
//! expectation values.

pub mod cases;
pub mod check;
pub mod defuse;
pub mod diag;
pub mod expand;
pub mod expand_report;
pub mod fmt;
pub mod gen;
pub mod layers;
pub mod lib_desc;
pub mod migrate;
pub mod source;
pub mod validate;
pub mod value;

pub use cases::{
    CaseConfig, CaseEnv, CmdDef, ConcurrencyGroup, GlobalEnv, InputGroup, InputValue, RangeSpec,
    TestDef,
};
pub use check::{run_check, CheckReport, CheckStats, REPORT_SCHEMA};
pub use defuse::{analyze_def_use, PARAM_PAGE_SLOTS};
pub use diag::{codes, Diagnostic};
pub use expand::{
    expand_test, ResolvedCmd, ResolvedExpectation, SubCase, MAX_COMBINATIONS_PER_INPUT_GROUP,
};
pub use expand_report::{run_expand, ExpandReport, ExpandedSubCase, ExpandedTest, EXPAND_SCHEMA};
pub use fmt::{normalize_toml, FmtError};
pub use gen::{default_lib_filename, generate_lib_description, GenError, GenReport};
pub use lib_desc::{
    validate as validate_lib_description, FuncDecl, LibDescription, LibEntry, SlotRole,
    SUPPORTED_VERSION,
};
pub use migrate::{migrate, MigrateError, MigrateSource, MigrationOutput};
pub use source::SourceDoc;
pub use validate::validate_cases;
pub use value::{
    parse_value, ConcreteValue, ParsedValue, ResolveError, ScalarRaw, ValueKind, ValueSyntaxError,
};
