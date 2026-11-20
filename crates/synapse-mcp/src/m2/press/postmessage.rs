use rmcp::ErrorData;
use serde::Serialize;
use synapse_action::ActionError;
use synapse_core::Key;

#[cfg(windows)]
use super::key_delivery_plan::{
    KeyboardCandidate, KeyboardTargetPicker, PostKeyPlan, class_keyboard_score, is_rich_edit_class,
    postmessage_key_delivery_plan,
};
use super::{action_error_to_mcp, key_label};

#[cfg(all(test, windows))]
#[path = "key_up_reparent_62.rs"]
mod key_up_reparent_62;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HwndKeyboardTargetState {
    pub root_hwnd: i64,
    pub hwnd: i64,
    pub class_name: String,
    pub text_len: Option<usize>,
    pub text_sha256: Option<String>,
    pub selection_start: Option<u32>,
    pub selection_end: Option<u32>,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct WindowCandidate {
    root_hwnd: windows::Win32::Foundation::HWND,
    hwnd: windows::Win32::Foundation::HWND,
    class_name: String,
}

#[cfg(windows)]
#[derive(Debug)]
struct ChildEnumContext {
    root_hwnd: windows::Win32::Foundation::HWND,
    candidates: Vec<WindowCandidate>,
}

pub(crate) fn hwnd_keyboard_target_state(
    root_hwnd: i64,
) -> Result<HwndKeyboardTargetState, ErrorData> {
    hwnd_keyboard_target_state_impl(root_hwnd).map_err(|error| action_error_to_mcp(&error))
}

/// The state of the control the keys were sent to (#2340 part B): the check
/// reads this control, never a fresh pick.
pub(crate) fn delivered_keyboard_target_state(
    root_hwnd: i64,
    target_hwnd: i64,
) -> Result<HwndKeyboardTargetState, ErrorData> {
    delivered_keyboard_target_state_impl(root_hwnd, target_hwnd)
        .map_err(|error| action_error_to_mcp(&error))
}

pub(crate) async fn post_key_sequence(
    root_hwnd: i64,
    target_hwnd: i64,
    keys: &[Key],
    hold_ms: u32,
    dispatch: impl FnOnce(i64, &mut dyn FnMut(i64) -> Result<(), ErrorData>) -> Result<(), ErrorData>,
) -> Result<HwndKeyboardTargetState, ErrorData> {
    let mut pending = None;
    dispatch(root_hwnd, &mut |checked_root| {
        if checked_root != root_hwnd {
            return Err(crate::m1::mcp_error(
                synapse_core::error_codes::ACTION_TARGET_INVALID,
                "act_press checked root differs from the captured target; no input was sent",
            ));
        }
        pending = Some(
            post_key_sequence_start(checked_root, target_hwnd, keys, hold_ms)
                .map_err(|error| action_error_to_mcp(&error))?,
        );
        Ok(())
    })?;
    let pending = pending.ok_or_else(|| {
        crate::m1::mcp_error(
            synapse_core::error_codes::ACTION_TARGET_INVALID,
            "act_press dispatch did not deliver to the checked root",
        )
    })?;
    post_key_sequence_finish(root_hwnd, pending, hold_ms)
        .await
        .map_err(|error| action_error_to_mcp(&error))
}

#[cfg(windows)]
struct PendingKeySequence {
    key_specs: Vec<KeySpec>,
    target_hwnd: i64,
    release_keys: bool,
}

#[cfg(not(windows))]
struct PendingKeySequence;

#[cfg(windows)]
fn post_key_sequence_start(
    root_hwnd: i64,
    target_hwnd: i64,
    keys: &[Key],
    hold_ms: u32,
) -> Result<PendingKeySequence, ActionError> {
    post_key_sequence_start_with(root_hwnd, target_hwnd, keys, hold_ms, |_| {})
}

#[cfg(windows)]
fn post_key_sequence_start_with(
    root_hwnd: i64,
    target_hwnd: i64,
    keys: &[Key],
    hold_ms: u32,
    after_select: impl FnOnce(i64),
) -> Result<PendingKeySequence, ActionError> {
    let key_specs = keys.iter().map(key_spec).collect::<Result<Vec<_>, _>>()?;
    // #1332: a Shift+navigation chord extends the selection ONLY if the control
    // reads the OS Shift key-state, which PostMessage WM_KEYDOWN cannot set — the
    // caret would just move WITHOUT selecting (a silent false success). Fail loud
    // with the correct alternatives instead.
    if let Some(nav) = shift_navigation_chord(&key_specs) {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_press PostMessage cannot extend a selection with Shift+{nav}: WM_KEYDOWN cannot set the OS Shift key-state the control reads, so the caret would move without selecting. Use target_act verb=set_selection for an exact background selection, or acquire the foreground input lease (backend=hardware) for real Shift+navigation."
            ),
        });
    }
    // #2340 part B: the caller picked the control once; the check reads it too.
    after_select(target_hwnd);
    ensure_keyboard_target_root(root_hwnd, target_hwnd)?;
    let target_window = hwnd_from_i64(target_hwnd)?;
    let target = WindowCandidate {
        root_hwnd: hwnd_from_i64(root_hwnd)?,
        hwnd: target_window,
        class_name: window_class_name(target_window),
    };
    let mut release_keys = false;
    if is_ctrl_a_shortcut(&key_specs) {
        select_all_edit_target(&target)?;
    } else if let Some(chord) = edit_control_chord(&key_specs) {
        // #1331: PostMessage WM_KEYDOWN cannot set OS key-state, so editing
        // chords (Ctrl+C/X/V/Z, Ctrl+Y/Ctrl+Shift+Z) never reach the control
        // as accelerators. Deliver the equivalent edit-control message directly
        // and verify the real effect (clipboard sequence number for copy/cut,
        // window-text change for paste/undo/redo) so we never falsely report
        // success on a chord the control ignored.
        send_edit_control_chord(&target, &chord)?;
    } else {
        // #371: a lone WM_CHAR suits text controls only; key-binding windows
        // (winit, games, canvases) read WM_KEYDOWN/WM_KEYUP.
        match postmessage_key_delivery_plan(
            &target.class_name,
            is_plain_printable_text(&key_specs),
            hold_ms,
        ) {
            PostKeyPlan::CharOnly => post_char_messages(target.hwnd, &key_specs)?,
            PostKeyPlan::KeyDownUp { emit_char } => {
                for spec in &key_specs {
                    post_key_message(
                        target.hwnd,
                        windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN,
                        spec,
                    )?;
                }
                release_keys = true;
                if emit_char {
                    post_char_messages(target.hwnd, &key_specs)?;
                }
            }
        }
    }
    Ok(PendingKeySequence {
        key_specs,
        target_hwnd,
        release_keys,
    })
}

