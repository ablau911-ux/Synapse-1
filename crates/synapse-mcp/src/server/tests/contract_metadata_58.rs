//! Regression tests for #58 punch-list items 1, 3, 4: persisted/advertised
//! contract metadata must match real behavior.

use super::*;

const NO_OPT_IN: SessionlessOperatorOptIn = SessionlessOperatorOptIn {
    shell_patterns: false,
    launch_patterns: false,
};

fn op_spec(tool: &str, operation: &str) -> FacadeOperationContractSpec {
    FACADE_TOOL_CONTRACTS
        .iter()
        .find(|contract| contract.tool_name == tool)
        .and_then(|contract| {
            contract
                .operations
                .iter()
                .find(|o| o.operation == operation)
        })
        .copied()
        .unwrap_or_else(|| panic!("missing contract row {tool}/{operation}"))
}

#[test]
fn must_fail_denied_break_glass_tools_cover_shell_launch_and_spawn() {
    let denied = denied_break_glass_tools(&[]);
    for name in [
        "act_launch",
        "act_run_shell",
        "act_run_shell_start",
        "act_run_shell_status",
        "act_run_shell_cancel",
        "act_spawn_agent",
    ] {
        assert!(
            denied.iter().any(|d| d == name),
            "{name} missing: {denied:?}"
        );
    }
}

#[test]
fn must_pass_subscribe_events_row_reports_real_failure_code() {
    let row = op_spec("subscribe", "events");
    assert_eq!(row.error_code, error_codes::TOOL_PARAMS_INVALID);
}

#[test]
fn must_pass_hygiene_scan_text_row_declares_conditional_mutation() {
    // persist=true writes CF_KV hygiene flag rows, so the row must not claim
    // to be read-only; the condition is spelled out in the source of truth.
    let row = op_spec("hygiene", "scan_text");
    assert!(row.mutates_state);
    assert!(row.source_of_truth.contains("only when persist=true"));
    assert!(row.readback_source_of_truth.is_some());
}

/// Raw implementation tools (exec + hazardous input primitives) must be
/// refused on the sessionless stdio admin lane exactly like on a scoped
/// break_glass session: the public surface is facade-only.
#[test]
fn must_fail_sessionless_admission_denies_raw_implementation_tools() {
    let raw = BREAK_GLASS_DENIED_EXEC_TOOLS
        .iter()
        .chain(BREAK_GLASS_HAZARDOUS_TOOLS.iter())
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    for name in &raw {
        let error = sessionless_tool_call_denial(name, &raw, NO_OPT_IN)
            .unwrap_or_else(|| panic!("sessionless call to raw tool {name} must be denied"));
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str);
        assert_eq!(
            code,
            Some(error_codes::TOOL_PROFILE_POLICY_DENIED),
            "{name}"
        );
    }
}

#[test]
fn must_pass_sessionless_admission_admits_public_facades_and_unknown_names() {
    let facades = PUBLIC_TOOL_NAMES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    for name in &facades {
        assert!(
            sessionless_tool_call_denial(name, &facades, NO_OPT_IN).is_none(),
            "facade {name} must stay callable on stdio admin"
        );
    }
    // Names the router does not know fall through to its normal not-found path.
    assert!(sessionless_tool_call_denial("no_such_tool", &facades, NO_OPT_IN).is_none());
}

/// The sessionless stdio admin lane stays broad: only the exec set and the
/// hazardous raw input primitives are denied; other raw tools stay admitted.
#[test]
fn must_pass_sessionless_admission_keeps_ordinary_raw_tools_admitted() {
    let full = ["health", "observe", "storage_put_probe_rows", "act_launch"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    for name in ["health", "observe", "storage_put_probe_rows"] {
        assert!(
            sessionless_tool_call_denial(name, &full, NO_OPT_IN).is_none(),
            "{name} must stay admitted on stdio admin"
        );
    }
    assert!(sessionless_tool_call_denial("act_launch", &full, NO_OPT_IN).is_some());
}

fn raw_exec_and_hazardous_names() -> Vec<String> {
    BREAK_GLASS_DENIED_EXEC_TOOLS
        .iter()
        .chain(BREAK_GLASS_HAZARDOUS_TOOLS.iter())
        .map(|name| (*name).to_owned())
        .collect()
}

/// An explicit `--allow-shell` allowlist is the operator opt-in for the raw
/// shell family on sessionless stdio; launch, spawn and hazardous input stay
/// denied without their own opt-in.
#[test]
fn must_fail_sessionless_admission_admits_shell_family_only_with_allow_shell_patterns() {
    let raw = raw_exec_and_hazardous_names();
    let shell_only = SessionlessOperatorOptIn {
        shell_patterns: true,
        launch_patterns: false,
    };
    for name in &raw {
        let denied = sessionless_tool_call_denial(name, &raw, shell_only).is_some();
        let is_shell = SESSIONLESS_SHELL_OPT_IN_TOOLS.contains(&name.as_str());
        assert_eq!(denied, !is_shell, "{name} with --allow-shell configured");
        assert!(
            sessionless_tool_call_denial(name, &raw, NO_OPT_IN).is_some(),
            "{name} without --allow-shell must stay denied"
        );
    }
    for name in [
        "act_run_shell",
        "act_run_shell_start",
        "act_run_shell_status",
        "act_run_shell_cancel",
    ] {
        assert!(
            sessionless_tool_call_denial(name, &raw, shell_only).is_none(),
            "{name}"
        );
    }
    assert!(sessionless_tool_call_denial("act_spawn_agent", &raw, shell_only).is_some());
    assert!(sessionless_tool_call_denial("act_launch", &raw, shell_only).is_some());
}

/// `--allow-launch` opts in act_launch only.
#[test]
fn must_fail_sessionless_admission_admits_launch_only_with_allow_launch_patterns() {
    let raw = raw_exec_and_hazardous_names();
    let launch_only = SessionlessOperatorOptIn {
        shell_patterns: false,
        launch_patterns: true,
    };
    for name in &raw {
        let denied = sessionless_tool_call_denial(name, &raw, launch_only).is_some();
        assert_eq!(
            denied,
            name != "act_launch",
            "{name} with --allow-launch configured"
        );
    }
}
