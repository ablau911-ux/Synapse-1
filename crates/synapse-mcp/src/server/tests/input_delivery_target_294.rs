//! #294: the per-call OS-input delivery target. Explicit HWND, then the
//! session target, then an explicit focus; a dead session target is refused,
//! never replaced by another window, and no target is never the foreground.
use serde_json::Value;
use synapse_core::error_codes;
use tempfile::TempDir;

use super::{ErrorData, SessionTarget, tests::service_with_temp_db};

/// Not a live window on any desktop (IsWindow is false).
const DEAD_HWND: i64 = 0x7FFF_FFF0;

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn focused_window() -> synapse_action::InputDeliveryTarget {
    synapse_action::InputDeliveryTarget {
        root_hwnd: 0x2942,
        pid: 2942,
        source: "act_focus_window",
    }
}

#[test]
fn must_fail_dead_session_target_is_refused_not_replaced_by_focus() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let session = "i294-dead-session-target";
    service.set_session_target(session, SessionTarget::Window { hwnd: DEAD_HWND })?;
    let (handle, _recording, _cancel) =
        service.m2_action_context_for_session_id(Some(session.to_owned()))?;
    assert!(handle.bind_session_delivery_target(focused_window())?);
    let Err(error) = service.resolve_input_delivery_target(None, &handle) else {
        anyhow::bail!("a closed session target must not fall back to another window");
    };
    // #315: a closed target window is TARGET_WINDOW_NOT_FOUND.
    assert_eq!(
        error_code(&error),
        Some(error_codes::TARGET_WINDOW_NOT_FOUND),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn must_fail_no_target_requires_one_and_names_the_remedy() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (handle, _recording, _cancel) =
        service.m2_action_context_for_session_id(Some("i294-no-target".to_owned()))?;
    let Err(error) = service.resolve_input_delivery_target(None, &handle) else {
        anyhow::bail!("no target must never resolve to the foreground");
    };
    assert_eq!(
        error_code(&error),
        Some(error_codes::ACTION_INPUT_TARGET_REQUIRED),
        "{error:?}"
    );
    assert!(
        error
            .message
            .contains(synapse_action::delivery_target::INPUT_TARGET_REMEDIATION),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn must_fail_dead_explicit_hwnd_is_target_invalid() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (handle, _recording, _cancel) =
        service.m2_action_context_for_session_id(Some("i294-dead-explicit".to_owned()))?;
    assert!(handle.bind_session_delivery_target(focused_window())?);
    let Err(error) = service.resolve_input_delivery_target(Some(DEAD_HWND), &handle) else {
        anyhow::bail!("a dead explicit HWND must be refused");
    };
    // #315: a closed target window is TARGET_WINDOW_NOT_FOUND.
    assert_eq!(
        error_code(&error),
        Some(error_codes::TARGET_WINDOW_NOT_FOUND),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn must_pass_explicit_focus_is_the_target_without_session_target() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (handle, _recording, _cancel) =
        service.m2_action_context_for_session_id(Some("i294-focus-only".to_owned()))?;
    assert!(handle.bind_session_delivery_target(focused_window())?);
    assert_eq!(
        service.implicit_input_delivery_hwnd(&handle)?,
        Some((focused_window().root_hwnd, "act_focus_window"))
    );
    Ok(())
}
