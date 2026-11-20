//! #22: a spawned agent starts from a cleared environment; only the provider
//! and gateway variables on the allow-list are forwarded from the daemon.

use super::child_environment::forwarded_agent_environment;

fn forwarded(pairs: &[(&str, &str)]) -> Vec<String> {
    forwarded_agent_environment(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    )
    .into_values()
    .map(|(key, _)| key)
    .collect()
}

#[test]
fn must_pass_provider_and_gateway_variables_are_forwarded_22() {
    let input = [
        ("OPENAI_BASE_URL", "https://gateway.example.test/v1"),
        ("openai_api_key", "k"),
        ("ANTHROPIC_BASE_URL", "https://gw.example.test"),
        ("CODEX_HOME", "codex-home-value"),
        ("CLAUDE_CONFIG_DIR", "claude-config-value"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("NODE_EXTRA_CA_CERTS", "ca-certs-value"),
        ("AWS_REGION", "eu-west-1"),
    ];
    let forwarded = forwarded_agent_environment(
        input
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    let expected: std::collections::BTreeMap<String, (String, String)> = input
        .iter()
        .map(|(key, value)| {
            (
                key.to_ascii_uppercase(),
                ((*key).to_owned(), (*value).to_owned()),
            )
        })
        .collect();
    // Uppercased lookup key, original spelling kept, value forwarded verbatim.
    assert_eq!(forwarded, expected);
    assert_eq!(
        forwarded.get("OPENAI_API_KEY"),
        Some(&("openai_api_key".to_owned(), "k".to_owned()))
    );
}

#[test]
fn must_fail_unrelated_daemon_variables_and_synapse_credentials_are_dropped_22() {
    let keys = forwarded(&[
        ("GITHUB_TOKEN", "secret"),
        ("AWS_SECRET_ACCESS_KEY", "secret"),
        ("DATABASE_URL", "postgres://x"),
        ("SYNAPSE_BEARER_TOKEN", "master"),
        ("SYNAPSE_TOKEN_FILE", "value"),
        ("PATH", "value"),
        ("", "empty-key"),
    ]);
    assert!(keys.is_empty(), "{keys:?}");
}
