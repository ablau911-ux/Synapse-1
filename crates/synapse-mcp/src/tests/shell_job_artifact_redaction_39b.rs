//! #39b: durable shell-job stdout/stderr artifacts must never persist secrets
//! on disk, including secrets split across read-chunk boundaries.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

use super::redacted_log::{StreamRedactor, finish_pumps, pump_redacted, record_artifact_errors};

const MASK: &str = "[redacted-shell-output]";

fn run_bytes(chunks: &[&[u8]]) -> Vec<u8> {
    let mut redactor = StreamRedactor::new();
    let mut out = Vec::new();
    for chunk in chunks {
        out.extend(redactor.push(chunk));
    }
    out.extend(redactor.finish());
    out
}

fn run_chunks(chunks: &[&[u8]]) -> String {
    String::from_utf8_lossy(&run_bytes(chunks)).into_owned()
}

#[test]
fn must_fail_env_style_secret_split_across_chunks_is_masked() {
    let out = run_chunks(&[b"API_TOKEN=hun", b"ter2secretvalue\nnext line\n"]);
    assert!(!out.contains("hunter2secretvalue"), "{out}");
    assert!(!out.contains("hun"), "{out}");
    assert!(out.contains(MASK), "{out}");
    assert!(out.contains("next line"), "{out}");
}

#[test]
fn must_fail_bare_keyword_secret_with_no_prefix_is_masked() {
    let out = run_chunks(&[b"SECRET_KEY=abcdefgh1234\nok\n"]);
    assert!(!out.contains("abcdefgh1234"), "{out}");
    assert!(out.contains(MASK), "{out}");
    assert!(out.contains("ok"), "{out}");
}

#[test]
fn must_fail_bearer_secret_split_mid_token_is_masked() {
    let out = run_chunks(&[b"Authorization: Bear", b"er abcDEF123", b"456ghi\n"]);
    assert!(!out.contains("abcDEF123456ghi"), "{out}");
    assert!(!out.contains("abcDEF123"), "{out}");
    assert!(out.contains(MASK), "{out}");
}

#[test]
fn must_fail_private_key_body_spanning_chunks_is_masked() {
    let out = run_chunks(&[
        b"before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA1234",
        b"\nsecretbodyline2\n-----END RSA PRIVATE ",
        b"KEY-----\nafter\n",
    ]);
    assert!(!out.contains("MIIEowIBAAKCAQEA1234"), "{out}");
    assert!(!out.contains("secretbodyline2"), "{out}");
    assert!(out.contains("before") && out.contains("after"), "{out}");
}

#[test]
fn must_fail_unterminated_final_line_is_flushed_and_masked_on_finish() {
    let out = run_chunks(&[b"password=sw0rdfish"]);
    assert!(!out.contains("sw0rdfish"), "{out}");
    assert!(out.contains(MASK), "{out}");
}

#[test]
fn must_pass_benign_output_and_remote_markers_pass_through_unchanged() {
    let text = "hello world\nSYNAPSE_REMOTE_PROCESS_V1 job_id=abcdef0123456789abcdef0123456789abcd pid=4 pgid=4\nplain\n";
    assert_eq!(run_chunks(&[text.as_bytes()]), text);
    let (a, b) = text.as_bytes().split_at(17);
    assert_eq!(run_chunks(&[a, b]), text);
}

#[tokio::test]
async fn must_pass_pump_writes_redacted_bytes_to_artifact_file() {
    let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = temp.path().join("stdout.log");
    let file = std::fs::File::create(&path).unwrap_or_else(|e| panic!("create: {e}"));
    let (mut writer, reader) = tokio::io::duplex(64);
    let pump = tokio::spawn(pump_redacted(reader, file));
    writer
        .write_all(b"SECRET_KEY=abcd")
        .await
        .unwrap_or_else(|e| panic!("w1: {e}"));
    writer
        .write_all(b"efgh1234\nok\n")
        .await
        .unwrap_or_else(|e| panic!("w2: {e}"));
    drop(writer);
    let outcome = pump.await.unwrap_or_else(|e| panic!("pump: {e}"));
    assert!(outcome.errors.is_empty(), "{outcome:?}");
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read: {e}"));
    assert!(!on_disk.contains("abcdefgh1234"), "{on_disk}");
    assert!(
        on_disk.contains(MASK) && on_disk.contains("ok"),
        "{on_disk}"
    );
}

