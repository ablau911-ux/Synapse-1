//! #192 / #219 / #220 through the REAL at-point scroll dispatcher.
//!
//! The scroll point lies over a window this test process created (see
//! `own_window_fixture`), so `act_scroll_with_handle_bound_target` (bound, like
//! `act_scroll`, to the root its claim check resolved) posts its WM_MOUSEWHEEL /
//! WM_MOUSEHWHEEL messages to that window and the test reads back exactly what
//! arrived. Every test first proves the point is over its own window; it never
//! posts to the owner's windows. The action emitter is a RecordingBackend, so
//! any SendInput use would be recorded instead of reaching the desktop.
//!
//! #192: dx > 0 scrolls RIGHT (+120 per notch), dy > 0 scrolls UP.
//! #219: the documented `target_act verb=scroll` example ["0","-1"] is ONE notch down.
//! #220: a real target_act scroll (verify_delta=false) is `delivered_unverified`.
//!
//! NEEDS-SEAM for #219/#220: `fn target_act_scroll_params(params:
//! &TargetActParams) -> Result<ActScrollParams, ErrorData>` — the parameter
//! building that `target_act_scroll` does today before `act_scroll`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use synapse_action::{ActionBackend, ActionEmitter, RecordingBackend};
use tokio_util::sync::CancellationToken;
use windows::Win32::UI::WindowsAndMessaging::{WM_MOUSEHWHEEL, WM_MOUSEWHEEL};

use super::*;
use crate::m2::ActScrollResponse;
use crate::server::own_window_fixture::{OwnWindows, WindowMessage, WindowSpec};

const WINDOW: WindowSpec = WindowSpec {
    x: 72,
    y: 72,
    w: 240,
    h: 160,
    label: None,
};

static TOKENS: AtomicUsize = AtomicUsize::new(1);

/// Own window whose centre is proven to be over it (and over nothing else).
fn own_window() -> (OwnWindows, Point) {
    let windows = OwnWindows::spawn(vec![WINDOW]);
    let point = windows.center(0);
    assert_eq!(
        crate::m2::root_hwnd_at_point(point),
        Some(windows.hwnd(0)),
        "precondition: the scroll point must be over the test's own window \
         (needs an interactive, unlocked desktop); refusing to post wheel input elsewhere"
    );
    assert!(
        windows.wheel_messages(0).is_empty(),
        "precondition: no wheel message yet"
    );
    (windows, point)
}

fn at(point: Point) -> Option<ActScrollPoint> {
    Some(ActScrollPoint {
        x: point.x,
        y: point.y,
    })
}

/// Runs the real dispatcher, then waits until the window processed everything
/// posted to it. Returns the result and the number of SendInput events.
async fn dispatch(
    windows: &OwnWindows,
    params: ActScrollParams,
) -> (Result<ActScrollResponse, ErrorData>, usize) {
    let cancel = CancellationToken::new();
    let recording = Arc::new(RecordingBackend::new());
    let backend: Arc<dyn ActionBackend> = recording.clone();
    let (handle, _snapshot, join) = ActionEmitter::spawn_with_backend(cancel.clone(), backend);
    // act_scroll binds the dispatch to the root its claim check resolved at the point.
    let checked_root = params
        .at
        .and_then(|at| crate::m2::root_hwnd_at_point(at.into()));
    let result =
        crate::m2::act_scroll_with_handle_bound_target(handle, None, params, checked_root).await;
    windows.barrier(0, TOKENS.fetch_add(1, Ordering::Relaxed));
    let sendinput_events = recording.events().len();
    cancel.cancel();
    let _ = join.await;
    (result, sendinput_events)
}

fn wheel(windows: &OwnWindows, message: u32, value: i64) -> WindowMessage {
    WindowMessage {
        hwnd: windows.hwnd(0),
        message,
        value,
    }
}

async fn scroll(dy: i32, dx: i32) -> (OwnWindows, Result<ActScrollResponse, ErrorData>, usize) {
    let (windows, point) = own_window();
    let params = ActScrollParams {
        dy,
        dx,
        at: at(point),
        ..ActScrollParams::default()
    };
    let (result, sendinput) = dispatch(&windows, params).await;
    (windows, result, sendinput)
}

#[tokio::test]
async fn must_pass_positive_dx_posts_one_hwheel_of_plus_120() {
    let (windows, result, sendinput) = scroll(0, 1).await;
    let response = result.expect("dispatch");
    assert_eq!(response.wheel_event_count, 1);
    assert_eq!(response.backend_used, "software_window_message");
    assert_eq!(
        windows.wheel_messages(0),
        [wheel(&windows, WM_MOUSEHWHEEL, 120)]
    );
    assert_eq!(sendinput, 0, "an at= scroll must not use SendInput");
}

