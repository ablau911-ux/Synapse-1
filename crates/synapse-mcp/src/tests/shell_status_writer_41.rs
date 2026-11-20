//! #41 re-review regression: durable job `status.json` writers.
//!
//! Monitor, cancel and reconcile writers shared one `status.json.tmp` staging name, so
//! overlapping writers could truncate each other's buffer, rename the wrong payload or lose
//! the staging path, and a stale non-terminal writer could replace a newer terminal status.
//! Every write now stages under a unique name, writers of one job are serialized, a stale
//! live status never replaces a committed terminal one, and a failed write or rename leaves
//! no staging file behind.

use super::status_commit::{staging_in_flight, staging_path};
use super::*;

#[cfg(test)]
#[path = "shell_reconciliation_followup_41.rs"]
mod shell_reconciliation_followup_41;
#[cfg(test)]
#[path = "shell_staging_collision_41.rs"]
mod shell_staging_collision_41;

const JOB_ID: &str = "issue41-status-writer";

struct Fixture {
    temp: tempfile::TempDir,
    paths: ShellJobPaths,
    status: ActRunShellJobStatus,
}

fn fixture() -> Fixture {
    let temp = tempfile::TempDir::new()
        .unwrap_or_else(|error| panic!("create temp shell status dir: {error}"));
    let paths = ShellJobPaths {
        job_dir: temp.path().to_path_buf(),
        stdout_path: temp.path().join("stdout.log"),
        stderr_path: temp.path().join("stderr.log"),
        status_path: temp.path().join("status.json"),
        request_path: temp.path().join("request.json"),
        remote_cleanup_path: temp.path().join("remote-cleanup.json"),
    };
    let params = ActRunShellStartParams {
        command: "powershell.exe".to_owned(),
        args: vec![
            "-NoProfile".to_owned(),
            "-Command".to_owned(),
            JOB_ID.to_owned(),
        ],
        working_dir: None,
        env: BTreeMap::new(),
        timeout_ms: None,
        job_id: Some(JOB_ID.to_owned()),
    };
    let authorization = RunShellAuthorization {
        command_line: shell_command_line_from_parts(&params.command, &params.args),
        matched_pattern: "__any_permitted__".to_owned(),
    };
    let request_sha = run_shell_start_request_sha256(&params)
        .unwrap_or_else(|error| panic!("start request should hash: {error}"));
    let status = shell_job_status_record(
        JOB_ID,
        "running",
        &params,
        &paths,
        &request_sha,
        &authorization,
        "2026-06-15T00:00:00Z".to_owned(),
        Some(4242),
        None,
    );
    Fixture {
        temp,
        paths,
        status,
    }
}

fn committed(paths: &ShellJobPaths) -> ActRunShellJobStatus {
    read_shell_job_status(&paths.status_path, JOB_ID)
        .unwrap_or_else(|error| panic!("committed status should parse: {error}"))
}

fn write(paths: &ShellJobPaths, status: &ActRunShellJobStatus) {
    write_shell_job_status(&paths.status_path, status)
        .unwrap_or_else(|error| panic!("status write should commit: {error}"));
}

fn terminal(fixture: &Fixture) -> ActRunShellJobStatus {
    let mut status = fixture.status.clone();
    status.status = "ok".to_owned();
    status.exit_code = Some(0);
    status.completed_at = Some("2026-06-15T00:00:05Z".to_owned());
    status.duration_ms = Some(5_000);
    status
}

fn staging_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read job dir: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

#[test]
fn must_fail_stale_live_writers_never_replace_a_committed_terminal_status_41() {
    let fixture = fixture();
    write(&fixture.paths, &terminal(&fixture));

    for stale in ["cancel_requested", "running", "finalizing"] {
        let mut candidate = fixture.status.clone();
        candidate.status = stale.to_owned();
        write(&fixture.paths, &candidate);

        let readback = committed(&fixture.paths);
        assert_eq!(readback.status, "ok", "stale {stale} must lose");
        assert_eq!(readback.exit_code, Some(0));
        assert_eq!(
            readback.completed_at.as_deref(),
            Some("2026-06-15T00:00:05Z")
        );
    }
}

