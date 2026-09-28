//! The `expect_le` comparison assertion (F-V-03).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_le`: the return value must be less than or equal to the
/// expected value (F-V-03).
#[derive(Debug)]
pub struct Le;

impl Assertion for Le {
    fn field_name(&self) -> &'static str {
        "expect_le"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        // The natural negation of `<=` is `>`.
        Some("expect_gt")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual <= *value,
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
    fn less_or_equal_compares_integers__F_V_03() {
        let assertion = Le;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 7));
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 6));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 8));
    }

    #[test]
    fn less_or_equal_never_matches_a_string_return__F_V_03() {
        let assertion = Le;
        assert!(!assertion.evaluate(&ConcreteValue::Str("7".to_string()), 6));
    }

    #[test]
    fn less_or_equal_negates_into_greater_than__F_V_03() {
        assert_eq!(Le.field_name(), "expect_le");
        assert_eq!(Le.negated_field_name(), Some("expect_gt"));
    }
}
