//! #8: the public `act` facade may delegate `verb=focus_window` to its hidden
//! implementation tool `act_focus_window` only for a foreground-capable
//! profile (break_glass / full_capability); the hidden raw tool stays absent
//! from `tools/list`, and a normal profile is refused before delegation.
//!
//! Contract: issue #8 "Expected" + fix comment; server/background_router.rs
//! `target_act` verb=focus_window (`allows_foreground_tier()` check, message
//! "act focus_window requires break_glass or full_capability"), then
//! `target_act_focus_window_missing_target_error` (`TARGET_NOT_SET`).
//!
//! No call here can reach a real activation: the admitted session has no
//! target, so the delegate route stops at its target check.

#![cfg(windows)]

mod g4_support;
#[allow(dead_code)]
mod support;

use g4_support::{
    act_invoke_refused, full_capability_session, lease_status, nested_code, normal_session,
    synthetic_daemon,
};
use serde_json::{Value, json};
use support::HttpMcpSession;
use synapse_core::error_codes;

async fn listed_tool_names(session: &mut HttpMcpSession) -> anyhow::Result<Vec<String>> {
    let list = session.tools_list().await?;
    Ok(list["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn focus_window() -> Value {
    json!({"verb": "focus_window"})
}

#[tokio::test]
async fn must_fail_normal_agent_focus_window_is_denied_before_delegation() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = normal_session(&daemon, "g4-8-normal").await?;

    let structured = act_invoke_refused(&mut session, focus_window()).await?;

    assert_eq!(structured["action"]["ok"], false, "{structured}");
    assert_eq!(
        structured["action"]["delegated_tool"], "act_focus_window",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::TOOL_PROFILE_POLICY_DENIED),
        "{structured}"
    );
    assert_eq!(
        structured["action"]["result"]["error"]["message"],
        "act focus_window requires break_glass or full_capability",
        "{structured}"
    );
    assert_eq!(
        lease_status(&mut session).await?["held"],
        false,
        "no lease side effect"
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// A foreground-capable profile passes the profile gate: the refusal now comes
/// from the delegate route's own target check, not TOOL_PROFILE_POLICY_DENIED.
#[tokio::test]
async fn must_fail_full_capability_focus_without_target_is_refused_by_target_check()
-> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut session = full_capability_session(&daemon).await?;

    let structured = act_invoke_refused(&mut session, focus_window()).await?;

    assert_eq!(
        structured["action"]["delegated_tool"], "act_focus_window",
        "{structured}"
    );
    assert_eq!(
        nested_code(&structured).as_deref(),
        Some(error_codes::TARGET_NOT_SET),
        "{structured}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

/// Must NOT change: the hidden implementation tool is never listed, for any profile.
#[tokio::test]
async fn must_pass_hidden_act_focus_window_stays_off_the_public_surface() -> anyhow::Result<()> {
    let daemon = synthetic_daemon().await?;
    let mut normal = normal_session(&daemon, "g4-8-list-normal").await?;
    let mut full = full_capability_session(&daemon).await?;

    for names in [
        listed_tool_names(&mut normal).await?,
        listed_tool_names(&mut full).await?,
    ] {
        assert!(
            names.iter().any(|name| name == "act"),
            "act facade listed: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name == "act_focus_window"),
            "hidden delegate must not be listed: {names:?}"
        );
    }
    drop(daemon.stop().await?);
    Ok(())
}