#[cfg(windows)]
async fn post_key_sequence_finish(
    root_hwnd: i64,
    pending: PendingKeySequence,
    hold_ms: u32,
) -> Result<HwndKeyboardTargetState, ActionError> {
    let PendingKeySequence {
        key_specs,
        target_hwnd,
        release_keys,
    } = pending;
    if hold_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(hold_ms))).await;
    }
    let target = hwnd_from_i64(target_hwnd)?;
    if release_keys {
        ensure_keyboard_target_root(root_hwnd, target_hwnd)?;
        for spec in key_specs.iter().rev() {
            post_key_message(
                target,
                windows::Win32::UI::WindowsAndMessaging::WM_KEYUP,
                spec,
            )?;
        }
    }
    delivered_keyboard_target_state_impl(root_hwnd, target_hwnd)
}

#[cfg(windows)]
fn ensure_keyboard_target_root(root_hwnd: i64, target_hwnd: i64) -> Result<(), ActionError> {
    let actual_root = synapse_action::delivery_target::live_root_hwnd(
        target_hwnd,
        "act_press keyboard destination check",
    )?;
    if actual_root != root_hwnd {
        return Err(ActionError::TargetInvalid {
            detail: "act_press keyboard destination left the checked root".to_owned(),
        });
    }
    Ok(())
}

