use serde::{Deserialize, Serialize};
use synapse_core::{ElementId, Rect};

use crate::{A11yResult, UIElement, platform};

/// Re-resolves a composite Synapse element id back to a live UIA element.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the runtime id cannot be found under the
/// HWND, `OBSERVE_INTERNAL` for invalid ids, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn re_resolve(id: &ElementId) -> A11yResult<UIElement> {
    platform::re_resolve(id)
}

/// Result of preparing an element-target click entirely on the UIA worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ElementClickAction {
    Invoked,
    Toggled {
        before_state: String,
        after_state: String,
    },
    Selected {
        was_selected: bool,
        is_selected: bool,
    },
    Expanded {
        before_state: ExpandState,
        after_state: ExpandState,
    },
    Collapsed {
        before_state: ExpandState,
        after_state: ExpandState,
    },
    LegacyDefaultAction {
        default_action: Option<String>,
    },
}

/// #2348 round 2 (FF-06): how one `act select` reached a native list.
/// `OptionRoute` (platform/windows/option_route.rs) chooses it from facts
/// read on the element; the reply names it and reads back where it says.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeOptionDelivery {
    /// `SelectionItemPattern.Select` on the option (a list box, or a list
    /// that is already open).
    UiaSelectionItemPattern,
    /// `ExpandCollapsePattern.Expand`, `Select` on the option, then
    /// `Collapse` again (a closed list that shows no option yet).
    UiaExpandThenSelect,
    /// A classic Win32 combo box: `CB_SETCURSEL`, then `CBN_SELCHANGE` to its
    /// parent window, as the combo box itself does when a person picks.
    ComboBoxMessages,
}

impl NativeOptionDelivery {
    /// Where the pick is read back.
    #[must_use]
    pub const fn source_of_truth(self) -> &'static str {
        match self {
            Self::UiaSelectionItemPattern | Self::UiaExpandThenSelect => {
                "uia_selection_item_pattern.is_selected"
            }
            Self::ComboBoxMessages => "combo_box.cb_getcursel",
        }
    }
}

/// #2348: readback of [`select_element_option`]: the option a native list or
/// combo box holds after the pick, read back where its `delivery` says
/// ([`NativeOptionDelivery::source_of_truth`]).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NativeOptionReadback {
    /// The option's visible name.
    name: String,
    /// Its place among the options, from 0.
    index: usize,
    /// How many options the element has.
    option_count: usize,
    was_selected: bool,
    is_selected: bool,
    /// How the option was reached (`OptionRoute`, #2348 round 2).
    delivery: NativeOptionDelivery,
}

impl NativeOptionReadback {
    #[must_use]
    pub(crate) const fn new(
        name: String,
        index: usize,
        option_count: usize,
        was_selected: bool,
        is_selected: bool,
        delivery: NativeOptionDelivery,
    ) -> Self {
        Self {
            name,
            index,
            option_count,
            was_selected,
            is_selected,
            delivery,
        }
    }

    /// The option was selected before the call (nothing was sent then).
    #[must_use]
    pub const fn was_selected(&self) -> bool {
        self.was_selected
    }

    /// How the option was reached.
    #[must_use]
    pub const fn delivery(&self) -> NativeOptionDelivery {
        self.delivery
    }
}

/// Readback from setting an element's native text/value on the UIA worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementValueSetReadback {
    pub method: String,
    pub before_value: String,
    pub after_value: String,
    pub expected_after_value: Option<String>,
    pub is_password: bool,
    pub before_password_len: Option<usize>,
    pub after_password_len: Option<usize>,
}

/// Readback from an element's native text/value pattern without mutating it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementValueReadback {
    pub method: String,
    pub value: String,
    pub is_readonly: bool,
    pub is_password: bool,
    pub password_len: Option<usize>,
}

/// Readback from setting the selection range on a native text/edit element.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementTextSelectionReadback {
    pub method: String,
    pub text_len: u32,
    pub requested_start: u32,
    pub requested_end: u32,
    pub before_start: u32,
    pub before_end: u32,
    pub after_start: u32,
    pub after_end: u32,
}

