//! #25: the real `act_spawn_agent_impl` entry point refuses an unknown Codex model with
//! `TOOL_PARAMS_INVALID` before any spawn artifact or child process exists.
//!
//! The existing `model_preflight_25` tests drive `validate_codex_model_with_sources`
//! only; deleting the call at the entry would leave them green. These tests go through
//! the service method. `CODEX_HOME` / `OPENAI_BASE_URL` are process-global, so they are
//! pinned under `crate::test_support::host_env_serial()` and restored on drop.

use std::time::{Duration, Instant};

use super::*;

const KNOWN_MODEL: &str = "gpt-issue25-known";

struct CodexEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    _home: tempfile::TempDir,
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl CodexEnv {
    fn pin(lock: std::sync::MutexGuard<'static, ()>) -> Self {
        let home = tempfile::tempdir().unwrap_or_else(|error| panic!("codex home: {error}"));
        let cache = format!(r#"{{"models":[{{"slug":"{KNOWN_MODEL}"}}]}}"#);
        std::fs::write(home.path().join("models_cache.json"), cache)
            .unwrap_or_else(|error| panic!("write cache: {error}"));
        let saved = ["CODEX_HOME", "OPENAI_BASE_URL"]
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect();
        // SAFETY: std documents set_var/remove_var as always safe on Windows; the lock
        // serializes host-environment fixtures that read or mutate these variables.
        unsafe {
            std::env::set_var("CODEX_HOME", home.path());
            std::env::remove_var("OPENAI_BASE_URL");
        }
        Self {
            _lock: lock,
            _home: home,
            saved,
        }
    }
}

impl Drop for CodexEnv {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..) {
            // SAFETY: see `CodexEnv::pin`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// A bind that is never the live daemon's port; nothing is launched.
const TEST_DAEMON_BIND: &str = "127.0.0.1:47025";

/// Since #308 the entry takes an already resolved `mcp_url`; it is resolved
/// by the production rule against a test bind, so the raw model still reaches
/// the entry validator under test.
fn spawn_params(model: Option<&str>) -> ActSpawnAgentParams {
    let mut value = json!({
        "cli": "codex",
        "prompt": "never-launch-test",
        "mcp_url": crate::m4::agent_mcp_url(None, TEST_DAEMON_BIND),
    });
    if let Some(model) = model {
        value["model"] = json!(model);
    }
    serde_json::from_value(value).unwrap_or_else(|error| panic!("spawn params: {error}"))
}

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn spawn_dir(spawn_id: &str) -> PathBuf {
    agent_spawn_root_dir()
        .unwrap_or_else(|error| panic!("spawn root: {error:?}"))
        .join(spawn_id)
}

// Shared host environment lock intentionally spans await through RAII restoration.
#[allow(clippy::await_holding_lock)]
async fn refuse_at_entry(model: &str, spawn_id: &str) -> ErrorData {
    let _env = CodexEnv::pin(crate::test_support::host_env_serial());
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("service root: {error}"));
    let service = SynapseService::try_new_isolated_for_tests(root.path())
        .unwrap_or_else(|error| panic!("isolated service: {error}"));
    let dir = spawn_dir(spawn_id);
    assert!(!dir.exists(), "precondition: no spawn dir for {spawn_id}");

    let started = Instant::now();
    let result = service
        .act_spawn_agent_impl(
            spawn_params(Some(model)),
            Some("issue25-session".to_owned()),
            spawn_id.to_owned(),
        )
        .await;
    let elapsed = started.elapsed();

    assert!(
        !dir.exists(),
        "a refused spawn must not create {}",
        dir.display()
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "refusal must be immediate: {elapsed:?}"
    );
    match result {
        Ok(_response) => panic!("unknown model {model:?} must be refused"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn must_fail_entry_refuses_unknown_codex_model_without_artifacts_25() {
    let spawn_id = format!("agent-spawn-issue25-unknown-{}", std::process::id());
    let error = refuse_at_entry("definitely-not-a-model", &spawn_id).await;

    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(
        error.message.contains("before creating spawn artifacts"),
        "{error:?}"
    );
}

#[tokio::test]
async fn must_fail_entry_refuses_whitespace_model_without_artifacts_25() {
    let spawn_id = format!("agent-spawn-issue25-shape-{}", std::process::id());
    let error = refuse_at_entry("bad model", &spawn_id).await;

    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
}

#[test]
fn must_pass_entry_validator_accepts_catalog_and_omitted_codex_models_25() {
    let _env = CodexEnv::pin(crate::test_support::host_env_serial());
    let known = spawn_params(Some(KNOWN_MODEL));
    let omitted = spawn_params(None);

    assert_eq!(
        validate_cli_model_before_spawn(&known, ActSpawnAgentCli::Codex).map_err(|e| e.message),
        Ok(())
    );
    assert_eq!(
        validate_cli_model_before_spawn(&omitted, ActSpawnAgentCli::Codex).map_err(|e| e.message),
        Ok(())
    );
}