#[cfg(not(windows))]
fn post_key_sequence_start(
    _root_hwnd: i64,
    _target_hwnd: i64,
    _keys: &[Key],
    _hold_ms: u32,
) -> Result<PendingKeySequence, ActionError> {
    Err(ActionError::BackendUnavailable {
        detail: "act_press PostMessage keyboard tier is only available on Windows".to_owned(),
    })
}

#[cfg(not(windows))]
async fn post_key_sequence_finish(
    _root_hwnd: i64,
    _pending: PendingKeySequence,
    _hold_ms: u32,
) -> Result<HwndKeyboardTargetState, ActionError> {
    Err(ActionError::BackendUnavailable {
        detail: "act_press PostMessage keyboard tier is only available on Windows".to_owned(),
    })
}

#[cfg(windows)]
fn hwnd_keyboard_target_state_impl(root_hwnd: i64) -> Result<HwndKeyboardTargetState, ActionError> {
    target_state(KeyboardTargetPicker::for_root(root_hwnd)?.target()?)
}

#[cfg(not(windows))]
fn hwnd_keyboard_target_state_impl(
    _root_hwnd: i64,
) -> Result<HwndKeyboardTargetState, ActionError> {
    Err(ActionError::BackendUnavailable {
        detail: "act_press PostMessage keyboard tier is only available on Windows".to_owned(),
    })
}

#[cfg(windows)]
fn delivered_keyboard_target_state_impl(
    root_hwnd: i64,
    target_hwnd: i64,
) -> Result<HwndKeyboardTargetState, ActionError> {
    ensure_keyboard_target_root(root_hwnd, target_hwnd)?;
    let target = hwnd_from_i64(target_hwnd)?;
    target_state(WindowCandidate {
        root_hwnd: hwnd_from_i64(root_hwnd)?,
        hwnd: target,
        class_name: window_class_name(target),
    })
}

#[cfg(not(windows))]
fn delivered_keyboard_target_state_impl(
    _root_hwnd: i64,
    _target_hwnd: i64,
) -> Result<HwndKeyboardTargetState, ActionError> {
    Err(ActionError::BackendUnavailable {
        detail: "act_press PostMessage keyboard tier is only available on Windows".to_owned(),
    })
}

/// #2328: the OS part of the keyboard target picker. `for_root` enumerates the
/// visible, enabled windows under the claimed root (the root last) and reads
/// the thread focus; `target` turns the pick into the delivery target.
#[cfg(windows)]
impl KeyboardTargetPicker {
    fn for_root(root_hwnd: i64) -> Result<Self, ActionError> {
        use std::ffi::c_void;
        use windows::Win32::{
            Foundation::LPARAM,
            UI::{
                Input::KeyboardAndMouse::IsWindowEnabled,
                WindowsAndMessaging::{EnumChildWindows, IsWindowVisible},
            },
        };

        let root = hwnd_from_i64(root_hwnd)?;
        synapse_action::delivery_target::ensure_live_window(
            root_hwnd,
            "act_press PostMessage root window",
        )?;
        let mut context = ChildEnumContext {
            root_hwnd: root,
            candidates: Vec::new(),
        };
        let context_ptr = (&raw mut context).cast::<c_void>();
        // SAFETY: `enum_keyboard_child` matches the WNDENUMPROC signature, and `lparam` is the
        // address of the live local `context`, which outlives this synchronous call and is not
        // otherwise accessed until it returns. A stale root HWND just makes the enumeration fail.
        let _ = unsafe {
            EnumChildWindows(
                Some(root),
                Some(enum_keyboard_child),
                LPARAM(context_ptr as isize),
            )
        };
        let root_class_name = window_class_name(root);
        let root_candidate = WindowCandidate {
            root_hwnd: root,
            hwnd: root,
            class_name: root_class_name,
        };
        let candidates: Vec<KeyboardCandidate> = context
            .candidates
            .into_iter()
            .chain(std::iter::once(root_candidate))
            // SAFETY: HWND is a plain handle value that is never dereferenced; IsWindowVisible only
            // reads it and returns FALSE for a stale or invalid handle.
            .filter(|candidate| unsafe { IsWindowVisible(candidate.hwnd) }.as_bool())
            // SAFETY: HWND is a plain handle value that is never dereferenced; IsWindowEnabled only
            // reads it and returns FALSE for a stale or invalid handle.
            .filter(|candidate| unsafe { IsWindowEnabled(candidate.hwnd) }.as_bool())
            .map(|candidate| {
                KeyboardCandidate::new(hwnd_to_i64(candidate.hwnd), candidate.class_name)
            })
            .collect();
        let thread_focus = synapse_a11y::focused_hwnd_in_window(root_hwnd)
            .ok()
            .flatten();
        Ok(Self::new(root_hwnd, candidates, thread_focus))
    }

