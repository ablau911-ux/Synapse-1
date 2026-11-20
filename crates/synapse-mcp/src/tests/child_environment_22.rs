use super::ActLaunchParams;
use super::child_environment::{
    child_base_environment, is_credential_env_key, scrub_credentials_from,
    spawned_agent_environment, untrusted_environment,
};
use std::collections::BTreeMap;
use std::ffi::OsString;

const BLOCKED_KEYS: [&str; 14] = [
    "SYNAPSE_BEARER_TOKEN",
    "synapse_bearer_token",
    "SyNaPsE_ToKeN_FiLe",
    "SYNAPSE_TOKEN_PATH",
    "SYNAPSE_BEARER_TOKEN_FILE",
    "SYNAPSE_HTTP_TOKEN_PATH",
    "SYNAPSE_LOCAL_AGENT_CAPABILITY",
    "APPDATA",
    "appdata",
    "LOCALAPPDATA",
    "USERPROFILE",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
];
const SYNTHETIC_MASTER: &str = "issue22-synthetic-master";

fn explicit_environment() -> BTreeMap<String, String> {
    let mut env: BTreeMap<_, _> = BLOCKED_KEYS
        .into_iter()
        .map(|key| {
            (
                key.to_owned(),
                "issue22-synthetic-secret-or-path".to_owned(),
            )
        })
        .collect();
    env.insert("SYNAPSE_22_PUBLIC".to_owned(), "kept".to_owned());
    env
}

fn launch_params(env: BTreeMap<String, String>) -> ActLaunchParams {
    serde_json::from_value(serde_json::json!({
        "target": "powershell.exe",
        "env": env,
    }))
    .expect("valid launch parameters")
}

fn untrusted(env: BTreeMap<String, String>) -> BTreeMap<String, String> {
    untrusted_environment(&launch_params(env), "act_launch")
        .expect("untrusted launch environment")
        .into_values()
        .collect()
}

fn assert_scrubbed(env: &BTreeMap<String, String>) {
    for blocked in BLOCKED_KEYS {
        assert!(
            !env.keys().any(|key| key.eq_ignore_ascii_case(blocked)),
            "child environment exposed {blocked}"
        );
    }
}

#[test]
fn must_fail_untrusted_launch_scrubs_credentials_and_profile_hints_22() {
    let env = untrusted(explicit_environment());
    assert_scrubbed(&env);
    assert_eq!(
        env.get("SYNAPSE_22_PUBLIC").map(String::as_str),
        Some("kept")
    );
    let inherited: BTreeMap<_, _> = child_base_environment().into_values().collect();
    assert_scrubbed(&inherited);
}

#[test]
fn must_fail_untrusted_launch_empty_and_boundary_values_stay_scrubbed_22() {
    let empty = untrusted(BTreeMap::new());
    assert_scrubbed(&empty);
    for value in [String::new(), "x".repeat(4096)] {
        let boundary = untrusted(BTreeMap::from([("sYnApSe_ToKeN_fIlE".to_owned(), value)]));
        assert_scrubbed(&boundary);
    }
    #[cfg(windows)]
    {
        for required in ["PATH", "PATHEXT", "ComSpec", "SystemRoot", "TEMP", "TMP"] {
            assert!(
                empty
                    .iter()
                    .any(|(key, value)| key.eq_ignore_ascii_case(required) && !value.is_empty()),
                "scrubbing removed required Windows variable {required}"
            );
        }
    }
}

#[cfg(windows)]
fn decode_block(block: &[u16]) -> BTreeMap<String, String> {
    assert!(
        block.ends_with(&[0, 0]),
        "block must be double-NUL terminated"
    );
    block
        .split(|unit| *unit == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let entry = String::from_utf16(entry).expect("UTF-16 environment entry");
            // Windows keeps per-drive cwd entries such as `=C:=C:\`.
            let (split, _) = entry
                .char_indices()
                .skip(1)
                .find(|(_, ch)| *ch == '=')
                .expect("key=value entry");
            (entry[..split].to_owned(), entry[split + 1..].to_owned())
        })
        .collect()
}

#[cfg(windows)]
#[test]
fn must_fail_launch_block_is_scrubbed_and_double_nul_terminated_22() {
    let params = launch_params(explicit_environment());
    let block = super::launch_environment_block(&params).expect("valid Windows environment");
    let decoded = decode_block(&block);
    assert_scrubbed(&decoded);
    assert_eq!(
        decoded.get("SYNAPSE_22_PUBLIC").map(String::as_str),
        Some("kept")
    );
}

