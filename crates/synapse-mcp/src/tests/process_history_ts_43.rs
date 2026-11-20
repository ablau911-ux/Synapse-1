//! #43: `CF_PROCESS_HISTORY` rows carry a root `ts_ns` (insert time) so the
//! storage TTL compaction filter can expire them.

use std::collections::BTreeMap;

use super::{ActLaunchParams, ActLaunchResponse, launch_process_history_row};

fn params() -> ActLaunchParams {
    ActLaunchParams {
        target: "notepad.exe".to_owned(),
        args: Vec::new(),
        working_dir: None,
        env: BTreeMap::new(),
        wait_for_window_title_regex: None,
        timeout_ms: 10_000,
        idempotency_key: None,
        cdp_debug: None,
        force_renderer_accessibility: None,
        windows_console_window_state: None,
        desktop: None,
    }
}

fn response() -> ActLaunchResponse {
    ActLaunchResponse {
        pid: 4321,
        hwnd: None,
        window_owner_pid: None,
        reused_existing_window: false,
        window_owner_rule: None,
        matched_title: None,
        launched_at: "2026-05-31T20:00:00Z".to_owned(),
        reason: None,
        cdp_debug_port: None,
        cdp_endpoint: None,
        cdp_user_data_dir: None,
        cdp_verified_url: None,
        cdp_verified_title: None,
        desktop: None,
    }
}

#[test]
fn must_pass_process_history_row_has_root_insert_time_ts_ns() {
    let before = crate::m3::episodes::now_ts_ns();
    let row = launch_process_history_row(&params(), &response(), "started")
        .unwrap_or_else(|error| panic!("row should encode: {error}"));
    let after = crate::m3::episodes::now_ts_ns();
    let value: serde_json::Value =
        serde_json::from_slice(&row).unwrap_or_else(|error| panic!("row should decode: {error}"));
    let ts_ns = value["ts_ns"]
        .as_u64()
        .unwrap_or_else(|| panic!("root ts_ns must be a u64: {value}"));
    assert!(
        (before..=after).contains(&ts_ns),
        "ts_ns {ts_ns} not in [{before}, {after}]"
    );
}

#[test]
fn must_fail_invalid_or_stale_launched_at_never_replaces_insert_time_ts_ns() {
    for launched_at in ["", "not-a-timestamp", "1970-01-01T00:00:00Z"] {
        let mut launch = response();
        launched_at.clone_into(&mut launch.launched_at);
        let before = crate::m3::episodes::now_ts_ns();
        let row = launch_process_history_row(&params(), &launch, "started")
            .unwrap_or_else(|error| panic!("row should encode for {launched_at:?}: {error}"));
        let after = crate::m3::episodes::now_ts_ns();
        let value: serde_json::Value = serde_json::from_slice(&row)
            .unwrap_or_else(|error| panic!("row should decode: {error}"));
        let ts_ns = value["ts_ns"]
            .as_u64()
            .unwrap_or_else(|| panic!("root ts_ns must be a u64 for {launched_at:?}: {value}"));
        assert!(
            (before..=after).contains(&ts_ns),
            "launched_at {launched_at:?} must not drive the TTL clock: ts_ns {ts_ns} not in [{before}, {after}]"
        );
        assert_eq!(
            value["launched_at"], launched_at,
            "launched_at is stored as given"
        );
    }
}
