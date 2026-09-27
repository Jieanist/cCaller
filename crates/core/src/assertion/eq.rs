//! The `expect_eq` assertion (FR-V-01).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_eq`: the return value must equal the expected value.
///
/// The FR-C-07 `!value` prefix folds this into `expect_ne`.
#[derive(Debug)]
pub struct Eq;

impl Assertion for Eq {
    fn field_name(&self) -> &'static str {
        "expect_eq"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        Some("expect_ne")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual == *value,
            // The wrapper returns an i64; a string expectation can never
            // equal it.
            ConcreteValue::Str(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_matches_integers__F_V_01() {
        let assertion = Eq;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 7));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 8));
    }

    #[test]
    fn equality_never_matches_a_string_return__F_V_01() {
        let assertion = Eq;
        assert!(!assertion.evaluate(&ConcreteValue::Str("7".to_string()), 7));
    }

    #[test]
    fn equality_negates_into_not_equal__F_V_01() {
        assert_eq!(Eq.field_name(), "expect_eq");
        assert_eq!(Eq.negated_field_name(), Some("expect_ne"));
    }
}
