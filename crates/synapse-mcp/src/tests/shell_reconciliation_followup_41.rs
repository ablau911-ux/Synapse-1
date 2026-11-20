//! #41: real writer/reconciler physical readback. Parent supplies fixture only.
use super::*;

#[test]
fn must_pass_reconciliation_retains_cancel_bit_on_terminal_commit_41() {
    let fx = fixture();
    let mut live = fx.status.clone();
    live.cancel_requested = true;
    write(&fx.paths, &live);
    assert!(committed(&fx.paths).cancel_requested);
    let result = write_shell_job_reconciliation_status(&fx.paths, terminal(&fx)).unwrap();
    assert_eq!(result.status, "ok");
    assert!(result.cancel_requested);
    assert_eq!(committed(&fx.paths).exit_code, Some(0));
    assert!(committed(&fx.paths).cancel_requested);
    let fx = fixture();
    let mut cancelling = fx.status.clone();
    cancelling.cancel_requested = true;
    write(&fx.paths, &cancelling);
    assert!(committed(&fx.paths).cancel_requested);
    write(&fx.paths, &terminal(&fx));
    assert!(
        committed(&fx.paths).cancel_requested,
        "monitor writer preserves cancellation bit"
    );
    assert_eq!(committed(&fx.paths).status, "ok");
}

#[test]
fn must_fail_weaker_cancel_cannot_replace_timeout_or_success_41() {
    for winner in ["timed_out", "ok"] {
        let fx = fixture();
        let mut latest = terminal(&fx);
        latest.status = winner.into();
        if winner == "timed_out" {
            latest.exit_code = None;
        }
        write(&fx.paths, &latest);
        assert_eq!(committed(&fx.paths).status, winner);
        let mut stale = fx.status.clone();
        stale.status = "cancelled".into();
        stale.cancel_requested = true;
        let result = write_shell_job_reconciliation_status(&fx.paths, stale.clone()).unwrap();
        assert_eq!(result.status, winner);
        assert!(result.cancel_requested);
        write(&fx.paths, &stale);
        let physical: ActRunShellJobStatus =
            serde_json::from_slice(&std::fs::read(&fx.paths.status_path).unwrap()).unwrap();
        assert_eq!(physical.status, winner);
        assert_eq!(physical.exit_code, latest.exit_code);
        assert!(physical.cancel_requested);
    }
}

#[test]
fn must_fail_status_reader_cannot_observe_mid_commit_state_41() {
    let fx = fixture();
    write(&fx.paths, &fx.status);
    assert_eq!(committed(&fx.paths).status, "running");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let (reader, premature) = status_commit::with_lock(&fx.paths.status_path, || {
        let path = fx.paths.status_path.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let value = read_shell_job_status(&path, JOB_ID).unwrap();
            done_tx.send(value).unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let premature = done_rx.recv_timeout(std::time::Duration::from_millis(200));
        write_shell_job_status_locked(&fx.paths.status_path, &terminal(&fx)).unwrap();
        (reader, premature)
    });
    reader.join().unwrap();
    assert!(
        matches!(premature, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "status read must share commit lock"
    );
    assert_eq!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .status,
        "ok"
    );
    assert_eq!(committed(&fx.paths).status, "ok");
}
