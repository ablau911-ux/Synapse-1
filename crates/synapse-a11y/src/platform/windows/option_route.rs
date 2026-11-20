//! #2348 R2/R3: which delivery one `act select` uses on a native list, chosen
//! from facts read on the element: the real Win32 class of its own window, its
//! ExpandCollapse state and how many options UIA shows under it. Pure: the
//! facts are read by `OptionPick` on the UIA worker, nothing here touches a
//! window.

use crate::{ExpandState, NativeOptionDelivery};

#[cfg(test)]
#[path = "option_route_table_2348.rs"]
mod option_route_table_2348;

/// The Win32 base class of a classic combo box, as `RealGetWindowClassW`
/// reports it (WinForms, MFC and VB6 combo boxes are superclasses of it).
const CLASSIC_COMBO_CLASS: &str = "ComboBox";

/// The facts one `act select` reads on its element before anything is sent.
pub(super) struct OptionRoute {
    /// Real Win32 class of the element's own window, trimmed; empty when the
    /// element has no window of its own (a page `<select>`, a WPF list).
    window_class: String,
    /// `None` when the element does not expose ExpandCollapsePattern.
    expand_state: Option<ExpandState>,
    /// Options (descendants with SelectionItemPattern) UIA shows now.
    options_seen: usize,
}

impl OptionRoute {
    pub(super) fn new(
        window_class: &str,
        expand_state: Option<ExpandState>,
        options_seen: usize,
    ) -> Self {
        Self {
            window_class: window_class.trim().to_owned(),
            expand_state,
            options_seen,
        }
    }

    /// How the option is picked; nothing is sent when it refuses.
    ///
    /// # Errors
    ///
    /// The refusal text when the list shows no options and cannot be opened.
    pub(super) fn delivery(&self) -> Result<NativeOptionDelivery, String> {
        if self.window_class.eq_ignore_ascii_case(CLASSIC_COMBO_CLASS) {
            return Ok(NativeOptionDelivery::ComboBoxMessages);
        }
        match (self.options_seen, self.expand_state) {
            (0, Some(ExpandState::Collapsed)) => Ok(NativeOptionDelivery::UiaExpandThenSelect),
            (0, None) => {
                Err("the list shows no options and cannot be opened (no ExpandCollapse)".to_owned())
            }
            (0, Some(state)) => Err(format!(
                "the list shows no options while its ExpandCollapse state is {state:?}"
            )),
            _ => Ok(NativeOptionDelivery::UiaSelectionItemPattern),
        }
    }
}
