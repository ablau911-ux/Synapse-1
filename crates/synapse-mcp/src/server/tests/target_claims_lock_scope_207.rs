//! #207: the global claims mutex must never be held across the WAL-synced
//! command-audit writes, and audit rows must not grow with the total claim
//! count. Claim/release/snapshot helpers return owned data and drop the guard.
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use super::{
    ClaimRefusal, MAX_TARGET_CLAIMS_PER_SESSION, SessionTarget, SharedTargetClaims,
    TargetClaimRegistry, attempt_claim, attempt_release, bounded_claims_view, snapshot_claims,
};

fn shared() -> SharedTargetClaims {
    Arc::new(Mutex::new(TargetClaimRegistry::default()))
}

fn live(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn window(hwnd: i64) -> SessionTarget {
    SessionTarget::Window { hwnd }
}

// Contract (#207, `target_claims.rs` `claims_audit_intent` / `claims_audit_final`
// doc): the claims mutex is never held across the WAL-synced command-audit
// writes. `audit_claims_lock_probe` records, for every audit write on this
// thread, whether the mutex was free at write time; the tests drive real
// handler paths and require every recorded write to see it free.

const CLIENT: &str = "claude-code";

fn isolated_service() -> (tempfile::TempDir, crate::server::SynapseService) {
    let root = tempfile::tempdir().expect("temp dir");
    let service = crate::server::SynapseService::try_new_isolated_for_tests(root.path())
        .expect("isolated service");
    (root, service)
}

fn seed_live_session(service: &crate::server::SynapseService, session_id: &str, started_at: u64) {
    use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
    let state = rmcp::transport::streamable_http_server::session::SessionState::new(
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new(CLIENT, "0.0.0-test"),
        ),
    );
    assert!(
        service
            .session_registry_ref()
            .lock()
            .expect("session registry")
            .record_initialized(session_id, &state, "http", started_at),
        "precondition: {session_id} registered"
    );
}

/// Claims of `owner` that are still unexpired at the real current time — the
/// same expiry rule `prune_inactive` applies (`expires_at_unix_ms > now`), so
/// a precondition never counts a claim the handler will prune.
fn claims_of(service: &crate::server::SynapseService, owner: &str) -> Vec<String> {
    let now = crate::server::session_registry::unix_time_ms_now();
    service
        .lock_target_claims()
        .expect("registry lock")
        .reads(now)
        .into_iter()
        .filter(|claim| claim.owner_session_id == owner && claim.expires_at_unix_ms > now)
        .map(|claim| claim.target_key)
        .collect()
}

/// Dashboard prune writes an intent and a final audit row around the prune;
/// both must see the claims mutex free, and the prune itself must happen.
#[tokio::test(flavor = "current_thread")]
async fn must_pass_dashboard_prune_audit_writes_run_with_the_claims_mutex_free() {
    let (_root, service) = isolated_service();
    let now = crate::server::session_registry::unix_time_ms_now();
    // A claim whose owner is not a live session: the prune must drop it.
    service
        .lock_target_claims()
        .expect("registry lock")
        .claim(
            "i207-ghost",
            window(0x2070),
            60_000,
            now,
            &live(&["i207-ghost"]),
        )
        .expect("seed ghost claim");
    assert_eq!(
        claims_of(&service, "i207-ghost"),
        vec!["window:0x2070".to_owned()],
        "precondition: ghost claim present"
    );

    super::audit_claims_lock_probe::start();
    let response = service
        .dashboard_target_claim_prune()
        .expect("dashboard prune");
    let observed = super::audit_claims_lock_probe::take();

    assert_eq!(
        observed,
        vec![true, true],
        "intent + final audit rows, each written with the claims mutex free"
    );
    assert_eq!(response.pruned_count, 1);
    assert_eq!(response.after_count, 0);
    assert!(claims_of(&service, "i207-ghost").is_empty());
}

