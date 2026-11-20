use serde_json::Value;
use sha2::{Digest as _, Sha256};
pub(super) fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Keyed `hmac-sha256:` digest of text (#174), e.g. an MCP session id.
pub(super) fn sha256_text(value: &str) -> String {
    keyed_sha256(value.as_bytes())
}

/// Keyed `hmac-sha256:` digest (#174) of one replay/lifecycle line. Lines carry
/// recorded user/agent content and error text, so a plain SHA-256 would be
/// brute-forceable; equal lines still give equal digests within one install.
pub(super) fn keyed_sha256(bytes: &[u8]) -> String {
    crate::content_digest::keyed_prefixed_digest(bytes)
}

/// Plain SHA-256 on purpose (#174): used for the digest of a whole artifact
/// file, which operators compare against `sha256sum` of the file on disk.
pub(super) fn prefixed_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}