    fn target(&self) -> Result<WindowCandidate, ActionError> {
        let root_hwnd = self.root_hwnd();
        let chosen = self.pick().ok_or_else(|| ActionError::ElementNotResolved {
            detail: format!(
                "act_press PostMessage could not resolve an enabled keyboard target under hwnd 0x{root_hwnd:x}"
            ),
        })?;
        Ok(WindowCandidate {
            root_hwnd: hwnd_from_i64(root_hwnd)?,
            hwnd: hwnd_from_i64(chosen.hwnd())?,
            class_name: chosen.class_name().to_owned(),
        })
    }
}

// SAFETY: only ever passed to EnumChildWindows by `KeyboardTargetPicker::for_root`, which supplies a
// valid `*mut ChildEnumContext` as `lparam`; the signature matches WNDENUMPROC.
#[cfg(windows)]
unsafe extern "system" fn enum_keyboard_child(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::IsWindowVisible,
    };

    // SAFETY: `lparam` is the `&raw mut context` passed by the enclosing EnumChildWindows call
    // in `KeyboardTargetPicker::for_root`; it is valid and exclusively borrowed for that call.
    let context = unsafe { &mut *(lparam.0 as *mut ChildEnumContext) };
    // SAFETY: HWND is a plain handle value supplied by the enumeration; both calls only read it
    // and return FALSE for an invalid handle.
    if unsafe { IsWindowVisible(hwnd) }.as_bool() && unsafe { IsWindowEnabled(hwnd) }.as_bool() {
        let class_name = window_class_name(hwnd);
        let score = class_keyboard_score(&class_name);
        if score > 0 {
            context.candidates.push(WindowCandidate {
                root_hwnd: context.root_hwnd,
                hwnd,
                class_name,
            });
        }
    }
    windows::core::BOOL(1)
}

#[cfg(windows)]
fn is_ctrl_a_shortcut(key_specs: &[KeySpec]) -> bool {
    key_specs.len() == 2
        && key_specs[0].label == "ctrl"
        && key_specs[1].label == "letter"
        && key_specs[1].char_code == Some(u16::from(b'a'))
}

#[cfg(windows)]
fn is_plain_printable_text(key_specs: &[KeySpec]) -> bool {
    key_specs.len() == 1 && key_specs[0].char_code.is_some()
}

#[cfg(windows)]
fn select_all_edit_target(target: &WindowCandidate) -> Result<(), ActionError> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW},
    };

    let class_name = target.class_name.to_ascii_lowercase();
    if !(class_name.contains("edit") || class_name.contains("richedit")) {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_press PostMessage Ctrl+A background delivery requires an edit/rich-edit target; resolved hwnd 0x{:x} class {:?}",
                hwnd_to_i64(target.hwnd),
                target.class_name
            ),
        });
    }

    const EM_SETSEL: u32 = 0x00B1;
    const TIMEOUT_MS: u32 = 250;
    // SAFETY: wparam/lparam are plain integers (select-all range 0..-1), never pointers, and the
    // result out-pointer is None, so nothing is dereferenced in the target process; the
    // SMTO_ABORTIFHUNG 250 ms timeout bounds the call and a stale HWND yields a 0 result.
    let result = unsafe {
        SendMessageTimeoutW(
            target.hwnd,
            EM_SETSEL,
            WPARAM(0),
            LPARAM(-1),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            None,
        )
    };
    if result.0 == 0 {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_press PostMessage Ctrl+A EM_SETSEL timed out or failed for hwnd 0x{:x}",
                hwnd_to_i64(target.hwnd)
            ),
        });
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Copy, Clone, Debug)]
struct EditChord {
    message: u32,
    name: &'static str,
    /// Redo has no standard message on a plain Edit control (EM_REDO is rich-edit
    /// only), so fail loud rather than silently no-op there.
    richedit_only: bool,
}

