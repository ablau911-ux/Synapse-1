//! Per-spawn scoped HTTP credentials (#22).
//!
//! `act_spawn_agent` never hands a child the daemon master token. Instead it
//! issues `HMAC-SHA256(master, domain || spawn_id || nonce)` and passes it in
//! the environment variable the Claude/Codex/local-model runners already read
//! (`SYNAPSE_BEARER_TOKEN`). HTTP auth accepts that credential only for its own
//! spawn: one MCP session (bound at its first `initialize`), that spawn's
//! `/agent-events` ingress, and that spawn's Codex app-server request bridge.
//! The credential is revoked when its bound MCP session is torn down, or when
//! the spawn fails before handing the session off. Records live in daemon
//! memory only, so a daemon restart revokes every scoped credential.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};

use axum::http::{HeaderMap, Method, Uri};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Environment variable carrying the scoped credential into spawned agents.
/// Generated Claude MCP config/hooks, the Codex runner and notify script, and
/// the local-model runner read this name.
pub(crate) const SPAWN_CREDENTIAL_ENV: &str = "SYNAPSE_BEARER_TOKEN";
pub(crate) const CREDENTIAL_PREFIX: &str = "synapse-spawn-v1.";
const HMAC_DOMAIN: &[u8] = b"synapse-spawn-credential-v1";
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";
/// Mirrors `server::permission_gate::SPAWN_ID_HEADER`.
const SPAWN_ID_HEADER: &str = "x-synapse-spawn-id";

static REGISTRY: LazyLock<SpawnCredentialRegistry> =
    LazyLock::new(SpawnCredentialRegistry::default);

pub(crate) fn registry() -> &'static SpawnCredentialRegistry {
    &REGISTRY
}

/// RFC 2104 HMAC over SHA-256 (verified against RFC 4231 vectors in tests).
pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let digest: [u8; 32] = Sha256::digest(key).into();
        block_key[..32].copy_from_slice(&digest);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block_key.map(|byte| byte ^ 0x36));
    inner.update(message);
    let inner_digest: [u8; 32] = inner.finalize().into();
    let mut outer = Sha256::new();
    outer.update(block_key.map(|byte| byte ^ 0x5c));
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Bearer value for an in-process Synapse client: a scoped spawn credential
/// from the environment wins over the operator token file, so a spawned
/// local-model runner never falls back to the master token.
pub(crate) fn client_bearer_token() -> anyhow::Result<String> {
    if let Ok(value) = std::env::var(SPAWN_CREDENTIAL_ENV) {
        let value = value.trim();
        if value.starts_with(CREDENTIAL_PREFIX) {
            return Ok(value.to_owned());
        }
    }
    crate::http::load_token_value()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpawnCredentialError {
    InvalidSpawnId,
    EmptyMaster,
    DuplicateSpawn,
    RegistryUnavailable,
}

impl std::fmt::Display for SpawnCredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSpawnId => "spawn id is empty or not ASCII alphanumerics and dashes",
            Self::EmptyMaster => "daemon master token is empty",
            Self::DuplicateSpawn => "a scoped credential is already live for this spawn id",
            Self::RegistryUnavailable => "scoped credential registry lock is poisoned",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScopeRefusal {
    UnknownCredential,
    SpawnMismatch,
    SessionMismatch,
    SessionAlreadyBound,
    RouteOutOfScope,
    RegistryUnavailable,
}

impl ScopeRefusal {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::UnknownCredential => "scoped_credential_unknown_or_revoked",
            Self::SpawnMismatch => "scoped_credential_spawn_mismatch",
            Self::SessionMismatch => "scoped_credential_session_mismatch",
            Self::SessionAlreadyBound => "scoped_credential_session_already_bound",
            Self::RouteOutOfScope => "scoped_credential_route_out_of_scope",
            Self::RegistryUnavailable => "scoped_credential_registry_unavailable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Binding {
    Unbound,
    Initializing,
    Bound(String),
}

struct CredentialRecord {
    digest: [u8; 32],
    binding: Binding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScopedRoute<'a> {
    Mcp {
        session_id: Option<&'a str>,
        is_post: bool,
    },
    AgentEvents {
        query: Option<&'a str>,
    },
    CodexBridge,
    /// `GET /health`, probed by the `--mode connect` adapter; the same payload
    /// is available to the session through the `health` tool.
    Health,
    OutOfScope,
}

/// The parts of an HTTP request a scoped credential is checked against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScopedRequest<'a> {
    route: ScopedRoute<'a>,
    spawn_header: Option<&'a str>,
}