// Codex second review, finding 1: only exact lifecycle records bypass redaction.
#[test]
fn must_fail_credential_appended_to_remote_marker_line_is_masked() {
    let out = run_chunks(&[
        b"SYNAPSE_REMOTE_PROCESS_V1 job_id=job-1 pid=4 pgid=4 password=hunter2leak\n",
        b"SYNAPSE_REMOTE_EXIT_V1 job_id=job-1 pid=4 pgid=4 exit_code=0 Authorization: Bearer zzleakzz\n",
    ]);
    assert!(!out.contains("hunter2leak"), "{out}");
    assert!(!out.contains("zzleakzz"), "{out}");
    // An exact record still passes through, even after unterminated user output.
    let marker =
        "SYNAPSE_REMOTE_PROCESS_V1 job_id=abcdef0123456789abcdef0123456789abcd pid=4 pgid=4 sid=\n";
    let out = run_chunks(&[b"user-stderr-without-newline", marker.as_bytes()]);
    assert!(out.ends_with(marker), "{out}");
}

// Finding 2: an unterminated PEM block must stay masked at EOF and on overflow.
#[test]
fn must_fail_open_private_key_block_is_masked_at_eof_and_on_overflow() {
    let out = run_chunks(&[b"-----BEGIN RSA PRIVATE KEY-----\nMIIEsecretbody\nmore-secret-tail"]);
    assert!(!out.contains("MIIEsecretbody"), "{out}");
    assert!(!out.contains("more-secret-tail"), "{out}");
    assert!(
        out.contains("-----BEGIN RSA PRIVATE KEY-----") && out.contains(MASK),
        "{out}"
    );

    // > 4 MiB of key body with no newline forces an overflow flush.
    let body = "MIIsecret ".repeat(500_000);
    let mut redactor = StreamRedactor::new();
    let mut out = redactor.push(b"-----BEGIN PRIVATE KEY-----\n");
    out.extend(redactor.push(body.as_bytes()));
    let flushed = String::from_utf8_lossy(&out).into_owned();
    assert!(!flushed.contains("MIIsecret"), "overflow flushed key body");
    out.extend(redactor.push(b"\n-----END PRIVATE KEY-----\nafter\n"));
    out.extend(redactor.finish());
    let out = String::from_utf8_lossy(&out);
    assert!(!out.contains("MIIsecret"), "key body leaked after END");
    assert!(out.contains("-----END PRIVATE KEY-----") && out.contains("after"));
}

// Finding 3: credential-shaped names only; unquoted value is a single token.
#[test]
fn must_fail_env_secret_filter_matches_only_credential_names_and_single_token_values() {
    let out = super::redact_shell_output_for_public_readback(
        "TOKENS=42\nSECRETARY=Alice\nAPI_TOKEN=abc123 visible-after\nSECRET_KEY=k9z\nTOKEN=t7q\nGITHUB_TOKEN='q1 q2'\n",
    );
    assert!(out.contains("TOKENS=42"), "{out}");
    assert!(out.contains("SECRETARY=Alice"), "{out}");
    assert!(out.contains("visible-after"), "{out}");
    for secret in ["abc123", "k9z", "t7q", "q1 q2"] {
        assert!(!out.contains(secret), "leaked {secret}: {out}");
    }
}

// Finding 4: a pump still draining at the deadline is aborted and joined.
#[tokio::test]
async fn must_fail_unfinished_pump_is_aborted_and_joined_before_finalizing() {
    let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = temp.path().join("stdout.log");
    let file = std::fs::File::create(&path).unwrap_or_else(|e| panic!("create: {e}"));
    // The writer half stays open, like a pipe inherited by an orphan.
    let (mut writer, reader) = tokio::io::duplex(64);
    let pump = tokio::spawn(pump_redacted(reader, file));
    writer
        .write_all(b"early\n")
        .await
        .unwrap_or_else(|e| panic!("early: {e}"));
    let errors = finish_pumps(vec![pump], Duration::from_millis(200)).await;
    assert!(errors.iter().any(|e| e.contains("aborted")), "{errors:?}");
    // The joined pump dropped its reader: late output can no longer reach disk.
    assert!(writer.write_all(b"late\n").await.is_err());
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read: {e}"));
    assert!(
        on_disk.contains("early") && !on_disk.contains("late"),
        "{on_disk}"
    );
}

