//! Regression tests for #53: one foreground escalation at a time; the
//! escalating session cannot run other lease/profile/act operations
//! mid-escalation, nor can its escalation start while one is part-way
//! through; other sessions are never fenced; teardown waits for the unwind.

use std::{sync::LazyLock, time::Duration};

use super::*;

/// The escalation slot and fences are process-global: these tests must not
/// interleave their phases.
static SLOT_TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

fn is_busy<T>(result: &Result<T, ErrorData>) -> bool {
    result
        .as_ref()
        .err()
        .and_then(|error| error.data.as_ref())
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|code| code == error_codes::ACTION_FOREGROUND_LEASE_BUSY)
}

/// Per-session fence readback. The fence map is process-global and other
/// tests admit operations for their own sessions concurrently, so a global
/// count is not a sound assertion; only this file's session ids are checked.
fn has_fence(session_id: &str) -> bool {
    fences().contains_key(session_id)
}

#[tokio::test]
async fn must_fail_escalation_and_same_session_operations_are_fenced_while_busy() {
    let _guard = SLOT_TEST_LOCK.lock().await;

    // A same-session operation in progress blocks that session's escalation.
    let op_a = admit_session_operation("fence-a", "control_lease_acquire");
    assert!(op_a.is_ok());
    assert!(is_busy(&ActForegroundSlot::claim("fence-a")));
    drop(op_a);

    // An escalation fences its own session and refuses a second escalation.
    let slot_a = ActForegroundSlot::claim("fence-a");
    assert!(slot_a.is_ok());
    assert!(is_busy(&ActForegroundSlot::claim("fence-b")));
    assert!(is_busy(&admit_session_operation(
        "fence-a",
        "act operation=invoke"
    )));
    drop(slot_a);

    // Released fences are pruned rather than accumulating per session.
    assert!(admit_session_operation("fence-a", "control_lease_release").is_ok());
    assert!(!has_fence("fence-a"), "fence-a fence must be pruned");
    assert!(!has_fence("fence-b"), "fence-b fence must be pruned");
}

#[tokio::test]
async fn must_pass_other_sessions_are_never_fenced_and_teardown_waits_for_the_unwind() {
    let _guard = SLOT_TEST_LOCK.lock().await;

    // Another session's escalation proceeds while this session runs an operation.
    let op_a = admit_session_operation("fence-a", "control_lease_acquire");
    assert!(op_a.is_ok());
    let slot_b = ActForegroundSlot::claim("fence-b");
    assert!(slot_b.is_ok());
    drop(slot_b);
    drop(op_a);

    // During one session's escalation, another session's operations still run.
    let slot_a = ActForegroundSlot::claim("fence-a");
    assert!(slot_a.is_ok());
    assert!(admit_session_operation("fence-b", "tool_profile_set").is_ok());

    // Teardown of the escalating session waits for its unwind: polled once
    // (zero timeout polls the inner future first), it must still be pending.
    let mut teardown = std::pin::pin!(await_session_escalation("fence-a"));
    let first_poll = tokio::time::timeout(Duration::ZERO, &mut teardown).await;
    assert!(
        first_poll.is_err(),
        "teardown of an escalating session must wait for its unwind"
    );
    // Another session's teardown does not wait.
    let other =
        tokio::time::timeout(Duration::from_secs(5), await_session_escalation("fence-b")).await;
    assert!(other.is_ok(), "fence-b teardown must not wait on fence-a");
    drop(slot_a);
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut teardown).await;
    assert!(
        joined.is_ok(),
        "teardown must finish once the escalation unwinds"
    );

    // Released fences are pruned rather than accumulating per session.
    assert!(admit_session_operation("fence-a", "control_lease_release").is_ok());
    assert!(!has_fence("fence-a"), "fence-a fence must be pruned");
    assert!(!has_fence("fence-b"), "fence-b fence must be pruned");
}
