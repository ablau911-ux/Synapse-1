//! #207: registered/unregistered pair through the real claim handler.
#![cfg(windows)]
use super::*;
use crate::server::{
    CdpTargetOwner, g7a_support as f,
    own_window_fixture::{OwnWindows, WindowSpec},
};

fn fixture() -> (tempfile::TempDir, SynapseService, OwnWindows) {
    let (dir, svc) = f::service();
    f::seed(&svc, "fu2-207", f::now());
    let windows = OwnWindows::spawn(vec![WindowSpec {
        x: 20,
        y: 20,
        w: 200,
        h: 100,
        label: Some("FU2 claim fixture"),
    }]);
    assert_eq!(
        synapse_a11y::foreground_context(windows.hwnd(0))
            .expect("owned live HWND")
            .hwnd,
        windows.hwnd(0)
    );
    (dir, svc, windows)
}

fn params(hwnd: i64, id: &str) -> TargetClaimParams {
    TargetClaimParams {
        target: Some(TargetClaimTargetParam::Cdp {
            window_hwnd: hwnd,
            cdp_target_id: id.into(),
        }),
        ttl_ms: 120_000,
    }
}

#[tokio::test]
async fn must_pass_registered_cdp_claim_handler_records_exact_target() {
    let (_dir, svc, windows) = fixture();
    let hwnd = windows.hwnd(0);
    let id = "FU2REGISTERED207";
    svc.register_cdp_target_owner(CdpTargetOwner {
        session_id: "fu2-207".into(),
        window_hwnd: hwnd,
        endpoint: "http://127.0.0.1:9222".into(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: id.into(),
        requested_url: "about:blank#207".into(),
        target_url: "about:blank#207".into(),
        created_at_unix_ms: f::now(),
    })
    .expect("register fixture target");
    assert_eq!(
        svc.read_persisted_cdp_target_owners_for_target_id(id)
            .expect("owner readback")
            .len(),
        1
    );
    assert_eq!(
        svc.lock_target_claims()
            .expect("claims")
            .reads(f::now())
            .len(),
        0
    );
    let result = svc
        .target_claim(
            Parameters(params(hwnd, id)),
            crate::test_support::request_context_for_session("fu2-207").await,
        )
        .await
        .expect("registered claim")
        .0;
    assert_eq!(result.session_id, "fu2-207");
    assert_eq!(result.claim.owner_session_id, "fu2-207");
    let rows = svc.lock_target_claims().expect("readback").reads(f::now());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target_key, result.claim.target_key);
    assert_eq!(rows[0].generation, result.claim.generation);
}

#[tokio::test]
async fn must_fail_unregistered_synthetic_cdp_claim_has_no_claim_or_audit_side_effect() {
    let (_dir, svc, windows) = fixture();
    let id = "FU2SYNTHETIC207";
    assert_eq!(
        svc.read_persisted_cdp_target_owners_for_target_id(id)
            .expect("owners")
            .len(),
        0
    );
    assert_eq!(
        svc.lock_target_claims()
            .expect("claims")
            .reads(f::now())
            .len(),
        0
    );
    let before = f::audit_rows(&svc);
    for _ in 0..2 {
        let error = svc
            .target_claim(
                Parameters(params(windows.hwnd(0), id)),
                crate::test_support::request_context_for_session("fu2-207").await,
            )
            .await
            .err()
            .expect("fabricated tab must be refused");
        assert_eq!(f::code(&error), error_codes::TARGET_WINDOW_NOT_FOUND);
        assert_eq!(
            svc.lock_target_claims()
                .expect("after")
                .reads(f::now())
                .len(),
            0
        );
        assert_eq!(f::audit_rows(&svc), before);
        assert_eq!(
            svc.read_persisted_cdp_target_owners_for_target_id(id)
                .expect("after owners")
                .len(),
            0
        );
    }
}
