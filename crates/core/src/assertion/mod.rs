//! Assertion extension point (FR-V-02, requirement spec 4.2/5.5).
//!
//! A Cmd's `expect_*` fields are routed through a registry that maps each
//! configuration field name to an [`Assertion`] implementation. The
//! configuration model therefore stays generic: it carries every `expect_*`
//! field by name and defers "what does this assertion mean" to the registry.
//!
//! Adding an assertion is a two-file change — a new file implementing
//! [`Assertion`] plus one entry in the [`ASSERTIONS`] table below. The
//! scheduling and configuration model are untouched (G-02, A-2).

mod eq;
mod ge;
mod ne;

use std::fmt::Debug;

use crate::config::value::ConcreteValue;

pub use eq::Eq;
pub use ge::Ge;
pub use ne::Ne;

/// Result of evaluating one resolved assertion against an actual value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionOutcome {
    /// Whether the assertion held.
    pub passed: bool,
    /// The expected side of a failure report, e.g. `expect_ne 7`.
    pub expectation: String,
}

/// One assertion kind registered under an `expect_*` configuration field.
///
/// An assertion knows its field name, how the FR-C-07 `!value` prefix
/// folds it into a counterpart, and how to compare an actual `i64` return
/// value against a resolved expected value.
pub trait Assertion: Debug + Send + Sync {
    /// The `expect_*` configuration field this assertion reads, e.g.
    /// `expect_eq`.
    fn field_name(&self) -> &'static str;

    /// The field a `!value` prefix folds into, e.g. `expect_eq` folds into
    /// `expect_ne`.
    ///
    /// `None` means the assertion has no natural negation; a `!` prefix on
    /// its value is then a load-time error.
    fn negated_field_name(&self) -> Option<&'static str>;

    /// Whether `actual` satisfies this assertion for `expected`.
    ///
    /// The wrapper returns an `i64` (requirement spec 7.1), so a string
    /// expectation can never match; numeric assertions report `false` for
    /// it while string assertions (P1) define their own reading.
    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool;
}

/// The registration point: every assertion kind keyed by its field name.
///
/// Adding an assertion means adding one entry here and the new file that
/// implements [`Assertion`]; nothing else changes (FR-V-02, G-02, A-2).
/// Order here is the canonical order used when several field names must be
/// listed in one diagnostic.
static ASSERTIONS: &[(&str, &dyn Assertion)] =
    &[("expect_eq", &Eq), ("expect_ne", &Ne), ("expect_ge", &Ge)];

/// Looks up a registered assertion by its configuration field name.
pub fn lookup(field_name: &str) -> Option<&'static dyn Assertion> {
    ASSERTIONS
        .iter()
        .find(|(name, _)| *name == field_name)
        .map(|(_, assertion)| *assertion)
}

/// Iterates the registry in registration order.
pub fn iter() -> impl Iterator<Item = (&'static str, &'static dyn Assertion)> {
    ASSERTIONS.iter().copied()
}

/// All registered assertion field names, in registration order.
pub fn field_names() -> impl Iterator<Item = &'static str> {
    ASSERTIONS.iter().map(|(name, _)| *name)
}

/// Formats assertion field names as backticked prose, e.g.
/// `` `expect_eq` and `expect_ne` ``.
pub fn backticked(names: &[&'static str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [many @ .., last] => format!("{}, and {}", many.join(", "), last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::value::ConcreteValue;

    #[test]
    fn registry_resolves_every_builtin_field__F_V_02() {
        for (name, assertion) in iter() {
            assert_eq!(assertion.field_name(), name);
        }
    }

    #[test]
    fn registry_lookup_finds_registered_and_none_else__F_V_02() {
        assert!(lookup("expect_eq").is_some());
        assert!(lookup("expect_ne").is_some());
        assert!(lookup("expect_gt").is_none());
        assert!(lookup("threads").is_none());
    }

    #[test]
    fn registry_order_lists_eq_before_ne__F_V_02() {
        // The conflict message must keep the historical
        // "`expect_eq` and `expect_ne`" order, so eq must precede ne in
        // the registration table.
        let names: Vec<&str> = field_names().collect();
        let eq = names.iter().position(|name| *name == "expect_eq").unwrap();
        let ne = names.iter().position(|name| *name == "expect_ne").unwrap();
        assert!(
            eq < ne,
            "expect_eq must precede expect_ne in registry order"
        );
    }

    #[test]
    fn backticked_joins_names_grammatically__F_V_02() {
        assert_eq!(backticked(&["expect_eq"]), "`expect_eq`");
        assert_eq!(
            backticked(&["expect_eq", "expect_ne"]),
            "`expect_eq` and `expect_ne`"
        );
        assert_eq!(
            backticked(&["expect_eq", "expect_ne", "expect_ge"]),
            "`expect_eq`, `expect_ne`, and `expect_ge`"
        );
    }

    #[test]
    fn outcome_carries_pass_fail_and_expectation__F_V_02() {
        let eq = lookup("expect_eq").expect("registered");
        let outcome = AssertionOutcome {
            passed: eq.evaluate(&ConcreteValue::Int(7), 7),
            expectation: format!("{} {}", eq.field_name(), ConcreteValue::Int(7)),
        };
        assert!(outcome.passed);
        assert_eq!(outcome.expectation, "expect_eq 7");
    }
}
