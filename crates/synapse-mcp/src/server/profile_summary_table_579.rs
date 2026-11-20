//! #579 frozen behaviour table: `profile` replies carry a short summary of the
//! session's profile, not full snapshots. Written by the manager from the
//! table in `merge/jobs/night-1120/today/579/table.md`; coders never edit it.
//! One case per row (P1-P5). Pure: every snapshot is built here from the
//! profile code's own pure helpers; no storage, OS, network or clock.

use super::*;

/// P1/P3: a status reply must fit in this many bytes (today about 10 KB).
const STATUS_REPLY_BUDGET_BYTES: usize = 2_000;
/// P4: a set reply, with the longest allowed reason, must fit in this many
/// bytes (today about 22 KB: two full snapshots).
const SET_REPLY_BUDGET_BYTES: usize = 6_000;
const SESSION_A: &str = "57900000-0000-4000-8000-000000000579";
/// The only keys a summary may carry (P2).
const SUMMARY_KEYS: [&str; 8] = [
    "foreground_route",
    "hidden_tool_route_policy",
    "profile",
    "profile_label",
    "session_id",
    "source",
    "visible_tool_count",
    "visible_tool_sha256",
];

fn failed(error: &ErrorData) -> anyhow::Error {
    anyhow::anyhow!("profile helper failed: {}", error.message)
}

/// Every tool the daemon registers, as the snapshot code sees it: the public
/// facades plus the hidden implementation tools that get route rows.
fn full_tool_names() -> Vec<String> {
    PUBLIC_TOOL_NAMES
        .iter()
        .chain(BREAK_GLASS_HAZARDOUS_TOOLS)
        .chain(BROWSER_DEBUGGER_ONLY_EXACT)
        .chain(BREAK_GLASS_DENIED_EXEC_TOOLS)
        .map(|name| (*name).to_owned())
        .collect()
}

fn foreground_route(holds_lease: bool, profile_allows: bool) -> ToolProfileForegroundRoute {
    let mut remaining_steps = Vec::new();
    if !holds_lease {
        remaining_steps.push(
            "act operation=lease_acquire (this session must own the foreground input lease)"
                .to_owned(),
        );
    }
    if !profile_allows {
        remaining_steps.push(
            "profile operation=set profile=break_glass confirm_break_glass=true reason=<why> (requires the lease first)"
                .to_owned(),
        );
    }
    ToolProfileForegroundRoute {
        holds_foreground_lease: holds_lease,
        profile_allows_foreground: profile_allows,
        foreground_route_ready: holds_lease && profile_allows,
        remaining_steps,
    }
}

fn profile_row(
    session_id: &str,
    profile: ToolProfileKind,
    source: &str,
    reason: Option<String>,
    visible: &[String],
) -> anyhow::Result<ToolProfileRowReadback> {
    Ok(ToolProfileRowReadback {
        cf_name: "sessions",
        // Hex of "mcp/tool-profile/v1/<session id>": 56 bytes.
        key_hex: "6d".repeat(56),
        value_len_bytes: 1_234,
        value_sha256: "b".repeat(64),
        record: ToolProfileAssignment {
            schema_version: 1,
            row_kind: "mcp_tool_profile".to_owned(),
            session_id: session_id.to_owned(),
            profile,
            source: source.to_owned(),
            reason,
            set_by_session_id: Some(session_id.to_owned()),
            stored_at_unix_ms: 1_763_600_000_000,
            allowed_tool_count: visible.len(),
            allowed_tool_sha256: sha256_json_hex(&visible).map_err(|error| failed(&error))?,
            denied_break_glass_tools: denied_break_glass_tools(visible),
        },
    })
}

