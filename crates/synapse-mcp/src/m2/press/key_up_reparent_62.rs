use super::*;
use crate::m2::native_race_windows_62::Windows;
use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};

fn f8() -> Key {
    Key {
        code: synapse_core::KeyCode::Named { value: "f8".into() },
        use_scancode: false,
    }
}

/// #2340 part B: the caller picks the receiving control once and passes it in.
fn picked(windows: &Windows) -> i64 {
    hwnd_keyboard_target_state(windows.root_id())
        .expect("keyboard target pick")
        .hwnd
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_unchanged_native_child_receives_one_key_down_and_up() {
    let windows = Windows::new();
    assert!(windows.drain(WM_KEYDOWN).is_empty());
    assert!(windows.drain(WM_KEYUP).is_empty());
    let state = post_key_sequence(
        windows.root_id(),
        picked(&windows),
        &[f8()],
        0,
        |root, deliver| deliver(root),
    )
    .await
    .expect("unchanged root delivery");
    assert_eq!(state.root_hwnd, windows.root_id());
    assert_eq!(state.hwnd, windows.child_id());
    assert_eq!(
        windows
            .drain(WM_KEYDOWN)
            .iter()
            .map(|v| v.0)
            .collect::<Vec<_>>(),
        [0x77]
    );
    assert_eq!(
        windows
            .drain(WM_KEYUP)
            .iter()
            .map(|v| v.0)
            .collect::<Vec<_>>(),
        [0x77]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_reparent_after_key_down_posts_no_key_up_to_foreign_root() {
    let windows = Windows::new();
    assert!(windows.drain(WM_KEYUP).is_empty());
    let error = post_key_sequence(
        windows.root_id(),
        picked(&windows),
        &[f8()],
        1,
        |root, deliver| {
            deliver(root)?;
            assert_eq!(
                windows.drain(WM_KEYDOWN).len(),
                1,
                "real initial posting occurred"
            );
            windows.reparent();
            Ok(())
        },
    )
    .await
    .expect_err("release must refuse changed native root");
    assert_eq!(
        error.data.as_ref().expect("error data")["code"],
        "ACTION_TARGET_INVALID"
    );
    assert_ne!(windows.root_of_child(), windows.root_id());
    assert!(
        windows.drain(WM_KEYUP).is_empty(),
        "no release message under foreign root"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_pending_release_with_already_reparented_child_has_no_post() {
    let windows = Windows::new();
    let pending = PendingKeySequence {
        key_specs: vec![key_spec(&f8()).expect("F8")],
        target_hwnd: windows.child_id(),
        release_keys: true,
    };
    windows.reparent();
    let error = post_key_sequence_finish(windows.root_id(), pending, 0)
        .await
        .expect_err("changed root refused before key-up");
    assert!(matches!(error, ActionError::TargetInvalid { .. }));
    assert!(windows.drain(WM_KEYUP).is_empty());
}

// NEEDS-SEAM: post_key_sequence_start must delegate to
// fn post_key_sequence_start_with(root_hwnd: i64, keys: &[Key],
//   after_select: impl FnOnce(i64)) -> Result<PendingKeySequence, ActionError>
// Invoke after_select with the selected child, before the first root guard/post.
#[test]
fn must_fail_child_reparented_after_selection_receives_no_initial_key_message() {
    let windows = Windows::new();
    assert_eq!(windows.root_of_child(), windows.root_id());
    let mut called = false;
    let error =
        post_key_sequence_start_with(windows.root_id(), picked(&windows), &[f8()], 0, |child| {
            called = true;
            assert_eq!(child, windows.child_id());
            windows.reparent();
        })
        .err()
        .expect("reparented selection must be refused before initial delivery");
    assert!(
        called,
        "race must occur after actual native child selection"
    );
    assert!(matches!(error, ActionError::TargetInvalid { .. }));
    assert!(windows.drain(WM_KEYDOWN).is_empty());
    assert!(windows.drain(WM_KEYUP).is_empty());
}
