//! Streaming redaction for durable shell-job stdout/stderr artifacts (#39b).
//!
//! The child's pipes are drained here and written to `stdout.log`/`stderr.log`
//! after passing through the same filter used for public readback
//! (`redact_shell_output_for_public_readback`), so secrets never reach disk.
//! Output is processed line by line on raw bytes: a secret split across read
//! chunks is still masked, an open PEM private-key block stays masked until its
//! `END` line (also across EOF and overflow flushes), only exact
//! `SYNAPSE_REMOTE_*` lifecycle records bypass the filter, and bytes the filter
//! does not touch are persisted exactly (non-UTF-8 output is not re-encoded).

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::sync::LazyLock;
use std::time::Duration;

use regex::bytes::Regex;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::task::JoinHandle;

use super::{ActRunShellJobStatus, error_codes, redact_shell_output_for_public_readback};

const MASK_LINE: &[u8] = b"\n[redacted-shell-output]\n";
const FILTER_UNAVAILABLE: &[u8] = b"[redacted-shell-output:filter-unavailable]\n";
/// Hard cap on held-back bytes so an endless line cannot grow memory unbounded.
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
/// Bytes kept back on an overflow flush so PEM armor is never split.
const OVERFLOW_TAIL_BYTES: usize = 128;
/// Window searched for whitespace so an overflow flush does not split a token.
const OVERFLOW_TOKEN_WINDOW_BYTES: usize = 4096;

struct Patterns {
    /// Exact lifecycle record emitted by the SSH tracker (`m4.rs` marker
    /// constants): known marker, known keys, per-key value grammar, then EOL.
    lifecycle_record: Regex,
    key_begin: Regex,
    key_end: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        lifecycle_record: Regex::new(
            r"(?-u)SYNAPSE_REMOTE_(?:PROCESS|EXIT|CLEANUP|LIVENESS)_V1(?: (?:job_id=[A-Za-z0-9_-]{1,128}|(?:pid|pgid|sid)=[0-9]{0,10}|exit_code=-?[0-9]{1,10}|(?:status|error)=[a-z_]{1,64}))*\r?\n?\z",
        )
        .ok()?,
        key_begin: Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").ok()?,
        key_end: Regex::new(r"-----END [A-Z0-9 ]*PRIVATE KEY-----").ok()?,
    })
});

#[derive(Default)]
pub(super) struct StreamRedactor {
    /// Raw bytes of the current incomplete line.
    pending: Vec<u8>,
    /// Ordinary text waiting to be filtered as one batch.
    text: Vec<u8>,
    /// Redacted bytes ready to persist.
    out: Vec<u8>,
    /// Inside a `BEGIN ... PRIVATE KEY` block whose `END` was not seen yet.
    in_private_key: bool,
}

impl StreamRedactor {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns the redacted bytes that are safe to persist now.
    pub(super) fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let complete = self
            .pending
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let cut = if complete == 0 && self.pending.len() > MAX_PENDING_BYTES {
            overflow_cut(&self.pending)
        } else {
            complete
        };
        if cut > 0 {
            let ready: Vec<u8> = self.pending.drain(..cut).collect();
            for line in ready.split_inclusive(|byte| *byte == b'\n') {
                self.line(line);
            }
        }
        self.take_output()
    }

    /// Flush whatever is still held at end of stream. An unterminated private
    /// key block stays masked.
    pub(super) fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            self.line(&rest);
        }
        self.take_output()
    }

    fn take_output(&mut self) -> Vec<u8> {
        self.flush_text();
        std::mem::take(&mut self.out)
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.out.extend(redact_bytes(&text));
        }
    }

    fn emit_verbatim(&mut self, bytes: &[u8]) {
        self.flush_text();
        self.out.extend_from_slice(bytes);
    }

    /// One complete line, or a partial one on overflow / end of stream.
    fn line(&mut self, line: &[u8]) {
        let Some(patterns) = PATTERNS.as_ref() else {
            self.text.clear();
            if !self.out.ends_with(FILTER_UNAVAILABLE) {
                self.out.extend_from_slice(FILTER_UNAVAILABLE);
            }
            return;
        };
        let mut rest = line;
        if self.in_private_key {
            // Key body is dropped until the END armor line.
            let Some(end) = patterns.key_end.find(line) else {
                return;
            };
            self.in_private_key = false;
            self.emit_verbatim(end.as_bytes());
            rest = &line[end.end()..];
        }
        if let Some(begin) = patterns.key_begin.find_iter(rest).last()
            && patterns.key_end.find_at(rest, begin.end()).is_none()
        {
            self.text.extend_from_slice(&rest[..begin.start()]);
            self.emit_verbatim(begin.as_bytes());
            self.out.extend_from_slice(MASK_LINE);
            self.in_private_key = true;
            return;
        }
        if let Some(record) = patterns.lifecycle_record.find(rest) {
            self.text.extend_from_slice(&rest[..record.start()]);
            self.emit_verbatim(record.as_bytes());
            return;
        }
        self.text.extend_from_slice(rest);
    }
}

