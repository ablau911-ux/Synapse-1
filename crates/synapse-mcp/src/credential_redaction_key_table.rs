//! Frozen behaviour table for #306: which storage keys `storage inspect` withholds.
//!
//! Written by the manager from the behaviour table in issue #306 before the fix (work method v3).
//! Code agents must not edit this file. Red evidence: reality check 2026-11-18 on candidate
//! `23627236`, where rows `plain_workspace_key` and `secret_inside_hex_segment` were both withheld.

use super::key_embeds_secret;

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn workspace_key(run_id: &str, user_key: &str) -> Vec<u8> {
    format!(
        "workspace-blackboard/v1/run_hex/{}/key_hex/{}",
        hex(run_id),
        hex(user_key)
    )
    .into_bytes()
}

/// (row name, key bytes, expected: withheld)
fn rows() -> Vec<(&'static str, Vec<u8>, bool)> {
    vec![
        // normal: the key seen in the reality check; its hex segments decode to harmless text
        (
            "plain_workspace_key",
            workspace_key("1794993688556-19436", "rcprobe/plainkey"),
            false,
        ),
        // hostile: a secret hidden inside a hex-encoded segment
        (
            "secret_inside_hex_segment",
            workspace_key(
                "1794993688556-19436",
                "rcprobe/sk-test-FAKE-00001111222233334444",
            ),
            true,
        ),
        // hostile: a credential name hidden inside a hex-encoded segment
        (
            "credential_name_inside_hex_segment",
            workspace_key("1794993688556-19436", "db/password"),
            true,
        ),
        // hostile: hex of hex of a secret still fails closed
        (
            "double_hex_secret",
            workspace_key(
                "1794993688556-19436",
                &hex("sk-test-FAKE-00001111222233334444"),
            ),
            true,
        ),
        // neighbour: ordinary text keys stay readable
        (
            "plain_text_key",
            b"routine_id_migration/v1/stable_ids".to_vec(),
            false,
        ),
        (
            "uuid_key",
            b"mcp/tool-profile/v1/73bd08f9-38c7-4b12-bc2e-f0a1b2c3d4e5".to_vec(),
            false,
        ),
        // neighbour: credential names and token prefixes in plain sight are withheld
        (
            "credential_name_part",
            b"sessions/v1/api_token/current".to_vec(),
            true,
        ),
        (
            "token_prefix",
            b"cache/ghp_FAKEFAKEFAKEFAKE0000111122223333".to_vec(),
            true,
        ),
        // boundary: a long hex run that is not text stays withheld (it looks like a key)
        (
            "opaque_hex_digest",
            b"blob/9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".to_vec(),
            true,
        ),
        // boundary: odd-length hex cannot be decoded and stays withheld
        (
            "odd_length_hex",
            b"blob/key_hex/726370726f62652f706c61696e6b657".to_vec(),
            true,
        ),
        // structurally invalid: binary sequence keys are not text and stay readable as hex
        (
            "binary_sequence_key",
            vec![
                0x18, 0xe9, 0x1e, 0x79, 0xe1, 0xd4, 0x56, 0x2c, 0x00, 0x00, 0x00, 0x07,
            ],
            false,
        ),
        // empty
        ("empty_key", Vec::new(), false),
    ]
}

#[test]
fn storage_key_withholding_table() {
    let failures: Vec<String> = rows()
        .into_iter()
        .filter_map(|(name, key, expected)| {
            let observed = key_embeds_secret(&key);
            (observed != expected)
                .then(|| format!("{name}: expected withheld={expected}, observed {observed}"))
        })
        .collect();
    assert!(failures.is_empty(), "rows failed:\n{}", failures.join("\n"));
}
