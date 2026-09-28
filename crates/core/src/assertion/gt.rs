//! The `expect_gt` comparison assertion (F-V-03).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_gt`: the return value must be strictly greater than the
/// expected value (F-V-03).
#[derive(Debug)]
pub struct Gt;

impl Assertion for Gt {
    fn field_name(&self) -> &'static str {
        "expect_gt"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        // The natural negation of `>` is `<=`.
        Some("expect_le")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual > *value,
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
    fn greater_than_compares_integers__F_V_03() {
        let assertion = Gt;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 8));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 7));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 6));
    }

    #[test]
    fn greater_than_never_matches_a_string_return__F_V_03() {
        let assertion = Gt;
        assert!(!assertion.evaluate(&ConcreteValue::Str("7".to_string()), 8));
    }

    #[test]
    fn greater_than_negates_into_less_or_equal__F_V_03() {
        assert_eq!(Gt.field_name(), "expect_gt");
        assert_eq!(Gt.negated_field_name(), Some("expect_le"));
    }
}