#[tokio::test]
async fn must_pass_negative_dx_posts_one_left_hwheel_per_notch() {
    let (windows, result, _) = scroll(0, -2).await;
    assert_eq!(result.expect("dispatch").wheel_event_count, 2);
    assert_eq!(
        windows.wheel_messages(0),
        [
            wheel(&windows, WM_MOUSEHWHEEL, -120),
            wheel(&windows, WM_MOUSEHWHEEL, -120)
        ]
    );
}

#[tokio::test]
async fn must_pass_positive_dy_posts_one_wheel_up_of_plus_120() {
    let (windows, result, _) = scroll(1, 0).await;
    assert_eq!(result.expect("dispatch").wheel_event_count, 1);
    assert_eq!(
        windows.wheel_messages(0),
        [wheel(&windows, WM_MOUSEWHEEL, 120)]
    );
}

#[tokio::test]
async fn must_fail_zero_scroll_posts_nothing() {
    let (windows, result, sendinput) = scroll(0, 0).await;
    let response = result.expect("a zero scroll is a valid no-op");
    assert!(!response.scrolled);
    assert_eq!(response.wheel_event_count, 0);
    assert!(windows.wheel_messages(0).is_empty());
    assert_eq!(sendinput, 0);
}

fn scroll_request(args: &[&str], point: Point, extra: Value) -> TargetActParams {
    let mut raw = json!({ "verb": "scroll", "args": args, "x": point.x, "y": point.y });
    if let (Some(raw), Some(extra)) = (raw.as_object_mut(), extra.as_object()) {
        raw.extend(extra.clone());
    }
    serde_json::from_value(raw).expect("target_act scroll params")
}

#[tokio::test]
async fn must_pass_documented_example_scrolls_exactly_one_notch_down() {
    let (windows, point) = own_window();
    let params = scroll_request(&TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS, point, json!({}));
    let scroll = target_act_scroll_params(&params).expect("documented example is valid");
    assert_eq!((scroll.dx, scroll.dy), (0, -1));

    let (result, sendinput) = dispatch(&windows, scroll).await;

    assert_eq!(result.expect("dispatch").wheel_event_count, 1);
    assert_eq!(
        windows.wheel_messages(0),
        [wheel(&windows, WM_MOUSEWHEEL, -120)]
    );
    assert_eq!(sendinput, 0);
}

#[tokio::test]
async fn must_fail_non_integer_or_non_screen_scroll_is_refused_and_posts_nothing() {
    let (windows, point) = own_window();
    for params in [
        scroll_request(&["0", "abc"], point, json!({})),
        scroll_request(&["0", "-1"], point, json!({ "coordinate_space": "window" })),
    ] {
        let error = target_act_scroll_params(&params).expect_err("must be refused");
        let code = error
            .data
            .as_ref()
            .and_then(|d| d.get("code"))
            .and_then(Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID), "{error:?}");
    }
    windows.barrier(0, TOKENS.fetch_add(1, Ordering::Relaxed));
    assert!(
        windows.wheel_messages(0).is_empty(),
        "a refused scroll posted input"
    );
}

#[tokio::test]
async fn must_pass_real_target_act_scroll_is_delivered_unverified() {
    let (windows, point) = own_window();
    let params = scroll_request(&TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS, point, json!({}));
    let scroll = target_act_scroll_params(&params).expect("valid");
    assert!(
        !scroll.verify_delta,
        "target_act scroll does not request verify_delta"
    );

    let (result, _) = dispatch(&windows, scroll).await;
    let response = result.expect("dispatch");
    assert_eq!(response.postcondition.status, "not_requested");
    let (delegated, ok, status, value) =
        target_act_delegate_response("act_scroll", Ok(Json(response))).expect("delegate");

    assert_eq!((delegated, ok, status), ("act_scroll", true, "ok"));
    assert_eq!(value["wheel_event_count"], 1);
    assert_eq!(
        DeliveryClaim::new("scroll", ok, status, &value).state(),
        "delivered_unverified"
    );
}

#[tokio::test]
async fn must_fail_refused_scroll_is_never_labelled_delivered() {
    // 273 notches fit one message and 1024 messages is the cap: one more notch
    // is refused before any message is posted.
    let (windows, result, sendinput) = scroll(273 * 1024 + 1, 0).await;
    let Err(error) = result else {
        panic!("an over-cap scroll must be refused");
    };
    let code = error
        .data
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(Value::as_str);
    assert_eq!(code, Some(error_codes::ACTION_TARGET_INVALID), "{error:?}");
    let (_, ok, status, value) =
        target_act_delegate_response::<ActScrollResponse>("act_scroll", Err(error))
            .expect("delegate");
    assert_eq!(
        DeliveryClaim::new("scroll", ok, status, &value).state(),
        "refused_before_delivery"
    );
    assert!(windows.wheel_messages(0).is_empty());
    assert_eq!(sendinput, 0);
}