impl<'a> ScopedRequest<'a> {
    pub(crate) fn from_http(method: &Method, uri: &'a Uri, headers: &'a HeaderMap) -> Self {
        let path = uri.path();
        let route = if path == "/mcp" || path.starts_with("/mcp/") {
            ScopedRoute::Mcp {
                session_id: header_str(headers, MCP_SESSION_ID_HEADER),
                is_post: *method == Method::POST,
            }
        } else if path == "/agent-events" && *method == Method::POST {
            ScopedRoute::AgentEvents { query: uri.query() }
        } else if path == "/codex-app-server/request" && *method == Method::POST {
            ScopedRoute::CodexBridge
        } else if path == "/health" && *method == Method::GET {
            ScopedRoute::Health
        } else {
            ScopedRoute::OutOfScope
        };
        Self {
            route,
            spawn_header: header_str(headers, SPAWN_ID_HEADER),
        }
    }

    /// The Codex request bridge names its spawn in the JSON body, which the
    /// HTTP layer must compare with the granted spawn before routing.
    pub(crate) const fn requires_body_spawn_id(&self) -> bool {
        matches!(self.route, ScopedRoute::CodexBridge)
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) enum ScopedGrant<'r> {
    Request { spawn_id: String },
    Initialize(InitializeClaim<'r>),
}

impl ScopedGrant<'_> {
    pub(crate) fn spawn_id(&self) -> &str {
        match self {
            Self::Request { spawn_id } => spawn_id,
            Self::Initialize(claim) => &claim.spawn_id,
        }
    }
}

/// Exclusive right to bind the credential's single MCP session. Dropping it
/// without `bind` (failed or cancelled initialize) re-opens the slot.
pub(crate) struct InitializeClaim<'r> {
    registry: &'r SpawnCredentialRegistry,
    spawn_id: String,
    settled: bool,
}

impl InitializeClaim<'_> {
    pub(crate) fn bind(mut self, session_id: Option<&str>) {
        self.settled = true;
        self.registry.settle(&self.spawn_id, session_id);
    }
}

impl Drop for InitializeClaim<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.registry.settle(&self.spawn_id, None);
        }
    }
}

#[derive(Default)]
pub(crate) struct SpawnCredentialRegistry {
    records: Mutex<BTreeMap<String, CredentialRecord>>,
}

impl SpawnCredentialRegistry {
    pub(crate) fn issue(
        &self,
        master: &str,
        spawn_id: &str,
    ) -> Result<String, SpawnCredentialError> {
        if spawn_id.is_empty()
            || !spawn_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return Err(SpawnCredentialError::InvalidSpawnId);
        }
        if master.trim().is_empty() {
            return Err(SpawnCredentialError::EmptyMaster);
        }
        let mut nonce = [0_u8; 32];
        nonce[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        nonce[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let mut message = Vec::with_capacity(HMAC_DOMAIN.len() + spawn_id.len() + 34);
        message.extend_from_slice(HMAC_DOMAIN);
        message.push(0);
        message.extend_from_slice(spawn_id.as_bytes());
        message.push(0);
        message.extend_from_slice(&nonce);
        let mac = hmac_sha256(master.trim().as_bytes(), &message);
        let mut credential = String::with_capacity(CREDENTIAL_PREFIX.len() + 64);
        credential.push_str(CREDENTIAL_PREFIX);
        for byte in mac {
            let _ = write!(credential, "{byte:02x}");
        }
        let mut records = self
            .records
            .lock()
            .map_err(|_| SpawnCredentialError::RegistryUnavailable)?;
        if records.contains_key(spawn_id) {
            return Err(SpawnCredentialError::DuplicateSpawn);
        }
        records.insert(
            spawn_id.to_owned(),
            CredentialRecord {
                digest: digest(&credential),
                binding: Binding::Unbound,
            },
        );
        Ok(credential)
    }

    pub(crate) fn is_live_credential(&self, candidate: &str) -> bool {
        self.records
            .lock()
            .is_ok_and(|records| find_spawn(&records, candidate).is_some())
    }

    pub(crate) fn authorize(
        &self,
        credential: &str,
        request: &ScopedRequest<'_>,
    ) -> Result<ScopedGrant<'_>, ScopeRefusal> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| ScopeRefusal::RegistryUnavailable)?;
        let spawn_id = find_spawn(&records, credential).ok_or(ScopeRefusal::UnknownCredential)?;
        if request
            .spawn_header
            .is_some_and(|claimed| claimed != spawn_id)
        {
            return Err(ScopeRefusal::SpawnMismatch);
        }
        let record = records
            .get_mut(&spawn_id)
            .ok_or(ScopeRefusal::UnknownCredential)?;
        match request.route {
            ScopedRoute::Mcp {
                session_id: Some(session_id),
                ..
            } => match &record.binding {
                Binding::Bound(bound) if bound == session_id => {
                    Ok(ScopedGrant::Request { spawn_id })
                }
                _ => Err(ScopeRefusal::SessionMismatch),
            },
            ScopedRoute::Mcp {
                session_id: None,
                is_post,
            } => {
                if !is_post || record.binding != Binding::Unbound {
                    return Err(ScopeRefusal::SessionAlreadyBound);
                }
                record.binding = Binding::Initializing;
                Ok(ScopedGrant::Initialize(InitializeClaim {
                    registry: self,
                    spawn_id,
                    settled: false,
                }))
            }
            ScopedRoute::AgentEvents { query } => {
                if query_names_only_spawn(query, &spawn_id) {
                    Ok(ScopedGrant::Request { spawn_id })
                } else {
                    Err(ScopeRefusal::SpawnMismatch)
                }
            }
            ScopedRoute::CodexBridge | ScopedRoute::Health => Ok(ScopedGrant::Request { spawn_id }),
            ScopedRoute::OutOfScope => Err(ScopeRefusal::RouteOutOfScope),
        }
    }

