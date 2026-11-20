//! #235: `shell operation=status` for a job id with no status file must say
//! the job was not found and name the job id, not surface a raw OS path error.

use tempfile::TempDir;

use super::*;

#[test]
fn must_fail_unknown_job_status_names_the_job_and_says_not_found() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let path = temp.path().join("missing-job").join("status.json");

    let error = read_shell_job_status(&path, "missing-job")
        .err()
        .unwrap_or_else(|| panic!("a missing status file must be refused"));

    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!("TOOL_PARAMS_INVALID"))
    );
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("reason")),
        Some(&serde_json::json!("job_not_found"))
    );
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("job_id")),
        Some(&serde_json::json!("missing-job"))
    );
    assert!(
        error.message.contains("missing-job") && error.message.contains("not found"),
        "message must name the unknown job: {}",
        error.message
    );
    assert!(
        !error.message.contains("os error"),
        "raw OS error leaked: {}",
        error.message
    );
}

#[test]
fn must_fail_invalid_json_status_file_reports_decode_error() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let path = temp.path().join("status.json");
    std::fs::write(&path, b"not json").unwrap_or_else(|error| panic!("write: {error}"));

    let error = read_shell_job_status(&path, "present-job")
        .err()
        .unwrap_or_else(|| panic!("an invalid status file must be refused"));

    assert!(
        error.message.contains("JSON is invalid"),
        "an existing file reaches the JSON decode step: {}",
        error.message
    );
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!(error_codes::STORAGE_READ_FAILED))
    );
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("reason")),
        Some(&serde_json::json!("job_status_decode_failed"))
    );
}
