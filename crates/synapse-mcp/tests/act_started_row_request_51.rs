//! #51: the `started` CF_ACTION_LOG row of act_press / act_click / act_type
//! carries the sanitized request, written by the REAL handlers (not a JSON
//! builder called from a unit test). Real daemon, recording backend (no real
//! input), rows read back from RocksDB after the daemon exited.
//!
//! Contract: `server/act_request_audit.rs` (`started_details` =
//! `{request, preflight}`; printable keys `<redacted:printable>` outside a
//! valid shortcut chord; unknown names `<redacted:unrecognized>`).

mod recording_daemon;

use recording_daemon::{RecordingDaemon, error_data, rows_of};
use serde_json::{Value, json};

fn started_requests(rows: &[Value], tool: &str) -> Vec<Value> {
    rows_of(rows, tool, "started")
        .into_iter()
        .map(|row| {
            assert!(
                row["details"]["preflight"].is_object(),
                "preflight stays nested next to the request: {row}"
            );
            row["details"]["request"].clone()
        })
        .collect()
}

async fn press(daemon: &mut RecordingDaemon, keys: Value) -> anyhow::Result<()> {
    // Schema-validated call (client parity). The outcome after the started row
    // is not under test; the row is written right after preflight.
    let _ = daemon
        .client
        .tools_call("act_press", json!({"keys": keys, "verify_delta": false}))
        .await;
    Ok(())
}

#[tokio::test]
async fn must_pass_press_and_click_started_rows_carry_the_sanitized_request() -> anyhow::Result<()>
{
    let mut daemon = RecordingDaemon::launch().await?;
    press(&mut daemon, json!(["ctrl", "c"])).await?;
    press(&mut daemon, json!(["p"])).await?;
    let _ = daemon
        .client
        .tools_call("act_click", json!({"target": {"x": 20, "y": 20}}))
        .await;
    let _ = daemon
        .client
        .tools_call("act_type", json!({"text": "abc", "verify_delta": false}))
        .await;

    let (_logs, rows) = daemon.finish().await?;
    let presses = started_requests(&rows, "act_press");
    assert_eq!(presses.len(), 2, "{presses:?}");
    assert_eq!(presses[0]["keys"], json!(["ctrl", "c"]));
    assert_eq!(presses[0]["key_count"], 2);
    assert_eq!(presses[1]["keys"], json!(["<redacted:printable>"]));
    assert_eq!(presses[1]["key_count"], 1);

    let clicks = started_requests(&rows, "act_click");
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(
        clicks[0]["target"],
        json!({"kind": "point", "x": 20, "y": 20})
    );
    assert_eq!(clicks[0]["clicks"], 1);

    let types = started_requests(&rows, "act_type");
    assert_eq!(types.len(), 1, "{types:?}");
    assert_eq!(types[0]["requested_len"], 3);
    Ok(())
}

#[tokio::test]
async fn must_fail_secret_key_name_is_refused_and_never_persisted() -> anyhow::Result<()> {
    let mut daemon = RecordingDaemon::launch().await?;
    let error = daemon
        .client
        .tools_call_error(
            "act_press",
            json!({"keys": ["hunter2secret"], "verify_delta": false}),
        )
        .await?;
    assert_eq!(
        error_data(&error)["code"],
        "ACTION_UNSUPPORTED_KEY",
        "{error}"
    );
    assert!(
        !error.to_string().contains("hunter2secret"),
        "the refusal must not echo the key: {error}"
    );

    let (_logs, rows) = daemon.finish().await?;
    let presses = started_requests(&rows, "act_press");
    assert_eq!(presses.len(), 1, "{presses:?}");
    assert_eq!(presses[0]["keys"], json!(["<redacted:unrecognized>"]));
    let leaked = rows
        .iter()
        .filter(|row| row.to_string().contains("hunter2secret"))
        .count();
    assert_eq!(leaked, 0, "no CF_ACTION_LOG row may contain the secret");
    Ok(())
}