/// `target_claim_adopt` (via `target_claim_adopt_for_session`) writes its
/// intent row, then re-checks the claim under the lock (`adopt_gate`). A
/// requester at the per-session cap is refused there, before any owner
/// teardown, and the closing error row is written. Both audit writes must see
/// the claims mutex free.
#[cfg(windows)]
#[tokio::test(flavor = "current_thread")]
async fn must_fail_adopt_refused_at_the_gate_writes_its_audit_rows_with_the_claims_mutex_free() {
    let (_root, service) = isolated_service();
    let now = crate::server::session_registry::unix_time_ms_now();
    let (owner, requester) = ("i207-owner", "i207-requester");
    seed_live_session(&service, owner, now.saturating_sub(1_000));
    seed_live_session(&service, requester, now);
    // #201: the owner is provably gone only when its recorded processes are
    // dead; u32::MAX / u32::MAX - 1 are never live pids.
    service
        .session_registry_ref()
        .lock()
        .expect("session registry")
        .record_spawned_agent(
            owner,
            crate::server::session_registry::SpawnedAgentRead {
                spawn_id: "i207-spawn".to_owned(),
                cli: "claude".to_owned(),
                launcher_process_id: u32::MAX,
                agent_process_id: Some(u32::MAX - 1),
                started_by_session_id: None,
                launched_at_unix_ms: now.saturating_sub(900),
                launch_target: "test".to_owned(),
                log_dir: "test".to_owned(),
                template_id: None,
                template_version: None,
                control: None,
            },
            now.saturating_sub(900),
        );
    let reads = service
        .session_registry_ref()
        .lock()
        .expect("session registry")
        .reads(now);
    for id in [owner, requester] {
        let read = reads
            .iter()
            .find(|read| read.session_id == id)
            .expect("precondition: session registered");
        assert_eq!(read.lifecycle, "live", "precondition: {id} live");
        assert_eq!(read.agent_kind, "claude", "precondition: {id} agent kind");
    }
    // The owner's claim sits on a real live top-level window
    // (`validate_claim_target` requires a live HWND).
    let hwnd = synapse_a11y::visible_top_level_window_contexts()
        .expect("enumerate visible top-level windows")
        .into_iter()
        .map(|context| context.hwnd)
        .find(|hwnd| synapse_a11y::foreground_context(*hwnd).is_ok())
        .expect("precondition: a live top-level window exists");
    // Requester at the cap with claims that are live at real time and outlast
    // the call (`attempt_claim` / `adopt_gate` count only unexpired claims of
    // live sessions). Fill handles skip the owner's window.
    let both = live(&[owner, requester]);
    let mut filled = 0_usize;
    let mut fill_hwnd = 0x7_0000_i64;
    while filled < MAX_TARGET_CLAIMS_PER_SESSION {
        fill_hwnd += 0x10;
        if fill_hwnd == hwnd {
            continue;
        }
        attempt_claim(
            &service.target_claims,
            requester,
            window(fill_hwnd),
            600_000,
            now,
            &both,
        )
        .expect("registry lock")
        .result
        .expect("fill claim");
        filled += 1;
    }
    let target = super::canonical_claim_target(window(hwnd));
    let owner_key = super::target_key(&target);
    service
        .lock_target_claims()
        .expect("registry lock")
        .claim(owner, target, 60_000, now, &live(&[owner, requester]))
        .expect("seed owner claim");
    assert_eq!(claims_of(&service, owner), vec![owner_key.clone()]);
    assert_eq!(
        claims_of(&service, requester).len(),
        MAX_TARGET_CLAIMS_PER_SESSION,
        "precondition: requester at the cap with claims unexpired at real time"
    );
    assert!(
        !claims_of(&service, requester).contains(&owner_key),
        "precondition: the owner's window is not one of the requester's claims"
    );
    let params: super::TargetClaimAdoptParams = serde_json::from_value(serde_json::json!({
        "owner_session_id": owner,
        "target": { "kind": "window", "window_hwnd": hwnd },
    }))
    .expect("valid adopt params");

    super::audit_claims_lock_probe::start();
    let error = service
        .target_claim_adopt_for_session(requester.to_owned(), params)
        .await
        .err()
        .expect("a requester at the cap cannot adopt");
    let observed = super::audit_claims_lock_probe::take();

    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!(
            synapse_core::error_codes::TARGET_CLAIM_ADOPT_REFUSED
        )),
        "{error:?}"
    );
    assert!(
        error.message.contains("maximum number of target claims"),
        "{}",
        error.message
    );
    assert_eq!(
        observed,
        vec![true, true],
        "intent + closing error audit rows, each written with the claims mutex free"
    );
    assert_eq!(
        claims_of(&service, owner),
        vec![owner_key],
        "refused before teardown: the owner keeps its claim"
    );
}

