//! Regression tests for #22: the owned-PTY command must receive exactly the
//! environment built by the child-environment policy, not the daemon's own.

use super::*;

fn spec_with_env(env: BTreeMap<String, String>) -> CaptureSpec {
    CaptureSpec {
        live_key: None,
        program: "agent.exe".to_owned(),
        args: Vec::new(),
        cwd: None,
        env,
        cols: 80,
        rows: 24,
        started_unix_secs: 0,
        title: None,
    }
}

#[test]
fn must_pass_command_environment_is_exactly_the_spec_environment() {
    // Any variable the daemon test process has stands in for a daemon secret.
    let (inherited_key, _) = std::env::vars_os()
        .find(|(key, _)| {
            key.to_str()
                .is_some_and(|key| key != "SYNAPSE_TEST_POLICY_VAR")
        })
        .expect("test process has at least one environment variable");
    let mut env = BTreeMap::new();
    env.insert("SYNAPSE_TEST_POLICY_VAR".to_owned(), "policy".to_owned());

    let command = command_from_spec(&spec_with_env(env));

    assert_eq!(
        command
            .get_env("SYNAPSE_TEST_POLICY_VAR")
            .and_then(|v| v.to_str()),
        Some("policy"),
        "policy variable must reach the child"
    );
    assert!(
        command.get_env(&inherited_key).is_none(),
        "daemon variable {inherited_key:?} must not leak into the spawned agent"
    );
    assert_eq!(
        command.iter_full_env_as_str().count(),
        1,
        "child environment must contain only the policy variables"
    );
}

#[test]
fn must_fail_empty_spec_environment_yields_empty_command_environment() {
    let command = command_from_spec(&spec_with_env(BTreeMap::new()));
    assert_eq!(command.iter_full_env_as_str().count(), 0);
}
