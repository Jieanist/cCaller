//! The `expect_lt` comparison assertion (F-V-03, fix E-02).

use crate::assertion::Assertion;
use crate::config::value::ConcreteValue;

/// `expect_lt`: the return value must be strictly less than the expected
/// value (F-V-03).
///
/// This is the counterpart `expect_ge = "!value"` folds into (FR-C-07);
/// until it was registered that fold was a load-time error, leaving the
/// `!expect_ge` spelling dangling (defect E-02).
#[derive(Debug)]
pub struct Lt;

impl Assertion for Lt {
    fn field_name(&self) -> &'static str {
        "expect_lt"
    }

    fn negated_field_name(&self) -> Option<&'static str> {
        // The natural negation of `<` is `>=`.
        Some("expect_ge")
    }

    fn evaluate(&self, expected: &ConcreteValue, actual: i64) -> bool {
        match expected {
            ConcreteValue::Int(value) => actual < *value,
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
    fn less_than_compares_integers__F_V_03() {
        let assertion = Lt;
        assert!(assertion.evaluate(&ConcreteValue::Int(7), 6));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 7));
        assert!(!assertion.evaluate(&ConcreteValue::Int(7), 8));
    }

    #[test]
    fn less_than_never_matches_a_string_return__F_V_03() {
        let assertion = Lt;
        assert!(!assertion.evaluate(&ConcreteValue::Str("7".to_string()), 6));
    }

    #[test]
    fn less_than_negates_into_greater_or_equal__F_V_03() {
        assert_eq!(Lt.field_name(), "expect_lt");
        assert_eq!(Lt.negated_field_name(), Some("expect_ge"));
    }
}
