//! Regression tests for #60: `DataPredicate::Regex` is matched through a
//! compiled-pattern cache; behavior must be unchanged.

use serde_json::json;
use synapse_core::{DataPredicate, filter::matches_data_predicate};

fn regex(pattern: &str) -> DataPredicate {
    DataPredicate::Regex {
        pattern: pattern.to_owned(),
    }
}

#[test]
fn must_pass_regex_predicate_matches_repeatedly() {
    let predicate = regex("^ab+c$");
    for _ in 0..3 {
        assert!(matches_data_predicate(&predicate, Some(&json!("abbbc"))));
        assert!(!matches_data_predicate(&predicate, Some(&json!("ac"))));
    }
}

#[test]
fn must_fail_invalid_pattern_never_matches_and_does_not_poison_the_cache() {
    let bad = regex("[");
    for _ in 0..3 {
        assert!(!matches_data_predicate(&bad, Some(&json!("["))));
    }
    assert!(matches_data_predicate(&regex("x"), Some(&json!("x"))));
}

#[test]
fn must_fail_non_string_and_absent_values_do_not_match() {
    let predicate = regex(".*");
    assert!(!matches_data_predicate(&predicate, None));
    assert!(!matches_data_predicate(&predicate, Some(&json!(5))));
}

#[test]
fn must_fail_cache_overflow_keeps_matching_correct() {
    for index in 0..600 {
        let pattern = format!("^p{index}$");
        assert!(matches_data_predicate(
            &regex(&pattern),
            Some(&json!(format!("p{index}")))
        ));
        assert!(!matches_data_predicate(
            &regex(&pattern),
            Some(&json!("other"))
        ));
    }
}
