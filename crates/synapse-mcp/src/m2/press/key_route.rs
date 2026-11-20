//! Which lane a key press takes on a window target (#2340 part B).
//! Pure decision; the OS calls stay in `postmessage` and in the service.

use super::key_delivery_plan::{class_keyboard_score, is_rich_edit_class};
use super::keys::MODIFIER_ORDER;

#[cfg(test)]
#[path = "key_route_table_569b.rs"]
mod key_route_table_569b;

/// A control whose class scores at least this much (edit, rich edit, text,
/// Scintilla) does an edit command from its own message.
const NATIVE_EDIT_BOX_SCORE: u8 = 80;

/// How one key press reaches a window target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyLane {
    /// Posted window messages (`WM_CHAR` / `WM_KEYDOWN` / `WM_KEYUP`).
    WindowMessages,
    /// An edit command sent as its own message (`WM_COPY`, `WM_PASTE`, `EM_SETSEL` ...).
    EditMessage,
    /// A real key press under the claimed lease: the session's claimed window is in front.
    ClaimedKeyPress,
    /// Refused before delivery: a chord needs the claimed window in front.
    RefusedNeedsFront,
    /// Refused before delivery: a system chord is never sent on a target-scoped route.
    RefusedSystemChord,
}

/// The keys of one press, as canonical key names (`ctrl`, `shift`, `alt`,
/// `super`, `a`, `f4`, `tab` ...): the held modifiers and the other keys.
pub(crate) struct KeyChord {
    ctrl: bool,
    shift: bool,
    alt: bool,
    win: bool,
    /// The keys that are not modifiers, in the given order.
    keys: Vec<String>,
}

impl KeyChord {
    /// Takes the canonical names `keys::normalized_keys` produced.
    pub(crate) fn of(labels: &[String]) -> Self {
        let held = |modifier: &str| labels.iter().any(|label| label == modifier);
        Self {
            ctrl: held("ctrl"),
            shift: held("shift"),
            alt: held("alt"),
            win: held("super"),
            keys: labels
                .iter()
                .filter(|label| !MODIFIER_ORDER.contains(&label.as_str()))
                .cloned()
                .collect(),
        }
    }

    /// The Win key, or a chord Windows keeps for itself: switch windows
    /// (Alt+Tab, Alt+Esc), Start (Ctrl+Esc), the security screen
    /// (Ctrl+Alt+Delete), Task Manager (Ctrl+Shift+Esc).
    fn is_system_chord(&self) -> bool {
        self.win
            || (self.alt && self.has_key("tab"))
            || ((self.alt || self.ctrl) && self.has_key("esc"))
            || (self.ctrl && self.alt && self.has_key("delete"))
    }

    /// Ctrl or Alt must be held down while the key goes in; window messages
    /// cannot hold a key down.
    const fn needs_held_modifier(&self) -> bool {
        self.ctrl || self.alt
    }

    /// The edit command of a Ctrl chord with exactly one letter and no Alt or
    /// Win key (the chords `postmessage` sends as edit messages).
    fn edit_command(&self) -> Option<EditCommand> {
        if !self.ctrl || self.alt || self.win {
            return None;
        }
        let [key] = self.keys.as_slice() else {
            return None;
        };
        match (key.as_str(), self.shift) {
            ("a", false) => Some(EditCommand::SelectAll),
            ("c", false) => Some(EditCommand::Copy),
            ("x", false) => Some(EditCommand::Cut),
            ("v", false) => Some(EditCommand::Paste),
            ("z", false) => Some(EditCommand::Undo),
            ("y", false) | ("z", true) => Some(EditCommand::Redo),
            _ => None,
        }
    }

    fn has_key(&self, key: &str) -> bool {
        self.keys.iter().any(|own| own == key)
    }
}

/// An edit command a control does from its own message (`EM_SETSEL`,
/// `WM_COPY`, `WM_CUT`, `WM_PASTE`, `WM_UNDO`, `EM_REDO`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditCommand {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

impl EditCommand {
    /// Every edit command fits a native edit box; redo (`EM_REDO`) only a
    /// rich-edit box.
    fn fits(self, target_class: &str) -> bool {
        class_keyboard_score(target_class) >= NATIVE_EDIT_BOX_SCORE
            && (self != Self::Redo || is_rich_edit_class(target_class))
    }
}

/// The lane of one key press on a window target.
pub(crate) struct KeyRoute {
    lane: KeyLane,
}

impl KeyRoute {
    /// Decides the lane (first match wins) from the chord, the class of the
    /// control that would receive it, and whether the session's claimed
    /// window is in front.
    pub(crate) fn for_window(
        chord: &KeyChord,
        target_class: &str,
        claimed_and_in_front: bool,
    ) -> Self {
        let lane = if chord.is_system_chord() {
            KeyLane::RefusedSystemChord
        } else if !chord.needs_held_modifier() {
            KeyLane::WindowMessages
        } else if chord
            .edit_command()
            .is_some_and(|command| command.fits(target_class))
        {
            KeyLane::EditMessage
        } else if claimed_and_in_front {
            KeyLane::ClaimedKeyPress
        } else {
            KeyLane::RefusedNeedsFront
        };
        Self { lane }
    }

    /// The lane.
    pub(crate) const fn lane(&self) -> KeyLane {
        self.lane
    }
}
