//! Configuration loading and static analysis for cCaller.
//!
//! Owns everything between a TOML file on disk and a validated, expanded run
//! plan. The [`value`] submodule defines the scalar grammar shared by command
//! arguments and expectation values (requirement spec 7.3).

pub mod value;

pub use value::{
    parse_value, ConcreteValue, ParsedValue, ResolveError, ScalarRaw, ValueKind, ValueSyntaxError,
};
