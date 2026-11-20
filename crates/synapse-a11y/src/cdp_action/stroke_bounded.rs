//! #193: bounded CDP background drag. The route budget is derived from the planned
//! stroke duration, and a stroke that fails or times out after `mousePressed`
//! always releases the page button on a fresh socket.

use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    A11yError, A11yResult, CDP_INPUT_COMMAND_TIMEOUT, CdpMouseStrokePoint, DispatchMouseEventType,
    Duration, MouseButton, cdp_page_websocket_url, cdp_stroke_action_point,
    dispatch_cdp_mouse_stroke_session, dispatch_raw_mouse_event, settle_and_close_raw_input_socket,
};

/// Slack on top of the planned stroke duration for connect, press, release and
/// scheduler jitter. The stroke itself is paced in real time.
const CDP_STROKE_BUDGET_MARGIN: Duration = Duration::from_secs(30);
/// Defensive clamp; the public stroke validation already caps duration at 60 s.
const CDP_STROKE_BUDGET_MAX_PLANNED_MS: f64 = 600_000.0;

/// Route budget = planned stroke duration + [`CDP_STROKE_BUDGET_MARGIN`].
pub(super) fn cdp_stroke_budget(points: &[CdpMouseStrokePoint]) -> Duration {
    let planned_ms = points.last().map_or(0.0, |point| point.elapsed_ms);
    let planned_ms = if planned_ms.is_finite() {
        planned_ms.clamp(0.0, CDP_STROKE_BUDGET_MAX_PLANNED_MS)
    } else {
        0.0
    };
    Duration::from_secs_f64(planned_ms / 1000.0) + CDP_STROKE_BUDGET_MARGIN
}

pub(super) async fn dispatch_cdp_mouse_stroke_raw(
    endpoint: &str,
    target_id: &str,
    points: &[CdpMouseStrokePoint],
    button: MouseButton,
) -> A11yResult<()> {
    let budget = cdp_stroke_budget(points);
    dispatch_cdp_mouse_stroke_bounded(endpoint, target_id, points, button, budget).await
}

/// Runs the stroke under `budget`. A timeout is reported as
/// `A11Y_CDP_STROKE_TIMEOUT`; after any failure or timeout that happened at or
/// after the press, the page button is released on a fresh socket.
pub(super) async fn dispatch_cdp_mouse_stroke_bounded(
    endpoint: &str,
    target_id: &str,
    points: &[CdpMouseStrokePoint],
    button: MouseButton,
    budget: Duration,
) -> A11yResult<()> {
    let pressed = AtomicBool::new(false);
    let outcome = tokio::time::timeout(
        budget,
        dispatch_cdp_mouse_stroke_session(endpoint, target_id, points, button.clone(), &pressed),
    )
    .await;
    let result = outcome.unwrap_or_else(|_elapsed| {
        Err(A11yError::CdpStrokeTimeout {
            detail: format!(
                "cdp mouse stroke for target {target_id} exceeded its {} ms budget",
                budget.as_millis()
            ),
        })
    });
    if result.is_err() && pressed.load(Ordering::SeqCst) {
        release_pressed_button(endpoint, target_id, points, button).await;
    }
    result
}

/// Best-effort `mouseReleased` on a fresh page socket so a dropped or failed stroke
/// cannot leave the page believing the button is still held.
async fn release_pressed_button(
    endpoint: &str,
    target_id: &str,
    points: &[CdpMouseStrokePoint],
    button: MouseButton,
) {
    let Some(last) = points.last().copied() else {
        return;
    };
    let release = async {
        let ws_url = cdp_page_websocket_url(endpoint, target_id)?;
        let (mut socket, _response) = tokio_tungstenite::connect_async(ws_url.as_str())
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("connect CDP page WebSocket {ws_url} for stroke release: {err}"),
            })?;
        let mut command_id = 1_u64;
        dispatch_raw_mouse_event(
            &mut socket,
            &mut command_id,
            DispatchMouseEventType::MouseReleased,
            cdp_stroke_action_point(last),
            button,
            Some(0),
            1,
            "recovery_release",
            points.len().saturating_sub(1),
        )
        .await?;
        settle_and_close_raw_input_socket(&mut socket).await;
        Ok::<(), A11yError>(())
    };
    match tokio::time::timeout(CDP_INPUT_COMMAND_TIMEOUT, release).await {
        Ok(Ok(())) => tracing::warn!(
            target_id,
            "cdp mouse stroke aborted after press; released the page button on a fresh socket"
        ),
        Ok(Err(error)) => tracing::error!(
            target_id,
            %error,
            "cdp mouse stroke aborted after press and the recovery release failed; the page may still hold the button"
        ),
        Err(_elapsed) => tracing::error!(
            target_id,
            "cdp mouse stroke aborted after press and the recovery release timed out; the page may still hold the button"
        ),
    }
}
