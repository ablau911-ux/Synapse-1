use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::IsWindow};

mod common;
mod delivery_identity;
pub use delivery_identity::{foreground_root_identity, window_root_identity};
mod events;
mod focus_parent;
mod process_identity;
pub use process_identity::launch_process_record;
mod resolve;
mod snapshot;
mod window;

/// The one native HWND check: asks `IsWindow`, then applies [`crate::hwnd_validity`].
fn valid_hwnd(raw: i64) -> crate::A11yResult<HWND> {
    let hwnd = HWND(isize::try_from(raw).map_or(std::ptr::null_mut(), |native| native as *mut _));
    // SAFETY: IsWindow accepts any handle value and only reads USER state.
    let is_window = !hwnd.0.is_null() && unsafe { IsWindow(Some(hwnd)) }.as_bool();
    crate::hwnd_validity(raw, is_window)?;
    Ok(hwnd)
}

pub fn ensure_live_window(raw: i64) -> crate::A11yResult<()> {
    valid_hwnd(raw).map(|_| ())
}

pub fn uia_worker_readback() -> crate::A11yResult<crate::UiaWorkerReadback> {
    common::worker_readback()
}

pub use events::{WinEventSubscription, subscribe_win_events};
pub use focus_parent::focused_spin_button_parent;
pub use resolve::{
    append_element_text, click_element_action, element_bounding_rect, element_framework_id,
    element_metadata, element_scroll_state, element_value, expand_state_of, expand_state_of_id,
    focus_element, invoke_element_pattern, re_resolve, replace_element_text_selection,
    scroll_element, scroll_element_into_view, select_element_option, set_element_checked,
    set_element_text_selection, set_element_value,
};
pub use snapshot::{
    chromium_renderer_accessibility_nodes_from_window, element_node_from_point,
    find_by_name_and_pattern, find_by_name_and_pattern_in_window, focused_element_node,
    focused_element_node_in_window, focused_hwnd_in_window, has_chromium_child_window, snapshot,
    snapshot_element, snapshot_window_from_hwnd,
};
pub use window::{
    WindowBoundsOutcome, close_window, current_foreground_context, element_from_point,
    focus_window_with_intent, focused_element, focused_window, foreground_context,
    is_top_level_window, is_window_minimized, is_window_visible, millis_since_last_input,
    set_window_bounds, snapshot_focused_window, snapshot_window_for_process, top_level_root_hwnd,
    top_level_window_hwnd_by_name, visible_top_level_window_contexts, window_dpi_scale_for_hwnd,
    window_for_process, window_from_hwnd, window_owner_chain, workstation_locked,
};
