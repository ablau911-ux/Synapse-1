//! #140 regression: `/health` has an absolute bound. A payload that stalls
//! (std-mutex contention, slow registry/process reads) must be answered with a
//! degraded payload instead of holding the response open.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use synapse_core::{Health, SubsystemHealth};

fn payload(ok: bool, marker: &str) -> Health {
    let mut subsystems = BTreeMap::new();
    subsystems.insert(
        marker.to_owned(),
        SubsystemHealth {
            status: if ok { "ok" } else { "degraded" }.to_owned(),
            ..SubsystemHealth::default()
        },
    );
    Health {
        ok,
        version: "test".to_owned(),
        build: "test".to_owned(),
        pid: 1,
        uptime_s: 0,
        tool_count: 0,
        tool_surface_sha256: String::new(),
        tool_names: Vec::new(),
        subsystems,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_stalled_health_payload_answers_degraded_within_the_bound() {
    let started = Instant::now();
    let health = super::bounded_health_payload(
        || {
            std::thread::sleep(Duration::from_millis(1_500));
            payload(true, "full")
        },
        Duration::from_millis(50),
        |reason| {
            assert!(reason.contains("50 ms"), "reason names the bound: {reason}");
            payload(false, "fallback")
        },
    )
    .await;
    assert!(!health.ok);
    assert!(health.subsystems.contains_key("fallback"));
    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "degraded answer must not wait for the stalled payload: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_fast_health_payload_is_returned_unchanged() {
    let health = super::bounded_health_payload(
        || payload(true, "full"),
        Duration::from_secs(10),
        |_reason| payload(false, "fallback"),
    )
    .await;
    assert!(health.ok);
    assert!(health.subsystems.contains_key("full"));
}

/// #140 rework: `synapse-setup.ps1` reads `subsystems.chrome_bridge`; the
/// degraded payload must carry it (unknown + reason) instead of omitting it.
#[test]
fn must_fail_degraded_payload_carries_chrome_bridge_unknown_with_health_timeout_reason() {
    let health = super::degraded_health_payload("health payload exceeded 3000 ms");
    let bridge = health.subsystems.get("chrome_bridge");
    assert_eq!(bridge.map(|entry| entry.status.as_str()), Some("unknown"));
    assert_eq!(
        bridge.and_then(|entry| entry.detail.as_deref()),
        Some("health_timeout")
    );
    assert!(health.subsystems.contains_key("health"));
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_timed_out_health_answer_has_chrome_bridge_field() {
    let health = super::bounded_health_payload(
        || {
            std::thread::sleep(Duration::from_millis(500));
            payload(true, "full")
        },
        Duration::from_millis(20),
        super::degraded_health_payload,
    )
    .await;
    assert!(!health.ok);
    assert_eq!(
        health
            .subsystems
            .get("chrome_bridge")
            .and_then(|entry| entry.detail.as_deref()),
        Some("health_timeout")
    );
}

#[test]
fn must_fail_degraded_payload_is_not_ok_and_carries_the_reason() {
    let health = super::degraded_health_payload("probe stalled");
    assert!(!health.ok);
    assert_eq!(health.pid, std::process::id());
    let subsystem = health.subsystems.get("health");
    assert_eq!(
        subsystem.map(|entry| entry.status.as_str()),
        Some("degraded")
    );
    assert_eq!(
        subsystem.and_then(|entry| entry.detail.as_deref()),
        Some("probe stalled")
    );
}
