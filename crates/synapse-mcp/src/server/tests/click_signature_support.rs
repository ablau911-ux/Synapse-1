//! Shared click-signature fixture for the `m2_tools` click regression tests
//! (#92, #107, #218, related-window).

use synapse_core::Rect;

use super::*;

pub(super) fn click_signature(
    hwnd: i64,
    pid: u32,
    process_name: &str,
    window_title: &str,
    element_count: usize,
) -> ClickDeltaSignature {
    ClickDeltaSignature {
        foreground_hwnd: hwnd,
        foreground_pid: pid,
        foreground_process: process_name.to_owned(),
        foreground_title: window_title.to_owned(),
        foreground_title_sha256: non_empty_sha256(window_title),
        focused_element_id: Some("focused.synthetic".to_owned()),
        focused_role: Some("Edit".to_owned()),
        focused_name_sha256: non_empty_sha256("synthetic focus"),
        focused_value_sha256: non_empty_sha256("synthetic value"),
        focused_bbox: Some(Rect {
            x: 1,
            y: 2,
            w: 300,
            h: 40,
        }),
        element_count,
        elements_sha256: format!("elements-{element_count}"),
        effect_elements_sha256: format!("effect-elements-{element_count}"),
        cdp_status: Some("unavailable".to_owned()),
        cdp_endpoint_present: false,
        web_path: None,
        cursor_position: None,
        pixel: ClickPixelSignature {
            // A successful capture (#191: only "ok" pixel evidence is compared).
            status: "ok".to_owned(),
            region: Rect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            bitmap_sha256: Some("pixel-signature".to_owned()),
            detail: Some("synthetic pixel signature".to_owned()),
        },
        point_pixel: None,
    }
}
