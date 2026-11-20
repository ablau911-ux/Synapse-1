use super::*;

fn seed_previous(temp: &Path, pid: u32) -> anyhow::Result<()> {
    let mut run = previous_run();
    run.pid = pid;
    // A genuinely live run started after boot; started_at=100 would (correctly) be
    // judged dead because it predates the machine boot (#144).
    run.started_at_unix_ms = now_unix_ms();
    write_json_atomic(&temp.join(RUN_CURRENT_FILE), &run)
}

fn start_at(db_path: &Path) -> anyhow::Result<DaemonLifecyclePaths> {
    configure(DaemonLifecycleConfig {
        mode: "http",
        bind_addr: None,
        db_path: db_path.to_path_buf(),
    })
}

#[test]
fn must_fail_ledger_read_error_does_not_block_startup() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    seed_previous(temp.path(), u32::MAX)?;
    fs::create_dir(temp.path().join(EXIT_EVENTS_FILE))?;
    start_at(temp.path())?;
    let current: RunRecord = read_optional_json(&temp.path().join(RUN_CURRENT_FILE))?
        .ok_or_else(|| anyhow::anyhow!("new run missing"))?;
    assert_eq!(current.pid, std::process::id());
    assert_ne!(current.run_id, "previous");
    Ok(())
}

#[test]
fn must_fail_non_utf8_ledger_is_unknown_and_does_not_block_startup() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    seed_previous(temp.path(), u32::MAX)?;
    let ledger = temp.path().join(EXIT_EVENTS_FILE);
    fs::write(&ledger, b"\xff\xfe\n")?;
    start_at(temp.path())?;
    // Unknown history: no unclean event is appended to the unreadable ledger.
    assert_eq!(fs::read(&ledger)?, b"\xff\xfe\n");
    // Startup still completed: this process is now the current run.
    let current: RunRecord = read_optional_json(&temp.path().join(RUN_CURRENT_FILE))?
        .ok_or_else(|| anyhow::anyhow!("new run missing"))?;
    assert_eq!(current.pid, std::process::id());
    assert_ne!(current.run_id, "previous");
    Ok(())
}

#[test]
fn must_pass_startup_writes_current_run_for_this_process() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    let paths = start_at(temp.path())?;
    assert_eq!(
        paths.run_current_path,
        temp.path().join(RUN_CURRENT_FILE).display().to_string()
    );
    assert_eq!(
        paths.exit_events_path,
        temp.path().join(EXIT_EVENTS_FILE).display().to_string()
    );
    let current: RunRecord = read_optional_json(&temp.path().join(RUN_CURRENT_FILE))?
        .ok_or_else(|| anyhow::anyhow!("new run missing"))?;
    assert_eq!(current.schema_version, SCHEMA_VERSION);
    assert_eq!(current.pid, std::process::id());
    assert_eq!(current.mode, "http");
    assert_eq!(current.db_path, temp.path().display().to_string());
    assert!(current.ended_at_unix_ms.is_none());
    assert!(current.ended_reason.is_none());
    // A first start has no previous run, so nothing is written to the exit ledger.
    assert!(!temp.path().join(EXIT_EVENTS_FILE).exists());
    Ok(())
}

#[test]
fn must_pass_gone_previous_run_without_exit_is_recorded_unclean() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    seed_previous(temp.path(), u32::MAX)?;
    start_at(temp.path())?;
    let exits = fs::read_to_string(temp.path().join(EXIT_EVENTS_FILE))?;
    let events: Vec<Value> = exits
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["run_id"], "previous");
    assert_eq!(events[0]["pid"], u32::MAX);
    assert_eq!(events[0]["event_kind"], "previous_run_unclean");
    assert_eq!(events[0]["cause"], "process_missing_on_startup");
    assert_eq!(events[0]["detail"]["new_pid"], std::process::id());
    let current: RunRecord = read_optional_json(&temp.path().join(RUN_CURRENT_FILE))?
        .ok_or_else(|| anyhow::anyhow!("new run missing"))?;
    assert_eq!(current.pid, std::process::id());
    assert_eq!(events[0]["detail"]["new_run_id"], current.run_id.as_str());
    Ok(())
}

#[test]
fn must_pass_previous_run_with_recorded_exit_is_not_reported_unclean() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    seed_previous(temp.path(), u32::MAX)?;
    let ledger = temp.path().join(EXIT_EVENTS_FILE);
    append_json_line(
        &ledger,
        &ExitEvent {
            schema_version: SCHEMA_VERSION,
            run_id: "previous".into(),
            pid: u32::MAX,
            event_kind: "daemon_exit".into(),
            cause: "shutdown".into(),
            detail: json!({}),
            recorded_at_unix_ms: now_unix_ms(),
            run: Some(previous_run()),
            last_tool_event: None,
            in_flight_tool_events: vec![],
            paths: DaemonLifecyclePaths {
                db_path: String::new(),
                run_current_path: String::new(),
                tool_last_path: String::new(),
                tool_events_path: String::new(),
                exit_events_path: String::new(),
            },
        },
    )?;
    let before = fs::read_to_string(&ledger)?;
    start_at(temp.path())?;
    // A previous run that recorded its own daemon_exit ended cleanly: the
    // ledger is left untouched and only the new run becomes current.
    assert_eq!(fs::read_to_string(&ledger)?, before);
    let current: RunRecord = read_optional_json(&temp.path().join(RUN_CURRENT_FILE))?
        .ok_or_else(|| anyhow::anyhow!("new run missing"))?;
    assert_eq!(current.pid, std::process::id());
    assert_ne!(current.run_id, "previous");
    Ok(())
}