/// Where to flush an over-long line: keep a tail back, prefer a whitespace
/// boundary, and never split a PEM armor line.
fn overflow_cut(pending: &[u8]) -> usize {
    let mut cut = pending.len().saturating_sub(OVERFLOW_TAIL_BYTES);
    let window = cut.saturating_sub(OVERFLOW_TOKEN_WINDOW_BYTES);
    if let Some(space) = pending[window..cut]
        .iter()
        .rposition(|byte| matches!(byte, b' ' | b'\t'))
    {
        cut = window + space + 1;
    }
    let armor_window = cut.saturating_sub(OVERFLOW_TAIL_BYTES);
    let armor = [b"-----BEGIN ".as_slice(), b"-----END ".as_slice()]
        .into_iter()
        .filter_map(|needle| rfind(&pending[armor_window..cut], needle))
        .max();
    if let Some(start) = armor.map(|offset| armor_window + offset)
        && find(&pending[start + 5..cut], b"-----").is_none()
    {
        cut = start;
    }
    if cut == 0 { pending.len() } else { cut }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// Filter output of the shared text filter, or `None` when it leaves the
/// (lossily decoded) text untouched.
fn filter_changes(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(bytes);
    let redacted = redact_shell_output_for_public_readback(&text);
    (redacted != text).then(|| redacted.into_bytes())
}

/// Redact ordinary text. Bytes are only decoded for matching: anything the
/// filter does not change is persisted exactly, including non-UTF-8 lines.
fn redact_bytes(bytes: &[u8]) -> Vec<u8> {
    let Some(redacted) = filter_changes(bytes) else {
        return bytes.to_vec();
    };
    if std::str::from_utf8(bytes).is_ok() {
        return redacted;
    }
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .flat_map(|line| filter_changes(line).unwrap_or_else(|| line.to_vec()))
        .collect()
}

/// What a pump observed besides the bytes it persisted.
#[derive(Debug, Default)]
pub(super) struct PumpOutcome {
    pub(super) errors: Vec<String>,
}

pub(super) type PumpHandle = JoinHandle<PumpOutcome>;

/// Drain `reader` into `file`, redacting as it goes. Keeps draining after a
/// write failure so the child never blocks on a full pipe. Writes are
/// synchronous (small appends to a local file) so an aborted pump cannot land
/// bytes after it has been joined.
pub(super) async fn pump_redacted<R: AsyncRead + Unpin>(
    mut reader: R,
    mut file: File,
) -> PumpOutcome {
    let mut redactor = StreamRedactor::new();
    let mut buffer = vec![0_u8; 8192];
    let mut outcome = PumpOutcome::default();
    let mut write_error = None;
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => {
                let out = redactor.push(&buffer[..read]);
                write_artifact(&mut file, &out, &mut write_error);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // A closed anonymous pipe can surface as BrokenPipe on Windows: EOF.
            Err(error) if error.kind() == ErrorKind::BrokenPipe => break,
            Err(error) => {
                tracing::error!(
                    code = "M4_SHELL_JOB_ARTIFACT_PIPE_READ_FAILED",
                    error = %error,
                    "shell job output pipe read failed; artifact is incomplete"
                );
                outcome.errors.push(format!("pipe read failed: {error}"));
                break;
            }
        }
    }
    let tail = redactor.finish();
    write_artifact(&mut file, &tail, &mut write_error);
    outcome.errors.extend(write_error);
    outcome
}

fn write_artifact(file: &mut File, bytes: &[u8], error: &mut Option<String>) {
    if error.is_some() || bytes.is_empty() {
        return;
    }
    if let Err(source) = file.write_all(bytes).and_then(|()| file.flush()) {
        tracing::error!(
            code = "M4_SHELL_JOB_REDACTED_ARTIFACT_WRITE_FAILED",
            error = %source,
            "shell job artifact write failed; discarding further output"
        );
        *error = Some(format!("artifact write failed: {source}"));
    }
}

/// Join the pumps, waiting at most `grace` overall. A pump still draining (its
/// pipe inherited by a surviving process) is aborted and joined so it can never
/// write after the terminal status. Returns the artifact errors to persist.
pub(super) async fn finish_pumps(pumps: Vec<PumpHandle>, grace: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + grace;
    let mut errors = Vec::new();
    for mut pump in pumps {
        let joined = match tokio::time::timeout_at(deadline, &mut pump).await {
            Ok(joined) => joined,
            Err(_elapsed) => {
                pump.abort();
                errors.push(format!(
                    "output pump still draining after {} ms (pipe held by a surviving process); aborted, artifact may be truncated",
                    grace.as_millis()
                ));
                pump.await
            }
        };
        match joined {
            Ok(outcome) => errors.extend(outcome.errors),
            Err(join_error) if join_error.is_cancelled() => {}
            Err(join_error) => errors.push(format!("output pump task failed: {join_error}")),
        }
    }
    errors
}

/// Persist artifact capture failures in the job status. Keeps an existing
/// terminal error code and is idempotent across repeated status writes.
pub(super) fn record_artifact_errors(status: &mut ActRunShellJobStatus, errors: &[String]) {
    if errors.is_empty() {
        return;
    }
    let detail = format!(
        "durable stdout/stderr artifact capture incomplete: {}",
        errors.join("; ")
    );
    if status
        .error_message
        .as_deref()
        .is_some_and(|message| message.contains(&detail))
    {
        return;
    }
    tracing::warn!(
        code = "M4_SHELL_JOB_ARTIFACT_INCOMPLETE",
        job_id = %status.job_id,
        detail = %detail,
        "readback=act_run_shell_status artifact capture incomplete"
    );
    if status.error_code.is_none() {
        status.error_code = Some(error_codes::STORAGE_WRITE_FAILED.to_owned());
    }
    match status.error_message.as_mut() {
        Some(message) => {
            message.push_str("; ");
            message.push_str(&detail);
        }
        None => status.error_message = Some(detail),
    }
}
