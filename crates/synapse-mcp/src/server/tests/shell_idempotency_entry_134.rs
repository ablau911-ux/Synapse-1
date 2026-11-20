//! #134: a crashed daemon's `in_progress` idempotency reservation must be settled by the
//! real `run_shell_with_idempotency` entry (the body of `act_run_shell`), persisted as
//! `indeterminate`, and must never re-run the command.
//!
//! `shell_idempotency_restart.rs` composes the recovery/replay helpers directly; deleting
//! the caller's recovery step would leave it green. Here the orphan row is seeded into the
//! isolated service's CF_KV, the service is dropped and reopened on the same DB (restart),
//! the entry is called, and both side-effect sources of truth are
//! read back separately: the marker file the command appends to and the CF_KV row.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

const LIMIT_MS: u64 = 60_000;

struct Fixture {
    root: tempfile::TempDir,
    work: tempfile::TempDir,
    service: SynapseService,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("service root: {error}"));
        let service = SynapseService::try_new_isolated_for_tests(root.path())
            .unwrap_or_else(|error| panic!("isolated service: {error}"));
        let work = tempfile::tempdir().unwrap_or_else(|error| panic!("work dir: {error}"));
        Self {
            root,
            work,
            service,
        }
    }

    /// Simulated crash + restart: the old daemon generation (and its DB handle) goes
    /// away without completing the row, and a new generation reopens the same RocksDB.
    fn restart(self) -> Self {
        let Self {
            root,
            work,
            service,
        } = self;
        drop(service);
        let service = SynapseService::try_new_isolated_for_tests(root.path())
            .unwrap_or_else(|error| panic!("reopen after restart: {error}"));
        Self {
            root,
            work,
            service,
        }
    }

    fn marker(&self) -> PathBuf {
        self.work.path().join("marker134.txt")
    }

    fn marker_lines(&self) -> usize {
        std::fs::read_to_string(self.marker()).map_or(0, |text| text.lines().count())
    }

    fn params(&self, key: &str, line: &str) -> ActRunShellParams {
        let tail = format!("echo {line}>>\"{}\"", self.marker().display());
        serde_json::from_value(json!({
            "command": "cmd.exe",
            "args": ["/d", "/c", tail],
            "timeout_ms": 10_000,
            "idempotency_key": key,
        }))
        .unwrap_or_else(|error| panic!("params: {error}"))
    }

    fn row(&self, params: &ActRunShellParams) -> Option<Value> {
        let key = row_key(params);
        let runtime = self
            .service
            .reflex_runtime()
            .unwrap_or_else(|e| panic!("{e:?}"));
        let runtime = runtime.lock().unwrap_or_else(|_| panic!("runtime lock"));
        let bytes = runtime
            .storage_kv_row(&key)
            .unwrap_or_else(|e| panic!("{e:?}"))?;
        Some(decode_json(&bytes).unwrap_or_else(|e| panic!("decode row: {e}")))
    }

    fn seed_orphan(&self, params: &ActRunShellParams) {
        let reservation = run_shell_idempotency_reservation_row(
            params,
            &authorization(params),
            Some("issue134-crashed-session"),
            "issue134-crashed-daemon",
        )
        .unwrap_or_else(|error| panic!("reservation: {error:?}"));
        let runtime = self
            .service
            .reflex_runtime()
            .unwrap_or_else(|e| panic!("{e:?}"));
        let runtime = runtime.lock().unwrap_or_else(|_| panic!("runtime lock"));
        runtime
            .storage_put_kv_rows(vec![(row_key(params), reservation)])
            .unwrap_or_else(|error| panic!("seed: {error:?}"));
    }

    async fn call(&self, params: ActRunShellParams) -> Result<ActRunShellResponse, ErrorData> {
        let auth = authorization(&params);
        run_shell_with_idempotency(&self.service, params, auth, LIMIT_MS, None).await
    }
}

fn row_key(params: &ActRunShellParams) -> Vec<u8> {
    run_shell_idempotency_row_key(params, None)
        .unwrap_or_else(|error| panic!("row key: {error:?}"))
        .unwrap_or_else(|| panic!("key present"))
}

fn authorization(params: &ActRunShellParams) -> RunShellAuthorization {
    let command_line = crate::m4::shell_command_line_from_parts(&params.command, &params.args);
    let config = crate::m4::M4ServiceConfig::from_cli_parts(
        vec![format!("^{}$", regex::escape(&command_line))],
        Vec::new(),
        LIMIT_MS,
    )
    .unwrap_or_else(|error| panic!("explicit fixture policy: {error}"));
    authorize_run_shell(&config, params)
        .unwrap_or_else(|error| panic!("fixture policy must authorize: {error:?}"))
}

fn status_of(row: Option<Value>) -> Option<String> {
    row?.get("status")?.as_str().map(ToOwned::to_owned)
}

fn reason(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("reason")?.as_str()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_orphan_reservation_settles_indeterminate_without_rerun_134() {
    let fixture = Fixture::new();
    let params = fixture.params("issue134-orphan", "ran");
    fixture.seed_orphan(&params);
    let fixture = fixture.restart();
    assert_eq!(
        status_of(fixture.row(&params)).as_deref(),
        Some("in_progress"),
        "precondition: the crashed generation's reservation survived the restart"
    );
    assert_eq!(fixture.marker_lines(), 0, "precondition: never executed");

    for attempt in 0..2 {
        let error = fixture.call(params.clone()).await.err();
        let error = error.unwrap_or_else(|| panic!("attempt {attempt} must not execute"));
        assert_eq!(
            reason(&error),
            Some("idempotency_outcome_indeterminate"),
            "{error:?}"
        );
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|data| data.get("automatic_retry_safe")),
            Some(&json!(false))
        );
    }

    assert_eq!(
        fixture.marker_lines(),
        0,
        "an orphan must never re-run the command"
    );
    let row = fixture
        .row(&params)
        .unwrap_or_else(|| panic!("row must persist"));
    assert_eq!(row["status"], "indeterminate", "{row}");
    assert!(row["completed_at"].is_string(), "{row}");
    assert!(row.get("response").is_none_or(Value::is_null), "{row}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_fresh_key_executes_once_then_replays_134() {
    let fixture = Fixture::new();
    let params = fixture.params("issue134-fresh", "once");
    assert!(fixture.row(&params).is_none(), "precondition: unused key");

    let first = fixture
        .call(params.clone())
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
    let second = fixture
        .call(params.clone())
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));

    assert_eq!(first.exit_code, Some(0));
    assert_eq!(fixture.marker_lines(), 1, "the command runs exactly once");
    assert_eq!(second.exit_code, first.exit_code);
    assert_eq!(second.stdout, first.stdout);
    assert_eq!(status_of(fixture.row(&params)).as_deref(), Some("ok"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_same_key_changed_parameters_conflict_without_execution_134() {
    let fixture = Fixture::new();
    let original = fixture.params("issue134-conflict", "first");
    fixture
        .call(original)
        .await
        .unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(fixture.marker_lines(), 1, "precondition: original ran once");

    let changed = fixture.params("issue134-conflict", "second");
    let error = fixture
        .call(changed)
        .await
        .err()
        .unwrap_or_else(|| panic!("must refuse"));

    assert_eq!(
        reason(&error),
        Some("idempotency_key_conflict"),
        "{error:?}"
    );
    assert_eq!(
        fixture.marker_lines(),
        1,
        "a conflicting request must not execute"
    );
}
