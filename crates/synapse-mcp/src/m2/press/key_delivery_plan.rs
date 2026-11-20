//! #371: which PostMessage key sequence a background key press posts to the
//! resolved keyboard target. Pure decision; the OS calls stay in `postmessage`.

#[cfg(test)]
#[path = "key_delivery_plan_table_371.rs"]
mod key_delivery_plan_table_371;

#[cfg(test)]
#[path = "keyboard_target_table.rs"]
#[allow(clippy::type_complexity)]
mod keyboard_target_table;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PostKeyPlan {
    /// Post only `WM_CHAR`.
    CharOnly,
    /// Post `WM_KEYDOWN`, optionally `WM_CHAR`, hold, then `WM_KEYUP`.
    KeyDownUp { emit_char: bool },
}

/// A single printable key goes to a text control as `WM_CHAR` only: its own
/// message loop would turn a posted `WM_KEYDOWN` into a second character. Every
/// other window (winit, games, canvases) binds keys from `WM_KEYDOWN`/`WM_KEYUP`
/// and gets those without `WM_CHAR`, so its own loop makes the one character. A
/// hold also needs key-down/key-up, on any class.
pub(super) fn postmessage_key_delivery_plan(
    class_name: &str,
    plain_printable: bool,
    hold_ms: u32,
) -> PostKeyPlan {
    if !plain_printable {
        return PostKeyPlan::KeyDownUp { emit_char: true };
    }
    if hold_ms == 0 && is_text_control_class(class_name) {
        PostKeyPlan::CharOnly
    } else {
        PostKeyPlan::KeyDownUp { emit_char: false }
    }
}

pub(crate) fn is_text_control_class(class_name: &str) -> bool {
    class_keyboard_score(class_name) > 1
}

/// How likely a window class is to be the keyboard target; anything above 1 is
/// a known text control.
pub(super) fn class_keyboard_score(class_name: &str) -> u8 {
    let lowered = class_name.to_ascii_lowercase();
    if lowered.contains("edit") || lowered.contains("richedit") {
        return 100;
    }
    if lowered.contains("text") || lowered.contains("scintilla") {
        return 80;
    }
    if lowered.contains("document") || lowered.contains("chrome_renderwidgethosthwnd") {
        return 40;
    }
    1
}

/// A rich-edit control (`RichEdit20W`, `RICHEDIT50W` ...): the only class
/// with a standard redo message (`EM_REDO`).
pub(super) fn is_rich_edit_class(class_name: &str) -> bool {
    class_name.to_ascii_lowercase().contains("richedit")
}

/// One visible, enabled window under the claimed root that could receive keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct KeyboardCandidate {
    hwnd: i64,
    score: u8,
    class_name: String,
}

impl KeyboardCandidate {
    /// A window and its class; the score comes from the class.
    pub(super) fn new(hwnd: i64, class_name: String) -> Self {
        Self {
            hwnd,
            score: class_keyboard_score(&class_name),
            class_name,
        }
    }

    pub(super) const fn hwnd(&self) -> i64 {
        self.hwnd
    }

    pub(super) fn class_name(&self) -> &str {
        &self.class_name
    }
}

/// #2328: which control under a claimed root receives a background key press.
/// `candidates` are in enumeration order with the root last; `thread_focus`
/// is the control that has the keyboard focus in the claimed window's GUI
/// thread, when there is one. `for_root` (in `postmessage`, the OS part)
/// reads them from the live window; `new` takes plain values.
#[derive(Clone, Debug)]
pub(super) struct KeyboardTargetPicker {
    root_hwnd: i64,
    candidates: Vec<KeyboardCandidate>,
    thread_focus: Option<i64>,
}

impl KeyboardTargetPicker {
    pub(super) const fn new(
        root_hwnd: i64,
        candidates: Vec<KeyboardCandidate>,
        thread_focus: Option<i64>,
    ) -> Self {
        Self {
            root_hwnd,
            candidates,
            thread_focus,
        }
    }

    pub(super) const fn root_hwnd(&self) -> i64 {
        self.root_hwnd
    }

    /// The focused child when it is listed (never the root itself), else the
    /// last candidate with the highest score.
    pub(super) fn pick(&self) -> Option<&KeyboardCandidate> {
        if let Some(focus) = self.thread_focus
            && focus != self.root_hwnd
            && let Some(focused) = self
                .candidates
                .iter()
                .find(|candidate| candidate.hwnd == focus)
        {
            return Some(focused);
        }
        self.candidates
            .iter()
            .max_by_key(|candidate| candidate.score)
    }
}