/// Maps a Ctrl-modified editing chord to the edit-control message that performs
/// it without OS key-state (#1331). Returns `None` for any non-editing chord so
/// the caller falls through to the generic key path.
#[cfg(windows)]
fn edit_control_chord(key_specs: &[KeySpec]) -> Option<EditChord> {
    const WM_CUT: u32 = 0x0300;
    const WM_COPY: u32 = 0x0301;
    const WM_PASTE: u32 = 0x0302;
    const WM_UNDO: u32 = 0x0304;
    const EM_REDO: u32 = 0x0454;

    let has_ctrl = key_specs.iter().any(|key| key.label == "ctrl");
    if !has_ctrl {
        return None;
    }
    if key_specs
        .iter()
        .any(|key| matches!(key.label, "alt" | "super"))
    {
        return None;
    }
    let has_shift = key_specs.iter().any(|key| key.label == "shift");
    let letters: Vec<u16> = key_specs
        .iter()
        .filter(|key| key.label == "letter")
        .filter_map(|key| key.char_code)
        .collect();
    if letters.len() != 1 {
        return None;
    }
    let letter = u8::try_from(letters[0]).ok()?.to_ascii_lowercase();
    match (letter, has_shift) {
        (b'c', false) => Some(EditChord {
            message: WM_COPY,
            name: "WM_COPY",
            richedit_only: false,
        }),
        (b'x', false) => Some(EditChord {
            message: WM_CUT,
            name: "WM_CUT",
            richedit_only: false,
        }),
        (b'v', false) => Some(EditChord {
            message: WM_PASTE,
            name: "WM_PASTE",
            richedit_only: false,
        }),
        (b'z', false) => Some(EditChord {
            message: WM_UNDO,
            name: "WM_UNDO",
            richedit_only: false,
        }),
        (b'z', true) | (b'y', false) => Some(EditChord {
            message: EM_REDO,
            name: "EM_REDO",
            richedit_only: true,
        }),
        _ => None,
    }
}

/// Windows clipboard sequence number, exposed so the act_press verify layer can
/// observe clipboard-mutating chords (#1331). Returns 0 off-Windows.
#[cfg(windows)]
pub(crate) fn clipboard_sequence_number() -> u32 {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    // SAFETY: GetClipboardSequenceNumber takes no arguments and only reads a system counter.
    unsafe { GetClipboardSequenceNumber() }
}

#[cfg(not(windows))]
pub(crate) fn clipboard_sequence_number() -> u32 {
    0
}

/// Detects a Shift + single-navigation-key chord (selection extension) that the
/// PostMessage tier cannot perform without OS key-state (#1332). Shift+letter
/// (uppercase typing) and Ctrl+Shift+Z (redo) are NOT navigation chords and pass
/// through. Returns the navigation key label when the chord must be refused.
#[cfg(windows)]
fn shift_navigation_chord(key_specs: &[KeySpec]) -> Option<&'static str> {
    if !key_specs.iter().any(|key| key.label == "shift") {
        return None;
    }
    let nav: Vec<&'static str> = key_specs
        .iter()
        .filter_map(|key| match key.label {
            label @ ("left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown") => {
                Some(label)
            }
            _ => None,
        })
        .collect();
    (nav.len() == 1).then(|| nav[0])
}

