//! #294 policy and batch-boundary regressions. Every batch uses a fake sender.
use std::cell::Cell;

use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP};

use super::delivery_guard::{
    DeliveryTargetId, InputKind, check_delivery, enter_delivery_scope, install_foreground_probe,
};
use super::input::{install_batch_sender, keyboard_input, mouse_input, send_input_batch};
use crate::ActionError;

const TARGET: DeliveryTargetId = DeliveryTargetId {
    root_hwnd: 11,
    pid: 101,
};
const OTHER: DeliveryTargetId = DeliveryTargetId {
    root_hwnd: 22,
    pid: 202,
};

thread_local! {
    static SENT_BATCHES: Cell<u32> = const { Cell::new(0) };
    static FOREGROUND: Cell<Option<DeliveryTargetId>> = const { Cell::new(None) };
}

fn probe() -> Option<DeliveryTargetId> {
    FOREGROUND.with(Cell::get)
}

fn sender(inputs: &[INPUT], _cb_size: i32) -> u32 {
    SENT_BATCHES.with(|count| count.set(count.get() + 1));
    u32::try_from(inputs.len()).map_or(0, |count| count)
}

fn reset(foreground: Option<DeliveryTargetId>) {
    SENT_BATCHES.with(|count| count.set(0));
    FOREGROUND.with(|slot| slot.set(foreground));
}

fn sent_batches() -> u32 {
    SENT_BATCHES.with(Cell::get)
}

#[test]
fn must_pass_keyboard_and_pointer_to_matching_target() -> Result<(), ActionError> {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        check_delivery(Some(TARGET), Some(TARGET), kind)?;
    }
    Ok(())
}

#[test]
fn must_fail_keyboard_and_pointer_without_target() {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        for foreground in [None, Some(TARGET)] {
            assert!(
                matches!(
                    check_delivery(None, foreground, kind),
                    Err(ActionError::InputTargetRequired { .. })
                ),
                "{kind:?} with foreground {foreground:?} must require a target"
            );
        }
    }
}

#[test]
fn must_fail_different_foreground_root() {
    let other_root = DeliveryTargetId {
        root_hwnd: 22,
        ..TARGET
    };
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        assert!(matches!(
            check_delivery(Some(TARGET), Some(other_root), kind),
            Err(ActionError::ForegroundLost { .. })
        ));
    }
}

#[test]
fn must_fail_same_root_with_different_pid() {
    let reused_hwnd = DeliveryTargetId { pid: 202, ..TARGET };
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        assert!(matches!(
            check_delivery(Some(TARGET), Some(reused_hwnd), kind),
            Err(ActionError::ForegroundLost { .. })
        ));
    }
}

#[test]
fn must_fail_missing_foreground() {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        assert!(matches!(
            check_delivery(Some(TARGET), None, kind),
            Err(ActionError::ForegroundLost { .. })
        ));
    }
}

#[test]
fn must_pass_release_only_without_target_or_foreground() -> Result<(), ActionError> {
    check_delivery(None, None, InputKind::ReleaseOnly)?;
    check_delivery(None, Some(OTHER), InputKind::ReleaseOnly)?;
    Ok(())
}

#[test]
fn must_pass_release_only_with_mismatching_foreground() -> Result<(), ActionError> {
    check_delivery(Some(TARGET), Some(OTHER), InputKind::ReleaseOnly)?;
    check_delivery(Some(TARGET), None, InputKind::ReleaseOnly)?;
    Ok(())
}

#[test]
fn must_fail_batch_to_non_foreground_target_without_sending() {
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Keyboard);
    let inputs = [keyboard_input(30, KEYBD_EVENT_FLAGS(0))];
    let result = send_input_batch(&inputs, "#294 wrong foreground");
    assert_eq!(sent_batches(), 0, "refusal must precede the OS sender");
    assert!(matches!(result, Err(ActionError::ForegroundLost { .. })));
}

#[test]
fn must_fail_batch_when_hwnd_is_reused_by_another_pid() {
    reset(Some(DeliveryTargetId { pid: 202, ..TARGET }));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Keyboard);
    let result = send_input_batch(
        &[keyboard_input(30, KEYBD_EVENT_FLAGS(0))],
        "#294 reused HWND",
    );
    assert_eq!(sent_batches(), 0);
    assert!(matches!(result, Err(ActionError::ForegroundLost { .. })));
}

#[test]
fn must_fail_batch_without_target_without_sending() {
    reset(Some(TARGET));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(None, InputKind::Keyboard);
    let result = send_input_batch(
        &[keyboard_input(30, KEYBD_EVENT_FLAGS(0))],
        "#294 no target",
    );
    assert_eq!(sent_batches(), 0);
    assert!(matches!(
        result,
        Err(ActionError::InputTargetRequired { .. })
    ));
}