#[cfg(windows)]
#[test]
fn must_fail_live_previous_pid_does_not_record_unclean_exit() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    start_at(temp.path())?;
    start_at(temp.path())?;
    assert!(!temp.path().join(EXIT_EVENTS_FILE).exists());
    // Legacy records have no creation time: a live PID must still be skipped.
    seed_previous(temp.path(), std::process::id())?;
    start_at(temp.path())?;
    assert!(!temp.path().join(EXIT_EVENTS_FILE).exists());
    Ok(())
}

#[cfg(windows)]
#[test]
fn must_fail_reused_pid_does_not_hide_unclean_previous_run() -> anyhow::Result<()> {
    let _serial = crate::test_support::daemon_lifecycle_serial();
    let temp = tempfile::tempdir()?;
    start_at(temp.path())?;
    let path = temp.path().join(RUN_CURRENT_FILE);
    let mut run: Value = serde_json::from_slice(&fs::read(&path)?)?;
    run["process_started_at_100ns"] = json!(1);
    write_json_atomic(&path, &run)?;
    start_at(temp.path())?;
    let exits = fs::read_to_string(temp.path().join(EXIT_EVENTS_FILE))?;
    let events: Vec<Value> = exits
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(events.len(), 1, "exactly one unclean event: {exits}");
    assert_eq!(events[0]["event_kind"], "previous_run_unclean");
    assert_eq!(events[0]["cause"], "process_missing_on_startup");
    assert_eq!(events[0]["pid"], std::process::id());
    Ok(())
}

fn previous_run() -> RunRecord {
    RunRecord {
        schema_version: SCHEMA_VERSION,
        run_id: "previous".into(),
        pid: 123,
        process_started_at_100ns: None,
        mode: "http".into(),
        bind_addr: None,
        db_path: "test".into(),
        started_at_unix_ms: 100,
        ended_at_unix_ms: None,
        ended_reason: None,
    }
}

#[test]
fn must_fail_unclean_detection_handles_missing_clean_and_recorded_runs() {
    assert!(unclean_previous_run(None, None, false).is_none());
    let mut run = previous_run();
    assert!(unclean_previous_run(Some(&run), None, true).is_none());
    run.ended_at_unix_ms = Some(200);
    assert!(unclean_previous_run(Some(&run), None, false).is_none());
}

#[test]
fn must_pass_unclean_detection_reports_unended_run() {
    let run = previous_run();
    let evidence = unclean_previous_run(Some(&run), None, false);
    assert_eq!(
        evidence.map(|(run, time, _)| (run.pid, time)),
        Some((123, 100))
    );
}

#[test]
fn must_fail_unclean_detection_only_uses_last_seen_from_matching_run() -> anyhow::Result<()> {
    let run = previous_run();
    let mut event: ToolEvent = serde_json::from_value(json!({
        "schema_version": 1, "run_id": "previous", "pid": 123, "seq": 1,
        "event_kind": "tool_call", "tool": "health", "status": "started",
        "started_at_unix_ms": 150
    }))?;
    assert_eq!(
        unclean_previous_run(Some(&run), Some(&event), false).map(|(_, t, _)| t),
        Some(150)
    );
    event.finished_at_unix_ms = Some(180);
    assert_eq!(
        unclean_previous_run(Some(&run), Some(&event), false).map(|(_, t, _)| t),
        Some(180)
    );
    event.run_id = "older".into();
    assert_eq!(
        unclean_previous_run(Some(&run), Some(&event), false).map(|(_, t, _)| t),
        Some(100)
    );
    Ok(())
}

#[test]
fn must_fail_exit_ledger_recovers_append_before_current_run_update() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("exit.jsonl");
    assert!(!has_exit_record(&path, "previous")?);
    let event = ExitEvent {
        schema_version: SCHEMA_VERSION,
        run_id: "previous".into(),
        pid: 123,
        event_kind: "panic".into(),
        cause: "panic".into(),
        detail: json!({}),
        recorded_at_unix_ms: 200,
        run: Some(previous_run()),
        last_tool_event: None,
        in_flight_tool_events: vec![],
        paths: DaemonLifecyclePaths {
            db_path: String::new(),
            run_current_path: String::new(),
            tool_last_path: String::new(),
            tool_events_path: String::new(),
            exit_events_path: String::new(),
        },
    };
    append_json_line(&path, &event)?;
    assert!(!has_exit_record(&path, "previous")?);
    append_json_line(
        &path,
        &ExitEvent {
            event_kind: "daemon_exit".into(),
            ..event
        },
    )?;
    assert!(has_exit_record(&path, "previous")?);
    assert!(!has_exit_record(&path, "other")?);
    OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"{partial")?;
    assert!(has_exit_record(&path, "previous")?);
    assert!(!has_exit_record(&path, "other")?);
    Ok(())
}
