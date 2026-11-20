//! #294: the human foreground window is never an implicit delivery target.
use super::resolve_type_delivery_target;
use synapse_action::ActionError;

#[test]
fn must_pass_explicit_target_wins_over_session_and_preflight() -> Result<(), ActionError> {
    assert_eq!(resolve_type_delivery_target(Some(11), Some(22), 99)?, 11);
    Ok(())
}

#[test]
fn must_pass_session_target_used_without_explicit_target() -> Result<(), ActionError> {
    assert_eq!(resolve_type_delivery_target(None, Some(22), 99)?, 22);
    Ok(())
}

#[test]
fn must_fail_type_without_any_delivery_target() {
    assert!(matches!(
        resolve_type_delivery_target(None, None, 0),
        Err(ActionError::InputTargetRequired { .. })
    ));
}

#[test]
fn must_fail_preflight_foreground_as_only_target() {
    assert!(matches!(
        resolve_type_delivery_target(None, None, 99),
        Err(ActionError::InputTargetRequired { .. })
    ));
}