#[test]
fn must_pass_attempt_claim_returns_the_claim_and_the_owned_claims_after() {
    let claims = shared();

    let attempt = attempt_claim(&claims, "a", window(0x1111), 10_000, 1_000, &live(&["a"]))
        .expect("registry lock");

    let (entry, outcome) = attempt.result.expect("claim succeeds");
    assert_eq!(outcome, "claimed");
    assert_eq!(entry.target_key, "window:0x1111");
    assert_eq!(attempt.claims_after.len(), 1);
    assert_eq!(attempt.claims_after[0].target_key, entry.target_key);
}

#[test]
fn must_fail_attempt_claim_conflict_reports_the_holder() {
    let claims = shared();
    let both = live(&["a", "b"]);
    attempt_claim(&claims, "a", window(0x2222), 10_000, 1_000, &both)
        .expect("registry lock")
        .result
        .expect("first claim");

    let attempt =
        attempt_claim(&claims, "b", window(0x2222), 10_000, 1_001, &both).expect("registry lock");

    let refusal = attempt.result.expect_err("live peer holds the target");
    let ClaimRefusal::Conflict(conflict) = refusal else {
        panic!("expected a conflict refusal, got {refusal:?}");
    };
    assert_eq!(conflict.entry.owner_session_id, "a");
    assert_eq!(attempt.claims_after.len(), 1);
    assert_eq!(attempt.claims_after[0].owner_session_id, "a");
}

#[test]
fn must_pass_attempt_release_returns_the_released_claim() {
    let claims = shared();
    let only_a = live(&["a"]);
    attempt_claim(&claims, "a", window(0x3333), 10_000, 1_000, &only_a)
        .expect("registry lock")
        .result
        .expect("claim");

    let attempt =
        attempt_release(&claims, "a", "window:0x3333", 1_500, &only_a).expect("registry lock");

    let released = attempt.result.expect("owner may release");
    assert_eq!(
        released.map(|entry| entry.target_key),
        Some("window:0x3333".to_owned())
    );
    assert!(attempt.claims_after.is_empty());
}

#[test]
fn must_pass_snapshot_claims_prunes_dead_owners() {
    let claims = shared();
    attempt_claim(&claims, "a", window(0x4444), 10_000, 1_000, &live(&["a"]))
        .expect("registry lock")
        .result
        .expect("claim");

    let snapshot = snapshot_claims(&claims, 1_001, &live(&["b"])).expect("registry lock");

    assert!(snapshot.is_empty(), "claim of a non-live owner is pruned");
    let kept = snapshot_claims(&claims, 1_002, &live(&["a", "b"])).expect("registry lock");
    assert!(kept.is_empty(), "a pruned claim stays pruned");
}

#[test]
fn must_pass_bounded_claims_view_does_not_grow_with_total_claim_count() {
    // 300 claims: 5 registries of 60 claims each (each session stays below
    // the per-session cap); the view only sees the combined claim list.
    let only_a = live(&["a"]);
    let mut all = Vec::new();
    for block in 0..5_i64 {
        let claims = shared();
        for index in 0..60_i64 {
            attempt_claim(
                &claims,
                "a",
                window(0x10_000 + block * 60 + index),
                10_000,
                1_000,
                &only_a,
            )
            .expect("registry lock")
            .result
            .expect("claim");
        }
        all.extend(snapshot_claims(&claims, 1_001, &only_a).expect("registry lock"));
    }
    assert_eq!(all.len(), 300);

    let view = bounded_claims_view(&all, "window:0x10000");

    assert_eq!(view["claim_count"], 300);
    assert_eq!(view["target_claim"]["target_key"], "window:0x10000");
    let encoded = serde_json::to_string(&view).expect("encode view");
    assert!(
        encoded.len() < 2_000,
        "audit view must stay O(1), was {} bytes",
        encoded.len()
    );
}