#[test]
fn must_pass_batch_to_foreground_target_unchanged() -> Result<(), ActionError> {
    reset(Some(TARGET));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Keyboard);
    send_input_batch(
        &[keyboard_input(30, KEYBD_EVENT_FLAGS(0))],
        "#294 matching target",
    )?;
    assert_eq!(sent_batches(), 1);
    Ok(())
}

#[test]
fn must_fail_next_character_batch_after_foreground_switch() -> Result<(), ActionError> {
    reset(Some(TARGET));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Keyboard);
    let inputs = [
        keyboard_input(30, KEYBD_EVENT_FLAGS(0)),
        keyboard_input(30, KEYEVENTF_KEYUP),
    ];
    send_input_batch(&inputs, "#294 first character")?;
    assert_eq!(sent_batches(), 1);
    FOREGROUND.with(|slot| slot.set(Some(OTHER)));
    let result = send_input_batch(&inputs, "#294 next character");
    assert_eq!(
        sent_batches(),
        1,
        "second character must never reach the sender"
    );
    assert!(matches!(result, Err(ActionError::ForegroundLost { .. })));
    Ok(())
}

#[test]
fn must_fail_pointer_batch_with_mismatching_foreground() {
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Pointer);
    let result = send_input_batch(
        &[mouse_input(
            0,
            0,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_MOVE,
        )],
        "#294 pointer batch",
    );
    assert_eq!(sent_batches(), 0);
    assert!(matches!(result, Err(ActionError::ForegroundLost { .. })));
}

#[test]
fn must_pass_release_batch_without_target() -> Result<(), ActionError> {
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(None, InputKind::ReleaseOnly);
    send_input_batch(&[keyboard_input(30, KEYEVENTF_KEYUP)], "#294 panic release")?;
    assert_eq!(sent_batches(), 1);
    Ok(())
}

#[test]
fn must_fail_untargeted_input_names_its_remediation() {
    for kind in [InputKind::Keyboard, InputKind::Pointer] {
        let result = check_delivery(None, Some(TARGET), kind);
        let Err(ActionError::InputTargetRequired { detail }) = result else {
            unreachable!("{kind:?} without a target must require one: {result:?}");
        };
        assert!(
            detail.contains(crate::delivery_target::INPUT_TARGET_REMEDIATION),
            "the refusal must tell the caller how to name a target: {detail}"
        );
    }
}

#[test]
fn must_fail_mixed_release_and_press_batch_is_checked() {
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Keyboard);
    let inputs = [
        keyboard_input(30, KEYEVENTF_KEYUP),
        mouse_input(
            0,
            0,
            0,
            windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTDOWN,
        ),
    ];
    let result = send_input_batch(&inputs, "#294 release then press");
    assert_eq!(
        sent_batches(),
        0,
        "a press hidden behind a release must not pass"
    );
    assert!(matches!(result, Err(ActionError::ForegroundLost { .. })));
}

#[test]
fn must_pass_button_release_batch_with_mismatching_foreground() -> Result<(), ActionError> {
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(Some(TARGET), InputKind::Pointer);
    let inputs = [mouse_input(
        0,
        0,
        0,
        windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_LEFTUP,
    )];
    send_input_batch(&inputs, "#294 button release after focus change")?;
    assert_eq!(sent_batches(), 1, "a held button must always be releasable");
    Ok(())
}

/// The operator's cursor-context restore runs with no target: a bare cursor
/// move inside the release scope must still reach the OS.
#[test]
fn must_pass_cursor_restore_move_in_release_scope() -> Result<(), ActionError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
    };
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(None, InputKind::ReleaseOnly);
    let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    send_input_batch(&[mouse_input(10, 10, 0, flags)], "#294 cursor restore")?;
    assert_eq!(sent_batches(), 1);
    Ok(())
}

#[test]
fn must_fail_press_inside_release_scope() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_WHEEL};
    reset(Some(OTHER));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(None, InputKind::ReleaseOnly);
    for inputs in [
        [mouse_input(0, 0, 0, MOUSEEVENTF_LEFTDOWN)],
        [mouse_input(0, 0, 120, MOUSEEVENTF_WHEEL)],
        [keyboard_input(30, KEYBD_EVENT_FLAGS(0))],
    ] {
        let result = send_input_batch(&inputs, "#294 press in release scope");
        assert!(
            matches!(result, Err(ActionError::InputTargetRequired { .. })),
            "a release scope never delivers presses: {result:?}"
        );
    }
    assert_eq!(sent_batches(), 0);
}

#[test]
fn must_fail_untargeted_cursor_move_outside_release_scope() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE};
    reset(Some(TARGET));
    let _probe = install_foreground_probe(probe);
    let _sender = install_batch_sender(sender);
    let _scope = enter_delivery_scope(None, InputKind::Keyboard);
    let result = send_input_batch(
        &[mouse_input(
            0,
            0,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
        )],
        "#294 untargeted move",
    );
    assert_eq!(sent_batches(), 0);
    assert!(matches!(
        result,
        Err(ActionError::InputTargetRequired { .. })
    ));
}