struct FailingReader {
    sent: bool,
}

impl AsyncRead for FailingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.sent {
            return Poll::Ready(Err(io::Error::other("pipe exploded")));
        }
        self.sent = true;
        buf.put_slice(b"line before failure\n");
        Poll::Ready(Ok(()))
    }
}

// Finding 5: read and write failures are reported, not swallowed.
#[tokio::test]
async fn must_fail_pump_reports_read_and_write_failures() {
    let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = temp.path().join("stderr.log");
    let file = std::fs::File::create(&path).unwrap_or_else(|e| panic!("create: {e}"));
    let outcome = pump_redacted(FailingReader { sent: false }, file).await;
    assert!(
        outcome.errors.iter().any(|e| e.contains("pipe exploded")),
        "{outcome:?}"
    );
    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read: {e}"));
    assert!(on_disk.contains("line before failure"), "{on_disk}");

    let read_only_path = temp.path().join("readonly.log");
    std::fs::write(&read_only_path, b"").unwrap_or_else(|e| panic!("seed: {e}"));
    let read_only = std::fs::File::open(&read_only_path).unwrap_or_else(|e| panic!("open: {e}"));
    let (mut writer, reader) = tokio::io::duplex(64);
    writer
        .write_all(b"data\n")
        .await
        .unwrap_or_else(|e| panic!("w: {e}"));
    drop(writer);
    let outcome = pump_redacted(reader, read_only).await;
    assert!(
        outcome.errors.iter().any(|e| e.contains("write failed")),
        "{outcome:?}"
    );
}

#[test]
fn must_fail_artifact_errors_are_persisted_in_terminal_status() {
    let mut status: super::ActRunShellJobStatus = serde_json::from_value(serde_json::json!({
        "schema_version": 1, "job_id": "job39", "status": "completed", "pid": null,
        "command": "cmd", "args": [], "command_line": "cmd", "working_dir": null,
        "env_keys": [], "timeout_ms": null, "started_at": "2026-10-14T00:00:00Z",
        "completed_at": null, "duration_ms": null, "exit_code": 0, "timed_out": false,
        "cancel_requested": false, "error_code": null, "error_message": null,
        "stdout_path": "o", "stderr_path": "e", "status_path": "s",
        "request_sha256": "x", "matched_pattern": "p"
    }))
    .unwrap_or_else(|e| panic!("status fixture: {e}"));
    let errors = vec!["artifact write failed: disk full".to_owned()];
    record_artifact_errors(&mut status, &errors);
    record_artifact_errors(&mut status, &errors);
    assert_eq!(
        status.error_code.as_deref(),
        Some(synapse_core::error_codes::STORAGE_WRITE_FAILED)
    );
    let message = status.error_message.clone().unwrap_or_default();
    assert_eq!(message.matches("disk full").count(), 1, "{message}");

    status.error_code = Some(synapse_core::error_codes::ACTION_BUDGET_EXPIRED.to_owned());
    status.error_message = Some("timeout cap expired".to_owned());
    record_artifact_errors(&mut status, &errors);
    assert_eq!(
        status.error_code.as_deref(),
        Some(synapse_core::error_codes::ACTION_BUDGET_EXPIRED)
    );
    let message = status.error_message.unwrap_or_default();
    assert!(
        message.contains("timeout cap expired") && message.contains("disk full"),
        "{message}"
    );
}

// Finding 6: non-UTF-8 bytes without secrets are preserved exactly.
#[test]
fn must_pass_non_utf8_artifact_bytes_are_preserved_exactly() {
    let input: &[u8] = b"bin \xff\xfe ok\nplain \xc3\x28 tail\n";
    assert_eq!(run_bytes(&[input]), input);
    let (a, b) = input.split_at(5);
    assert_eq!(run_bytes(&[a, b]), input);
    let leaky = run_bytes(&[b"\xff password=hunter2\n"]);
    assert!(!String::from_utf8_lossy(&leaky).contains("hunter2"));
}
