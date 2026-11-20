//! Live UIA parent lookup for the focused date segment (#80).

use uiautomation::types::ElementMode;

use crate::{A11yResult, FocusedParentReadback};

use super::common::{
    TreeView, cached_hwnd, cached_role, cached_runtime_id_hex_or_fallback, cached_value,
    create_cache_request, map_uia_error, with_automation,
};

/// Upper bound on siblings visited under the parent (a date control has 3 segments).
const MAX_PARENT_CHILDREN: usize = 64;

fn is_spin_segment(role: &str) -> bool {
    let normalized: String = role
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    normalized == "spinbutton" || normalized == "spinner"
}

pub fn focused_spin_button_parent() -> A11yResult<Option<FocusedParentReadback>> {
    with_automation(|automation| {
        let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Control)?;
        let focused = automation
            .get_focused_element_build_cache(&cache)
            .map_err(map_uia_error)?;
        if !is_spin_segment(&cached_role(&focused)) {
            return Ok(None);
        }
        let walker = automation
            .get_control_view_walker()
            .map_err(map_uia_error)?;
        let Ok(parent) = walker.get_parent_build_cache(&focused, &cache) else {
            return Ok(None);
        };
        let hwnd = cached_hwnd(&focused).unwrap_or(0);
        let parent_id = cached_runtime_id_hex_or_fallback(&parent, hwnd)?.hex;
        let mut child_spin_values = Vec::new();
        if let Ok(mut child) = walker.get_first_child_build_cache(&parent, &cache) {
            for _ in 0..MAX_PARENT_CHILDREN {
                if is_spin_segment(&cached_role(&child))
                    && let Some(value) = cached_value(&child)
                {
                    child_spin_values.push(value);
                }
                match walker.get_next_sibling_build_cache(&child, &cache) {
                    Ok(next) => child = next,
                    Err(_) => break,
                }
            }
        }
        Ok(Some(FocusedParentReadback {
            parent_id,
            parent_value: cached_value(&parent),
            child_spin_values,
        }))
    })
}