/// Sends an edit-control message to the resolved keyboard target (#1331). The
/// real effect is verified by the act_press delta layer (clipboard sequence for
/// copy/cut, target text change for paste/undo/redo) so a chord the control
/// ignored fails loud as ACTION_NO_OBSERVED_DELTA rather than a false success.
/// Redo is refused here on non-rich-edit controls (no standard message).
#[cfg(windows)]
fn send_edit_control_chord(target: &WindowCandidate, chord: &EditChord) -> Result<(), ActionError> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW},
    };

    if chord.richedit_only && !is_rich_edit_class(&target.class_name) {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_press PostMessage {} (redo) requires a rich-edit target; resolved hwnd 0x{:x} class {:?} has no standard redo message — use the foreground input lease for redo on this control",
                chord.name,
                hwnd_to_i64(target.hwnd),
                target.class_name
            ),
        });
    }

    const TIMEOUT_MS: u32 = 250;
    let mut message_result = 0_usize;
    // SAFETY: wparam/lparam are zero, never pointers; the result out-pointer refers to the live
    // local `message_result`. The SMTO_ABORTIFHUNG 250 ms timeout bounds the call and a stale
    // HWND yields a 0 result.
    let send_result = unsafe {
        SendMessageTimeoutW(
            target.hwnd,
            chord.message,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            Some(&raw mut message_result),
        )
    };
    if send_result.0 == 0 {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_press PostMessage {} (SendMessageTimeout) timed out or failed for hwnd 0x{:x}",
                chord.name,
                hwnd_to_i64(target.hwnd)
            ),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn target_state(target: WindowCandidate) -> Result<HwndKeyboardTargetState, ActionError> {
    let text = window_text(target.hwnd)?;
    let selection = edit_selection(target.hwnd);
    Ok(HwndKeyboardTargetState {
        root_hwnd: hwnd_to_i64(target.root_hwnd),
        hwnd: hwnd_to_i64(target.hwnd),
        class_name: target.class_name,
        text_len: Some(text.chars().count()),
        text_sha256: Some(crate::content_digest::keyed_text_digest_hex(&text)),
        selection_start: selection.map(|(start, _end)| start),
        selection_end: selection.map(|(_start, end)| end),
    })
}

#[cfg(windows)]
#[derive(Copy, Clone, Debug)]
struct KeySpec {
    label: &'static str,
    vk: u16,
    char_code: Option<u16>,
    ctrl_char_code: Option<u16>,
}

#[cfg(windows)]
fn key_spec(key: &Key) -> Result<KeySpec, ActionError> {
    let label = key_label(key);
    let label_ref = label.as_str();
    let spec = match label_ref {
        "ctrl" => KeySpec {
            label: "ctrl",
            vk: 0x11,
            char_code: None,
            ctrl_char_code: None,
        },
        "shift" => KeySpec {
            label: "shift",
            vk: 0x10,
            char_code: None,
            ctrl_char_code: None,
        },
        "alt" => KeySpec {
            label: "alt",
            vk: 0x12,
            char_code: None,
            ctrl_char_code: None,
        },
        "super" => KeySpec {
            label: "super",
            vk: 0x5B,
            char_code: None,
            ctrl_char_code: None,
        },
        "backspace" => named_key("backspace", 0x08),
        "tab" => named_key("tab", 0x09),
        "enter" => named_key("enter", 0x0D),
        "esc" => named_key("esc", 0x1B),
        "space" => KeySpec {
            label: "space",
            vk: 0x20,
            char_code: Some(u16::from(b' ')),
            ctrl_char_code: None,
        },
        "pageup" => named_key("pageup", 0x21),
        "pagedown" => named_key("pagedown", 0x22),
        "end" => named_key("end", 0x23),
        "home" => named_key("home", 0x24),
        "left" => named_key("left", 0x25),
        "up" => named_key("up", 0x26),
        "right" => named_key("right", 0x27),
        "down" => named_key("down", 0x28),
        "insert" => named_key("insert", 0x2D),
        "delete" => named_key("delete", 0x2E),
        "`" => KeySpec {
            label: "`",
            vk: 0xC0,
            char_code: Some(u16::from(b'`')),
            ctrl_char_code: None,
        },
        label if label.len() == 1 && label.as_bytes()[0].is_ascii_alphabetic() => {
            let byte = label.as_bytes()[0].to_ascii_uppercase();
            KeySpec {
                label: "letter",
                vk: u16::from(byte),
                char_code: Some(u16::from(label.as_bytes()[0].to_ascii_lowercase())),
                ctrl_char_code: Some(u16::from(
                    label.as_bytes()[0].to_ascii_uppercase() - b'A' + 1,
                )),
            }
        }
        label if label.len() == 1 && label.as_bytes()[0].is_ascii_digit() => KeySpec {
            label: "digit",
            vk: u16::from(label.as_bytes()[0]),
            char_code: Some(u16::from(label.as_bytes()[0])),
            ctrl_char_code: None,
        },
        label if label.starts_with('f') => {
            let number =
                label[1..]
                    .parse::<u16>()
                    .map_err(|error| ActionError::UnsupportedKey {
                        detail: format!(
                            "act_press PostMessage unsupported function key {label:?}: {error}"
                        ),
                    })?;
            if !(1..=24).contains(&number) {
                return Err(ActionError::UnsupportedKey {
                    detail: format!("act_press PostMessage unsupported function key {label:?}"),
                });
            }
            KeySpec {
                label: "function",
                vk: 0x70 + number - 1,
                char_code: None,
                ctrl_char_code: None,
            }
        }
        _ => {
            return Err(ActionError::UnsupportedKey {
                detail: format!("act_press PostMessage unsupported key {label:?}"),
            });
        }
    };
    Ok(spec)
}

