//! Frozen behaviour table for #308 rows 308l and 308m: respawn keeps every recorded
//! spawn setting, and a spawned agent's MCP url is its own daemon's bind.
//!
//! Written by the manager from the behaviour table in issue #308 (work method v3).
//! Code agents must not edit this file. Deviation logged in #308: written after the
//! code; red evidence is the reality run on 1cc289c9 (replacement lost mcp_url and
//! require_approval_gate, dialled the fixed default :7700 and died with 401).

use serde_json::json;

use super::{RespawnManifest, respawn_request, respawn_settings_from_manifest};
use crate::m4::agent_mcp_url;

fn identity() -> RespawnManifest {
    RespawnManifest {
        agent_kind: "claude".to_owned(),
        model: Some("claude-haiku-5-5".to_owned()),
        model_ref: None,
        working_dir: Some("C:/work".to_owned()),
        source: "spawn-manifest.json".to_owned(),
    }
}

#[test]
fn agent_mcp_url_table() {
    // normal: no requested url -> this daemon's own bind
    assert_eq!(
        agent_mcp_url(None, "127.0.0.1:7753"),
        "http://127.0.0.1:7753/mcp",
        "row own_bind_candidate"
    );
    // the live bind gives the live url, and only then
    assert_eq!(
        agent_mcp_url(None, "127.0.0.1:7700"),
        "http://127.0.0.1:7700/mcp",
        "row own_bind_live"
    );
    // a candidate never hands out the live port
    assert!(
        !agent_mcp_url(None, "127.0.0.1:7754").contains("7700"),
        "row candidate_never_7700"
    );
    // an explicit url from the caller is kept as given
    assert_eq!(
        agent_mcp_url(Some("http://127.0.0.1:7999/mcp"), "127.0.0.1:7753"),
        "http://127.0.0.1:7999/mcp",
        "row requested_url_kept"
    );
}

#[test]
fn respawn_keeps_recorded_settings_table() {
    // normal: every recorded setting is carried unchanged; only the prompt is new
    let manifest = json!({
        "mcp_url": "http://127.0.0.1:7753/mcp",
        "require_approval_gate": false,
        "wait_timeout_ms": 45_000,
        "hold_open_ms": 1_500,
    });
    let settings = respawn_settings_from_manifest(&manifest);
    let built = respawn_request(&identity(), &settings, "resume here");
    let Ok(request) = built else {
        panic!("row all_settings_kept: not built: {built:?}");
    };
    assert_eq!(
        request["mcp_url"],
        json!("http://127.0.0.1:7753/mcp"),
        "row all_settings_kept: mcp_url"
    );
    assert_eq!(
        request["require_approval_gate"],
        json!(false),
        "row all_settings_kept: approval gate"
    );
    assert_eq!(
        request["wait_timeout_ms"],
        json!(45_000),
        "row all_settings_kept: wait"
    );
    assert_eq!(
        request["hold_open_ms"],
        json!(1_500),
        "row all_settings_kept: hold"
    );
    assert_eq!(
        request["model"],
        json!("claude-haiku-5-5"),
        "row all_settings_kept: model"
    );
    assert_eq!(
        request["prompt"],
        json!("resume here"),
        "row all_settings_kept: prompt"
    );

    // the approval gate recorded as true stays true
    let gated = respawn_settings_from_manifest(&json!({"require_approval_gate": true}));
    let built = respawn_request(&identity(), &gated, "p");
    assert!(
        built
            .as_ref()
            .is_ok_and(|r| r["require_approval_gate"] == json!(true)),
        "row gate_true_kept: {built:?}"
    );

    // empty / old record: nothing recorded -> no url invented (the daemon bind is used later)
    let old = respawn_settings_from_manifest(&json!({}));
    let built = respawn_request(&identity(), &old, "p");
    assert!(
        built.as_ref().is_ok_and(|r| r.get("mcp_url").is_none()),
        "row old_record_no_url_invented: {built:?}"
    );

    // boundary: a blank recorded url counts as not recorded
    let blank = respawn_settings_from_manifest(&json!({"mcp_url": "   "}));
    assert_eq!(blank.mcp_url, None, "row blank_url_not_recorded");

    // structurally invalid: wrong types are ignored, never guessed
    let wrong = respawn_settings_from_manifest(
        &json!({"require_approval_gate": "no", "wait_timeout_ms": "soon"}),
    );
    assert_eq!(wrong.require_approval_gate, None, "row wrong_type_gate");
    assert_eq!(wrong.wait_timeout_ms, None, "row wrong_type_wait");
}
