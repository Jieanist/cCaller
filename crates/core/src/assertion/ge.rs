//! The `expect_ge` comparison assertion (F-V-03).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_ge`: the return value must be greater than or equal to the
/// expected value (F-V-03).
///
/// The natural negation of `>=` is `<`, so the FR-C-07 `!value` prefix
/// folds this into `expect_lt`.
#[derive(Debug)]
pub struct Ge;

impl Assertion for Ge {
    fn field_name(&self) -> &'static str {
        "expect_ge"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        Some("expect_lt")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual >= *value,
            // The wrapper returns an i64; a string expectation can never
            // satisfy an ordering.
            ConcreteValue::Str(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greater_or_equal_compares_integers__F_V_03() {
        let assertion = Ge;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 7));
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 8));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 6));
    }

    #[test]
    fn greater_or_equal_never_matches_a_string_return__F_V_03() {
        let assertion = Ge;
        assert!(!assertion.evaluate(&ConcreteValue::Str("7".to_string()), 8));
    }

    #[test]
    fn greater_or_equal_negates_into_less_than__F_V_03() {
        assert_eq!(Ge.field_name(), "expect_ge");
        assert_eq!(Ge.negated_field_name(), Some("expect_lt"));
    }
}