#[test]
fn must_pass_live_statuses_still_advance_to_terminal_41() {
    let fixture = fixture();
    write(&fixture.paths, &fixture.status);

    let mut cancelling = fixture.status.clone();
    cancelling.status = "cancel_requested".to_owned();
    write(&fixture.paths, &cancelling);
    assert_eq!(committed(&fixture.paths).status, "cancel_requested");

    write(&fixture.paths, &terminal(&fixture));
    assert_eq!(committed(&fixture.paths).status, "ok");
}

#[test]
fn must_pass_every_staging_path_is_unique_and_beside_the_target_41() {
    let fixture = fixture();
    let first = staging_path(&fixture.paths.status_path);
    let second = staging_path(&fixture.paths.status_path);

    assert_ne!(first, second);
    assert_eq!(first.parent(), fixture.paths.status_path.parent());
    assert_eq!(second.parent(), fixture.paths.status_path.parent());
    assert!(!staging_in_flight(&fixture.paths.status_path));

    std::fs::write(&first, b"{}").unwrap_or_else(|error| panic!("plant staging: {error}"));
    assert!(staging_in_flight(&fixture.paths.status_path));
}

#[test]
fn must_fail_overlapping_writers_never_fail_or_publish_a_torn_payload_41() {
    let fixture = fixture();
    write(&fixture.paths, &fixture.status);

    let writers: Vec<_> = (0_u64..8)
        .map(|writer| {
            let path = fixture.paths.status_path.clone();
            let base = fixture.status.clone();
            std::thread::spawn(move || {
                let mut failures = Vec::new();
                for round in 0_u64..40 {
                    let mut candidate = base.clone();
                    candidate.status = if round % 2 == 0 {
                        "running".to_owned()
                    } else {
                        "finalizing".to_owned()
                    };
                    candidate.duration_ms = Some(writer * 1_000 + round);
                    if let Err(error) = write_shell_job_status(&path, &candidate) {
                        failures.push(format!("writer {writer} round {round}: {error}"));
                    }
                }
                failures
            })
        })
        .collect();
    let failures: Vec<String> = writers
        .into_iter()
        .flat_map(|handle| {
            handle
                .join()
                .unwrap_or_else(|_| vec!["writer panicked".to_owned()])
        })
        .collect();

    assert!(failures.is_empty(), "{failures:#?}");
    let readback = committed(&fixture.paths);
    let duration = readback.duration_ms.unwrap_or(u64::MAX);
    assert!(
        (0_u64..8).any(|writer| (0_u64..40).any(|round| duration == writer * 1_000 + round)),
        "final payload must be exactly one writer's candidate, got {duration}"
    );
    assert_eq!(staging_files(fixture.temp.path()), Vec::<String>::new());
}

#[test]
fn must_fail_failed_commit_leaves_no_staging_file_behind_41() {
    let fixture = fixture();
    // A directory in the status slot makes the final rename fail.
    std::fs::create_dir(&fixture.paths.status_path)
        .unwrap_or_else(|error| panic!("occupy status slot: {error}"));

    let result = write_shell_job_status(&fixture.paths.status_path, &fixture.status);

    let Err(error) = result else {
        panic!("rename onto a directory must fail");
    };
    let data = error.data.as_ref();
    assert_eq!(
        data.and_then(|data| data.get("code")),
        Some(&json!(error_codes::STORAGE_WRITE_FAILED)),
        "{error:?}"
    );
    assert_eq!(
        data.and_then(|data| data.get("reason")),
        Some(&json!("job_status_rename_failed")),
        "{error:?}"
    );
    assert!(fixture.paths.status_path.is_dir());
    assert_eq!(staging_files(fixture.temp.path()), Vec::<String>::new());
}
