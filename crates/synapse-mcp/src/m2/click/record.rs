use rmcp::ErrorData;
use synapse_action::{ActionHandle, DoubleClickTiming, EmitState, RecordedInput, RecordingBackend};
use synapse_core::{Action, AimCurve, MouseButton, MouseTarget};
use tokio::time::{Duration, sleep};

use super::action_error_to_mcp;

pub(super) async fn execute_actor_actions(
    handle: ActionHandle,
    actions: Vec<Action>,
    timing: DoubleClickTiming,
) -> Result<(), ErrorData> {
    execute_actor_actions_with(actions, timing, |action| {
        let handle = handle.clone();
        async move { handle.execute(action).await }
    })
    .await
}

async fn execute_actor_actions_with<F>(
    actions: Vec<Action>,
    timing: DoubleClickTiming,
    mut execute: impl FnMut(Action) -> F,
) -> Result<(), ErrorData>
where
    F: std::future::Future<Output = Result<(), synapse_action::ActionError>>,
{
    let action_count = actions.len();
    let mut button_delivered = false;
    for (action_index, action) in actions.into_iter().enumerate() {
        let button = matches!(&action, Action::MouseButton { .. });
        execute(action)
            .await
            .map_err(|error| actor_click_error(&error, button, button_delivered))?;
        button_delivered |= button;
        maybe_sleep_between_clicks(action_index, action_count, timing).await;
    }
    Ok(())
}

fn actor_click_error(error: &synapse_action::ActionError, button: bool, prior: bool) -> ErrorData {
    let error = action_error_to_mcp(error);
    if prior {
        crate::m2::postcondition::mark_input_delivered(error)
    } else if button {
        crate::m2::postcondition::mark_input_possibly_delivered(error)
    } else {
        super::element::mark_click_not_dispatched(error)
    }
}

pub(super) async fn execute_recording(
    recording: &RecordingBackend,
    actions: &[Action],
    click_count: u8,
    timing: DoubleClickTiming,
) -> Result<(), ErrorData> {
    let mut new_events = Vec::new();
    let mut emit_state = EmitState::new();
    let action_count = actions.len();
    let mut button_delivered = false;
    for (action_index, action) in actions.iter().enumerate() {
        new_events.extend(
            recording
                .execute_capturing(action, &mut emit_state)
                .map_err(|error| {
                    actor_click_error(
                        &error,
                        matches!(action, Action::MouseButton { .. }),
                        button_delivered,
                    )
                })?,
        );
        button_delivered |= matches!(action, Action::MouseButton { .. });
        maybe_sleep_between_clicks(action_index, action_count, timing).await;
    }
    let event_sequence = event_sequence(&new_events);
    let button_event_count = mouse_button_event_count(&new_events);
    let scheduled_inter_click_total_ms =
        u32::from(click_count.saturating_sub(1)) * timing.inter_click_delay_ms;
    tracing::info!(
        code = "M2_ACT_CLICK_RECORDING_READBACK",
        kind = "act_click",
        recorded_event_total = recording.event_count(),
        new_event_count = new_events.len(),
        click_count,
        button_event_count,
        double_click_window_ms = timing.window_ms,
        inter_click_delay_ms = timing.inter_click_delay_ms,
        scheduled_inter_click_total_ms,
        event_sequence,
        "readback=recording_backend tool=act_click after_events_readback"
    );
    Ok(())
}

async fn maybe_sleep_between_clicks(
    action_index: usize,
    action_count: usize,
    timing: DoubleClickTiming,
) {
    if should_delay_between_clicks(action_index, action_count) {
        let delay = Duration::from_millis(u64::from(timing.inter_click_delay_ms));
        sleep(delay).await;
    }
}

const fn should_delay_between_clicks(action_index: usize, action_count: usize) -> bool {
    action_index >= 1 && action_index + 1 < action_count
}

fn mouse_button_event_count(events: &[RecordedInput]) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RecordedInput::MouseButtonDown { .. } | RecordedInput::MouseButtonUp { .. }
            )
        })
        .count()
}

fn event_sequence(events: &[RecordedInput]) -> String {
    events.iter().map(event_label).collect::<Vec<_>>().join(">")
}

fn event_label(event: &RecordedInput) -> String {
    match event {
        RecordedInput::MouseMove {
            to,
            curve,
            duration_ms,
        } => format!(
            "mouse_move:{}:{}:{}",
            mouse_target_label(to),
            movement_profile_label(curve),
            duration_ms
        ),
        RecordedInput::MouseButtonDown { button } => format!("down:{}", button_label(*button)),
        RecordedInput::MouseButtonUp { button } => format!("up:{}", button_label(*button)),
        RecordedInput::DelayMs { ms } => format!("delay:{ms}"),
        other => other.log_label(),
    }
}

fn mouse_target_label(target: &MouseTarget) -> String {
    match target {
        MouseTarget::Screen { point } => format!("screen({},{})", point.x, point.y),
        MouseTarget::Element { element_id } => format!("element({element_id})"),
    }
}

const fn movement_profile_label(curve: &AimCurve) -> &'static str {
    match curve {
        AimCurve::Natural { .. } => "natural_fast",
        AimCurve::Instant => "instant",
        AimCurve::Linear => "linear",
        AimCurve::EaseInOut => "ease_in_out",
        AimCurve::Bezier { .. } => "bezier",
    }
}

const fn button_label(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::X1 => "x1",
        MouseButton::X2 => "x2",
    }
}

#[cfg(test)]
#[path = "tests/foreground_dispatch_189.rs"]
mod foreground_dispatch_189;
