//! Credential classification for audit, storage, and ARIA readbacks.

use std::sync::LazyLock;

use regex::Regex;

use crate::content_digest::keyed_prefixed_digest;

pub(crate) const REDACTED: &str = "[REDACTED]";
pub(crate) const CREDENTIAL_CLASS: &str = "credential";

// Keep these names in sync with CREDENTIAL_NAMES in the extension's ARIA builder.
const CREDENTIAL_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "cookie",
    "authorization",
    "auth",
    "bearer",
    "api key",
    "apikey",
    "session",
    "sessionid",
    "csrf",
    "cvv",
    "cvc",
    "pin",
    "otp",
    "card number",
    "cardnumber",
    "cc number",
    "ccnumber",
    "credential",
    "private key",
    "privatekey",
    "passcode",
    "one time code",
    "one timecode",
    "onetime code",
    "onetimecode",
    "recovery code",
    "recoverycode",
];
const TOKEN_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "bearer ",
    "xoxb-",
    "xoxp-",
    "synapse-activation-v1-",
];
/// A known prefix only counts when this many characters follow it, so words
/// such as `sk-learn` stay readable.
const TOKEN_PREFIX_MIN_TAIL: usize = 8;
/// Shortest unbroken run of ASCII letters and digits that reads as an opaque
/// token. Paths, versions, dates and UUIDs break into shorter runs.
const OPAQUE_RUN_MIN_LEN: usize = 16;

static ACRONYM_BOUNDARY: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").ok());
static CAMEL_BOUNDARY: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").ok());

pub(crate) fn credential_name(name: &str) -> bool {
    let (Some(acronym_boundary), Some(camel_boundary)) =
        (ACRONYM_BOUNDARY.as_ref(), CAMEL_BOUNDARY.as_ref())
    else {
        return true;
    };
    let acronyms_split = acronym_boundary.replace_all(name, "$1 $2");
    let words_split = camel_boundary.replace_all(&acronyms_split, "$1 $2");
    matches_credential_parts(&words_split) || matches_credential_parts(name)
}

fn matches_credential_parts(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let parts: Vec<_> = lower
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .filter(|part| !part.is_empty())
        .collect();
    CREDENTIAL_NAMES.iter().any(|name| {
        let width = name.split_ascii_whitespace().count();
        parts
            .windows(width)
            .any(|parts| parts.iter().copied().eq(name.split_ascii_whitespace()))
    })
}

pub(crate) fn sensitive_control_attributes(input_type: &str, autocomplete: &str) -> bool {
    input_type.eq_ignore_ascii_case("password")
        || input_type.eq_ignore_ascii_case("hidden")
        || autocomplete.split_whitespace().any(|part| {
            let part = part.to_ascii_lowercase();
            part.starts_with("cc-") || part.contains("password") || part.contains("one-time-code")
        })
}

/// Token shape: a known token prefix followed by at least
/// `TOKEN_PREFIX_MIN_TAIL` characters, or an unbroken run of at least
/// `OPAQUE_RUN_MIN_LEN` ASCII letters and digits that mixes both (hex digests,
/// base64 and base62 keys, JWT segments). Sentences, URLs without such a run,
/// file paths, versions, dates, plain numbers and UUIDs stay readable.
pub(crate) fn token_like_value(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '\u{0060}']);
    known_token_prefix(value)
        || value.split_whitespace().any(|word| {
            let word =
                word.trim_matches(['"', '\'', '(', ')', '[', ']', '{', '}', '<', '>', ',', ';']);
            known_token_prefix(word)
                || word
                    .split(|ch: char| !ch.is_ascii_alphanumeric())
                    .any(opaque_run)
        })
}

fn known_token_prefix(value: &str) -> bool {
    TOKEN_PREFIXES.iter().any(|prefix| {
        value.len() >= prefix.len() + TOKEN_PREFIX_MIN_TAIL
            && value
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    })
}

fn opaque_run(run: &str) -> bool {
    run.len() >= OPAQUE_RUN_MIN_LEN
        && run.bytes().any(|byte| byte.is_ascii_alphabetic())
        && run.bytes().any(|byte| byte.is_ascii_digit())
}

/// Whether a storage key embeds a credential name or a token-shaped value,
/// so its reversible hex form must be withheld. The key is judged as text;
/// a `/` segment that is hex-encoded printable text is judged by its decoded
/// text, decoded once only, so whatever is still opaque after one decode
/// stays token-like. Binary keys have no readable words or long runs, and
/// the empty key has nothing to hide.
pub fn key_embeds_secret(key: &[u8]) -> bool {
    let text = String::from_utf8_lossy(key);
    let judged = text
        .split('/')
        .map(hex_text_segment)
        .collect::<Vec<_>>()
        .join("/");
    credential_name(&judged) || token_like_value(&judged) || judged.split('/').any(token_like_value)
}

/// A path segment written as even-length hex of printable text, decoded; any
/// other segment unchanged.
fn hex_text_segment(segment: &str) -> String {
    let decoded = (!segment.is_empty()
        && segment.len().is_multiple_of(2)
        && segment.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| {
        (0..segment.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(segment.get(index..index + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
    })
    .flatten()
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .filter(|text| !text.chars().any(char::is_control));
    decoded.unwrap_or_else(|| segment.to_owned())
}

/// Whether a rendered label carries a copy of the protected value.
pub(crate) fn label_contains_value(label: &str, value: &str) -> bool {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    label == value || (value.chars().take(3).count() == 3 && label.contains(&value))
}

pub(crate) fn classified_marker(class: &str) -> String {
    format!("[redacted:{class}]")
}

pub(crate) struct Fingerprint {
    pub(crate) len_bytes: u64,
    pub(crate) digest: String,
}

pub(crate) fn fingerprint(bytes: &[u8]) -> Fingerprint {
    Fingerprint {
        len_bytes: bytes.len() as u64,
        digest: keyed_prefixed_digest(bytes),
    }
}

#[cfg(test)]
#[path = "credential_redaction_key_table.rs"]
mod key_table;