    pub(crate) fn revoke_spawn(&self, spawn_id: &str) -> bool {
        self.records
            .lock()
            .is_ok_and(|mut records| records.remove(spawn_id).is_some())
    }

    /// Session teardown hook: revokes the credential bound to `session_id`.
    pub(crate) fn revoke_session(&self, session_id: &str) -> Option<String> {
        let mut records = self.records.lock().ok()?;
        let spawn_id = records
            .iter()
            .find(|(_, record)| matches!(&record.binding, Binding::Bound(bound) if bound == session_id))
            .map(|(spawn_id, _)| spawn_id.clone())?;
        records.remove(&spawn_id);
        Some(spawn_id)
    }

    fn settle(&self, spawn_id: &str, session_id: Option<&str>) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        if let Some(record) = records.get_mut(spawn_id)
            && record.binding == Binding::Initializing
        {
            record.binding = session_id.map_or(Binding::Unbound, |session| {
                Binding::Bound(session.to_owned())
            });
        }
    }
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn find_spawn(records: &BTreeMap<String, CredentialRecord>, candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if !candidate.starts_with(CREDENTIAL_PREFIX) {
        return None;
    }
    let candidate_digest = digest(candidate);
    records
        .iter()
        .find(|(_, record)| bool::from(record.digest.ct_eq(&candidate_digest)))
        .map(|(spawn_id, _)| spawn_id.clone())
}

fn query_names_only_spawn(query: Option<&str>, spawn_id: &str) -> bool {
    let mut named = query
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.strip_prefix("spawn_id="))
        .peekable();
    named.peek().is_some() && named.all(|value| value == spawn_id)
}

/// Spawn-side owner of a freshly issued credential. Dropping it revokes the
/// credential (spawn failed or was cancelled); `hand_off_to_session` moves
/// revocation to the bound session's teardown.
pub(crate) struct SpawnCredentialLease {
    spawn_id: String,
    credential: String,
    handed_off: bool,
}

impl SpawnCredentialLease {
    pub(crate) fn issue(spawn_id: &str) -> Result<Self, String> {
        let master = crate::http::load_token_value()
            .map_err(|error| format!("load daemon master token: {error:#}"))?;
        Self::issue_with_master(&master, spawn_id)
    }

    pub(crate) fn issue_with_master(master: &str, spawn_id: &str) -> Result<Self, String> {
        let credential = registry()
            .issue(master, spawn_id)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            spawn_id: spawn_id.to_owned(),
            credential,
            handed_off: false,
        })
    }

    pub(crate) fn credential(&self) -> &str {
        &self.credential
    }

    pub(crate) fn hand_off_to_session(mut self) {
        self.handed_off = true;
    }
}

impl Drop for SpawnCredentialLease {
    fn drop(&mut self) {
        if !self.handed_off {
            registry().revoke_spawn(&self.spawn_id);
        }
    }
}
