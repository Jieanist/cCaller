//! The `expect_ne` assertion (FR-V-01).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_ne`: the return value must differ from the expected value.
///
/// The FR-C-07 `!value` prefix folds this into `expect_eq`.
#[derive(Debug)]
pub struct Ne;

impl Assertion for Ne {
    fn field_name(&self) -> &'static str {
        "expect_ne"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        Some("expect_eq")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual != *value,
            // The wrapper returns an i64, so it always differs from a
            // string expectation.
            ConcreteValue::Str(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_equal_matches_integers__F_V_01() {
        let assertion = Ne;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 8));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 7));
    }

    #[test]
    fn not_equal_always_matches_a_string_return__F_V_01() {
        let assertion = Ne;
        assert!(assertion.evaluate(&ConcreteValue::Str("7".to_string()), 7));
    }

    #[test]
    fn not_equal_negates_into_equal__F_V_01() {
        assert_eq!(Ne.field_name(), "expect_ne");
        assert_eq!(Ne.negated_field_name(), Some("expect_eq"));
    }
}
