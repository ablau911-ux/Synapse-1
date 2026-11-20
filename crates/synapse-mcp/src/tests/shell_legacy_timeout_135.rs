//! FU1 #135: restore authorization above the removed shell timeout cap.
#![cfg(windows)]
use super::*;

fn params(timeout_ms: u64) -> ActRunShellParams {
    serde_json::from_value(serde_json::json!({
        "command":"cmd.exe", "args":["/c", "echo long-timeout-ok"],
        "timeout_ms":timeout_ms
    }))
    .unwrap()
}

fn config() -> M4ServiceConfig {
    M4ServiceConfig::from_parts_with_any_env(Vec::new(), Vec::new(), 30_000, Some("1"), Some("0"))
        .unwrap()
}

#[test]
fn must_pass_shell_timeout_above_legacy_cap_authorizes_135() {
    let params = params(1_200_000);
    assert_eq!(params.timeout_ms, 1_200_000);
    let authorization = authorize_run_shell(&config(), &params).unwrap();
    assert_eq!(
        authorization.command_line,
        "\"cmd.exe\" /c \"echo long-timeout-ok\""
    );
    assert_eq!(params.timeout_ms, 1_200_000);
}

#[test]
fn must_fail_shell_zero_timeout_refuses_before_execution_135() {
    let params = params(0);
    let before = shell_command_line(&params);
    let error = match authorize_run_shell(&config(), &params) {
        Ok(_) => panic!("zero timeout authorized"),
        Err(error) => error,
    };
    assert_eq!(
        error.data.as_ref().and_then(|v| v.get("code")),
        Some(&serde_json::json!(error_codes::TOOL_PARAMS_INVALID))
    );
    assert_eq!(error.message, "act_run_shell timeout_ms must be >= 1");
    assert_eq!(params.timeout_ms, 0);
    assert_eq!(shell_command_line(&params), before);
}
