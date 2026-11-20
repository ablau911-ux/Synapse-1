//! Frozen behaviour table for #308 rows 308g and 308h: the respawn request rebuilt
//! from the prior spawn record, and its validation before anything is killed.
//!
//! Written by the manager from the behaviour table in issue #308 (work method v3).
//! Code agents must not edit this file. Deviation logged in #308: written after the
//! code; red evidence is the old build (260fbcd5 and earlier), which re-sent the
//! stored `model_ref` for claude/codex spawns and validated only after the kill
//! (reality check 2026-11-19 02:44: respawn left 0 agents on both builds).

use super::{RespawnManifest, respawn_request_from_record, validate_respawn_request};
use crate::m4::ActSpawnAgentRequest;

fn record(
    kind: &str,
    model: Option<&str>,
    model_ref: Option<&str>,
    working_dir: Option<&str>,
) -> RespawnManifest {
    RespawnManifest {
        agent_kind: kind.to_owned(),
        model: model.map(str::to_owned),
        model_ref: model_ref.map(str::to_owned),
        working_dir: working_dir.map(str::to_owned),
        source: "spawn-manifest.json".to_owned(),
    }
}

fn work_dir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

#[test]
fn respawn_request_from_record_table() {
    let dir = work_dir();
    // (row name, record, expected model, expected model_ref)
    let rows = [
        (
            "claude_with_model_drops_duplicate_model_ref",
            record(
                "claude",
                Some("claude-haiku-5-5"),
                Some("claude-haiku-5-5"),
                Some(&dir),
            ),
            Some("claude-haiku-5-5"),
            None,
        ),
        (
            "codex_with_model_drops_model_ref",
            record(
                "codex",
                Some("gpt-6.2-sol"),
                Some("gpt-6.2-sol"),
                Some(&dir),
            ),
            Some("gpt-6.2-sol"),
            None,
        ),
        (
            "claude_without_model",
            record("claude", None, None, Some(&dir)),
            None,
            None,
        ),
        (
            "local_model_keeps_model_ref",
            record("local_model", None, Some("my-local"), Some(&dir)),
            None,
            Some("my-local"),
        ),
        (
            "local_model_hyphen_form_keeps_model_ref",
            record("local-model", None, Some("my-local"), Some(&dir)),
            None,
            Some("my-local"),
        ),
    ];
    for (name, rec, model, model_ref) in rows {
        let built = respawn_request_from_record(&rec, "resume");
        let Ok(value) = built else {
            panic!("row {name}: request not built: {built:?}");
        };
        assert_eq!(
            value.get("model").and_then(|v| v.as_str()),
            model,
            "row {name}: model"
        );
        assert_eq!(
            value.get("model_ref").and_then(|v| v.as_str()),
            model_ref,
            "row {name}: model_ref"
        );
    }

    // structurally invalid: an unknown kind is refused, never guessed
    let unknown = respawn_request_from_record(&record("notepad", None, None, Some(&dir)), "resume");
    assert!(
        unknown
            .as_ref()
            .is_err_and(|e| e.message.contains("AGENT_RESPAWN_KIND_UNSUPPORTED")),
        "row unknown_kind: {unknown:?}"
    );
}

fn request(value: serde_json::Value) -> ActSpawnAgentRequest {
    match serde_json::from_value(value) {
        Ok(request) => request,
        Err(error) => panic!("table setup: request does not deserialize: {error}"),
    }
}

#[test]
fn respawn_request_validation_table() {
    let dir = work_dir();

    // the bug: a non-local kind with model_ref must be refused by validation (before any kill)
    let bad_ref = validate_respawn_request(&request(serde_json::json!({
        "cli": "claude", "prompt": "resume", "model": "claude-haiku-5-5",
        "model_ref": "claude-haiku-5-5", "working_dir": dir,
    })));
    assert!(bad_ref.is_err(), "row model_ref_on_claude_refused");

    // empty: no recorded working dir -> refused with the named code
    let no_dir = validate_respawn_request(&request(serde_json::json!({
        "cli": "claude", "prompt": "resume", "model": "claude-haiku-5-5",
    })));
    assert!(
        no_dir
            .as_ref()
            .is_err_and(|e| e.message.contains("AGENT_RESPAWN_WORKING_DIR_MISSING")),
        "row working_dir_missing: {:?}",
        no_dir.err()
    );

    // normal: the request rebuilt for a claude haiku record validates
    let rebuilt = respawn_request_from_record(
        &record(
            "claude",
            Some("claude-haiku-5-5"),
            Some("claude-haiku-5-5"),
            Some(&dir),
        ),
        "resume",
    );
    let Ok(rebuilt) = rebuilt else {
        panic!("row rebuilt_claude_valid: not built: {rebuilt:?}");
    };
    let valid = validate_respawn_request(&request(rebuilt));
    assert!(valid.is_ok(), "row rebuilt_claude_valid: {:?}", valid.err());
}