/// Readback from replacing the current selection on a native text/edit element.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementTextInsertReadback {
    pub method: String,
    pub mode: String,
    pub before_text_utf16_len: u32,
    pub after_text_utf16_len: u32,
    pub requested_text_utf16_len: u32,
    pub inserted_text_utf16_len: u32,
    pub expected_after_text_utf16_len: u32,
    pub normalized_text: bool,
    pub before_start: u32,
    pub before_end: u32,
    pub replace_start: u32,
    pub replace_end: u32,
    pub after_start: u32,
    pub after_end: u32,
}

/// Live metadata read from a re-resolved element on the UIA worker.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementMetadataReadback {
    pub name: String,
    pub role: String,
    pub automation_id: Option<String>,
    pub bbox: Rect,
    pub enabled: bool,
    pub keyboard_focusable: bool,
    pub patterns: Vec<synapse_core::UiaPattern>,
    pub value: Option<String>,
}

/// UIA scroll state read separately from the target element.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementScrollStateReadback {
    pub bbox: Rect,
    pub horizontal_scroll_percent: Option<f64>,
    pub vertical_scroll_percent: Option<f64>,
    pub horizontal_view_size: Option<f64>,
    pub vertical_view_size: Option<f64>,
    pub horizontally_scrollable: Option<bool>,
    pub vertically_scrollable: Option<bool>,
}

/// Readback from scrolling an element through UIA control patterns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ElementScrollReadback {
    pub method: String,
    pub before: ElementScrollStateReadback,
    pub after: ElementScrollStateReadback,
    pub requested_dy: i32,
    pub requested_dx: i32,
    pub scroll_call_count: u32,
}

/// Resolves an element and reads its current bounding rectangle as plain data.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn element_bounding_rect(id: &ElementId) -> A11yResult<Rect> {
    platform::element_bounding_rect(id)
}

/// Resolves an element and reads its UI Automation `FrameworkId` live
/// (`WPF`, `XAML`, `WinForm`, `Win32` ...).
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn element_framework_id(id: &ElementId) -> A11yResult<String> {
    platform::element_framework_id(id)
}

/// Attempts a semantic UIA click for an element using supported UIA control
/// patterns.
///
/// No coordinate fallback is synthesized here; unsupported elements return
/// `ACTION_ELEMENT_PATTERN_UNSUPPORTED` so the caller can decide the next
/// explicit delivery tier.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// typed unsupported-pattern error when no click-like UIA pattern is exposed, a
/// structured UIA error for pattern method failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn click_element_action(id: &ElementId) -> A11yResult<ElementClickAction> {
    platform::click_element_action(id)
}

/// Invokes a re-resolved element using its UIA InvokePattern.
///
/// # Errors
///
/// Returns a UIA error if the element or pattern is unavailable or invoke fails.
pub fn invoke_element_pattern(id: &ElementId) -> A11yResult<()> {
    platform::invoke_element_pattern(id)
}

/// #2348: selects one option of a native list or combo box by `choice`
/// (visible name, value or index), through SelectionItemPattern; a combo box
/// with no option yet is opened through ExpandCollapse first. An option that
/// is already selected is success and nothing is sent.
///
/// # Errors
///
/// `ACTION_TARGET_INVALID` (`OptionRefused`) when the element has no option,
/// none with that name, or two; a typed UIA pattern error otherwise;
/// `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn select_element_option(
    id: &ElementId,
    choice: &crate::OptionChoice,
) -> A11yResult<NativeOptionReadback> {
    platform::select_element_option(id, choice)
}

/// Sets a UIA TogglePattern to the requested state and returns its before/after states.
///
/// # Errors
///
/// Returns a UIA error if the element or pattern is unavailable or the requested
/// state cannot be read back.
pub fn set_element_checked(id: &ElementId, checked: bool) -> A11yResult<(String, String)> {
    platform::set_element_checked(id, checked)
}

/// Sets UIA focus on a re-resolved element without returning the COM element.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for focus failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn focus_element(id: &ElementId) -> A11yResult<()> {
    platform::focus_element(id)
}

/// Sets a re-resolved element's text/value and reads it back.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA/native-message error when the element exposes neither
/// writable `ValuePattern` nor a native edit HWND text-message route, and
/// `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn set_element_value(id: &ElementId, value: &str) -> A11yResult<ElementValueSetReadback> {
    platform::set_element_value(id, value)
}