#[test]
fn must_fail_bounded_claims_view_without_matching_claim_has_null_target_claim() {
    let view = bounded_claims_view(&[], "window:0x1");

    assert_eq!(view["claim_count"], 0);
    assert!(view["target_claim"].is_null());
}

/// Fills `session` up to the per-session claim cap with distinct windows.
fn fill_to_cap(claims: &SharedTargetClaims, session: &str) {
    let live = live(&[session]);
    for index in 0..MAX_TARGET_CLAIMS_PER_SESSION {
        let hwnd = 0x7000 + i64::try_from(index).unwrap_or(0) * 0x10;
        let attempt =
            attempt_claim(claims, session, window(hwnd), 10_000, 1_000, &live).expect("claim");
        assert!(attempt.result.is_ok(), "claim {index} must succeed");
    }
}

#[test]
fn must_fail_a_claim_beyond_the_per_session_cap_is_refused_with_a_typed_error() {
    let claims = shared();
    fill_to_cap(&claims, "a");
    // The refusal is an attempt result (written through the caller's final
    // audit row), not an early error that would orphan the intent row.
    let attempt = attempt_claim(&claims, "a", window(0x6fff0), 10_000, 1_001, &live(&["a"]))
        .expect("registry lock");
    let refusal = attempt.result.expect_err("over-cap claim must be refused");
    assert!(matches!(refusal, ClaimRefusal::LimitReached), "{refusal:?}");
    let error = refusal.error("target_claim", "a", "target_claim");
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!(
            synapse_core::error_codes::TARGET_CLAIM_LIMIT_REACHED
        ))
    );
    assert_eq!(attempt.claims_after.len(), MAX_TARGET_CLAIMS_PER_SESSION);
}

#[test]
fn must_fail_adoption_at_the_cap_is_refused_before_the_owner_is_torn_down() {
    let claims = shared();
    fill_to_cap(&claims, "a");
    // b owns the target a wants to adopt.
    attempt_claim(
        &claims,
        "b",
        window(0x6ff00),
        10_000,
        1_000,
        &live(&["a", "b"]),
    )
    .expect("registry lock")
    .result
    .expect("b claims");
    let guard = claims.lock().expect("registry lock");
    let prior = guard
        .get(&super::target_key(&window(0x6ff00)))
        .expect("b's claim");
    let gate = guard.adopt_gate(&prior, "a", &window(0x6ff00));
    assert!(
        gate.is_err_and(|reason| reason.contains("maximum number of target claims")),
        "adoption at the cap must stop before the owner teardown"
    );
}

#[test]
fn must_pass_adoption_below_the_cap_passes_the_pre_teardown_gate() {
    let claims = shared();
    attempt_claim(
        &claims,
        "b",
        window(0x6ff00),
        10_000,
        1_000,
        &live(&["a", "b"]),
    )
    .expect("registry lock")
    .result
    .expect("b claims");
    let guard = claims.lock().expect("registry lock");
    let prior = guard
        .get(&super::target_key(&window(0x6ff00)))
        .expect("b's claim");
    assert_eq!(guard.adopt_gate(&prior, "a", &window(0x6ff00)), Ok(()));
}

#[test]
fn must_pass_renewing_a_held_claim_and_other_sessions_are_not_capped() {
    let claims = shared();
    fill_to_cap(&claims, "a");
    // Renewing one of a's own claims is not a new claim.
    let renewal = attempt_claim(&claims, "a", window(0x7000), 10_000, 1_002, &live(&["a"]))
        .expect("renewal at the cap");
    assert!(renewal.result.is_ok());
    // The cap is per session.
    let other = attempt_claim(
        &claims,
        "b",
        window(0x6fff0),
        10_000,
        1_002,
        &live(&["a", "b"]),
    )
    .expect("another session's claim");
    assert!(other.result.is_ok());
}
