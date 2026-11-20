use synapse_core::{Action, Backend, Key, KeyCode};
use tokio_util::sync::CancellationToken;

use super::{ForegroundRestoreCurrentDecision, M2State, foreground_restore_current_decision};

#[tokio::test]
async fn must_pass_m2_state_spawns_running_emitter_inside_runtime() {
    let before = Some("true");
    let state = M2State::from_recording_backend_env(before);
    let snapshot = match state.snapshot_handle.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => panic!("snapshot failed: {err}"),
    };
    println!(
        "readback=m2_state scenario=runtime_actor before_env={before:?} after_recording_enabled={} emitter_retained={} emitter_running={} held_keys={:?} held_key_timer_count={} held_buttons={:?} pad_state_len={}",
        state.recording_enabled(),
        state.emitter_retained(),
        state.emitter_running(),
        snapshot.held_keys,
        snapshot.held_key_timer_count,
        snapshot.held_buttons,
        snapshot.pad_state.len()
    );
    assert!(state.recording_enabled());
    assert!(!state.emitter_retained());
    assert!(state.emitter_running());
    assert!(snapshot.held_keys.is_empty());
    assert_eq!(snapshot.held_key_timer_count, 0);
    assert!(snapshot.held_buttons.is_empty());
    assert!(snapshot.pad_state.is_empty());
}

#[tokio::test]
async fn must_pass_m2_state_uses_injected_cancel_token_to_release_all_on_shutdown() {
    let before = Some("false");
    let cancel = CancellationToken::new();
    let substitute: std::sync::Arc<dyn synapse_action::ActionBackend> =
        std::sync::Arc::new(synapse_action::RecordingBackend::new());
    let mut state = M2State::from_recording_backend_env_with_actor_backend(
        before,
        cancel.clone(),
        "shutdown",
        None,
        Some(substitute),
    );
    let key = key_named("m2-cancel-token");
    println!(
        "readback=m2_state scenario=injected_cancel before_cancelled:{} emitter_running:{}",
        cancel.is_cancelled(),
        state.emitter_running()
    );

    state
        .emitter_handle
        .execute(Action::KeyDown {
            key: key.clone(),
            backend: Backend::Software,
        })
        .await
        .unwrap_or_else(|error| panic!("KeyDown should reach emitter before cancel: {error}"));
    let before_cancel = state
        .snapshot_handle
        .snapshot()
        .await
        .unwrap_or_else(|error| panic!("snapshot before cancel should succeed: {error}"));
    assert_eq!(before_cancel.held_keys, vec![key.clone()]);
    let done = state
        .emitter_done_receiver()
        .unwrap_or_else(|| panic!("runtime M2 state should expose emitter done receiver"));

    cancel.cancel();
    let join = state
        .emitter_task
        .take()
        .unwrap_or_else(|| panic!("runtime M2 state should retain emitter join handle"));
    let after_cancel = join
        .await
        .unwrap_or_else(|error| panic!("emitter join should complete after cancel: {error}"));
    let done_snapshot = done
        .borrow()
        .clone()
        .unwrap_or_else(|| panic!("emitter done receiver should contain final snapshot"));

    println!(
        "readback=m2_state scenario=injected_cancel after_cancelled:{} before_held_keys:{:?} after_held_keys:{:?} done_held_keys:{:?} after_timer_count:{} after_buttons:{:?} after_pad_state_len:{}",
        cancel.is_cancelled(),
        before_cancel.held_keys,
        after_cancel.held_keys,
        done_snapshot.held_keys,
        after_cancel.held_key_timer_count,
        after_cancel.held_buttons,
        after_cancel.pad_state.len()
    );
    assert!(cancel.is_cancelled());
    assert!(after_cancel.held_keys.is_empty());
    assert_eq!(done_snapshot, after_cancel);
    assert_eq!(after_cancel.held_key_timer_count, 0);
    assert!(after_cancel.held_buttons.is_empty());
    assert!(after_cancel.pad_state.is_empty());
}

#[test]
fn must_fail_foreground_restore_current_decision_skips_when_current_foreground_moved() {
    let captured_hwnd = 0x1001;
    let expected_pid = 42;
    // The same-hwnd-and-pid pass case lives in
    // foreground_own_focus_217::must_pass_prior_window_already_foreground_needs_no_restore.
    let cases = [
        (
            "different_hwnd",
            0x2002,
            expected_pid,
            ForegroundRestoreCurrentDecision::SkipHumanMoved,
        ),
        (
            "same_hwnd_different_pid",
            captured_hwnd,
            99,
            ForegroundRestoreCurrentDecision::SkipHumanMoved,
        ),
    ];

    for (name, current_hwnd, current_pid, expected) in cases {
        let actual = foreground_restore_current_decision(
            captured_hwnd,
            expected_pid,
            current_hwnd,
            current_pid,
        );
        println!(
            "readback=foreground_restore_decision scenario={name} captured_hwnd=0x{captured_hwnd:x} expected_pid={expected_pid} current_hwnd=0x{current_hwnd:x} current_pid={current_pid} expected={expected:?} actual={actual:?}"
        );
        assert_eq!(actual, expected);
    }
}

fn key_named(name: &str) -> Key {
    Key {
        code: KeyCode::Named {
            value: name.to_owned(),
        },
        use_scancode: false,
    }
}
