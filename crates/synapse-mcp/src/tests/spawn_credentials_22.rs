use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};

use crate::spawn_credentials::{
    ScopeRefusal, ScopedGrant, ScopedRequest, SpawnCredentialError, SpawnCredentialLease,
    SpawnCredentialRegistry, hmac_sha256, registry,
};

const MASTER: &str = "issue22-synthetic-master";
const SPAWN: &str = "agent-spawn-issue22-scope";

struct Probe {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
}

impl Probe {
    fn new(method: Method, uri: &str, headers: &[(&'static str, &str)]) -> Self {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).expect("synthetic header value"),
            );
        }
        Self {
            method,
            uri: uri.parse().expect("synthetic uri"),
            headers: map,
        }
    }

    fn request(&self) -> ScopedRequest<'_> {
        ScopedRequest::from_http(&self.method, &self.uri, &self.headers)
    }
}

fn mcp(session: Option<&str>) -> Probe {
    match session {
        Some(session) => Probe::new(Method::POST, "/mcp", &[("mcp-session-id", session)]),
        None => Probe::new(Method::POST, "/mcp", &[]),
    }
}

fn refusal(result: Result<ScopedGrant<'_>, ScopeRefusal>) -> ScopeRefusal {
    match result {
        Ok(grant) => panic!("expected refusal, got grant for {}", grant.spawn_id()),
        Err(refusal) => refusal,
    }
}

fn hex(bytes: [u8; 32]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[test]
fn must_pass_hmac_sha256_matches_rfc4231_vectors_22() {
    assert_eq!(
        hex(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        hex(hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn must_pass_issue_mints_unique_scoped_credentials_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    assert!(credential.starts_with("synapse-spawn-v1."));
    assert_eq!(credential.len(), "synapse-spawn-v1.".len() + 64);
    assert!(registry.is_live_credential(&credential));
    assert!(!registry.is_live_credential(MASTER));
    let other = registry
        .issue(MASTER, "agent-spawn-issue22-other")
        .expect("issue");
    assert_ne!(credential, other, "nonce must make credentials unique");
    assert!(registry.is_live_credential(&other));
}

#[test]
fn must_fail_issue_rejects_duplicate_invalid_id_and_empty_master_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    assert_eq!(
        registry.issue(MASTER, SPAWN),
        Err(SpawnCredentialError::DuplicateSpawn)
    );
    // The refused duplicate neither replaced nor added a credential: the
    // first one still authorizes, and exactly one record exists for SPAWN.
    assert!(registry.is_live_credential(&credential));
    let health = Probe::new(Method::GET, "/health", &[]);
    let grant = registry
        .authorize(&credential, &health.request())
        .expect("original credential stays valid after a refused duplicate");
    assert_eq!(grant.spawn_id(), SPAWN);
    drop(grant);

    for invalid in ["", "agent spawn", "agent-spawn\0x", "agent-spawn-ü"] {
        assert_eq!(
            registry.issue(MASTER, invalid),
            Err(SpawnCredentialError::InvalidSpawnId)
        );
        assert!(
            !registry.revoke_spawn(invalid),
            "refused id {invalid:?} must leave no record"
        );
    }
    let empty_master_id = "agent-spawn-issue22-empty-master";
    assert_eq!(
        registry.issue("  ", empty_master_id),
        Err(SpawnCredentialError::EmptyMaster)
    );
    assert!(
        !registry.revoke_spawn(empty_master_id),
        "refused empty master must leave no record"
    );

    assert!(registry.revoke_spawn(SPAWN), "the original record exists");
    assert!(
        !registry.revoke_spawn(SPAWN),
        "the refused duplicate added no second record"
    );
    assert!(!registry.is_live_credential(&credential));
}

#[test]
fn must_fail_scoped_credential_binds_exactly_one_mcp_session_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    let initialize = mcp(None);
    let Ok(ScopedGrant::Initialize(claim)) = registry.authorize(&credential, &initialize.request())
    else {
        panic!("first initialize must receive the binding claim");
    };
    let racing = refusal(registry.authorize(&credential, &initialize.request()));
    assert_eq!(racing, ScopeRefusal::SessionAlreadyBound);
    claim.bind(Some("session-a"));

    let own = mcp(Some("session-a"));
    assert!(matches!(
        registry.authorize(&credential, &own.request()),
        Ok(ScopedGrant::Request { .. })
    ));
    let delete = Probe::new(Method::DELETE, "/mcp", &[("mcp-session-id", "session-a")]);
    let delete_grant = registry.authorize(&credential, &delete.request());
    assert!(
        matches!(&delete_grant, Ok(ScopedGrant::Request { .. })),
        "own-session DELETE must be a plain scoped request"
    );
    assert_eq!(
        delete_grant.as_ref().map(|grant| grant.spawn_id()).ok(),
        Some(SPAWN)
    );
    drop(delete_grant);
    let foreign = mcp(Some("session-b"));
    let foreign = refusal(registry.authorize(&credential, &foreign.request()));
    assert_eq!(foreign, ScopeRefusal::SessionMismatch);
    let second = refusal(registry.authorize(&credential, &initialize.request()));
    assert_eq!(second, ScopeRefusal::SessionAlreadyBound);
    let sessionless_get = Probe::new(Method::GET, "/mcp", &[]);
    let get = refusal(registry.authorize(&credential, &sessionless_get.request()));
    assert_eq!(get, ScopeRefusal::SessionAlreadyBound);
}

#[test]
fn must_pass_failed_or_dropped_initialize_reopens_the_binding_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    let initialize = mcp(None);
    for attempt in 0..2 {
        let Ok(ScopedGrant::Initialize(claim)) =
            registry.authorize(&credential, &initialize.request())
        else {
            panic!("initialize attempt {attempt} must receive the claim");
        };
        if attempt == 0 {
            drop(claim);
        } else {
            claim.bind(None);
        }
    }
    let pending = mcp(Some("session-never-bound"));
    let pending = refusal(registry.authorize(&credential, &pending.request()));
    assert_eq!(pending, ScopeRefusal::SessionMismatch);
}

#[test]
fn must_fail_scoped_credential_routes_are_limited_to_its_own_spawn_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    let allowed = [
        Probe::new(
            Method::POST,
            &format!("/agent-events?spawn_id={SPAWN}&source=codex_notify"),
            &[],
        ),
        Probe::new(Method::POST, "/codex-app-server/request", &[]),
        Probe::new(Method::GET, "/health", &[]),
    ];
    for probe in &allowed {
        let grant = registry
            .authorize(&credential, &probe.request())
            .expect("in scope");
        assert_eq!(grant.spawn_id(), SPAWN);
    }
    assert!(allowed[1].request().requires_body_spawn_id());
    assert!(!allowed[0].request().requires_body_spawn_id());

    let duplicate = format!("/agent-events?spawn_id={SPAWN}&spawn_id=agent-spawn-other");
    let mismatched: [&str; 4] = [
        "/agent-events?spawn_id=agent-spawn-other&source=codex_notify",
        duplicate.as_str(),
        "/agent-events?source=codex_notify",
        "/agent-events",
    ];
    for uri in mismatched {
        let probe = Probe::new(Method::POST, uri, &[]);
        let refused = refusal(registry.authorize(&credential, &probe.request()));
        assert_eq!(refused, ScopeRefusal::SpawnMismatch, "{uri}");
    }
    for (method, uri) in [
        (Method::POST, "/shutdown"),
        (Method::GET, "/events"),
        (Method::GET, "/agent-events/stats"),
        (Method::POST, "/chrome-debugger/native/register"),
        (Method::GET, "/agent-events"),
        (Method::POST, "/health"),
    ] {
        let probe = Probe::new(method, uri, &[]);
        let refused = refusal(registry.authorize(&credential, &probe.request()));
        assert_eq!(refused, ScopeRefusal::RouteOutOfScope, "{uri}");
    }
}