/// The malformed entry must be refused by `validate_launch_environment_entry`
/// with `TOOL_PARAMS_INVALID`, not by an unrelated failure.
#[cfg(windows)]
fn assert_params_invalid(error: Option<rmcp::ErrorData>, label: &str) {
    let Some(error) = error else {
        panic!("{label}: malformed env entry must be refused");
    };
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str),
        Some(super::error_codes::TOOL_PARAMS_INVALID),
        "{label}: {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn must_fail_launch_rejects_malformed_entries_even_when_blocked_22() {
    for (key, value) in [
        ("", "value"),
        ("bad=key", "value"),
        ("APPDATA", "bad\0value"),
        ("SYNAPSE_BEARER_TOKEN", "bad\0value"),
    ] {
        let invalid = launch_params(BTreeMap::from([(key.to_owned(), value.to_owned())]));
        let label = format!("{key:?}={value:?}");
        assert_params_invalid(super::launch_environment_block(&invalid).err(), &label);
        assert_params_invalid(untrusted_environment(&invalid, "act_launch").err(), &label);
        assert_params_invalid(spawned_agent_environment(&invalid).err(), &label);
    }
}

#[cfg(windows)]
#[test]
fn must_fail_inherited_helper_block_has_no_credentials_22() {
    let decoded = decode_block(&super::child_environment::inherited_environment_block());
    assert!(!decoded.keys().any(|key| is_credential_env_key(key)));
    assert!(decoded.keys().any(|key| key.eq_ignore_ascii_case("PATH")));
}

#[test]
fn must_fail_credential_key_policy_is_case_insensitive_and_narrow_22() {
    for key in [
        "SYNAPSE_BEARER_TOKEN",
        "synapse_token_path",
        "SYNAPSE_SPAWN_CREDENTIAL",
        "SYNAPSE_LOCAL_AGENT_CAPABILITY",
    ] {
        assert!(is_credential_env_key(key), "{key} must be a credential");
    }
    for key in [
        "PATH",
        "SYNAPSE_AGENT_SPAWN_ID",
        "SYNAPSE_MCP_URL",
        "MY_SYNAPSE_TOKEN",
        "",
    ] {
        assert!(
            !is_credential_env_key(key),
            "{key} must not be a credential"
        );
    }
}

fn removed_keys(envs: std::process::CommandEnvs<'_>) -> Vec<String> {
    let mut removed: Vec<_> = envs
        .filter(|(_, value)| value.is_none())
        .map(|(key, _)| key.to_string_lossy().to_ascii_uppercase())
        .collect();
    removed.sort();
    removed
}

#[test]
fn must_fail_internal_helper_scrub_removes_only_credentials_22() {
    let inherited = [
        "SYNAPSE_BEARER_TOKEN",
        "Synapse_Token_Path",
        "SYNAPSE_SPAWN_CREDENTIAL",
        "PATH",
        "USERPROFILE",
        "SYNAPSE_AGENT_SPAWN_ID",
    ]
    .map(OsString::from);
    let expected = [
        "SYNAPSE_BEARER_TOKEN",
        "SYNAPSE_SPAWN_CREDENTIAL",
        "SYNAPSE_TOKEN_PATH",
    ];
    let mut std_command = std::process::Command::new("issue22-helper-probe");
    scrub_credentials_from(&mut std_command, inherited.clone());
    assert_eq!(removed_keys(std_command.get_envs()), expected);

    let mut tokio_command = tokio::process::Command::new("issue22-helper-probe");
    scrub_credentials_from(&mut tokio_command, inherited);
    assert_eq!(removed_keys(tokio_command.as_std().get_envs()), expected);

    let mut untouched = std::process::Command::new("issue22-helper-probe");
    scrub_credentials_from(&mut untouched, Vec::<OsString>::new());
    assert_eq!(untouched.get_envs().count(), 0);
}

