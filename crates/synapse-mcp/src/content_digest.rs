//! Keyed digests of user content (#136).
//!
//! Persisted readbacks (audit rows, error payloads, delta signatures) used to
//! embed a plain SHA-256 of field values, typed text and clipboard contents. A
//! low-entropy secret (a PIN, a single keystroke) is recovered from such a
//! digest by hashing candidates. Content digests are therefore HMAC-SHA256
//! under a per-install key:
//!
//! - the key is generated once and persisted DPAPI-wrapped (CurrentUser scope,
//!   purpose-specific entropy) under the local Synapse directory, so digests
//!   stay comparable across daemon restarts for forensics;
//! - without the key (an offline copy of the database or logs) a digest cannot
//!   be tested against candidate values;
//! - digests are NOT recomputable by clients: they are equality tokens inside
//!   this install only;
//! - if DPAPI is unavailable the key is held in memory only and
//!   `CONTENT_DIGEST_KEY_UNPROTECTED` is logged; an existing key file that
//!   cannot be read is never overwritten (`CONTENT_DIGEST_KEY_UNREADABLE`).
//!   Digests never fall back to an unkeyed hash;
//! - how the process key was obtained is observable (#269): `health` carries a
//!   `content_digest_key` row, `persistent` or `ephemeral` plus the reason
//!   code, never key material.
//!
//! DPAPI protects against an offline copy of the files, not against another
//! process running as the same Windows user.
//!
//! Digests of non-secret structural data (paths, row payloads, ids) keep using
//! plain SHA-256 at their own call sites; every digest of content a user typed,
//! read or copied must come from this module.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use synapse_core::SubsystemHealth;
use uuid::Uuid;

const KEY_LEN: usize = 32;
const KEY_FILE_NAME: &str = "content-digest-key.dpapi";
/// DPAPI purpose entropy: a blob minted for this key cannot be unwrapped by
/// any other Synapse DPAPI consumer.
const KEY_ENTROPY: &[u8] = b"synapse/content-digest-key/v1";

pub(crate) const WARN_KEY_UNPROTECTED: &str = "CONTENT_DIGEST_KEY_UNPROTECTED";
pub(crate) const WARN_KEY_UNREADABLE: &str = "CONTENT_DIGEST_KEY_UNREADABLE";

type DigestKey = [u8; KEY_LEN];

static INSTALL_KEY: OnceLock<(DigestKey, DigestKeyState)> = OnceLock::new();

/// How the process digest key was obtained (#269).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DigestKeyState {
    /// Loaded from, or newly installed in, the DPAPI-protected key file.
    Persistent,
    /// Held in memory for this process only; `reason` is a `WARN_KEY_*` code.
    Ephemeral { reason: &'static str },
}

/// HMAC-SHA256 of `bytes` under the per-install key, hex-encoded.
pub(crate) fn keyed_digest_hex(bytes: &[u8]) -> String {
    crate::server::escalation::hmac_sha256_hex(install_key(), bytes)
}

/// Keyed digest of text.
pub(crate) fn keyed_text_digest_hex(text: &str) -> String {
    keyed_digest_hex(text.as_bytes())
}

/// Marker for persisted keyed digests (`hmac-sha256:<hex>`); rows written
/// before the #136/#166/#174 hardening carry the unkeyed `sha256:<hex>` form.
pub(crate) const KEYED_DIGEST_PREFIX: &str = "hmac-sha256:";

/// `hmac-sha256:<hex>` keyed digest of `bytes` (see [`keyed_digest_hex`]).
pub(crate) fn keyed_prefixed_digest(bytes: &[u8]) -> String {
    format!("{KEYED_DIGEST_PREFIX}{}", keyed_digest_hex(bytes))
}

fn install_key() -> &'static DigestKey {
    &process_key().0
}

/// The process key and how it was obtained; resolved once, by whichever of the
/// first digest or the first `health` read comes first.
fn process_key() -> &'static (DigestKey, DigestKeyState) {
    INSTALL_KEY.get_or_init(|| {
        if cfg!(test) {
            // Unit tests stay hermetic: no key file under the real profile.
            return (random_key(), DigestKeyState::Persistent);
        }
        load_or_create_key_with_state(&default_key_path(), &DpapiKeyWrap)
    })
}

