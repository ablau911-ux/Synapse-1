//! Exact-value scrub of the credentials Synapse itself holds (#39).
//!
//! The generic redactor deliberately keeps hex strings readable (git SHAs,
//! SHA-256 digests), so a 64-hex daemon token printed by a shell command would
//! pass it. This scrub masks the *exact* secret values Synapse knows, wherever
//! they appear and whatever their shape, before the heuristic filters run.
//! Credential files are re-read when their size or mtime changes, so a rotated
//! token is covered without a daemon restart.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use regex::Regex;

const MASK: &str = "[redacted-shell-output]";
/// Values shorter than this are never scrubbed by value: they would mangle
/// ordinary output and are not credentials Synapse generates.
const MIN_SECRET_BYTES: usize = 8;
/// Credential files under `%APPDATA%\synapse` (operator token, Chrome bridge).
const CREDENTIAL_FILES: [&str; 2] = ["token.txt", "chrome-bridge-token.txt"];
/// Environment variables that carry a Synapse bearer credential.
const CREDENTIAL_ENV: [&str; 1] = [crate::spawn_credentials::SPAWN_CREDENTIAL_ENV];

type FileStamp = (SystemTime, u64);

struct CachedFile {
    stamp: FileStamp,
    value: Option<String>,
}

/// Credentials Synapse holds, scrubbed from shell output by exact value.
pub(super) struct KnownSecrets {
    files: Vec<PathBuf>,
    env_names: Vec<String>,
    cache: Mutex<BTreeMap<PathBuf, CachedFile>>,
    /// Shape of a per-spawn scoped credential (`prefix` + 64 hex).
    spawn_credential: Option<Regex>,
}

impl KnownSecrets {
    pub(super) fn new(files: Vec<PathBuf>, env_names: Vec<String>) -> Self {
        let spawn_credential = Regex::new(&format!(
            "{}[0-9a-fA-F]{{64}}",
            regex::escape(crate::spawn_credentials::CREDENTIAL_PREFIX)
        ))
        .ok();
        Self {
            files,
            env_names,
            cache: Mutex::new(BTreeMap::new()),
            spawn_credential,
        }
    }

    /// The running profile's credentials. Unit tests stay hermetic: they never
    /// read the real profile or environment.
    pub(super) fn global() -> &'static Self {
        static GLOBAL: OnceLock<KnownSecrets> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            if cfg!(test) {
                return Self::new(Vec::new(), Vec::new());
            }
            let files: Vec<PathBuf> = std::env::var_os("APPDATA")
                .map(|appdata| PathBuf::from(appdata).join("synapse"))
                .map(|dir| CREDENTIAL_FILES.iter().map(|name| dir.join(name)).collect())
                .unwrap_or_default();
            Self::new(
                files,
                CREDENTIAL_ENV
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
            )
        })
    }

    /// Masks every known credential in `text`. Returns `None` when the filter
    /// is unavailable, so the caller fails closed.
    pub(super) fn scrub<'a>(&self, text: &'a str) -> Option<Cow<'a, str>> {
        let spawn_credential = self.spawn_credential.as_ref()?;
        let mut scrubbed = Cow::Borrowed(text);
        for secret in self.current_values() {
            if scrubbed.contains(secret.as_str()) {
                scrubbed = Cow::Owned(scrubbed.replace(secret.as_str(), MASK));
            }
        }
        if spawn_credential.is_match(&scrubbed) {
            scrubbed = Cow::Owned(spawn_credential.replace_all(&scrubbed, MASK).into_owned());
        }
        Some(scrubbed)
    }

    /// Current secret values, longest first so a value that contains another
    /// is masked whole.
    fn current_values(&self) -> Vec<String> {
        let mut values = Vec::new();
        {
            let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
            for path in &self.files {
                let Some(stamp) = file_stamp(path) else {
                    cache.remove(path);
                    continue;
                };
                if cache.get(path).is_none_or(|cached| cached.stamp != stamp) {
                    let value = fs::read_to_string(path)
                        .ok()
                        .map(|raw| raw.trim().to_owned());
                    cache.insert(path.clone(), CachedFile { stamp, value });
                }
                if let Some(value) = cache.get(path).and_then(|cached| cached.value.clone()) {
                    values.push(value);
                }
            }
        }
        for name in &self.env_names {
            if let Ok(value) = std::env::var(name) {
                values.push(value.trim().to_owned());
            }
        }
        values.retain(|value| value.len() >= MIN_SECRET_BYTES);
        values.sort_unstable_by(|left, right| {
            right.len().cmp(&left.len()).then_with(|| left.cmp(right))
        });
        values.dedup();
        values
    }
}

fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
