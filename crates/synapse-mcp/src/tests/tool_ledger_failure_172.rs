//! #172: a failing diagnostic tool ledger (`daemon-tool-last.json`,
//! `daemon-tool-events.jsonl`) must never refuse a tool call or turn an
//! already-executed action into an error. The failure is logged (rate-limited)
//! and recorded in `last_error`; the real tool result is returned.
use super::*;

fn start(tool: &str) -> ToolCallStart {
    ToolCallStart {
        tool: tool.to_owned(),
        mcp_session_id: Some("session-172".to_owned()),
        audit_context: None,
        audit_context_read_error: None,
        foreground: None,
        foreground_read_error: None,
        session_target: None,
        session_target_read_error: None,
    }
}

/// Configures a ledger, then makes `path` unwritable by putting a directory
/// where the file (or its atomic-rename target) must be.
fn configure_with_blocked(
    temp: &tempfile::TempDir,
    block_last: bool,
    block_events: bool,
) -> anyhow::Result<DaemonLifecyclePaths> {
    let paths = configure(DaemonLifecycleConfig {
        mode: "http",
        bind_addr: Some("127.0.0.1:7700".to_owned()),
        db_path: temp.path().to_path_buf(),
    })?;
    for (block, path) in [
        (block_last, &paths.tool_last_path),
        (block_events, &paths.tool_events_path),
    ] {
        if block {
            let _ = fs::remove_file(path);
            fs::create_dir_all(path)?;
        }
    }
    Ok(paths)
}

fn last_error() -> Option<String> {
    diagnostic_value()
        .get("last_error")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[test]
fn must_fail_begin_succeeds_when_the_last_tool_file_cannot_be_replaced() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    let paths = configure_with_blocked(&temp, true, false)?;
    let guard = begin_tool_call(start("act"))?;
    let error = last_error().ok_or_else(|| anyhow::anyhow!("failure must be recorded"))?;
    assert!(
        error.contains(&paths.tool_last_path),
        "last_error must name the blocked last-tool file: {error}"
    );
    assert!(error.contains("daemon-tool-last.json"), "{error}");
    guard.finish_ok_with_effective_target(None)?;
    Ok(())
}

#[test]
fn must_fail_finish_succeeds_for_every_outcome_when_both_ledger_files_fail() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    configure_with_blocked(&temp, true, true)?;
    begin_tool_call(start("a"))?.finish_ok_with_effective_target(None)?;
    begin_tool_call(start("b"))?.finish_error(json!({"message": "x"}))?;
    begin_tool_call(start("c"))?
        .finish_error_with_effective_target(json!({"message": "x"}), None)?;
    begin_tool_call(start("d"))?.finish_panic(json!({"payload": "boom"}))?;
    assert!(last_error().is_some());
    Ok(())
}

#[test]
fn must_fail_context_event_survives_a_failing_ledger() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    let paths = configure_with_blocked(&temp, true, true)?;
    record_context_event(ContextEvent {
        event_kind: "foreground_context_restore",
        tool: "act_press",
        status: "restored",
        mcp_session_id: None,
        foreground: None,
        foreground_read_error: None,
        detail: json!({}),
    })?;
    let error = last_error().ok_or_else(|| anyhow::anyhow!("failure must be recorded"))?;
    assert!(
        error.contains(&paths.tool_events_path),
        "last_error must name the blocked events file: {error}"
    );
    Ok(())
}

#[test]
fn must_pass_healthy_ledger_records_event_and_clears_last_error() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    // Start broken so `last_error` is set, then heal the slot: the next
    // successful write must record the event and clear the error.
    let paths = configure_with_blocked(&temp, true, false)?;
    let guard = begin_tool_call(start("healthy-172"))?;
    assert!(
        last_error().is_some(),
        "precondition: the blocked write failed"
    );
    fs::remove_dir_all(&paths.tool_last_path)?;

    guard.finish_ok_with_effective_target(None)?;

    assert_eq!(last_error(), None, "a successful write clears last_error");
    let last: Value = serde_json::from_str(&fs::read_to_string(&paths.tool_last_path)?)?;
    assert_eq!(
        last.get("tool").and_then(Value::as_str),
        Some("healthy-172")
    );
    let events = fs::read_to_string(&paths.tool_events_path)?;
    let healthy_lines = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event.get("tool").and_then(Value::as_str) == Some("healthy-172"))
        .count();
    assert_eq!(
        healthy_lines, 2,
        "start and finish events are appended: {events}"
    );
    Ok(())
}

#[test]
fn must_fail_best_effort_begin_returns_a_usable_guard_when_the_ledger_is_unconfigured()
-> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    reset_for_test();
    let guard = begin_tool_call_best_effort(start("observe"));
    guard.finish_ok_with_effective_target(None)?;
    Ok(())
}

#[test]
fn must_pass_failure_log_limiter_admits_one_line_per_interval_and_counts_the_rest() {
    let limiter = FailureLogLimiter::new(1_000);
    assert_eq!(limiter.admit(10), Some(0), "first failure always logs");
    assert_eq!(limiter.admit(500), None);
    assert_eq!(limiter.admit(900), None);
    assert_eq!(limiter.admit(1_010), Some(2), "reports what was suppressed");
    assert_eq!(limiter.admit(1_011), None);
}
