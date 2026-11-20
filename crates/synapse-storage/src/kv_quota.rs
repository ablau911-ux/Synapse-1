//! Per-namespace byte budgets inside `CF_KV` (#164).
//!
//! `CF_KV` holds unrelated state under one shared hard cap. Producers that a
//! single agent can drive (mailbox self-sends, 256 KiB workspace values) are
//! bounded by their own budget, and rows the daemon cannot run without
//! (sessions, approvals, escalations, shell-job idempotency, agent tasks) are
//! exempt from the shared cap so they can always write. Budgets and the
//! critical prefix list come from `synapse_core::retention`; each budget can be
//! overridden with `SYNAPSE_KV_QUOTA_<NAME>_MIB`.

use std::collections::BTreeMap;

use rocksdb::{ColumnFamilyRef, DB, Direction, IteratorMode};
use synapse_core::retention::{KV_CRITICAL_PREFIXES, KV_NAMESPACE_QUOTAS};

use crate::{MIB, RawRow, StorageError, StorageResult};

#[derive(Clone, Debug)]
struct Namespace {
    cf: &'static str,
    name: &'static str,
    prefix: &'static [u8],
    quota_bytes: u64,
}

impl Namespace {
    fn cache_key(&self) -> String {
        format!("{}#{}", self.cf, self.name)
    }

    fn owns(&self, key: &[u8]) -> bool {
        key.starts_with(self.prefix)
    }
}

/// The column family an admission check runs against.
pub(crate) struct KvScope<'a, 'b> {
    pub(crate) db: &'a DB,
    pub(crate) cf: &'a ColumnFamilyRef<'b>,
    pub(crate) cf_name: &'a str,
}

/// Resolved namespace budgets for one opened database.
#[derive(Clone, Debug)]
pub(crate) struct KvQuotaPolicy {
    namespaces: Vec<Namespace>,
}

impl KvQuotaPolicy {
    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub(crate) fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let namespaces = KV_NAMESPACE_QUOTAS
            .iter()
            .map(|default| {
                let env_name = format!("SYNAPSE_KV_QUOTA_{}_MIB", default.name.to_uppercase());
                let quota_mb = lookup(&env_name)
                    .and_then(|raw| raw.trim().parse::<u64>().ok())
                    .filter(|mb| *mb > 0)
                    .unwrap_or(default.quota_mb);
                Namespace {
                    cf: default.cf,
                    name: default.name,
                    prefix: default.key_prefix.as_bytes(),
                    quota_bytes: quota_mb.saturating_mul(MIB as u64),
                }
            })
            .collect();
        Self { namespaces }
    }

    /// Budget in bytes for the named namespace, if configured.
    #[cfg(test)]
    pub(crate) fn quota_bytes(&self, name: &str) -> Option<u64> {
        self.namespaces
            .iter()
            .find(|namespace| namespace.name == name)
            .map(|namespace| namespace.quota_bytes)
    }

    /// True when every row of the batch is critical state that must never be
    /// refused by the shared column-family cap.
    pub(crate) fn exempts_from_shared_cap(cf_name: &str, puts: &[RawRow]) -> bool {
        cf_name == crate::cf::CF_KV
            && !puts.is_empty()
            && puts.iter().all(|(key, _)| {
                KV_CRITICAL_PREFIXES
                    .iter()
                    .any(|prefix| key.starts_with(prefix.as_bytes()))
            })
    }

    /// Admits one batch against every namespace budget it touches.
    ///
    /// `cache` is the per-`Db` projected-bytes cache shared with the column
    /// family cap; it only over-estimates (deletes are not subtracted), so a
    /// budget breach is re-verified against an exact post-flush scan before
    /// any write is refused.
    pub(crate) fn admit(
        &self,
        scope: &KvScope<'_, '_>,
        cache: &mut BTreeMap<String, u64>,
        puts: &[RawRow],
        deletes: &[Vec<u8>],
        flush: &dyn Fn() -> StorageResult<()>,
    ) -> StorageResult<()> {
        let KvScope { db, cf, cf_name } = *scope;
        for namespace in self.namespaces.iter().filter(|ns| ns.cf == cf_name) {
            let owned: Vec<&RawRow> = puts.iter().filter(|(key, _)| namespace.owns(key)).collect();
            if owned.is_empty() {
                continue;
            }
            let incoming: u64 = owned
                .iter()
                .map(|(key, value)| key.len().saturating_add(value.len()) as u64)
                .sum();
            let cache_key = namespace.cache_key();
            let current = if let Some(bytes) = cache.get(&cache_key) {
                *bytes
            } else {
                exact_prefix_bytes(db, cf, cf_name, namespace.prefix)?
            };
            let mut projected = current.saturating_add(incoming);
            if projected > namespace.quota_bytes {
                flush()?;
                let baseline = exact_prefix_bytes(db, cf, cf_name, namespace.prefix)?;
                projected = baseline;
                let mut changes = BTreeMap::<&[u8], Option<&[u8]>>::new();
                for key in deletes.iter().filter(|key| namespace.owns(key)) {
                    changes.insert(key, None);
                }
                for (key, value) in owned {
                    changes.insert(key, Some(value));
                }
                for (key, value) in changes {
                    let previous =
                        db.get_cf(cf, key)
                            .map_err(|source| StorageError::ReadFailed {
                                cf_name: cf_name.to_owned(),
                                detail: source.to_string(),
                            })?;
                    if let Some(previous) = previous {
                        projected =
                            projected.saturating_sub(key.len() as u64 + previous.len() as u64);
                    }
                    if let Some(value) = value {
                        projected = projected.saturating_add(key.len() as u64 + value.len() as u64);
                    }
                }
                if projected > baseline && projected > namespace.quota_bytes {
                    return Err(StorageError::NamespaceQuotaReached {
                        cf_name: cf_name.to_owned(),
                        namespace: namespace.name.to_owned(),
                        quota_bytes: namespace.quota_bytes,
                        used_bytes: baseline,
                        incoming_bytes: projected - baseline,
                    });
                }
            }
            cache.insert(cache_key, projected);
        }
        Ok(())
    }
}

fn exact_prefix_bytes(
    db: &DB,
    cf: &ColumnFamilyRef<'_>,
    cf_name: &str,
    prefix: &[u8],
) -> StorageResult<u64> {
    let mut bytes = 0_u64;
    for row in db.iterator_cf(cf, IteratorMode::From(prefix, Direction::Forward)) {
        let (key, value) = row.map_err(|source| StorageError::ReadFailed {
            cf_name: cf_name.to_owned(),
            detail: source.to_string(),
        })?;
        if !key.starts_with(prefix) {
            break;
        }
        bytes = bytes.saturating_add(key.len().saturating_add(value.len()) as u64);
    }
    Ok(bytes)
}