#[test]
fn must_fail_spawn_header_and_unknown_credentials_are_refused_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    let impersonating = Probe::new(
        Method::POST,
        "/mcp",
        &[("x-synapse-spawn-id", "agent-spawn-someone-else")],
    );
    let refused = refusal(registry.authorize(&credential, &impersonating.request()));
    assert_eq!(refused, ScopeRefusal::SpawnMismatch);
    let own_header = Probe::new(Method::POST, "/mcp", &[("x-synapse-spawn-id", SPAWN)]);
    let grant = registry.authorize(&credential, &own_header.request());
    assert!(
        matches!(&grant, Ok(ScopedGrant::Initialize(_))),
        "own spawn header on a sessionless initialize must get the claim"
    );
    assert_eq!(
        grant.as_ref().map(|grant| grant.spawn_id()).ok(),
        Some(SPAWN)
    );
    drop(grant);

    let probe = Probe::new(Method::GET, "/health", &[]);
    let forged = format!("synapse-spawn-v1.{}", "0".repeat(64));
    for candidate in [
        MASTER,
        "",
        forged.as_str(),
        &credential[..credential.len() - 1],
    ] {
        let refused = refusal(registry.authorize(candidate, &probe.request()));
        assert_eq!(refused, ScopeRefusal::UnknownCredential);
    }
}

#[test]
fn must_fail_session_teardown_and_spawn_revocation_kill_the_credential_22() {
    let registry = SpawnCredentialRegistry::default();
    let credential = registry.issue(MASTER, SPAWN).expect("issue");
    let initialize = mcp(None);
    let Ok(ScopedGrant::Initialize(claim)) = registry.authorize(&credential, &initialize.request())
    else {
        panic!("initialize must receive the claim");
    };
    claim.bind(Some("session-a"));
    assert_eq!(registry.revoke_session("session-unknown"), None);
    assert_eq!(registry.revoke_session("session-a").as_deref(), Some(SPAWN));
    assert!(!registry.is_live_credential(&credential));
    let own = mcp(Some("session-a"));
    let refused = refusal(registry.authorize(&credential, &own.request()));
    assert_eq!(refused, ScopeRefusal::UnknownCredential);
    assert_eq!(registry.revoke_session("session-a"), None);

    let unbound = registry
        .issue(MASTER, "agent-spawn-issue22-unbound")
        .expect("issue");
    assert!(registry.revoke_spawn("agent-spawn-issue22-unbound"));
    assert!(!registry.revoke_spawn("agent-spawn-issue22-unbound"));
    assert!(!registry.is_live_credential(&unbound));
}

#[test]
fn must_pass_spawn_lease_revokes_on_failure_and_hands_off_on_success_22() {
    let failed_id = "agent-spawn-issue22-lease-failed";
    let failed = SpawnCredentialLease::issue_with_master(MASTER, failed_id).expect("lease");
    let failed_credential = failed.credential().to_owned();
    assert!(registry().is_live_credential(&failed_credential));
    drop(failed);
    assert!(!registry().is_live_credential(&failed_credential));

    let ready_id = "agent-spawn-issue22-lease-ready";
    let ready = SpawnCredentialLease::issue_with_master(MASTER, ready_id).expect("lease");
    let ready_credential = ready.credential().to_owned();
    ready.hand_off_to_session();
    assert!(registry().is_live_credential(&ready_credential));
    assert!(registry().revoke_spawn(ready_id));
}
