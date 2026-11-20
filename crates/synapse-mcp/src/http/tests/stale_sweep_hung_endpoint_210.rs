//! Regression test for #210: one hung CDP endpoint must not stop the stale
//! sweep from releasing expired input leases for the whole daemon.
//!
//! A stale session owns a CDP tab on an endpoint that accepts TCP but never
//! answers HTTP. Before the fix the single sequential sweep awaited that
//! teardown forever, so no later lease expiry was ever released.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{net::TcpListener, sync::Arc, time::Duration};

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::http::stale_sweep::spawn as spawn_stale_session_input_cleanup;
use crate::{server::CdpTargetOwner, test_support};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_hung_cdp_teardown_does_not_stop_lease_expiry_release() {
    let _serial = test_support::lease_serial("stale_sweep_hung_210");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");

    // Stale session whose CDP owner endpoint never answers.
    let silent = TcpListener::bind("127.0.0.1:0").expect("bind silent listener");
    let port = silent.local_addr().expect("local addr").port();
    service
        .cdp_target_owners_ref()
        .lock()
        .expect("owners")
        .insert(
            "owner-210".to_owned(),
            CdpTargetOwner {
                session_id: "stale-cdp-session-210".to_owned(),
                window_hwnd: 0x2100,
                endpoint: format!("http://127.0.0.1:{port}"),
                chrome_window_id: None,
                capture_window_hwnd: None,
                cdp_target_id: "TARGET210".to_owned(),
                requested_url: "http://127.0.0.1/test".to_owned(),
                target_url: "http://127.0.0.1/test".to_owned(),
                created_at_unix_ms: 1,
            },
        );
    assert!(matches!(
        synapse_action::lease::try_acquire(
            "lease-owner-210",
            synapse_action::lease::ttl_from_ms(synapse_action::MIN_LEASE_TTL_MS),
        ),
        synapse_action::LeaseOutcome::Acquired(_)
    ));
    assert_lease_held_by("lease-owner-210");

    let cancel = CancellationToken::new();
    let sweeper = spawn_stale_session_input_cleanup(
        Arc::new(LocalSessionManager::default()),
        lifecycle,
        super::super::inflight_ids::InflightRequestIds::default(),
        cancel.clone(),
    );

    // Let the lease lapse while the first tick is stuck on the silent endpoint,
    // then record the expiry exactly like a health/status read would.
    tokio::time::sleep(Duration::from_millis(
        synapse_action::MIN_LEASE_TTL_MS + 300,
    ))
    .await;
    let _status = synapse_action::lease::status();

    let mut drained = false;
    for _ in 0..50 {
        if synapse_action::lease::expired_cleanup_snapshot().is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cancel.cancel();
    let _joined = tokio::time::timeout(Duration::from_secs(5), sweeper).await;
    // Precondition proof: the stale teardown really dialed the silent endpoint
    // (otherwise the hang this test guards against never happened).
    silent.set_nonblocking(true).expect("nonblocking");
    let dialed = silent.accept();
    drop(silent);

    assert!(
        dialed.is_ok(),
        "stale teardown never connected to the silent CDP endpoint: {dialed:?}"
    );
    assert!(
        drained,
        "expired lease cleanup stayed pending while a teardown hung on a silent CDP endpoint"
    );
    assert_lease_released("lease-owner-210");
}

/// The lease is held by `owner` right now (taken before the sweeper starts).
fn assert_lease_held_by(owner: &str) {
    let status = synapse_action::lease::status();
    assert!(status.held, "precondition: lease must be held: {status:?}");
    assert_eq!(status.owner_session_id.as_deref(), Some(owner));
}

/// After the drain the owner holds no lease and has no pending expiry cleanup.
fn assert_lease_released(owner: &str) {
    let status = synapse_action::lease::status();
    assert!(!status.held, "lease still held after expiry: {status:?}");
    assert!(
        synapse_action::lease::expired_cleanup_snapshot()
            .iter()
            .all(|pending| pending.owner_session_id.as_deref() != Some(owner)),
        "{owner} still pending expired-lease cleanup"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_sweep_releases_expired_lease_with_no_stale_cdp_owner() {
    let _serial = test_support::lease_serial("stale_sweep_hung_210");
    let temp = TempDir::new().expect("temp dir");
    let service = test_support::service_in(temp.path()).expect("service");
    let lifecycle = service.session_lifecycle_state().expect("lifecycle");
    assert!(matches!(
        synapse_action::lease::try_acquire(
            "lease-owner-210-healthy",
            synapse_action::lease::ttl_from_ms(synapse_action::MIN_LEASE_TTL_MS),
        ),
        synapse_action::LeaseOutcome::Acquired(_)
    ));
    assert_lease_held_by("lease-owner-210-healthy");

    let cancel = CancellationToken::new();
    let sweeper = spawn_stale_session_input_cleanup(
        Arc::new(LocalSessionManager::default()),
        lifecycle,
        super::super::inflight_ids::InflightRequestIds::default(),
        cancel.clone(),
    );

    tokio::time::sleep(Duration::from_millis(
        synapse_action::MIN_LEASE_TTL_MS + 300,
    ))
    .await;
    let _status = synapse_action::lease::status();

    let mut drained = false;
    for _ in 0..50 {
        if synapse_action::lease::expired_cleanup_snapshot().is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(5), sweeper).await;

    assert!(
        drained,
        "expired lease cleanup stayed pending with no stale owner"
    );
    assert!(joined.is_ok(), "sweeper must stop promptly after cancel");
    assert_lease_released("lease-owner-210-healthy");
}