/// A full snapshot as `tool_profile_snapshot` builds it, with a fixed Codex
/// client surface (the real one reads files) and the given lease state.
fn snapshot(
    session_id: Option<&str>,
    profile: ToolProfileKind,
    source: &str,
    route: ToolProfileForegroundRoute,
) -> anyhow::Result<ToolProfileSnapshot> {
    let full = full_tool_names();
    let public_tool_registry =
        public_tool_registry_snapshot_for(&full).map_err(|error| failed(&error))?;
    let facade_contract = facade_contract_snapshot_for(&public_tool_registry.public_tool_names)
        .map_err(|error| failed(&error))?;
    let visible = if session_id.is_some() {
        visible_tool_names_for_profile(profile, &full)
    } else {
        full.clone()
    };
    let policy_row = session_id
        .map(|session_id| profile_row(session_id, profile, source, None, &visible))
        .transpose()?;
    let codex_client_surface = CodexClientSurfaceSnapshot {
        source_of_truth: "%APPDATA%\\synapse\\codex-tool-surface.json",
        status: CodexClientSurfaceStatus::HostSnapshotMissing,
        diagnostic_code: "CODEX_CLIENT_SURFACE_HOST_SNAPSHOT_MISSING",
        remediation: "rerun scripts\\synapse-setup.ps1",
        host_snapshot: CodexToolSurfaceSnapshotReadback {
            path: "C:\\synapse-test\\codex-tool-surface.json".to_owned(),
            exists: false,
            len_bytes: None,
            sha256: None,
            read_error: None,
            tool_count: None,
            tool_surface_sha256: None,
            tool_names: Vec::new(),
        },
        latest_restart_handoff: None,
        live_stale_codex_process: None,
        public_tools_missing_from_host_snapshot: public_tool_registry.public_tool_names.clone(),
        host_snapshot_tools_missing_from_public_registry: Vec::new(),
    };
    Ok(ToolProfileSnapshot {
        source_of_truth: TOOL_PROFILE_SOURCE_OF_TRUTH,
        session_id: session_id.map(ToOwned::to_owned),
        profile,
        profile_label: profile.label(),
        source: source.to_owned(),
        implementation_tool_count: full.len(),
        visible_tool_count: visible.len(),
        visible_tool_sha256: sha256_json_hex(&visible).map_err(|error| failed(&error))?,
        denied_break_glass_tools: denied_break_glass_tools(&visible),
        foreground_capability: foreground_capability_policy(profile),
        hidden_tool_routes: hidden_tool_capability_routes(&visible),
        hidden_tool_route_policy: HIDDEN_TOOL_ROUTE_POLICY,
        public_tool_registry,
        facade_contract,
        codex_client_surface,
        foreground_route: route,
        policy_row,
        visible_tool_names: visible,
    })
}

fn status_reply(snapshot: &ToolProfileSnapshot) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::to_value(ProfileResponse {
        operation: ProfileOperation::Status,
        source_of_truth: TOOL_PROFILE_SOURCE_OF_TRUTH,
        status: Some(ToolProfileStatusResponse {
            snapshot: ProfileSummary::of(snapshot),
        }),
        set: None,
    })?)
}

fn audit_row() -> ToolProfileAuditReadback {
    ToolProfileAuditReadback {
        cf_name: "audit_log",
        key_hex: "d".repeat(80),
        value_len_bytes: 2_345,
        value_sha256: "e".repeat(64),
    }
}

fn set_reply(
    before: &ToolProfileSnapshot,
    after: &ToolProfileSnapshot,
    reason: &str,
) -> anyhow::Result<serde_json::Value> {
    let row_readback = profile_row(
        SESSION_A,
        after.profile,
        "profile",
        Some(reason.to_owned()),
        &after.visible_tool_names,
    )?;
    Ok(serde_json::to_value(ProfileResponse {
        operation: ProfileOperation::Set,
        source_of_truth: TOOL_PROFILE_SOURCE_OF_TRUTH,
        status: None,
        set: Some(ToolProfileSetResponse {
            before: ProfileSummary::of(before),
            after: ProfileSummary::of(after),
            row_readback,
            intent_audit: audit_row(),
            final_audit: audit_row(),
            lease_proof: ToolProfileLeaseProof {
                required: false,
                held: false,
                owner_session_id: None,
                caller_is_owner: false,
                expires_in_ms: None,
            },
        }),
    })?)
}

fn reply_bytes(reply: &serde_json::Value) -> anyhow::Result<usize> {
    Ok(serde_json::to_vec(reply)?.len())
}

/// The summary names the snapshot's own profile, numbers, policy and route.
fn assert_summarizes(
    summary: &serde_json::Value,
    snapshot: &ToolProfileSnapshot,
) -> anyhow::Result<()> {
    assert_eq!(
        summary["profile"],
        serde_json::to_value(snapshot.profile)?,
        "{summary}"
    );
    assert_eq!(
        summary["profile_label"], snapshot.profile_label,
        "{summary}"
    );
    assert_eq!(summary["source"], snapshot.source.as_str(), "{summary}");
    assert_eq!(
        summary["visible_tool_count"], snapshot.visible_tool_count,
        "{summary}"
    );
    assert_eq!(
        summary["visible_tool_sha256"],
        snapshot.visible_tool_sha256.as_str(),
        "{summary}"
    );
    assert_eq!(
        summary["hidden_tool_route_policy"],
        serde_json::to_value(&snapshot.hidden_tool_route_policy)?,
        "{summary}"
    );
    assert_eq!(
        summary["foreground_route"],
        serde_json::to_value(&snapshot.foreground_route)?,
        "{summary}"
    );
    Ok(())
}

#[test]
fn p1_status_of_a_normal_agent_session_is_a_short_summary_of_its_snapshot() -> anyhow::Result<()> {
    let snapshot = snapshot(
        Some(SESSION_A),
        ToolProfileKind::NormalAgent,
        "default_normal_agent",
        foreground_route(false, false),
    )?;
    let reply = status_reply(&snapshot)?;
    assert_eq!(reply["operation"], "status");
    let summary = &reply["status"]["snapshot"];
    assert_eq!(summary["session_id"], SESSION_A, "{summary}");
    assert_eq!(summary["profile"], "normal_agent", "{summary}");
    assert_summarizes(summary, &snapshot)?;
    assert_eq!(
        summary["foreground_route"]["remaining_steps"]
            .as_array()
            .map(Vec::len),
        Some(2),
        "{summary}"
    );
    let bytes = reply_bytes(&reply)?;
    assert!(
        bytes <= STATUS_REPLY_BUDGET_BYTES,
        "status reply is {bytes} bytes"
    );
    Ok(())
}

