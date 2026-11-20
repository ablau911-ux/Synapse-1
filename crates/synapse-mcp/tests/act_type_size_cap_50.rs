//! #50: typing requests are bounded BEFORE delivery, proven through the real
//! handlers (`act_type`, public `act` facade) of the real daemon, not only the
//! action builders. The recording backend replaces SendInput, so the "no side
//! effect" readback is the recorder's own log line count plus the
//! CF_ACTION_LOG row written by the handler.
//!
//! Contract: `m2/type_text.rs` MAX_ACT_TYPE_UTF16_UNITS = 65_536,
//! MAX_FOREGROUND_TYPE_UTF16_UNITS = 4_096, 2 ms/unit dispatch cost,
//! 20_000 ms budget; error shapes `type_request_size_error` /
//! `foreground_type_budget_error`.

mod recording_daemon;

use recording_daemon::{RecordingDaemon, error_data, recording_count, rows_of, structured};
use serde_json::{Value, json};

const FOREGROUND_UNIT_CAP: usize = 4_096;
const ABSOLUTE_UNIT_CAP: usize = 65_536;

fn burst_type(len: usize) -> Value {
    json!({
        "text": "x".repeat(len),
        "dynamics": "burst",
        "verify_delta": false,
    })
}

#[tokio::test]
async fn must_pass_burst_text_at_the_foreground_unit_cap_is_typed_once() -> anyhow::Result<()> {
    let mut daemon = RecordingDaemon::launch().await?;
    let response = daemon
        .client
        .tools_call("act_type", burst_type(FOREGROUND_UNIT_CAP))
        .await?;
    let body = structured(&response)?;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["chars_typed"], FOREGROUND_UNIT_CAP, "{body}");

    let (logs, rows) = daemon.finish().await?;
    assert_eq!(
        recording_count(&logs, "act_type"),
        1,
        "exactly one delivery"
    );
    assert_eq!(rows_of(&rows, "act_type", "ok").len(), 1);
    Ok(())
}

#[tokio::test]
async fn must_fail_one_unit_past_the_foreground_cap_is_refused_before_delivery()
-> anyhow::Result<()> {
    let mut daemon = RecordingDaemon::launch().await?;
    let error = daemon
        .client
        .tools_call_error("act_type", burst_type(FOREGROUND_UNIT_CAP + 1))
        .await?;
    let data = error_data(&error);
    assert_eq!(data["code"], "TOOL_PARAMS_INVALID", "{error}");
    assert_eq!(data["reason"], "foreground_type_budget_exceeded", "{error}");
    assert_eq!(data["requested_utf16_units"], FOREGROUND_UNIT_CAP + 1);
    assert_eq!(data["maximum_foreground_utf16_units"], FOREGROUND_UNIT_CAP);
    assert_eq!(data["refused_before_delivery"], true);

    let (logs, rows) = daemon.finish().await?;
    assert_eq!(recording_count(&logs, "act_type"), 0, "no key may be typed");
    let errors = rows_of(&rows, "act_type", "error");
    assert_eq!(errors.len(), 1, "the refusal is audited once");
    assert_eq!(errors[0]["error_code"], "TOOL_PARAMS_INVALID");
    Ok(())
}

#[tokio::test]
async fn must_fail_facade_text_over_the_absolute_cap_is_refused_before_any_route()
-> anyhow::Result<()> {
    let mut daemon = RecordingDaemon::launch().await?;
    let error = daemon
        .client
        .tools_call_error(
            "act",
            json!({
                "operation": "invoke",
                "action": {"verb": "type", "text": "x".repeat(ABSOLUTE_UNIT_CAP + 1)},
            }),
        )
        .await?;
    let data = error_data(&error);
    assert_eq!(data["code"], "TOOL_PARAMS_INVALID", "{error}");
    assert_eq!(data["reason"], "type_text_request_too_large", "{error}");
    assert_eq!(data["requested_utf16_units"], ABSOLUTE_UNIT_CAP + 1);
    assert_eq!(data["maximum_utf16_units"], ABSOLUTE_UNIT_CAP);

    let (logs, rows) = daemon.finish().await?;
    assert_eq!(recording_count(&logs, "act_type"), 0);
    assert!(
        rows_of(&rows, "act_type", "started").is_empty(),
        "the refusal lands before act_type is even started"
    );
    Ok(())
}

#[tokio::test]
async fn must_fail_coordinate_type_over_budget_is_refused_before_the_focus_click()
-> anyhow::Result<()> {
    let mut daemon = RecordingDaemon::launch().await?;
    let error = daemon
        .client
        .tools_call_error(
            "act",
            json!({
                "operation": "invoke",
                "action": {
                    "verb": "type",
                    "text": "x".repeat(FOREGROUND_UNIT_CAP + 1),
                    "x": 20,
                    "y": 20,
                },
            }),
        )
        .await?;
    let data = error_data(&error);
    assert_eq!(data["code"], "TOOL_PARAMS_INVALID", "{error}");
    assert_eq!(data["reason"], "foreground_type_budget_exceeded", "{error}");

    let (logs, rows) = daemon.finish().await?;
    assert_eq!(recording_count(&logs, "act_click"), 0, "no focus click");
    assert_eq!(recording_count(&logs, "act_type"), 0, "no key");
    assert!(rows_of(&rows, "act_click", "started").is_empty());
    Ok(())
}
