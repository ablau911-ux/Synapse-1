//! #39 (first-review fail): shell output redaction must also cover
//! (1) the exact credentials Synapse itself holds, whatever their shape
//! (a 64-hex daemon token looks like a digest), and (2) `KEY=value` /
//! `KEY: value` secrets that appear mid-line, not only at a line start.

use std::fs;

use super::known_secrets::KnownSecrets;
use super::redacted_log::StreamRedactor;
use super::{redact_shell_output_for_public_readback, redact_shell_output_with};

const MASK: &str = "[redacted-shell-output]";
const DAEMON_TOKEN: &str = "9f2c7a41d0b86e35a1f4c9d27b60e8a35c1d94f07a2b6e83d5c0194f7a3b8e26";
const GIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn with_token_file(token: &str) -> (tempfile::TempDir, KnownSecrets) {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let path = dir.path().join("token.txt");
    fs::write(&path, format!("{token}\r\n")).unwrap_or_else(|error| panic!("write token: {error}"));
    (dir, KnownSecrets::new(vec![path], Vec::new()))
}

fn redact(output: &str, secrets: &KnownSecrets) -> String {
    redact_shell_output_with(output, secrets)
}

#[test]
fn must_fail_known_daemon_token_is_masked_even_though_it_looks_like_a_digest() {
    let (_dir, secrets) = with_token_file(DAEMON_TOKEN);
    let output = format!("L22 tokenfile {DAEMON_TOKEN}\n{DAEMON_TOKEN}\nplain {DAEMON_TOKEN}.");
    let redacted = redact(&output, &secrets);
    assert!(!redacted.contains(DAEMON_TOKEN), "token leaked: {redacted}");
    assert_eq!(redacted.matches(MASK).count(), 3, "{redacted}");
    assert!(redacted.contains("L22 tokenfile"));
}

#[test]
fn must_pass_unknown_hex_digests_stay_readable() {
    let (_dir, secrets) = with_token_file(DAEMON_TOKEN);
    let output = format!("git={GIT_SHA} sha256:{SHA256}");
    assert_eq!(redact(&output, &secrets), output);
}

#[test]
fn must_pass_token_file_rotation_is_picked_up_without_restart() {
    let (dir, secrets) = with_token_file(DAEMON_TOKEN);
    // Short (< 32 chars) so the generic opaque-token rule never masks it: only
    // a re-read of the rotated credential file can.
    let rotated = "rotated-qwerty-zz";
    let text = format!("a {rotated} b");
    let control = KnownSecrets::new(Vec::new(), Vec::new());
    assert_eq!(
        redact(&text, &control),
        text,
        "control: without the credential file the rotated value stays readable"
    );
    assert!(redact(DAEMON_TOKEN, &secrets).contains(MASK));
    assert_eq!(
        redact(&text, &secrets),
        text,
        "before rotation the value is not a known secret"
    );
    fs::write(dir.path().join("token.txt"), rotated)
        .unwrap_or_else(|error| panic!("rotate: {error}"));
    let redacted = redact(&text, &secrets);
    assert!(
        !redacted.contains(rotated),
        "rotated token leaked: {redacted}"
    );
    assert_eq!(redacted, format!("a {MASK} b"));
}

#[test]
fn must_fail_missing_or_short_credential_files_are_harmless() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let short = dir.path().join("short.txt");
    fs::write(&short, "ab").unwrap_or_else(|error| panic!("write: {error}"));
    let secrets = KnownSecrets::new(vec![dir.path().join("absent.txt"), short], Vec::new());
    let output = "ab absent ab";
    assert_eq!(redact(output, &secrets), output);
}

#[test]
fn must_fail_known_environment_credential_is_masked() {
    let secrets = KnownSecrets::new(Vec::new(), vec!["SYNAPSE_TEST_39D_CREDENTIAL".to_owned()]);
    let value = "env-credential-value-39d-abcdef";
    // SAFETY: this variable name is unique to this test; no other test or
    // thread reads or writes it, and std documents it as safe on Windows.
    unsafe { std::env::set_var("SYNAPSE_TEST_39D_CREDENTIAL", value) };
    let redacted = redact(&format!("x {value} y"), &secrets);
    // SAFETY: as above.
    unsafe { std::env::remove_var("SYNAPSE_TEST_39D_CREDENTIAL") };
    assert!(!redacted.contains(value), "{redacted}");
}

#[test]
fn must_fail_scoped_spawn_credential_shape_is_masked() {
    let credential = format!("synapse-spawn-v1.{SHA256}");
    let (_dir, secrets) = with_token_file(DAEMON_TOKEN);
    let redacted = redact(
        &format!("env SYNAPSE_BEARER_TOKEN is {credential} ok"),
        &secrets,
    );
    assert!(!redacted.contains(SHA256), "{redacted}");
}

#[test]
fn must_fail_mid_line_environment_style_secrets_are_masked() {
    let cases = [
        (
            "L01 SECRET_KEY=test-not-a-real-secret",
            "test-not-a-real-secret",
        ),
        (
            "export API_TOKEN=rv72-export-token-val",
            "rv72-export-token-val",
        ),
        ("DB_PASSWORD: rv72-db-pass-yaml", "rv72-db-pass-yaml"),
        ("x; set DB_PASSWORD=hunter2hunter2", "hunter2hunter2"),
        (
            "run $env:AWS_SECRET_ACCESS_KEY=\"abc def ghi\" now",
            "abc def ghi",
        ),
        (
            "export api_token=lowercase-secret-val",
            "lowercase-secret-val",
        ),
        ("echo 'SERVICE_API_TOKEN=tiny'", "tiny"),
    ];
    for (input, secret) in cases {
        let redacted = redact_shell_output_for_public_readback(input);
        assert!(!redacted.contains(secret), "leaked {secret:?}: {redacted}");
        assert!(redacted.contains(MASK), "{redacted}");
    }
}

#[test]
fn must_pass_line_start_live_diagnostics_and_mask_real_secrets_39() {
    for input in [
        "info TOKENS=42 SECRETARY=Alice token_count=42",
        "tokenizer initialized; token_count=7; max_tokens: 512",
        "prefix STATUS_CODE=200 SECRETS_FOUND=0",
    ] {
        assert_eq!(redact_shell_output_for_public_readback(input), input);
    }
    // Each name starts its own line: pins the (?im)^ environment regex, which
    // the mid-line test never reached on HEAD 126d5f73.
    let input = "token_count=42\nTOKENS=42\nSECRETARY=Alice\nAPI_TOKEN=actual-secret-39\nSECRET_KEY=another-secret-39\n";
    let redacted = redact_shell_output_for_public_readback(input);
    assert_eq!(
        redacted,
        format!(
            "token_count=42\nTOKENS=42\nSECRETARY=Alice\nAPI_TOKEN={MASK}\nSECRET_KEY={MASK}\n"
        )
    );
}

#[test]
fn must_fail_durable_stream_masks_mid_line_secrets() {
    let mut redactor = StreamRedactor::new();
    let mut out = redactor.push(b"L01 SECRET_KEY=durable-secret-val\nexport API_TOKEN=durable2\n");
    out.extend(redactor.finish());
    let text = String::from_utf8_lossy(&out);
    assert!(!text.contains("durable-secret-val"), "{text}");
    assert!(!text.contains("durable2"), "{text}");
}