#[test]
fn p2_status_reply_leaves_out_the_lists_found_in_tools_list_health_and_telemetry()
-> anyhow::Result<()> {
    let snapshot = snapshot(
        Some(SESSION_A),
        ToolProfileKind::NormalAgent,
        "default_normal_agent",
        foreground_route(false, false),
    )?;
    let reply = status_reply(&snapshot)?;
    let summary = &reply["status"]["snapshot"];
    let mut keys = summary
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    keys.sort_unstable();
    assert_eq!(keys, SUMMARY_KEYS, "{summary}");
    let text = reply.to_string();
    for absent in [
        "\"act_click\"",
        "\"browser_tabs\"",
        "codex",
        "facade_contract_sha256",
    ] {
        assert!(
            !text.contains(absent),
            "status reply still carries {absent}: {text}"
        );
    }
    // Still this session's numbers, not a blank summary.
    assert_eq!(
        summary["visible_tool_count"], snapshot.visible_tool_count,
        "{summary}"
    );
    assert_eq!(
        summary["visible_tool_sha256"],
        snapshot.visible_tool_sha256.as_str(),
        "{summary}"
    );
    Ok(())
}

#[test]
fn p3_status_without_a_session_names_break_glass_and_has_no_session_id() -> anyhow::Result<()> {
    let snapshot = snapshot(
        None,
        ToolProfileKind::BreakGlass,
        "unscoped_stdio_admin",
        foreground_route(true, true),
    )?;
    let reply = status_reply(&snapshot)?;
    let summary = &reply["status"]["snapshot"];
    assert!(summary.get("session_id").is_none(), "{summary}");
    assert_eq!(summary["profile"], "break_glass", "{summary}");
    assert_eq!(summary["source"], "unscoped_stdio_admin", "{summary}");
    assert_eq!(
        summary["foreground_route"]["foreground_route_ready"], true,
        "{summary}"
    );
    assert_eq!(
        summary["foreground_route"]["remaining_steps"],
        serde_json::json!([]),
        "{summary}"
    );
    assert_summarizes(summary, &snapshot)?;
    let bytes = reply_bytes(&reply)?;
    assert!(
        bytes <= STATUS_REPLY_BUDGET_BYTES,
        "status reply is {bytes} bytes"
    );
    Ok(())
}

#[test]
fn p4_set_reply_carries_two_summaries_the_row_readback_and_the_reason_once() -> anyhow::Result<()> {
    let before = snapshot(
        Some(SESSION_A),
        ToolProfileKind::NormalAgent,
        "default_normal_agent",
        foreground_route(false, false),
    )?;
    let after = snapshot(
        Some(SESSION_A),
        ToolProfileKind::BrowserDebugger,
        "profile",
        foreground_route(false, false),
    )?;
    // The longest reason `normalize_reason` accepts.
    let reason = "r".repeat(1_024);
    let reply = set_reply(&before, &after, &reason)?;
    assert_eq!(reply["operation"], "set");
    let set = &reply["set"];
    assert_eq!(set["before"]["profile"], "normal_agent", "{set}");
    assert_eq!(set["after"]["profile"], "browser_debugger", "{set}");
    assert_summarizes(&set["before"], &before)?;
    assert_summarizes(&set["after"], &after)?;
    assert_eq!(
        set["row_readback"]["record"]["profile"], "browser_debugger",
        "{set}"
    );
    assert_eq!(set["row_readback"]["cf_name"], "sessions", "{set}");
    assert!(set["intent_audit"]["key_hex"].is_string(), "{set}");
    assert!(set["final_audit"]["key_hex"].is_string(), "{set}");
    assert_eq!(set["lease_proof"]["required"], false, "{set}");
    let text = reply.to_string();
    assert_eq!(
        text.matches(reason.as_str()).count(),
        1,
        "reason must appear once"
    );
    let bytes = reply_bytes(&reply)?;
    assert!(
        bytes <= SET_REPLY_BUDGET_BYTES,
        "set reply is {bytes} bytes"
    );
    Ok(())
}

#[test]
fn p5_set_to_the_same_profile_gives_equal_before_and_after_summaries() -> anyhow::Result<()> {
    let before = snapshot(
        Some(SESSION_A),
        ToolProfileKind::NormalAgent,
        "profile",
        foreground_route(false, false),
    )?;
    let after = before.clone();
    let reply = set_reply(&before, &after, "keep the normal profile")?;
    let set = &reply["set"];
    assert_eq!(set["before"], set["after"], "{set}");
    assert_eq!(set["after"]["profile"], "normal_agent", "{set}");
    assert_summarizes(&set["after"], &after)?;
    Ok(())
}
