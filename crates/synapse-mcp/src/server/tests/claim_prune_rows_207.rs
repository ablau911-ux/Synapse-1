//! #207. Parent: target_claims. EXPECT-RED: serialized prune rows embed registry.
use super::*;
use crate::server::g7a_support as f;

fn fill(svc: &SynapseService) -> Vec<String> {
    let now = f::now();
    let live = BTreeSet::from(["g7a-207-ghost".to_owned()]);
    let mut claims = svc.lock_target_claims().expect("claims");
    for hwnd in 0x20700..0x20720 {
        claims
            .claim(
                "g7a-207-ghost",
                SessionTarget::Window { hwnd },
                600_000,
                now,
                &live,
            )
            .expect("seed");
    }
    claims.prune_inactive(f::now(), &live);
    let keys: Vec<_> = claims
        .reads(f::now())
        .into_iter()
        .map(|r| r.target_key)
        .collect();
    assert_eq!(keys.len(), 32);
    keys
}

#[test]
fn must_pass_prune_keeps_exact_counts_without_serializing_unrelated_claims() {
    let (_dir, svc) = f::service();
    let keys = fill(&svc);
    assert_eq!(f::audit_rows(&svc).len(), 0);
    let response = svc.dashboard_target_claim_prune().expect("prune");
    assert_eq!(
        (
            response.before_count,
            response.after_count,
            response.pruned_count
        ),
        (32, 0, 32)
    );
    assert_eq!(
        svc.lock_target_claims()
            .expect("claims")
            .reads(f::now())
            .len(),
        0
    );
    let rows = f::audit_rows(&svc);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["phase"], "intent");
    assert_eq!(rows[1]["phase"], "final");
    for row in rows {
        let payload = serde_json::to_string(&row).expect("encode");
        for key in &keys {
            assert!(
                !payload.contains(key),
                "prune audit must contain counts, not every target: {key}"
            );
        }
    }
}

#[test]
fn must_fail_poisoned_claim_lock_refuses_prune_without_audit_or_claim_mutation() {
    let (_dir, svc) = f::service();
    let keys = fill(&svc);
    let claims = Arc::clone(&svc.target_claims);
    let panicked = std::thread::spawn(move || {
        let _guard = claims.lock().expect("lock");
        panic!("synthetic claims lock failure");
    })
    .join();
    assert!(panicked.is_err());
    assert!(svc.target_claims.is_poisoned());
    let before = f::audit_rows(&svc);
    let error = svc
        .dashboard_target_claim_prune()
        .expect_err("poisoned lock");
    assert_eq!(f::code(&error), error_codes::TOOL_INTERNAL_ERROR);
    assert_eq!(f::audit_rows(&svc), before);
    let guard = svc
        .target_claims
        .lock()
        .expect_err("remains poisoned")
        .into_inner();
    assert_eq!(
        guard
            .reads(f::now())
            .into_iter()
            .map(|r| r.target_key)
            .collect::<Vec<_>>(),
        keys
    );
}
