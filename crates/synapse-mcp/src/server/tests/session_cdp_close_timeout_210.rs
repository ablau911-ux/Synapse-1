//! Regression tests for #210: a raw-CDP owner close against an endpoint that
//! accepts TCP but never answers must fail within a bound instead of hanging
//! the teardown (and every caller awaiting it) forever.
#![cfg(windows)]
#![allow(clippy::expect_used)]

use std::{
    net::TcpListener,
    num::NonZeroUsize,
    path::Path,
    time::{Duration, Instant},
};

use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

fn owner_on(endpoint: String) -> CdpTargetOwner {
    CdpTargetOwner {
        session_id: "session-210".to_owned(),
        window_hwnd: 0x2100,
        endpoint,
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: "TARGET210".to_owned(),
        requested_url: "http://127.0.0.1/test".to_owned(),
        target_url: "http://127.0.0.1/test".to_owned(),
        created_at_unix_ms: 1,
    }
}

#[tokio::test]
async fn must_fail_close_against_a_silent_endpoint_times_out_with_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent listener");
    let port = listener.local_addr().expect("local addr").port();
    let owner = owner_on(format!("http://127.0.0.1:{port}"));

    let started = Instant::now();
    let outcome = close_cdp_target_bounded("TARGET210", &owner, Duration::from_millis(300)).await;

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close must be bounded, took {:?}",
        started.elapsed()
    );
    match outcome {
        Err(detail) => assert!(detail.contains("timed out after 300 ms"), "{detail}"),
        Ok(outcome) => panic!("a silent endpoint cannot close a target: {outcome:?}"),
    }
    drop(listener);
}

#[tokio::test]
async fn must_pass_a_refusing_endpoint_answers_promptly_without_a_timeout() {
    // Nothing listens on the port: the close attempt fails fast with a real
    // connection error, never the bounded-timeout error.
    let port = TcpListener::bind("127.0.0.1:0")
        .expect("bind probe listener")
        .local_addr()
        .expect("local addr")
        .port();
    let owner = owner_on(format!("http://127.0.0.1:{port}"));
    // Fixture precondition readback: the probe listener is gone, so the port
    // really refuses connections (it was not re-bound by another process).
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "precondition: 127.0.0.1:{port} must refuse connections"
    );

    let started = Instant::now();
    let outcome = close_cdp_target_bounded("TARGET210", &owner, Duration::from_secs(30)).await;
    let elapsed = started.elapsed();

    // Contract: `session_lifecycle.rs` `raw_cdp_close_error_outcome` — an
    // unreachable owner endpoint (`A11yError::CdpUnreachable`, a refused
    // connect) reclaims the row as `EndpointUnreachable`; only a real timeout
    // is the bounded-timeout error (`close_cdp_target_bounded` doc).
    assert_eq!(
        outcome,
        Ok(CdpCleanupCloseOutcome::EndpointUnreachable),
        "a refusing endpoint answers with the unreachable outcome, not a timeout"
    );
    // A refused connect on Windows retries for ~2 s (SYN retransmits), so the
    // ceiling sits well above that and well below the 30 s bound: a timeout
    // cannot hide inside it.
    assert!(
        elapsed < Duration::from_secs(10),
        "a refusing endpoint must fail fast, not wait for the 30 s bound, took {elapsed:?}"
    );
}

fn service(path: &Path) -> SynapseService {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(path.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).expect("nonzero"),
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
    .expect("service")
}

/// Drives the real session teardown path (`cleanup_session_cdp_targets`, which
/// uses the production close bound) against an endpoint that accepts TCP and
/// never answers. The close must fail (never reclaim): the persisted
/// `CF_SESSIONS` owner row has to survive so the retry path can take over.
/// Takes as long as the production attach/close bound (about 10 s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_a_timed_out_close_is_a_failure_not_a_reclaim() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent listener");
    let port = listener.local_addr().expect("local addr").port();
    let temp = tempfile::tempdir().expect("tempdir");
    let svc = service(temp.path());
    let owner = owner_on(format!("http://127.0.0.1:{port}"));
    let session_id = owner.session_id.clone();
    svc.register_cdp_target_owner(owner)
        .expect("register + persist owner row");
    let persisted_rows = |svc: &SynapseService| {
        crate::server::session_continuity::read_persisted_cdp_target_owners_for_session(
            &svc.m3_state,
            &session_id,
        )
        .expect("read persisted owner rows")
        .len()
    };
    assert_eq!(
        persisted_rows(&svc),
        1,
        "owner row persisted before teardown"
    );

    let report =
        cleanup_session_cdp_targets(&svc.m3_state, &svc.cdp_target_owners, &session_id).await;

    assert_eq!(report.owned_before, 1, "{report:?}");
    assert_eq!(report.failed, 1, "a hung close is a failure: {report:?}");
    assert_eq!(report.closed, 0, "{report:?}");
    assert_eq!(report.already_absent, 0, "{report:?}");
    assert_eq!(report.endpoint_unreachable, 0, "{report:?}");
    assert_eq!(report.persisted_rows_deleted, 0, "{report:?}");
    assert!(
        report
            .error_messages
            .iter()
            .any(|message| message.contains("timed out")),
        "the failure must name the timeout: {report:?}"
    );
    assert_eq!(
        persisted_rows(&svc),
        1,
        "the CF_SESSIONS owner row must be kept for the retry path"
    );
    drop(listener);
}
