//! #240: the degraded `/health` payload inserted its `chrome_bridge` entry
//! twice; the second insert silently overwrote the first, so the reported row
//! depended on statement order. A `BTreeMap` output can never show the
//! duplicate, so the output alone cannot prove the removal: a source guard
//! pins ONE `chrome_bridge` insert per health builder, and the behaviour tests
//! pin the single entry's value that `synapse-setup.ps1` reads.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::degraded_health_payload;

const TRANSPORT_SRC: &str = include_str!("../transport.rs");
const HEALTH_SRC: &str = include_str!("../../server/health.rs");
const BRIDGE_INSERT: &str = "\"chrome_bridge\".to_owned()";

/// Body of `fn <name>(` up to the first line that is exactly `}` (top-level fn end).
fn fn_body<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} not found"));
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .or_else(|| rest.find("\n}\r\n"))
        .expect("fn end");
    &rest[..end]
}

fn bridge_inserts(body: &str) -> usize {
    body.matches(BRIDGE_INSERT).count()
}

#[test]
fn must_pass_degraded_health_inserts_chrome_bridge_exactly_once() {
    assert_eq!(
        bridge_inserts(fn_body(TRANSPORT_SRC, "degraded_health_payload")),
        1,
        "degraded_health_payload must insert chrome_bridge once (one source of truth)"
    );
}

#[test]
fn must_pass_full_health_inserts_chrome_bridge_exactly_once() {
    assert_eq!(
        bridge_inserts(fn_body(
            HEALTH_SRC,
            "health_payload_with_http_sessions_and_session"
        )),
        1,
        "full health payload must insert chrome_bridge once"
    );
}

#[test]
fn must_fail_guard_flags_a_double_insert() {
    let doubled = "fn degraded_health_payload(reason: &str) -> Health {\n    \
                   subsystems.insert(\"chrome_bridge\".to_owned(), a);\n    \
                   subsystems.insert(\"chrome_bridge\".to_owned(), b);\n}\n";
    assert_eq!(
        bridge_inserts(fn_body(doubled, "degraded_health_payload")),
        2
    );
}

#[test]
fn must_pass_degraded_chrome_bridge_row_is_unknown_with_machine_reason() {
    for (reason, expected) in [
        ("health payload exceeded 3000 ms", "health_timeout"),
        ("health blocking task failed", "health_unavailable"),
    ] {
        let health = degraded_health_payload(reason);
        let bridge = health
            .subsystems
            .get("chrome_bridge")
            .expect("chrome_bridge row");
        assert_eq!(bridge.status, "unknown", "reason {reason}");
        assert_eq!(bridge.detail.as_deref(), Some(expected), "reason {reason}");
        assert!(!health.ok);
        let json = serde_json::to_string(&health).expect("serialize health");
        assert_eq!(json.matches("\"chrome_bridge\"").count(), 1, "{json}");
    }
}