#[cfg(windows)]
const fn named_key(label: &'static str, vk: u16) -> KeySpec {
    KeySpec {
        label,
        vk,
        char_code: None,
        ctrl_char_code: None,
    }
}

#[cfg(windows)]
fn post_char_messages(
    hwnd: windows::Win32::Foundation::HWND,
    key_specs: &[KeySpec],
) -> Result<(), ActionError> {
    let ctrl_down = key_specs.iter().any(|key| key.label == "ctrl");
    let alt_or_super_down = key_specs
        .iter()
        .any(|key| matches!(key.label, "alt" | "super"));
    let Some(final_key) = key_specs.last() else {
        return Ok(());
    };
    let char_code = if ctrl_down {
        final_key.ctrl_char_code
    } else if alt_or_super_down {
        None
    } else {
        final_key.char_code
    };
    if let Some(char_code) = char_code {
        post_char_message(hwnd, char_code, final_key)?;
    }
    Ok(())
}

#[cfg(windows)]
fn post_key_message(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    key: &KeySpec,
) -> Result<(), ActionError> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW},
            WindowsAndMessaging::{PostMessageW, WM_KEYUP},
        },
    };

    // SAFETY: MapVirtualKeyW takes two plain integers and only does a keyboard-layout table
    // lookup; it has no pointer arguments.
    let scan = unsafe { MapVirtualKeyW(u32::from(key.vk), MAPVK_VK_TO_VSC) } & 0xff;
    let mut bits = 1_u32 | (scan << 16);
    if message == WM_KEYUP {
        bits |= 1 << 30;
        bits |= 1 << 31;
    }
    let lparam = LPARAM(isize::try_from(bits).unwrap_or(isize::MAX));
    // SAFETY: wparam (virtual-key code) and lparam (packed key flags) are plain integers, never
    // pointers, so nothing can dangle in the target's queue; a stale HWND makes PostMessageW
    // return an error, which is mapped below.
    unsafe { PostMessageW(Some(hwnd), message, WPARAM(usize::from(key.vk)), lparam) }.map_err(
        |error| ActionError::BackendUnavailable {
            detail: format!(
                "PostMessageW act_press keyboard message 0x{message:x} failed for hwnd 0x{:x}: {error}",
                hwnd_to_i64(hwnd)
            ),
        },
    )
}

