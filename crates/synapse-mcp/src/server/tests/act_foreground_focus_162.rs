use super::{FOREGROUND_INPUT_VERBS, act_foreground_requires_claimed_focus};

#[test]
fn must_pass_foreground_operation_focuses_before_every_input_verb() {
    // Expectations come from the #162 contract (every real-input verb), not
    // from the production constant, so dropping a verb from the constant fails.
    let contract_input_verbs = [
        "click",
        "dblclick",
        "type",
        "key",
        "key_chord",
        "press",
        "scroll",
    ];
    for verb in contract_input_verbs {
        assert!(
            FOREGROUND_INPUT_VERBS.contains(&verb),
            "verb={verb} is a real-input verb and must be in FOREGROUND_INPUT_VERBS"
        );
        assert!(
            act_foreground_requires_claimed_focus(true, verb, false),
            "act operation=foreground verb={verb} must focus the claimed target first"
        );
    }
    assert_eq!(
        FOREGROUND_INPUT_VERBS.len(),
        contract_input_verbs.len(),
        "FOREGROUND_INPUT_VERBS must hold exactly the contract input verbs: {FOREGROUND_INPUT_VERBS:?}"
    );
}

#[test]
fn must_pass_foreground_operation_key_chord_always_focuses() {
    assert!(act_foreground_requires_claimed_focus(
        true, "anything", true
    ));
}

#[test]
fn must_fail_invoke_never_focuses_so_background_preflight_stays_on_invoke() {
    for verb in FOREGROUND_INPUT_VERBS {
        assert!(
            !act_foreground_requires_claimed_focus(false, verb, false),
            "act operation=invoke verb={verb} must not take the foreground focus step"
        );
    }
    assert!(!act_foreground_requires_claimed_focus(false, "press", true));
}

#[test]
fn must_fail_foreground_operation_non_input_verbs_pass_through() {
    for verb in [
        "read",
        "screenshot",
        "navigate",
        "set_field",
        "focus_window",
        "hover",
    ] {
        assert!(
            !act_foreground_requires_claimed_focus(true, verb, false),
            "verb={verb} is not an input verb and must not be force-focused"
        );
    }
}

#[test]
fn must_fail_verb_matching_is_exact() {
    assert!(!act_foreground_requires_claimed_focus(
        true, "clicks", false
    ));
    assert!(!act_foreground_requires_claimed_focus(true, "", false));
}