fn default_key_path() -> PathBuf {
    crate::m3::local_synapse_dir().join(KEY_FILE_NAME)
}

fn random_key() -> DigestKey {
    let mut key = [0_u8; KEY_LEN];
    key[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    key[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    key
}

/// How the persisted key blob is wrapped; a trait so tests can drive the
/// load/create/corrupt/unavailable paths without touching real DPAPI.
pub(crate) trait KeyWrap {
    fn wrap(&self, key: &[u8]) -> anyhow::Result<Vec<u8>>;
    fn unwrap_blob(&self, blob: &[u8]) -> anyhow::Result<Vec<u8>>;
}

struct DpapiKeyWrap;

impl KeyWrap for DpapiKeyWrap {
    fn wrap(&self, key: &[u8]) -> anyhow::Result<Vec<u8>> {
        crate::secret_crypto::protect_with_entropy(key, KEY_ENTROPY)
    }

    fn unwrap_blob(&self, blob: &[u8]) -> anyhow::Result<Vec<u8>> {
        crate::secret_crypto::unprotect_with_entropy(blob, KEY_ENTROPY)
    }
}

/// The key alone, for tests that do not care how it was obtained.
#[cfg(test)]
pub(crate) fn load_or_create_key(path: &Path, wrap: &dyn KeyWrap) -> DigestKey {
    load_or_create_key_with_state(path, wrap).0
}

/// Loads the persisted key, or creates and installs one, and reports how the
/// key was obtained (#269). Never fails.
///
/// - A missing file (and only that) triggers creation. The key is written to a
///   unique temporary file and installed with a no-clobber hard link, so two
///   daemons starting together converge on one winner; the loser reads and
///   returns the winner's key.
/// - Any other failure (read error, DPAPI unwrap failure, malformed blob)
///   leaves the existing file untouched, because it may be recoverable (a
///   transient I/O or DPAPI fault, another Windows account): an in-memory key
///   is used for this process and [`WARN_KEY_UNREADABLE`] is logged. Delete
///   the file to rotate the key deliberately.
/// - If the key cannot be wrapped or written, an in-memory key is used and
///   [`WARN_KEY_UNPROTECTED`] is logged.
///
/// The state is `Persistent` when the key was loaded from, or newly installed
/// in, the key file (including the lost-race winner's key); `Ephemeral` with
/// [`WARN_KEY_UNREADABLE`] or [`WARN_KEY_UNPROTECTED`] in the two cases above.
pub(crate) fn load_or_create_key_with_state(
    path: &Path,
    wrap: &dyn KeyWrap,
) -> (DigestKey, DigestKeyState) {
    match read_key(path, wrap) {
        KeyRead::Loaded(key) => return (key, DigestKeyState::Persistent),
        KeyRead::Unreadable(reason) => return (random_key(), unreadable_key(path, &reason)),
        KeyRead::Missing => {}
    }
    let key = random_key();
    match install_key_no_clobber(path, wrap, &key) {
        Ok(Installed::Ours) => (key, DigestKeyState::Persistent),
        Ok(Installed::LostRace) => match read_key(path, wrap) {
            KeyRead::Loaded(winner) => (winner, DigestKeyState::Persistent),
            KeyRead::Missing => (key, unreadable_key(path, "key file vanished after race")),
            KeyRead::Unreadable(reason) => (key, unreadable_key(path, &reason)),
        },
        Err(error) => {
            tracing::warn!(
                code = WARN_KEY_UNPROTECTED,
                path = %path.display(),
                error = %error,
                "content-digest key could not be DPAPI-protected and persisted; using an \
                 in-memory key, digests will not match across restarts"
            );
            (
                key,
                DigestKeyState::Ephemeral {
                    reason: WARN_KEY_UNPROTECTED,
                },
            )
        }
    }
}

/// Logs [`WARN_KEY_UNREADABLE`] and returns the matching ephemeral state; the
/// key file itself is left untouched.
fn unreadable_key(path: &Path, reason: &str) -> DigestKeyState {
    tracing::warn!(
        code = WARN_KEY_UNREADABLE,
        path = %path.display(),
        reason = %reason,
        "persisted content-digest key could not be read; the file is left untouched \
         and an in-memory key is used for this process, so digests will not match \
         earlier ones (delete the file to rotate the key deliberately)"
    );
    DigestKeyState::Ephemeral {
        reason: WARN_KEY_UNREADABLE,
    }
}

enum KeyRead {
    Loaded(DigestKey),
    Missing,
    Unreadable(String),
}

fn read_key(path: &Path, wrap: &dyn KeyWrap) -> KeyRead {
    let blob = match fs::read(path) {
        Ok(blob) => blob,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return KeyRead::Missing,
        Err(error) => return KeyRead::Unreadable(format!("read: {error}")),
    };
    match wrap.unwrap_blob(&blob) {
        Ok(plain) => match DigestKey::try_from(plain.as_slice()) {
            Ok(key) => KeyRead::Loaded(key),
            Err(_) => KeyRead::Unreadable(format!("unexpected key length {}", plain.len())),
        },
        Err(error) => KeyRead::Unreadable(format!("unwrap: {error}")),
    }
}

enum Installed {
    Ours,
    LostRace,
}

fn install_key_no_clobber(
    path: &Path,
    wrap: &dyn KeyWrap,
    key: &DigestKey,
) -> anyhow::Result<Installed> {
    let blob = wrap.wrap(key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    {
        use std::io::Write as _;
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&blob)?;
        file.sync_all()?;
    }
    // hard_link fails if `path` exists: atomic create-if-absent, and `path`
    // appears fully written or not at all.
    let linked = fs::hard_link(&tmp, path);
    let _cleanup = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(Installed::Ours),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(Installed::LostRace),
        Err(error) => Err(error.into()),
    }
}

/// State of the process digest key (#269). Resolves the key like a digest
/// does, so it is correct before the first digest is computed. Under test a
/// forced state, if any, wins.
pub(crate) fn digest_key_state() -> DigestKeyState {
    forced_key_state().unwrap_or_else(|| process_key().1)
}

/// The `content_digest_key` row of `health` (#269): `persistent` or
/// `ephemeral` with the reason code in `detail`. Never an `error` status (an
/// in-memory key is degraded forensics, not a failed daemon) and never key
/// material.
pub(crate) fn health_subsystem() -> SubsystemHealth {
    let (status, detail) = match digest_key_state() {
        DigestKeyState::Persistent => (
            "persistent",
            "content-digest key loaded from the DPAPI-protected key file; digests stay \
             comparable across restarts"
                .to_owned(),
        ),
        DigestKeyState::Ephemeral { reason } => (
            "ephemeral",
            format!(
                "{reason}: the content-digest key is held in memory for this process only; \
                 digests will not match earlier ones or survive a restart"
            ),
        ),
    };
    SubsystemHealth {
        status: status.to_owned(),
        detail: Some(detail),
        ..SubsystemHealth::default()
    }
}

#[cfg(test)]
static FORCED_KEY_STATE: std::sync::Mutex<Option<DigestKeyState>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn forced_key_state() -> Option<DigestKeyState> {
    *FORCED_KEY_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(not(test))]
fn forced_key_state() -> Option<DigestKeyState> {
    None
}

/// Test-only override of the state `digest_key_state` reports (`None` clears).
#[cfg(test)]
pub(crate) fn force_digest_key_state_for_tests(state: Option<DigestKeyState>) {
    *FORCED_KEY_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
}

/// Test-only copy of the key this process digests with, to prove no key bytes
/// reach health or logs.
#[cfg(test)]
pub(crate) fn active_digest_key_for_tests() -> [u8; KEY_LEN] {
    *install_key()
}

#[cfg(test)]
#[path = "tests/content_digest.rs"]
mod content_digest_tests;