#[test]
fn must_pass_spawned_agent_env_admits_only_a_live_scoped_credential_22() {
    let spawn_id = "agent-spawn-issue22-env-admission";
    let registry = crate::spawn_credentials::registry();
    let credential = registry
        .issue(SYNTHETIC_MASTER, spawn_id)
        .expect("issue scoped credential");
    let mut caller = explicit_environment();
    caller.insert("SYNAPSE_BEARER_TOKEN".to_owned(), credential.clone());
    let env = spawned_agent_environment(&launch_params(caller.clone())).expect("spawn env");
    let credentials: Vec<_> = env
        .keys()
        .filter(|key| is_credential_env_key(key))
        .collect();
    assert_eq!(credentials, ["SYNAPSE_BEARER_TOKEN"]);
    assert_eq!(env.get("SYNAPSE_BEARER_TOKEN"), Some(&credential));
    assert_eq!(
        env.get("SYNAPSE_22_PUBLIC").map(String::as_str),
        Some("kept")
    );

    let master = BTreeMap::from([(
        "SYNAPSE_BEARER_TOKEN".to_owned(),
        SYNTHETIC_MASTER.to_owned(),
    )]);
    let env = spawned_agent_environment(&launch_params(master)).expect("spawn env");
    assert!(!env.keys().any(|key| is_credential_env_key(key)));

    assert!(registry.revoke_spawn(spawn_id));
    let env = spawned_agent_environment(&launch_params(caller)).expect("spawn env");
    assert!(!env.keys().any(|key| is_credential_env_key(key)));
}

#[cfg(windows)]
#[test]
fn must_pass_spawned_agent_env_keeps_profile_paths_for_cli_login_22() {
    let env = spawned_agent_environment(&launch_params(BTreeMap::new())).expect("spawn env");
    for required in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "PATH", "TEMP"] {
        assert!(
            env.iter()
                .any(|(key, value)| key.eq_ignore_ascii_case(required) && !value.is_empty()),
            "spawned agent lost {required}"
        );
    }
}

#[cfg(windows)]
fn shell_probe() -> serde_json::Value {
    let keys = BLOCKED_KEYS
        .iter()
        .map(|key| format!("'{key}'"))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "foreach ($key in @({keys})) {{ \
         if ($null -ne [Environment]::GetEnvironmentVariable($key)) {{ exit 22 }} \
         }}; if ($env:SYNAPSE_22_PUBLIC -ne 'kept') {{ exit 23 }}; \
         Write-Output 'environment-scrubbed'; exit 0"
    );
    serde_json::json!({
        "command": "powershell.exe",
        "args": ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
        "env": explicit_environment(),
    })
}

#[cfg(windows)]
#[tokio::test]
async fn must_fail_inline_shell_child_scrubs_credentials_and_hints_22() {
    let params = serde_json::from_value(shell_probe()).expect("valid inline shell parameters");
    let spawned = super::spawn_shell_child(&params, None).expect("inline child must spawn");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        spawned.child.wait_with_output(),
    )
    .await
    .expect("inline probe must finish")
    .expect("inline probe output");
    assert_eq!(
        output.status.code(),
        Some(0),
        "child found a blocked environment key"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "environment-scrubbed"
    );
    drop(spawned.process_job);
}

#[cfg(windows)]
#[tokio::test]
async fn must_fail_durable_shell_child_scrubs_credentials_and_hints_22() {
    let params = serde_json::from_value(shell_probe()).expect("valid durable shell parameters");
    let dir = tempfile::tempdir().expect("durable probe directory");
    let stdout_path = dir.path().join("stdout.log");
    let stderr_path = dir.path().join("stderr.log");
    let stdout = std::fs::File::create(&stdout_path).expect("stdout file");
    let stderr = std::fs::File::create(&stderr_path).expect("stderr file");
    let mut spawned =
        super::spawn_shell_job_child(&params, "issue22-env-probe", stdout, stderr, None)
            .expect("durable child must spawn");
    let status = tokio::time::timeout(std::time::Duration::from_secs(20), spawned.child.wait())
        .await
        .expect("durable probe must finish")
        .expect("durable probe exit status");
    for pump in spawned.pumps {
        pump.await.expect("durable output pump must finish");
    }
    assert_eq!(
        status.code(),
        Some(0),
        "child found a blocked environment key"
    );
    let stdout = std::fs::read_to_string(stdout_path).expect("durable stdout readback");
    assert_eq!(stdout.trim(), "environment-scrubbed");
    let stderr = std::fs::read_to_string(stderr_path).expect("durable stderr readback");
    assert!(stderr.is_empty());
    drop(spawned.process_job);
}
