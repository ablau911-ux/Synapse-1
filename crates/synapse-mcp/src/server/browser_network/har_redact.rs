//! Credential-header redaction for recorded HAR files (#184).

/// Value stored in place of a credential header value.
pub(super) const HAR_REDACTED_VALUE: &str = "[REDACTED]";

/// Headers whose values carry credentials and must never be persisted.
const SENSITIVE_HAR_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

pub(super) fn is_sensitive_har_header(name: &str) -> bool {
    SENSITIVE_HAR_HEADERS
        .iter()
        .any(|sensitive| name.eq_ignore_ascii_case(sensitive))
}

/// Returns the value to store for `name`: redacted for credential headers.
pub(super) fn har_header_value_for_storage(name: &str, value: String) -> String {
    if is_sensitive_har_header(name) {
        HAR_REDACTED_VALUE.to_owned()
    } else {
        value
    }
}

/// True for a header whose stored value is the redaction marker; replaying it
/// would hand the page a junk credential header.
pub(super) fn is_redacted_har_header(name: &str, value: &str) -> bool {
    is_sensitive_har_header(name) && value == HAR_REDACTED_VALUE
}
