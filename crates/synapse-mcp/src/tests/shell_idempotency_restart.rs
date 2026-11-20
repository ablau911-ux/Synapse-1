use super::*;

fn request() -> Result<ActRunShellParams, serde_json::Error> {
    serde_json::from_value(json!({
        "command": "cmd.exe", "args": ["/c", "echo once"],
        "timeout_ms": 30_000, "idempotency_key": "issue-134-restart"
    }))
}

fn reservation(params: &ActRunShellParams) -> Result<Vec<u8>, ErrorData> {
    run_shell_idempotency_reservation_row(
        params,
        &RunShellAuthorization {
            command_line: "cmd.exe /c echo once".to_owned(),
            matched_pattern: "synthetic-test".to_owned(),
        },
        Some("old-session"),
        "old-daemon",
    )
}

fn reason(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("reason")?.as_str()
}

#[test]
fn must_fail_shell_idempotency_restart_new_http_session_settles_orphan()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let original = reservation(&params)?;
    let recovered = run_shell_idempotency_recover_after_restart(
        &params,
        &original,
        Some("new-session"),
        "new-daemon",
        false,
    )?
    .ok_or("orphan must become terminal across HTTP sessions")?;
    let row: RunShellIdempotencyRow = decode_json(&recovered)?;
    assert_eq!(row.status, "indeterminate");
    assert_eq!(row.session_id.as_deref(), Some("old-session"));
    assert!(row.response.is_none());
    assert!(row.completed_at.is_some());
    let Err(error) = run_shell_idempotency_replay(&params, &recovered, Some("new-session")) else {
        panic!("an unknown outcome must never authorize re-execution");
    };
    assert_eq!(reason(&error), Some("idempotency_outcome_indeterminate"));
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|d| d.get("automatic_retry_safe")),
        Some(&json!(false))
    );
    assert!(
        run_shell_idempotency_recover_after_restart(
            &params,
            &recovered,
            Some("third-session"),
            "third-daemon",
            false,
        )?
        .is_none()
    );
    Ok(())
}

#[test]
fn must_fail_shell_idempotency_restart_live_cross_session_reservation_is_not_stolen()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let original = reservation(&params)?;
    assert!(
        run_shell_idempotency_recover_after_restart(
            &params,
            &original,
            Some("other-session"),
            "old-daemon",
            true,
        )?
        .is_none()
    );
    let Err(error) = run_shell_idempotency_replay(&params, &original, Some("other-session")) else {
        panic!("live reservation must refuse another session");
    };
    assert_eq!(
        reason(&error),
        Some("idempotency_key_owned_by_different_session")
    );
    let Err(error) = run_shell_idempotency_replay(&params, &original, Some("old-session")) else {
        panic!("live reservation must refuse a concurrent duplicate");
    };
    assert_eq!(reason(&error), Some("idempotency_in_progress"));
    Ok(())
}

#[test]
fn must_fail_shell_idempotency_restart_conflicting_request_cannot_recover()
-> Result<(), Box<dyn std::error::Error>> {
    let mut params = request()?;
    let original = reservation(&params)?;
    params.args.push("different".to_owned());
    let Err(error) = run_shell_idempotency_recover_after_restart(
        &params,
        &original,
        Some("new-session"),
        "new-daemon",
        false,
    ) else {
        panic!("changed request must fail closed");
    };
    assert_eq!(reason(&error), Some("idempotency_key_conflict"));
    Ok(())
}

#[test]
fn must_fail_shell_idempotency_restart_legacy_ownerless_row_settles()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let mut row: RunShellIdempotencyRow = decode_json(&reservation(&params)?)?;
    row.schema_version = 2;
    row.reservation_owner_id = None;
    let recovered = run_shell_idempotency_recover_after_restart(
        &params,
        &encode_json(&row)?,
        Some("new-session"),
        "new-daemon",
        false,
    )?
    .ok_or("legacy orphan must settle")?;
    assert_eq!(
        decode_json::<RunShellIdempotencyRow>(&recovered)?.status,
        "indeterminate"
    );
    Ok(())
}

#[test]
fn must_fail_shell_idempotency_restart_completed_response_stays_private()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let mut row: RunShellIdempotencyRow = decode_json(&reservation(&params)?)?;
    row.status = "ok".to_owned();
    row.response = Some(serde_json::from_value(json!({
        "exit_code": 0, "stdout": "private-output", "stderr": "",
        "duration_ms": 1, "timed_out": false,
        "stdout_truncated": false, "stderr_truncated": false
    }))?);
    let bytes = encode_json(&row)?;
    assert!(
        run_shell_idempotency_recover_after_restart(
            &params,
            &bytes,
            Some("new-session"),
            "new-daemon",
            false,
        )?
        .is_none()
    );
    let Err(error) = run_shell_idempotency_replay(&params, &bytes, Some("new-session")) else {
        panic!("completed output must stay private");
    };
    assert_eq!(
        reason(&error),
        Some("idempotency_key_owned_by_different_session")
    );
    assert!(!serde_json::to_string(&error)?.contains("private-output"));
    assert_eq!(
        run_shell_idempotency_replay(&params, &bytes, Some("old-session"))?.stdout,
        "private-output"
    );
    Ok(())
}