/// Reads a re-resolved element's current text/value.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA/native-message error when the element exposes neither
/// `ValuePattern` nor a native edit HWND text-message readback route, and
/// `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn element_value(id: &ElementId) -> A11yResult<ElementValueReadback> {
    platform::element_value(id)
}

/// Sets a re-resolved native edit/rich-edit element's selection range and
/// verifies it with a separate native message readback.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured unsupported-pattern error when the target is not backed by a
/// native edit HWND, and `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn set_element_text_selection(
    id: &ElementId,
    start: u32,
    end: u32,
) -> A11yResult<ElementTextSelectionReadback> {
    platform::set_element_text_selection(id, start, end)
}

/// Replaces the current selection on a native edit/rich-edit element and
/// verifies the resulting text with a separate native message readback.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured unsupported-pattern error when the target is not backed by a
/// native edit HWND, and `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn replace_element_text_selection(
    id: &ElementId,
    text: &str,
) -> A11yResult<ElementTextInsertReadback> {
    platform::replace_element_text_selection(id, text)
}

/// Appends text to a native edit/rich-edit element by setting the selection to
/// the exact end of the native text buffer before replacing the selection.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured unsupported-pattern error when the target is not backed by a
/// native edit HWND, and `A11Y_NOT_AVAILABLE` on non-Windows.
pub fn append_element_text(id: &ElementId, text: &str) -> A11yResult<ElementTextInsertReadback> {
    platform::append_element_text(id, text)
}

/// Reads live metadata for a re-resolved element without mutating it.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows.
pub fn element_metadata(id: &ElementId) -> A11yResult<ElementMetadataReadback> {
    platform::element_metadata(id)
}

/// Scrolls a re-resolved element into its container's viewport through UIA
/// `ScrollItemPattern.ScrollIntoView` (#882). Composite tools call this before
/// a coordinate click so an off-viewport target's stale bbox cannot steer the
/// click into another window.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// typed unsupported-pattern error when `ScrollItemPattern` is not exposed, a
/// structured UIA error for pattern method failures, or `A11Y_NOT_AVAILABLE`
/// on non-Windows platforms.
pub fn scroll_element_into_view(id: &ElementId) -> A11yResult<()> {
    platform::scroll_element_into_view(id)
}

/// Scrolls a re-resolved element through UIA `ScrollPattern` or
/// `ScrollItemPattern` and returns before/after target readback.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// typed unsupported-pattern error when neither scroll pattern is available, a
/// structured UIA error for pattern method failures, or `A11Y_NOT_AVAILABLE`
/// on non-Windows platforms.
pub fn scroll_element(id: &ElementId, dy: i32, dx: i32) -> A11yResult<ElementScrollReadback> {
    platform::scroll_element(id, dy, dx)
}

/// Reads a re-resolved element's current UIA scroll state without mutating it.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for state read failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn element_scroll_state(id: &ElementId) -> A11yResult<ElementScrollStateReadback> {
    platform::element_scroll_state(id)
}

/// Read-only mirror of `uiautomation::types::ExpandCollapseState`. Kept
/// independent of the underlying crate so callers don't need a uiautomation
/// dependency just to compare against a literal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandState {
    Collapsed,
    Expanded,
    PartiallyExpanded,
    LeafNode,
}

/// Reads `ExpandCollapsePattern::CurrentExpandCollapseState` from the given
/// element.
///
/// Used by `act_click(use_invoke_pattern=true)` manual verification tests to
/// assert menu/expander state flipped after an invoke.
///
/// # Errors
///
/// Returns the same structured UIA errors as the other a11y accessors;
/// `A11Y_PATTERN_UNAVAILABLE`-class error when the element does not expose
/// `ExpandCollapsePattern`; `A11Y_NOT_AVAILABLE` on non-Windows platforms.
pub fn expand_state_of(element: &UIElement) -> A11yResult<ExpandState> {
    platform::expand_state_of(element)
}

/// Reads `ExpandCollapsePattern::CurrentExpandCollapseState` for a re-resolved
/// element without returning the COM element.
///
/// # Errors
///
/// Returns the same structured UIA errors as `expand_state_of`, or
/// `A11Y_NOT_AVAILABLE` on non-Windows platforms.
pub fn expand_state_of_id(id: &ElementId) -> A11yResult<ExpandState> {
    platform::expand_state_of_id(id)
}
