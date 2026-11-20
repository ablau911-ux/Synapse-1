//! #317 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the #317 table: a
//! coordinate-free `target_act verb=scroll` uses the session target window,
//! in the background, and never wheels a covered point of another window.

use super::{
    SessionTarget, TargetActParams, TargetActScrollRoute, session_scroll_point,
    target_act_error_code, target_act_scroll_params, target_act_scroll_route,
};
use synapse_core::error_codes::{ACTION_TARGET_INVALID, TOOL_PARAMS_INVALID};
use synapse_core::{Point, Rect};

const TARGET: i64 = 0x0A_0A0A;
const OTHER_AGENT: i64 = 0x0B_0B0B;
const OWNER: i64 = 0x0C_0C0C;

const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn target_act_scroll_route_table_317() {
    let window = SessionTarget::Window { hwnd: TARGET };
    let tab = SessionTarget::Cdp {
        window_hwnd: TARGET,
        cdp_target_id: "tab-317".to_owned(),
    };
    // (row, x/y given, session target, expected route)
    let rows: [(&str, bool, Option<&SessionTarget>, TargetActScrollRoute); 5] = [
        (
            "317_normal_window_target_no_point_scrolls_the_session_window",
            false,
            Some(&window),
            TargetActScrollRoute::SessionWindow { hwnd: TARGET },
        ),
        (
            "317_4c_empty_no_target_no_point_keeps_todays_path",
            false,
            None,
            TargetActScrollRoute::Unchanged,
        ),
        (
            "317_4h_tab_target_no_point_wheels_that_tab_through_the_bridge",
            false,
            Some(&tab),
            TargetActScrollRoute::BridgeTab,
        ),
        (
            "317_4i_neighbour_point_given_wins_over_window_target",
            true,
            Some(&window),
            TargetActScrollRoute::Point,
        ),
        (
            "317_4i_neighbour_point_given_without_target",
            true,
            None,
            TargetActScrollRoute::Point,
        ),
    ];
    let mut failures = Vec::new();
    for (row, has_point, target, expected) in rows {
        let got = target_act_scroll_route(has_point, target);
        if got != expected {
            failures.push(format!("row {row}: route {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#317 route rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn session_scroll_point_table_317() {
    // (row, window bounds, root under the chosen point, expected point or error code)
    type Row = (
        &'static str,
        Rect,
        Option<i64>,
        Result<(i32, i32), &'static str>,
    );
    let rows: [Row; 9] = [
        (
            "317_normal_centre_over_the_target_is_the_wheel_point",
            rect(0, 0, 800, 600),
            Some(TARGET),
            Ok((400, 300)),
        ),
        (
            "317_boundary_one_pixel_window",
            rect(10, 10, 1, 1),
            Some(TARGET),
            Ok((10, 10)),
        ),
        (
            "317_boundary_negative_origin_second_monitor",
            rect(-1920, 0, 1920, 1080),
            Some(TARGET),
            Ok((-960, 540)),
        ),
        (
            "317_4d_boundary_zero_area_is_refused",
            rect(10, 10, 0, 300),
            Some(TARGET),
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "317_4d_minimized_offscreen_centre_over_no_window_is_refused",
            rect(-32000, -32000, 160, 28),
            None,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "317_hostile_negative_size_is_refused",
            rect(100, 100, -50, 20),
            Some(TARGET),
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "317_hostile_centre_outside_screen_space_is_refused",
            rect(i32::MAX - 10, 0, 100, 100),
            Some(TARGET),
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "317_4g_someone_else_covers_the_centre_is_refused_never_wheeled",
            rect(0, 0, 800, 600),
            Some(OWNER),
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "317_4f_second_actor_window_covers_the_centre_is_refused",
            rect(0, 0, 800, 600),
            Some(OTHER_AGENT),
            Err(ACTION_TARGET_INVALID),
        ),
    ];
    let mut failures = Vec::new();
    for (row, bounds, hit, expected) in rows {
        let got = session_scroll_point(TARGET, bounds, |_point: Point| hit)
            .map(|point| (point.x, point.y));
        let got = match &got {
            Ok(point) => Ok(*point),
            Err(error) => Err(target_act_error_code(error).unwrap_or("<no code>")),
        };
        if got != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#317 point rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn target_act_scroll_params_table_317() {
    // (row, args, expected error code; None = accepted)
    let rows: [(&str, &[&str], Option<&str>); 3] = [
        (
            "317_4e_hostile_non_integer_args_are_refused",
            &["x", "1"],
            Some(TOOL_PARAMS_INVALID),
        ),
        (
            "317_4e_hostile_zero_deltas_are_refused",
            &["0", "0"],
            Some(TOOL_PARAMS_INVALID),
        ),
        (
            "317_4e_neighbour_documented_example_is_accepted",
            &["0", "-1"],
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (row, args, expected) in rows {
        let raw = serde_json::json!({ "verb": "scroll", "args": args });
        let params: TargetActParams = match serde_json::from_value(raw) {
            Ok(params) => params,
            Err(error) => {
                failures.push(format!("row {row}: request did not parse: {error}"));
                continue;
            }
        };
        let got = target_act_scroll_params(&params).err().map(|error| {
            target_act_error_code(&error)
                .unwrap_or("<no code>")
                .to_owned()
        });
        if got.as_deref() != expected {
            failures.push(format!("row {row}: error {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#317 params rows failed:\n{}",
        failures.join("\n")
    );
}