#[test]
fn must_fail_shell_idempotency_restart_invalid_rows_cannot_recover()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let original: RunShellIdempotencyRow = decode_json(&reservation(&params)?)?;
    let mut rows = vec![(b"invalid-json".to_vec(), "row decode failed")];
    let mut row = original.clone();
    row.schema_version = 99;
    rows.push((encode_json(&row)?, "unexpected schema/tool"));
    row = original.clone();
    row.tool = "other-tool".to_owned();
    rows.push((encode_json(&row)?, "unexpected schema/tool"));
    row = original;
    row.idempotency_key_sha256 = "wrong-key".to_owned();
    rows.push((encode_json(&row)?, "key hash mismatch"));
    for (bytes, expected_message) in rows {
        let Err(error) = run_shell_idempotency_recover_after_restart(
            &params,
            &bytes,
            Some("new-session"),
            "new-daemon",
            false,
        ) else {
            panic!("invalid row ({expected_message}) must fail closed");
        };
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str),
            Some(error_codes::TOOL_INTERNAL_ERROR),
            "{error:?}"
        );
        assert!(
            error.message.contains(expected_message),
            "expected {expected_message:?} in {:?}",
            error.message
        );
    }
    Ok(())
}

fn authorization() -> RunShellAuthorization {
    RunShellAuthorization {
        command_line: "cmd.exe /c echo once".to_owned(),
        matched_pattern: "synthetic-test".to_owned(),
    }
}

#[test]
fn must_pass_shell_idempotency_reservation_is_in_progress_and_owned_by_caller()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let row: RunShellIdempotencyRow = decode_json(&reservation(&params)?)?;
    assert_eq!(row.schema_version, 3);
    assert_eq!(row.tool, "act_run_shell");
    assert_eq!(row.status, "in_progress");
    assert_eq!(row.session_id.as_deref(), Some("old-session"));
    assert_eq!(row.reservation_owner_id.as_deref(), Some("old-daemon"));
    assert_eq!(
        row.idempotency_key_sha256,
        sha256_hex("issue-134-restart".as_bytes())
    );
    assert_eq!(row.matched_pattern, "synthetic-test");
    assert!(!row.started_at.is_empty());
    assert!(row.completed_at.is_none());
    assert!(row.response.is_none());
    Ok(())
}

#[test]
fn must_pass_shell_idempotency_restart_orphan_fence_keeps_request_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let original_bytes = reservation(&params)?;
    let original: RunShellIdempotencyRow = decode_json(&original_bytes)?;
    let recovered: RunShellIdempotencyRow = decode_json(
        &run_shell_idempotency_recover_after_restart(
            &params,
            &original_bytes,
            Some("old-session"),
            "new-daemon",
            false,
        )?
        .ok_or("an orphan of a dead daemon must be fenced")?,
    )?;
    assert_eq!(recovered.schema_version, 3);
    assert_eq!(recovered.status, "indeterminate");
    assert_eq!(recovered.request_sha256, original.request_sha256);
    assert_eq!(
        recovered.idempotency_key_sha256,
        original.idempotency_key_sha256
    );
    assert_eq!(recovered.command_line, original.command_line);
    assert_eq!(recovered.started_at, original.started_at);
    assert_eq!(
        recovered.reservation_owner_id, original.reservation_owner_id,
        "the fence records the abandoned owner, not the recovering daemon"
    );
    Ok(())
}

#[test]
fn must_pass_shell_idempotency_restart_completed_row_replays_to_its_session()
-> Result<(), Box<dyn std::error::Error>> {
    let params = request()?;
    let response: ActRunShellResponse = serde_json::from_value(json!({
        "exit_code": 0, "stdout": "once", "stderr": "",
        "duration_ms": 1, "timed_out": false,
        "stdout_truncated": false, "stderr_truncated": false
    }))?;
    let bytes = run_shell_idempotency_completed_row(
        &params,
        &authorization(),
        &response,
        Some("old-session"),
    )?;
    // A completed row is terminal: restart recovery leaves it untouched.
    assert!(
        run_shell_idempotency_recover_after_restart(
            &params,
            &bytes,
            Some("old-session"),
            "new-daemon",
            false,
        )?
        .is_none()
    );
    let replayed = run_shell_idempotency_replay(&params, &bytes, Some("old-session"))?;
    assert_eq!(replayed.stdout, "once");
    assert_eq!(replayed.exit_code, Some(0));
    Ok(())
}
