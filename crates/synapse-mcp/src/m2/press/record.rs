use rmcp::ErrorData;
use synapse_action::{EmitState, RecordedInput, RecordingBackend, redacted_key_label};
use synapse_core::Action;

use super::action_error_to_mcp;

pub(in crate::m2::press) fn execute_recording(
    recording: &RecordingBackend,
    action: &Action,
) -> Result<(), ErrorData> {
    let mut emit_state = EmitState::new();
    let new_events = recording
        .execute_capturing(action, &mut emit_state)
        .map_err(|error| action_error_to_mcp(&error))?;
    let event_sequence = event_sequence(&new_events);
    tracing::info!(
        code = "M2_ACT_PRESS_RECORDING_READBACK",
        kind = "act_press",
        recorded_event_total = recording.event_count(),
        new_event_count = new_events.len(),
        event_sequence,
        "readback=recording_backend tool=act_press after_events_readback"
    );
    Ok(())
}

pub(in crate::m2::press) fn event_sequence(events: &[RecordedInput]) -> String {
    events.iter().map(event_label).collect::<Vec<_>>().join(">")
}

fn event_label(event: &RecordedInput) -> String {
    match event {
        RecordedInput::KeyDown { key } => format!("down:{}", redacted_key_label(key)),
        RecordedInput::KeyUp { key } => format!("up:{}", redacted_key_label(key)),
        RecordedInput::DelayMs { ms } => format!("delay:{ms}"),
        other => other.log_label(),
    }
}