#[cfg(windows)]
fn post_char_message(
    hwnd: windows::Win32::Foundation::HWND,
    char_code: u16,
    key: &KeySpec,
) -> Result<(), ActionError> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW},
            WindowsAndMessaging::{PostMessageW, WM_CHAR},
        },
    };

    // SAFETY: MapVirtualKeyW takes two plain integers and only does a keyboard-layout table
    // lookup; it has no pointer arguments.
    let scan = unsafe { MapVirtualKeyW(u32::from(key.vk), MAPVK_VK_TO_VSC) } & 0xff;
    let bits = 1_u32 | (scan << 16);
    let lparam = LPARAM(isize::try_from(bits).unwrap_or(isize::MAX));
    // SAFETY: wparam (UTF-16 code unit) and lparam (packed key flags) are plain integers, never
    // pointers, so nothing can dangle in the target's queue; a stale HWND makes PostMessageW
    // return an error, which is mapped below.
    unsafe { PostMessageW(Some(hwnd), WM_CHAR, WPARAM(usize::from(char_code)), lparam) }.map_err(
        |error| ActionError::BackendUnavailable {
            detail: format!(
                "PostMessageW act_press WM_CHAR failed for hwnd 0x{:x}: {error}",
                hwnd_to_i64(hwnd)
            ),
        },
    )
}

#[cfg(windows)]
fn window_text(hwnd: windows::Win32::Foundation::HWND) -> Result<String, ActionError> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_GETTEXT, WM_GETTEXTLENGTH,
        },
    };

    const TIMEOUT_MS: u32 = 250;
    let mut length_result = 0_usize;
    // SAFETY: WM_GETTEXTLENGTH carries zero wparam/lparam (no pointers); the result out-pointer
    // refers to the live local `length_result`. The SMTO_ABORTIFHUNG 250 ms timeout bounds the
    // call and a stale HWND yields a 0 result.
    let length_lresult = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXTLENGTH,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            Some(&raw mut length_result),
        )
    };
    if length_lresult.0 == 0 && length_result == 0 {
        return Ok(String::new());
    }
    let capacity = length_result.saturating_add(1).min(1_048_576);
    let mut buffer = vec![0_u16; capacity];
    let mut copied = 0_usize;
    // SAFETY: lparam points at `buffer`, a live UTF-16 allocation of exactly `buffer.len()`
    // elements, which is the capacity passed in wparam; the call is synchronous so `buffer`
    // outlives it. WM_GETTEXT is a system message whose buffer the OS marshals, including
    // across processes. The out-pointer refers to the live local `copied`.
    let copied_lresult = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            Some(&raw mut copied),
        )
    };
    if copied_lresult.0 == 0 && copied == 0 {
        return Ok(String::new());
    }
    let text_len = copied.min(buffer.len().saturating_sub(1));
    Ok(String::from_utf16_lossy(&buffer[..text_len]))
}

#[cfg(windows)]
fn edit_selection(hwnd: windows::Win32::Foundation::HWND) -> Option<(u32, u32)> {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SendMessageTimeoutW},
    };

    const EM_GETSEL: u32 = 0x00B0;
    const TIMEOUT_MS: u32 = 250;
    let mut start = 0_u32;
    let mut end = 0_u32;
    // SAFETY: wparam/lparam point at the live locals `start` and `end` (EM_GETSEL out-params),
    // which outlive this synchronous call; EM_GETSEL is a system message whose DWORD out-pointers
    // the OS marshals, including across processes. The result out-pointer is None.
    let result = unsafe {
        SendMessageTimeoutW(
            hwnd,
            EM_GETSEL,
            WPARAM((&raw mut start) as usize),
            LPARAM((&raw mut end) as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            None,
        )
    };
    (result.0 != 0).then_some((start, end))
}

#[cfg(windows)]
fn window_class_name(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let mut buffer = vec![0_u16; 256];
    // SAFETY: `buffer` is a live, writable UTF-16 slice and GetClassNameW writes at most its
    // length; a stale HWND returns 0, which yields an empty class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
}

#[cfg(windows)]
fn hwnd_from_i64(hwnd: i64) -> Result<windows::Win32::Foundation::HWND, ActionError> {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;

    if hwnd == 0 {
        return Err(ActionError::TargetInvalid {
            detail: "act_press PostMessage target hwnd is null".to_owned(),
        });
    }
    Ok(HWND(hwnd as isize as *mut c_void))
}

#[cfg(windows)]
fn hwnd_to_i64(hwnd: windows::Win32::Foundation::HWND) -> i64 {
    hwnd.0 as isize as i64
}
