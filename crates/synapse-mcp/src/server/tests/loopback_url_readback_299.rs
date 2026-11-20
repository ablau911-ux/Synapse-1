//! #299 MY TDD contract: an agent must be able to check which page its own tab
//! shows. Public readback keeps the PATH of loopback URLs (`localhost`,
//! `*.localhost`, 127.0.0.0/8, `::1`); query, fragment and credentials stay
//! redacted, every other host keeps the full #185 policy, and the daemon log
//! keeps redacting loopback paths too.

use super::{RedactedUrl, redact_url_for_public_readback};

#[test]
fn must_fail_loopback_readback_keeps_the_path() {
    for (url, public) in [
        (
            "http://localhost:3016/garage/cards/12",
            "http://localhost:3016/garage/cards/12",
        ),
        (
            "http://127.0.0.1:8780/adam/GMA16/issues/299",
            "http://127.0.0.1:8780/adam/GMA16/issues/299",
        ),
        ("http://127.12.0.9/a/b", "http://127.12.0.9/a/b"),
        ("http://[::1]:7700/x/y", "http://[::1]:7700/x/y"),
        (
            "http://gitea.localhost:8780/adam/GMA16",
            "http://gitea.localhost:8780/adam/GMA16",
        ),
        (
            "http://LOCALHOST:3016/Garage",
            "http://localhost:3016/Garage",
        ),
    ] {
        assert_eq!(redact_url_for_public_readback(url), public, "{url}");
    }
}

#[test]
fn must_fail_loopback_query_fragment_and_credentials_stay_redacted() {
    let public = redact_url_for_public_readback(
        "http://user:hunter2@localhost:3016/user/login?redirect_to=SECRET123#FRAG456",
    );
    for secret in ["hunter2", "SECRET123", "FRAG456", "user:"] {
        assert!(!public.contains(secret), "{secret} leaked: {public}");
    }
    assert_eq!(public, "http://localhost:3016/user/login?redacted#redacted");
    assert_eq!(
        redact_url_for_public_readback("view-source:http://localhost:3016/a/b?c=1"),
        "view-source:http://localhost:3016/a/b?redacted"
    );
}

#[test]
fn must_fail_other_hosts_keep_the_full_redaction() {
    for (url, public) in [
        (
            "https://example.com/docs/page",
            "https://example.com/redacted",
        ),
        (
            "http://localhost.evil.test/a/b",
            "http://localhost.evil.test/redacted",
        ),
        (
            "http://127.0.0.1.nip.io/a/b",
            "http://127.0.0.1.nip.io/redacted",
        ),
        (
            "http://10.0.0.5:3016/garage",
            "http://10.0.0.5:3016/redacted",
        ),
    ] {
        assert_eq!(redact_url_for_public_readback(url), public, "{url}");
    }
}

#[test]
fn must_fail_the_daemon_log_still_redacts_loopback_paths() {
    assert_eq!(
        RedactedUrl("http://localhost:3016/garage/cards/12?tab=2").to_string(),
        "http://localhost:3016/redacted?redacted"
    );
}
