//! #58 regression: configured `--allow-shell` / `--allow-launch` patterns must restrict.
//!
//! `allow_shell_any` / `allow_launch_any` used to default to true whenever the
//! `SYNAPSE_ALLOW_*_ANY` env var was unset, so a configured allowlist was silently
//! bypassed. Now: patterns configured and no explicit env => enforced; no patterns
//! and no explicit env => the permissive default is unchanged; an explicit env value
//! always wins. The env values are injected, so no test mutates the process env.

use super::*;

const SHELL_PATTERN: &str = r"^git status$";
const LAUNCH_PATTERN: &str = r"^notepad\.exe$";

fn config(
    shell: &[&str],
    launch: &[&str],
    shell_any_env: Option<&str>,
    launch_any_env: Option<&str>,
) -> M4ServiceConfig {
    match M4ServiceConfig::from_parts_with_any_env(
        shell.iter().map(|p| (*p).to_owned()).collect(),
        launch.iter().map(|p| (*p).to_owned()).collect(),
        DEFAULT_RUN_SHELL_INLINE_AWAIT_LIMIT_MS,
        shell_any_env,
        launch_any_env,
    ) {
        Ok(config) => config,
        Err(error) => panic!("synthetic config should build: {error:#}"),
    }
}

fn shell_params(command: &str, args: &[&str]) -> ActRunShellParams {
    match serde_json::from_value(serde_json::json!({
        "command": command,
        "args": args,
    })) {
        Ok(params) => params,
        Err(error) => panic!("synthetic shell params should parse: {error}"),
    }
}

fn launch_params(target: &str) -> ActLaunchParams {
    match serde_json::from_value(serde_json::json!({ "target": target })) {
        Ok(params) => params,
        Err(error) => panic!("synthetic launch params should parse: {error}"),
    }
}

/// Asserts the real policy gate refused with the exact code and reason.
fn assert_policy_refusal(error: &ErrorData, code: &str, reason: &str) {
    let data = error
        .data
        .as_ref()
        .unwrap_or_else(|| panic!("policy refusal must carry data: {error:?}"));
    assert_eq!(
        data.get("code").and_then(serde_json::Value::as_str),
        Some(code),
        "{data}"
    );
    assert_eq!(
        data.get("reason").and_then(serde_json::Value::as_str),
        Some(reason),
        "{data}"
    );
}

#[test]
fn must_fail_configured_shell_patterns_refuse_non_matching_commands() {
    let config = config(&[SHELL_PATTERN], &[], None, None);

    assert!(!config.allow_shell_any(), "patterns configured => not any");
    assert_eq!(config.shell_match("git status"), Some(SHELL_PATTERN));
    assert_eq!(config.shell_match("rm -rf C:\\"), None);
    assert_eq!(config.shell_match("git status && calc"), None);

    // Drive the real `act_run_shell` policy gate, not only the matcher.
    let Err(error) = authorize_run_shell(&config, &shell_params("rm", &["-rf", "C:\\"])) else {
        panic!("a non-allowlisted command must be refused by the policy gate");
    };
    assert_policy_refusal(
        &error,
        error_codes::SAFETY_SHELL_DENIED_BY_POLICY,
        "shell_command_not_allowlisted",
    );
}

#[test]
fn must_fail_configured_launch_patterns_refuse_non_matching_targets() {
    let config = config(&[], &[LAUNCH_PATTERN], None, None);

    assert!(!config.allow_launch_any(), "patterns configured => not any");
    assert_eq!(config.launch_match("notepad.exe"), Some(LAUNCH_PATTERN));
    assert_eq!(config.launch_match("calc.exe"), None);

    // Drive the real `act_launch` policy gate, not only the matcher.
    let Err(error) = validate_launch_authorized(&config, &launch_params("calc.exe")) else {
        panic!("a non-allowlisted launch target must be refused by the policy gate");
    };
    assert_policy_refusal(
        &error,
        error_codes::SAFETY_LAUNCH_DENIED_BY_POLICY,
        "launch_command_not_allowlisted",
    );
}

#[test]
fn must_pass_shell_and_launch_any_are_decided_independently() {
    let shell_only = config(&[SHELL_PATTERN], &[], None, None);
    assert!(!shell_only.allow_shell_any());
    assert!(
        shell_only.allow_launch_any(),
        "no launch patterns => launch keeps the permissive default"
    );

    let launch_only = config(&[], &[LAUNCH_PATTERN], None, None);
    assert!(launch_only.allow_shell_any());
    assert!(!launch_only.allow_launch_any());
}

#[test]
fn must_pass_no_patterns_keeps_permissive_default() {
    let config = config(&[], &[], None, None);

    assert!(config.allow_shell_any());
    assert!(config.allow_launch_any());
    assert_eq!(
        config.shell_match("anything at all"),
        Some("__any_permitted__")
    );
    assert_eq!(
        config.launch_match("anything.exe"),
        Some("__any_permitted__")
    );
}

#[test]
fn must_fail_explicit_falsey_env_disables_any_even_without_patterns() {
    for value in ["0", "false", "No", " OFF "] {
        let config = config(&[], &[], Some(value), Some(value));
        assert!(!config.allow_shell_any(), "{value:?}");
        assert!(!config.allow_launch_any(), "{value:?}");
        assert_eq!(config.shell_match("git status"), None, "{value:?}");
        assert_eq!(config.launch_match("notepad.exe"), None, "{value:?}");

        // The real gates refuse with the "no policy" reason, since no pattern exists.
        let Err(shell_error) = authorize_run_shell(&config, &shell_params("git", &["status"]))
        else {
            panic!("{value:?}: an explicit falsey env with no patterns must refuse shell");
        };
        assert_policy_refusal(
            &shell_error,
            error_codes::SAFETY_SHELL_DENIED_BY_POLICY,
            "no_allow_shell_policy",
        );
        let Err(launch_error) = validate_launch_authorized(&config, &launch_params("notepad.exe"))
        else {
            panic!("{value:?}: an explicit falsey env with no patterns must refuse launch");
        };
        assert_policy_refusal(
            &launch_error,
            error_codes::SAFETY_LAUNCH_DENIED_BY_POLICY,
            "no_allow_launch_policy",
        );
    }
}

#[test]
fn must_pass_explicit_truthy_env_keeps_any_even_with_patterns() {
    let config = config(&[SHELL_PATTERN], &[LAUNCH_PATTERN], Some("1"), Some("true"));

    assert!(config.allow_shell_any());
    assert!(config.allow_launch_any());
    assert_eq!(config.shell_match("rm -rf C:\\"), Some("__any_permitted__"));
    assert_eq!(config.launch_match("calc.exe"), Some("__any_permitted__"));
}

#[test]
fn must_fail_explicit_falsey_env_with_patterns_still_enforces() {
    let config = config(&[SHELL_PATTERN], &[LAUNCH_PATTERN], Some("0"), Some("0"));

    assert!(!config.allow_shell_any());
    assert!(!config.allow_launch_any());
    assert_eq!(config.shell_match("git status"), Some(SHELL_PATTERN));
    assert_eq!(config.shell_match("calc"), None);
}
