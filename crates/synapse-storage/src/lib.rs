pub mod agent_events;
pub mod agent_transcripts;
mod batch;
mod blocking;
pub mod cf;
pub mod codecs;
pub mod compaction;
mod db_handle;
#[cfg(any(test, feature = "test-support"))]
mod diagnostic_hooks;
pub mod episodes;
pub mod error;
mod exact_read;
mod gc;
mod kv_quota;
mod pressure;
pub mod routines;
pub mod timeline;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, ColumnFamilyRef, DB, DBCompressionType,
    Direction, IteratorMode, Options, SliceTransform, WriteBatch, WriteOptions,
};
use synapse_core::error_codes;
use synapse_core::retention::DEFAULTS;

pub use batch::BatcherStatus;
pub use codecs::{decode_json, encode_json};
pub use error::{StorageError, StorageResult};
pub use gc::{GcCfReport, GcReport, GcTask};
pub use pressure::{DiskPressureLevel, PressureReport, PressureTask};

const MIB: usize = 1024 * 1024;
const DEFAULT_WRITE_BUFFER_BYTES: usize = 64 * MIB;
const MODEL_CACHE_WRITE_BUFFER_BYTES: usize = 256 * MIB;
const BLOCK_CACHE_BYTES: usize = 64 * MIB;
const SCHEMA_VERSION_KEY: &[u8] = b"__schema_version";
const TIMELINE_PERIODIC_COMPACTION_SECONDS: u64 = 86_400;
const STORAGE_WRITES_SHED_TOTAL: &str = "storage_writes_shed_total";
const ESTIMATE_LIVE_DATA_SIZE: &str = "rocksdb.estimate-live-data-size";
const ESTIMATE_NUM_KEYS: &str = "rocksdb.estimate-num-keys";

fn batch_bytes(kvs: &[(Vec<u8>, Vec<u8>)]) -> u64 {
    kvs.iter()
        .map(|(key, value)| key.len().saturating_add(value.len()) as u64)
        .sum()
}

fn exact_cf_bytes(db: &DB, cf: &ColumnFamilyRef<'_>, cf_name: &str) -> StorageResult<u64> {
    let mut bytes = 0u64;
    for row in db.iterator_cf(cf, IteratorMode::Start) {
        let (key, value) = row.map_err(|source| StorageError::ReadFailed {
            cf_name: cf_name.to_owned(),
            detail: source.to_string(),
        })?;
        bytes = bytes.saturating_add(key.len().saturating_add(value.len()) as u64);
    }
    Ok(bytes)
}

/// One raw storage row: key bytes and value bytes.
pub type RawRow = (Vec<u8>, Vec<u8>);
/// A bounded scan window plus whether more rows remain past it.
pub type ScanWindow = (Vec<RawRow>, bool);
/// Per-CF integer storage metrics plus CFs whose `RocksDB` property was absent.
pub type CfEstimateMap = (BTreeMap<String, u64>, Vec<String>);

/// One synchronous mutation in a cross-column-family coordination batch.
///
/// This is intentionally reserved for state transitions whose durable marker
/// and durable journal row must become visible together. Ordinary ingestion
/// should keep using the single-column-family batched APIs above.
#[derive(Clone, Debug)]
pub enum CrossCfBatchMutation {
    Put {
        cf_name: &'static str,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        cf_name: &'static str,
        key: Vec<u8>,
    },
}

/// Opened storage handle.
pub struct Db {
    pub path: PathBuf,
    pub schema_version: u32,
    batcher: batch::Batcher,
    inner: Arc<db_handle::DbHandle>,
    pressure: Arc<pressure::PressureState>,
    quota_bytes: Mutex<BTreeMap<String, u64>>,
    kv_quota: kv_quota::KvQuotaPolicy,
    #[cfg(any(test, feature = "test-support"))]
    test_support: diagnostic_hooks::DbTestSupport,
    /// Test-only pause point between quota reservation and batcher enqueue.
    #[cfg(test)]
    reservation_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl fmt::Debug for Db {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Db")
            .field("path", &self.path)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Current `RocksDB` instance; replaced when the batcher reopens it (#33).
    fn db(&self) -> StorageResult<Arc<DB>> {
        self.inner.get()
    }

    #[cfg(test)]
    pub(crate) fn raw(&self) -> StorageResult<Arc<DB>> {
        self.db()
    }

    fn quota_lock(&self) -> StorageResult<std::sync::MutexGuard<'_, BTreeMap<String, u64>>> {
        self.quota_bytes
            .lock()
            .map_err(|error| StorageError::WriteFailed {
                cf_name: "storage_quota".to_owned(),
                detail: error.to_string(),
            })
    }

    fn check_hard_cap(
        &self,
        quota_bytes: &mut BTreeMap<String, u64>,
        cf_name: &str,
        puts: &[(Vec<u8>, Vec<u8>)],
        deletes: &[Vec<u8>],
    ) -> StorageResult<()> {
        let Some(default) = DEFAULTS.iter().find(|default| default.cf == cf_name) else {
            return Ok(());
        };
        let db = self.db()?;
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "column family handle missing".to_owned(),
            })?;
        // #164: bounded namespaces are refused at their own budget, and
        // critical rows skip the shared cap so they can always write.
        let scope = kv_quota::KvScope {
            db: &db,
            cf: &cf,
            cf_name,
        };
        self.kv_quota
            .admit(&scope, quota_bytes, puts, deletes, &|| self.batcher.flush())?;
        if kv_quota::KvQuotaPolicy::exempts_from_shared_cap(cf_name, puts) {
            if let Some(bytes) = quota_bytes.get_mut(cf_name) {
                *bytes = bytes.saturating_add(batch_bytes(puts));
            }
            return Ok(());
        }
        let mut estimated_bytes = db
            .property_int_value_cf(&cf, ESTIMATE_LIVE_DATA_SIZE)
            .map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?
            .ok_or_else(|| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: format!("{ESTIMATE_LIVE_DATA_SIZE} unavailable for quota admission"),
            })?;
        let hard_cap = default.hard_cap_mb.saturating_mul(MIB as u64);
        let incoming_bytes = batch_bytes(puts);
        let current_bytes = if let Some(bytes) = quota_bytes.get(cf_name) {
            *bytes
        } else {
            exact_cf_bytes(&db, &cf, cf_name)?
        };
        let mut projected = current_bytes.saturating_add(incoming_bytes);
        if projected > hard_cap || estimated_bytes.saturating_add(incoming_bytes) > hard_cap {
            self.batcher.flush()?;
            db.flush_cf(&cf)
                .map_err(|source| StorageError::WriteFailed {
                    cf_name: cf_name.to_owned(),
                    detail: source.to_string(),
                })?;
            estimated_bytes = db
                .property_int_value_cf(&cf, ESTIMATE_LIVE_DATA_SIZE)
                .map_err(|source| StorageError::ReadFailed {
                    cf_name: cf_name.to_owned(),
                    detail: source.to_string(),
                })?
                .ok_or_else(|| StorageError::ReadFailed {
                    cf_name: cf_name.to_owned(),
                    detail: format!("{ESTIMATE_LIVE_DATA_SIZE} unavailable after flush"),
                })?;
            let baseline = exact_cf_bytes(&db, &cf, cf_name)?;
            projected = baseline;
            let mut changes = BTreeMap::<&[u8], Option<&[u8]>>::new();
            for key in deletes {
                changes.insert(key, None);
            }
            for (key, value) in puts {
                changes.insert(key, Some(value));
            }
            for (key, value) in changes {
                if let Some(previous) =
                    db.get_cf(&cf, key)
                        .map_err(|source| StorageError::ReadFailed {
                            cf_name: cf_name.to_owned(),
                            detail: source.to_string(),
                        })?
                {
                    projected = projected.saturating_sub(key.len() as u64 + previous.len() as u64);
                }
                if let Some(value) = value {
                    projected = projected.saturating_add(key.len() as u64 + value.len() as u64);
                }
            }
            if projected > baseline
                && estimated_bytes.saturating_add(projected.saturating_sub(baseline)) > hard_cap
            {
                return Err(StorageError::HardCapReached {
                    cf_name: cf_name.to_owned(),
                    hard_cap,
                    estimated_bytes,
                });
            }
        }
        quota_bytes.insert(cf_name.to_owned(), projected);
        Ok(())
    }

    /// Opens the `RocksDB` storage at `path`.
    ///
    /// `RocksDB` refuses to open a database without naming every column
    /// family that physically exists, so a binary older than the newest CF
    /// would brick the daemon after any rollback (observed live 2026-06-12:
    /// a pre-`CF_ROUTINES` binary died at startup with `Column families not
    /// opened: CF_ROUTINES`). Unknown on-disk CFs are therefore opened with
    /// default options and a loud structured warning: their rows are
    /// preserved untouched for the newer binary that owns them, and the
    /// schema-version sentinel still rejects genuinely incompatible layouts.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::OpenFailed`] when `RocksDB` cannot open or
    /// initialize the database, or [`StorageError::SchemaMismatch`] when the
    /// stored schema sentinel differs from `schema_version`.
    #[tracing::instrument(skip_all, fields(storage_path = %path.display(), schema_version))]
    pub fn open(path: &Path, schema_version: u32) -> StorageResult<Self> {
        let inner = open_rocksdb(path, schema_version)?;
        let inner = Arc::new(db_handle::DbHandle::new(inner, path, schema_version));
        let batcher = batch::Batcher::spawn(Arc::clone(&inner));
        let pressure = Arc::new(pressure::PressureState::default());

        Ok(Self {
            path: path.to_path_buf(),
            schema_version,
            batcher,
            inner,
            pressure,
            quota_bytes: Mutex::new(BTreeMap::new()),
            kv_quota: kv_quota::KvQuotaPolicy::from_env(),
            #[cfg(any(test, feature = "test-support"))]
            test_support: diagnostic_hooks::DbTestSupport::default(),
            #[cfg(test)]
            reservation_hook: Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(crate) fn set_reservation_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.reservation_hook.lock() {
            *slot = Some(hook);
        }
    }

    #[cfg(test)]
    fn run_reservation_hook(&self) {
        let hook = self
            .reservation_hook
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Writes one column-family batch, returning after `RocksDB` accepts it.
    ///
    /// Requests are bounded to 64 MiB including allocation overhead; a queue
    /// that stays full past the admission wait fails closed. The batcher
    /// commits as soon as its queue is idle, so a lone writer is not delayed.
    /// Call `flush` for synchronous WAL durability.
    ///
    /// Callers aggregate producer-side and submit batches; per-frame single
    /// writes intentionally are not part of this API.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteShed`] when disk-pressure policy refuses
    /// the batch; no row in that batch was persisted. Other failures return
    /// [`StorageError::WriteFailed`].
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn put_batch<I, K, V>(&self, cf_name: &str, kvs: I) -> StorageResult<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Vec<u8>>,
        V: Into<Vec<u8>>,
    {
        self.db()?
            .cf_handle(cf_name)
            .ok_or_else(|| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "column family handle missing".to_owned(),
            })?;
        let kvs = kvs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<Vec<_>>();
        if kvs.is_empty() {
            return Ok(());
        }
        if !self.pressure.permits_write(cf_name) {
            // A dropped write must never be acknowledged as persisted. Callers
            // handling rebuildable/cache CFs may explicitly treat this typed
            // error as best-effort shedding.
            synapse_telemetry::metrics::counter!(
                STORAGE_WRITES_SHED_TOTAL,
                "cf" => cf_name.to_owned()
            )
            .increment(kvs.len() as u64);
            tracing::warn!(
                code = error_codes::STORAGE_WRITE_SHED,
                cf = cf_name,
                pressure_level = ?self.pressure.level(),
                dropped_rows = kvs.len(),
                metric_name = STORAGE_WRITES_SHED_TOTAL,
                "storage write dropped under disk pressure"
            );
            return Err(StorageError::WriteShed {
                cf_name: cf_name.to_owned(),
                pressure_level: self.pressure.level(),
                dropped_rows: kvs.len(),
            });
        }
        // Reserve quota and enqueue as one step under the quota lock (#34): a
        // slow-path reconcile in another writer flushes the queue behind this
        // write, so it always sees it as persisted and never forgets the
        // reservation. Only the enqueue is inside the lock; the commit wait is
        // outside so writers do not serialize on it (#33).
        let receipt = {
            let mut quota = self.quota_lock()?;
            let before = quota.clone();
            self.check_hard_cap(&mut quota, cf_name, &kvs, &[])?;
            #[cfg(test)]
            self.run_reservation_hook();
            match self.batcher.submit(cf_name, kvs) {
                Ok(receipt) => receipt,
                Err(error) => {
                    // Never enqueued: release the reservation.
                    *quota = before;
                    return Err(error);
                }
            }
        };
        receipt.wait()
    }

    /// Health of the background write batcher: whether a commit failure is
    /// still latched, why, and how often it has recovered.
    #[must_use]
    pub fn batcher_status(&self) -> BatcherStatus {
        self.batcher.status()
    }

    /// Writes a key/value batch while bypassing the pressure ingestion gate.
    ///
    /// This is reserved for bounded storage-maintenance rewrites, such as
    /// retention backfills and durable maintenance reports. Normal data
    /// ingestion must use [`Self::put_batch`] so disk-pressure policy can
    /// shed non-critical writes.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when the column family is missing
    /// or `RocksDB` rejects the write batch.
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn put_batch_pressure_bypass<I, K, V>(&self, cf_name: &str, kvs: I) -> StorageResult<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Vec<u8>>,
        V: Into<Vec<u8>>,
    {
        let db = self.db()?;
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "column family handle missing".to_owned(),
            })?;
        let kvs = kvs
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<Vec<_>>();
        if kvs.is_empty() {
            return Ok(());
        }
        let mut quota = self.quota_lock()?;
        self.check_hard_cap(&mut quota, cf_name, &kvs, &[])?;
        let mut batch = WriteBatch::default();
        for (key, value) in kvs {
            batch.put_cf(&cf, key, value);
        }
        // Sync the WAL for durability without forcing this CF's memtable into
        // an SST. Per-write CF flushes rotate tiny WAL files that cold CFs can
        // retain, making the next daemon startup replay thousands of files.
        let mut options = WriteOptions::default();
        options.set_sync(true);
        db.write_opt(batch, &options)
            .map_err(|source| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })
    }

    /// Applies key deletes and key/value writes to one column family in a
    /// single synchronous `RocksDB` batch while bypassing pressure shedding.
    ///
    /// This is reserved for bounded coordination-state rewrites where readers
    /// must never observe a release gap between deleting one owner row and
    /// writing another.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when the column family is missing
    /// or `RocksDB` rejects the write batch.
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn mutate_batch_pressure_bypass<D, K, P, PK, PV>(
        &self,
        cf_name: &str,
        deletes: D,
        puts: P,
    ) -> StorageResult<()>
    where
        D: IntoIterator<Item = K>,
        K: Into<Vec<u8>>,
        P: IntoIterator<Item = (PK, PV)>,
        PK: Into<Vec<u8>>,
        PV: Into<Vec<u8>>,
    {
        let db = self.db()?;
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "column family handle missing".to_owned(),
            })?;
        let deletes = deletes.into_iter().map(Into::into).collect::<Vec<_>>();
        let puts = puts
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<Vec<_>>();
        if deletes.is_empty() && puts.is_empty() {
            return Ok(());
        }
        let mut quota = self.quota_lock()?;
        if !puts.is_empty() {
            self.check_hard_cap(&mut quota, cf_name, &puts, &deletes)?;
        }
        let mut batch = WriteBatch::default();
        for key in deletes {
            batch.delete_cf(&cf, key);
        }
        for (key, value) in puts {
            batch.put_cf(&cf, key, value);
        }
        // Sync the WAL for durability without forcing this CF's memtable into
        // an SST. Per-write CF flushes rotate tiny WAL files that cold CFs can
        // retain, making the next daemon startup replay thousands of files.
        let mut options = WriteOptions::default();
        options.set_sync(true);
        db.write_opt(batch, &options)
            .map_err(|source| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })
    }

    /// Applies mutations spanning multiple column families in one durable
    /// `RocksDB` write batch.
    ///
    /// Coordination callers use this when a durable ownership row and its
    /// corresponding journal event are one logical transition. Readers cannot
    /// observe one mutation without the other after the write succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when a named column family is
    /// absent or `RocksDB` rejects the durable batch write.
    #[tracing::instrument(skip_all)]
    pub fn mutate_cross_cf_batch_pressure_bypass<I>(&self, mutations: I) -> StorageResult<()>
    where
        I: IntoIterator<Item = CrossCfBatchMutation>,
    {
        let mutations = mutations.into_iter().collect::<Vec<_>>();
        if mutations.is_empty() {
            return Ok(());
        }
        let db = self.db()?;
        let mut quota = self.quota_lock()?;
        let mut incoming = BTreeMap::<&str, (Vec<RawRow>, Vec<Vec<u8>>)>::new();
        for mutation in &mutations {
            match mutation {
                CrossCfBatchMutation::Put {
                    cf_name,
                    key,
                    value,
                } => {
                    incoming
                        .entry(cf_name)
                        .or_default()
                        .0
                        .push((key.clone(), value.clone()));
                }
                CrossCfBatchMutation::Delete { cf_name, key } => {
                    incoming.entry(cf_name).or_default().1.push(key.clone());
                }
            }
        }
        for (cf_name, (puts, deletes)) in incoming {
            if !puts.is_empty() {
                self.check_hard_cap(&mut quota, cf_name, &puts, &deletes)?;
            }
        }
        let mut batch = WriteBatch::default();
        for mutation in mutations {
            let cf_name = match &mutation {
                CrossCfBatchMutation::Put { cf_name, .. }
                | CrossCfBatchMutation::Delete { cf_name, .. } => *cf_name,
            };
            let cf = db
                .cf_handle(cf_name)
                .ok_or_else(|| StorageError::WriteFailed {
                    cf_name: cf_name.to_owned(),
                    detail: "column family handle missing".to_owned(),
                })?;
            match mutation {
                CrossCfBatchMutation::Put { key, value, .. } => batch.put_cf(&cf, key, value),
                CrossCfBatchMutation::Delete { key, .. } => batch.delete_cf(&cf, key),
            }
        }
        let mut options = WriteOptions::default();
        options.set_sync(true);
        db.write_opt(batch, &options)
            .map_err(|source| StorageError::WriteFailed {
                cf_name: "cross_cf_batch".to_owned(),
                detail: source.to_string(),
            })?;
        Ok(())
    }

    /// Deletes key rows from one column family and synchronizes the WAL.
    ///
    /// Deletions are allowed under disk pressure because they reduce retained
    /// local state.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when the column family is missing
    /// or `RocksDB` rejects the delete batch.
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn delete_batch<I, K>(&self, cf_name: &str, keys: I) -> StorageResult<()>
    where
        I: IntoIterator<Item = K>,
        K: Into<Vec<u8>>,
    {
        let db = self.db()?;
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "column family handle missing".to_owned(),
            })?;
        let keys = keys.into_iter().map(Into::into).collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(());
        }
        let mut batch = WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        // Sync the WAL for durability without forcing this CF's memtable into
        // an SST. Per-write CF flushes rotate tiny WAL files that cold CFs can
        // retain, making the next daemon startup replay thousands of files.
        let mut options = WriteOptions::default();
        options.set_sync(true);
        #[cfg(any(test, feature = "test-support"))]
        self.check_delete_batch_fault_for_test(cf_name, &keys)?;
        db.write_opt(batch, &options)
            .map_err(|source| StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })
    }

    /// Flushes pending batched writes with synchronous `RocksDB` write options.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when the background batcher is
    /// unavailable or `RocksDB` rejects the flush.
    #[tracing::instrument(skip_all)]
    pub fn flush(&self) -> StorageResult<()> {
        #[cfg(any(test, feature = "test-support"))]
        self.check_flush_fault_for_test()?;
        self.batcher.flush()
    }

    /// Runs one storage garbage-collection pass immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when `RocksDB` property reads, deletes, flushes,
    /// or compactions fail.
    #[tracing::instrument(skip_all)]
    pub fn run_gc_once(&self) -> StorageResult<GcReport> {
        gc::run_once(&*self.db()?, &gc::GcConfig::from_retention_defaults())
    }

    /// Runs one row-count-scaled GC pass for deterministic regression tests.
    ///
    /// This avoids writing gigabytes to hit production byte caps.
    ///
    /// # Errors
    ///
    /// Returns a storage error when `RocksDB` property reads, deletes, flushes,
    /// or compactions fail.
    #[doc(hidden)]
    #[tracing::instrument(skip_all)]
    pub fn run_gc_once_with_row_caps(
        &self,
        cf_name: &'static str,
        soft_cap_rows: u64,
        hard_cap_rows: u64,
    ) -> StorageResult<GcReport> {
        gc::run_once(
            &*self.db()?,
            &gc::GcConfig::for_row_caps(
                Duration::from_mins(5),
                cf_name,
                soft_cap_rows,
                hard_cap_rows,
            ),
        )
    }

    /// Spawns the periodic storage garbage-collection task.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when no Tokio runtime is active.
    #[tracing::instrument(skip_all)]
    pub fn spawn_gc_task(&self) -> StorageResult<GcTask> {
        gc::spawn(
            Arc::clone(&self.inner),
            gc::GcConfig::from_retention_defaults(),
        )
    }

    /// Spawns the periodic storage garbage-collection task with an explicit
    /// tick interval (#60: the daemon configures it from the environment).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when `interval` is zero or no
    /// Tokio runtime is active.
    #[tracing::instrument(skip_all)]
    pub fn spawn_gc_task_with_interval(&self, interval: Duration) -> StorageResult<GcTask> {
        if interval.is_zero() {
            return Err(StorageError::WriteFailed {
                cf_name: "storage_gc".to_owned(),
                detail: "GC interval must be non-zero".to_owned(),
            });
        }
        gc::spawn(
            Arc::clone(&self.inner),
            gc::GcConfig::from_retention_defaults().with_interval(interval),
        )
    }

    /// Returns the current DB-volume disk-pressure level.
    #[must_use]
    pub fn pressure_level(&self) -> DiskPressureLevel {
        self.pressure.level()
    }

    /// Returns whether the current pressure policy permits writes to `cf_name`.
    #[must_use]
    pub fn pressure_permits_write(&self, cf_name: &str) -> bool {
        self.pressure.permits_write(cf_name)
    }

    /// Returns the in-process disk-pressure transition code history.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] if the pressure state cannot be read.
    #[tracing::instrument(skip_all)]
    pub fn pressure_transition_codes(&self) -> StorageResult<Vec<&'static str>> {
        self.pressure.transition_codes()
    }

    /// Returns approximate logical bytes currently stored in each Synapse column family.
    ///
    /// This scans row keys and values so health reports reflect persisted data
    /// even when `RocksDB` has not compacted files yet.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when a column family cannot be scanned.
    #[tracing::instrument(skip_all)]
    pub fn cf_sizes(&self) -> StorageResult<BTreeMap<String, u64>> {
        let mut sizes = BTreeMap::new();
        for cf_name in cf::ALL_COLUMN_FAMILIES {
            let mut bytes = 0_u64;
            for (key, value) in self.scan_cf(cf_name)? {
                bytes = bytes.saturating_add(key.len() as u64);
                bytes = bytes.saturating_add(value.len() as u64);
            }
            sizes.insert(cf_name.to_owned(), bytes);
        }
        Ok(sizes)
    }

    /// Returns `RocksDB`'s live-data-size estimate for each Synapse column family.
    ///
    /// This is metadata-backed and intentionally cheaper than [`Self::cf_sizes`],
    /// which scans every key/value byte to compute exact logical sizes.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when a column family property cannot
    /// be read.
    #[tracing::instrument(skip_all)]
    pub fn cf_live_data_size_estimates(&self) -> StorageResult<CfEstimateMap> {
        self.cf_property_estimates(ESTIMATE_LIVE_DATA_SIZE)
    }

    /// Returns exact row counts for each Synapse column family.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when a column family cannot be scanned.
    #[tracing::instrument(skip_all)]
    pub fn cf_row_counts(&self) -> StorageResult<BTreeMap<String, u64>> {
        let mut counts = BTreeMap::new();
        for cf_name in cf::ALL_COLUMN_FAMILIES {
            counts.insert(cf_name.to_owned(), self.scan_cf(cf_name)?.len() as u64);
        }
        Ok(counts)
    }

    /// Returns `RocksDB`'s estimated row count for each Synapse column family.
    ///
    /// This is metadata-backed and intentionally cheaper than [`Self::cf_row_counts`],
    /// which scans every key in every column family.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when a column family property cannot
    /// be read.
    #[tracing::instrument(skip_all)]
    pub fn cf_estimated_row_counts(&self) -> StorageResult<CfEstimateMap> {
        self.cf_property_estimates(ESTIMATE_NUM_KEYS)
    }

    /// Runs one disk-pressure check immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when disk free-space probing or pressure-triggered
    /// compaction fails.
    #[tracing::instrument(skip_all)]
    pub fn run_pressure_check_once(&self) -> StorageResult<PressureReport> {
        pressure::run_once(
            &*self.db()?,
            &self.pressure,
            &self.path,
            &pressure::PressureConfig::default(),
        )
    }

    /// Applies one synthetic free-byte sample for deterministic regression tests.
    ///
    /// This uses the production thresholds and responder actions while avoiding
    /// host-volume manipulation.
    ///
    /// # Errors
    ///
    /// Returns a storage error when pressure-triggered compaction fails.
    #[doc(hidden)]
    #[tracing::instrument(skip_all)]
    pub fn run_pressure_check_with_free_bytes_sample(
        &self,
        free_bytes: u64,
    ) -> StorageResult<PressureReport> {
        pressure::run_once_with_free_bytes(
            &*self.db()?,
            &self.pressure,
            &pressure::PressureConfig::default(),
            free_bytes,
        )
    }

    /// Spawns the periodic DB-volume disk-pressure task.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when no Tokio runtime is active.
    #[tracing::instrument(skip_all)]
    pub fn spawn_pressure_task(&self) -> StorageResult<PressureTask> {
        pressure::spawn(
            Arc::clone(&self.inner),
            Arc::clone(&self.pressure),
            self.path.clone(),
            pressure::PressureConfig::default(),
        )
    }

    /// Spawns the periodic DB-volume disk-pressure task with an explicit poll
    /// interval (#60: the daemon configures it from the environment).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::WriteFailed`] when `interval` is zero or no
    /// Tokio runtime is active.
    #[tracing::instrument(skip_all)]
    pub fn spawn_pressure_task_with_interval(
        &self,
        interval: Duration,
    ) -> StorageResult<PressureTask> {
        if interval.is_zero() {
            return Err(StorageError::WriteFailed {
                cf_name: "storage_disk_pressure".to_owned(),
                detail: "disk-pressure poll interval must be non-zero".to_owned(),
            });
        }
        pressure::spawn(
            Arc::clone(&self.inner),
            Arc::clone(&self.pressure),
            self.path.clone(),
            pressure::PressureConfig::default().with_interval(interval),
        )
    }

    /// Scans a column family into owned key/value bytes.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn scan_cf(&self, cf_name: &str) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        #[cfg(any(test, feature = "test-support"))]
        self.check_read_fault_for_test(cf_name, b"")?;
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        let mut rows = Vec::new();
        for item in db.iterator_cf(&handle, IteratorMode::Start) {
            let (key, value) = item.map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?;
            rows.push((key.to_vec(), value.to_vec()));
        }
        Ok(rows)
    }

    /// Scans a column family from a key prefix into owned key/value bytes.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    #[tracing::instrument(skip_all, fields(cf_name, prefix_len = prefix.len()))]
    pub fn scan_cf_prefix(
        &self,
        cf_name: &str,
        prefix: &[u8],
    ) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.scan_cf_prefix_from(cf_name, prefix, prefix)
    }

    /// Scans a column family from `start_key` while rows still match `prefix`.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    #[tracing::instrument(skip_all, fields(cf_name, prefix_len = prefix.len(), start_key_len = start_key.len()))]
    pub fn scan_cf_prefix_from(
        &self,
        cf_name: &str,
        prefix: &[u8],
        start_key: &[u8],
    ) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        #[cfg(any(test, feature = "test-support"))]
        self.check_read_fault_for_test(cf_name, start_key)?;
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        let mut rows = Vec::new();
        for item in db.iterator_cf(&handle, IteratorMode::From(start_key, Direction::Forward)) {
            let (key, value) = item.map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?;
            if !key.starts_with(prefix) {
                break;
            }
            rows.push((key.to_vec(), value.to_vec()));
        }
        Ok(rows)
    }

    /// Reads at most `max_rows` rows and `max_value_bytes` of values from one
    /// prefix, with one in-prefix lookahead to report whether another page
    /// exists. The first row is always included so callers can make progress
    /// even when its value alone exceeds the byte budget.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    pub fn scan_cf_prefix_window(
        &self,
        cf_name: &str,
        prefix: &[u8],
        start_key: &[u8],
        max_rows: usize,
        max_value_bytes: usize,
    ) -> StorageResult<ScanWindow> {
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        let mut rows = Vec::new();
        let mut value_bytes = 0_usize;
        for item in db.iterator_cf(&handle, IteratorMode::From(start_key, Direction::Forward)) {
            let (key, value) = item.map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?;
            if !key.starts_with(prefix) {
                break;
            }
            if rows.len() == max_rows
                || (!rows.is_empty() && value_bytes.saturating_add(value.len()) > max_value_bytes)
            {
                return Ok((rows, true));
            }
            value_bytes = value_bytes.saturating_add(value.len());
            rows.push((key.to_vec(), value.to_vec()));
        }
        Ok((rows, false))
    }

    /// Scans up to `max_rows` rows starting at `start_key` (inclusive) and
    /// reports whether more rows remain past the returned window.
    ///
    /// Unlike [`Self::scan_cf_prefix_from`], iteration stops after
    /// `max_rows + 1` steps, so callers can page through arbitrarily large
    /// column families without materializing them.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    #[tracing::instrument(skip_all, fields(cf_name, start_key_len = start_key.len(), max_rows))]
    pub fn scan_cf_from(
        &self,
        cf_name: &str,
        start_key: &[u8],
        max_rows: usize,
    ) -> StorageResult<ScanWindow> {
        #[cfg(any(test, feature = "test-support"))]
        self.check_read_fault_for_test(cf_name, start_key)?;
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        let mut rows = Vec::new();
        let mut more = false;
        for item in db.iterator_cf(&handle, IteratorMode::From(start_key, Direction::Forward)) {
            let (key, value) = item.map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?;
            if rows.len() == max_rows {
                more = true;
                break;
            }
            rows.push((key.to_vec(), value.to_vec()));
        }
        Ok((rows, more))
    }

    /// Scans up to `max_rows` rows from the end of one column family.
    ///
    /// Returned rows preserve normal ascending key order, matching the tail
    /// produced by a full forward scan without materializing the whole CF.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or `RocksDB` iteration fails.
    #[tracing::instrument(skip_all, fields(cf_name, max_rows))]
    pub fn scan_cf_tail(&self, cf_name: &str, max_rows: usize) -> StorageResult<Vec<RawRow>> {
        if max_rows == 0 {
            return Ok(Vec::new());
        }
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        let mut rows = Vec::new();
        for item in db.iterator_cf(&handle, IteratorMode::End) {
            let (key, value) = item.map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })?;
            rows.push((key.to_vec(), value.to_vec()));
            if rows.len() >= max_rows {
                break;
            }
        }
        rows.reverse();
        Ok(rows)
    }

    /// Compacts a whole column family.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing.
    #[tracing::instrument(skip_all, fields(cf_name))]
    pub fn compact_cf(&self, cf_name: &str) -> StorageResult<()> {
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        db.compact_range_cf(&handle, None::<&[u8]>, None::<&[u8]>);
        Ok(())
    }

    /// Compacts one key range of a column family.
    ///
    /// `RocksDB`'s documented remedy for tombstone buildup after a bulk
    /// scan-and-delete is `CompactRange` over exactly the deleted range, so
    /// space is reclaimed and iterators do not slow down on tombstone runs
    /// (ADR 2026-06-11-timeline-data-model §6, purge mechanics).
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing.
    #[tracing::instrument(skip_all, fields(cf_name, start_len = start.len(), end_len = end.len()))]
    pub fn compact_cf_range(&self, cf_name: &str, start: &[u8], end: &[u8]) -> StorageResult<()> {
        let db = self.db()?;
        let handle = cf_handle(&db, cf_name)?;
        db.compact_range_cf(&handle, Some(start), Some(end));
        Ok(())
    }

    fn cf_property_estimates(&self, property: &str) -> StorageResult<CfEstimateMap> {
        let mut values = BTreeMap::new();
        let mut missing = Vec::new();
        let db = self.db()?;
        for cf_name in cf::ALL_COLUMN_FAMILIES {
            let handle = cf_handle(&db, cf_name)?;
            if let Some(value) = db
                .property_int_value_cf(&handle, property)
                .map_err(|source| StorageError::ReadFailed {
                    cf_name: cf_name.to_owned(),
                    detail: source.to_string(),
                })?
            {
                values.insert(cf_name.to_owned(), value);
            } else {
                values.insert(cf_name.to_owned(), 0);
                missing.push(cf_name.to_owned());
            }
        }
        Ok((values, missing))
    }
}

/// Opens the `RocksDB` database at `path` and verifies its schema sentinel.
fn open_rocksdb(path: &Path, schema_version: u32) -> StorageResult<DB> {
    let options = db_options();
    // An error here means the database does not exist yet (fresh open);
    // create_missing_column_families covers that path.
    let existing_cfs = DB::list_cf(&Options::default(), path).unwrap_or_default();
    let unknown_cfs: Vec<String> = existing_cfs
        .into_iter()
        .filter(|name| {
            name != rocksdb::DEFAULT_COLUMN_FAMILY_NAME
                && !cf::ALL_COLUMN_FAMILIES.contains(&name.as_str())
        })
        .collect();
    for name in &unknown_cfs {
        tracing::warn!(
            code = "STORAGE_UNKNOWN_CF_OPENED",
            cf_name = %name,
            "database holds a column family this binary does not know; \
             opening it untouched with default options (newer-binary data, \
             preserved for rollback safety)"
        );
    }
    let descriptors = cf::ALL_COLUMN_FAMILIES
        .into_iter()
        .map(|name| ColumnFamilyDescriptor::new(name, cf_options(name)))
        .chain(
            unknown_cfs
                .iter()
                .map(|name| ColumnFamilyDescriptor::new(name, Options::default())),
        );
    let inner = DB::open_cf_descriptors(&options, path, descriptors)
        .map_err(|source| open_failed(path, &source))?;
    verify_schema_version(&inner, path, schema_version)?;

    for name in cf::ALL_COLUMN_FAMILIES {
        if inner.cf_handle(name).is_none() {
            return Err(open_failed_detail(
                path,
                format!("column family handle missing after open: {name}"),
            ));
        }
    }
    Ok(inner)
}

fn db_options() -> Options {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    options.set_max_background_jobs(2);
    options.set_compression_type(DBCompressionType::Lz4);
    options.set_max_open_files(256);
    options.set_keep_log_file_num(8);
    options.set_write_buffer_size(DEFAULT_WRITE_BUFFER_BYTES);
    options.set_max_write_buffer_number(3);
    options.set_target_file_size_base(DEFAULT_WRITE_BUFFER_BYTES as u64);
    options.set_level_zero_file_num_compaction_trigger(4);
    apply_block_cache(&mut options);
    options
}

fn cf_options(name: &'static str) -> Options {
    let mut options = Options::default();
    options.set_write_buffer_size(DEFAULT_WRITE_BUFFER_BYTES);
    options.set_max_write_buffer_number(3);
    options.set_target_file_size_base(DEFAULT_WRITE_BUFFER_BYTES as u64);
    options.set_level_zero_file_num_compaction_trigger(4);
    options.set_compression_type(DBCompressionType::Lz4);

    match name {
        cf::CF_EVENTS | cf::CF_ACTION_LOG | cf::CF_REFLEX_AUDIT => {
            options.set_compression_type(DBCompressionType::Lz4);
            options.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
        }
        cf::CF_MODEL_CACHE => {
            options.set_compression_type(DBCompressionType::None);
            options.set_write_buffer_size(MODEL_CACHE_WRITE_BUFFER_BYTES);
        }
        cf::CF_OBSERVATIONS | cf::CF_SESSIONS => {
            options.set_compression_type(DBCompressionType::Zstd);
        }
        cf::CF_TIMELINE | cf::CF_EPISODES | cf::CF_AGENT_EVENTS => {
            options.set_compression_type(DBCompressionType::Zstd);
            options.set_prefix_extractor(SliceTransform::create_fixed_prefix(8));
            // Long-TTL rows live in cold SST files that normal write churn
            // never compacts; force every file through the TTL compaction
            // filter at least daily (ADR 2026-06-11-timeline-data-model;
            // CF_EPISODES shares the long-retention profile, #846;
            // CF_AGENT_EVENTS keys/expires the same way, #897).
            options.set_periodic_compaction_seconds(TIMELINE_PERIODIC_COMPACTION_SECONDS);
        }
        cf::CF_AGENT_TRANSCRIPTS => {
            // Same long-retention TTL profile as CF_AGENT_EVENTS (#900), but
            // keys are spawn-id-prefixed (variable length), so no fixed
            // 8-byte prefix extractor applies.
            options.set_compression_type(DBCompressionType::Zstd);
            options.set_periodic_compaction_seconds(TIMELINE_PERIODIC_COMPACTION_SECONDS);
        }
        _ => {}
    }

    compaction::install_ttl_filter(&mut options, name);
    apply_block_cache(&mut options);
    options
}

/// Per-column-family block cache size in MiB (default 64, one private LRU per
/// CF). Override with `SYNAPSE_STORAGE_BLOCK_CACHE_MIB`.
const BLOCK_CACHE_MIB_ENV: &str = "SYNAPSE_STORAGE_BLOCK_CACHE_MIB";
/// Opt in to ONE block cache shared by every column family of every database
/// in the process, sized in MiB (#60). Unset keeps the per-CF default.
const SHARED_BLOCK_CACHE_MIB_ENV: &str = "SYNAPSE_STORAGE_SHARED_BLOCK_CACHE_MIB";

fn env_mib(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|mib| *mib > 0)
}

/// Resolves the block cache for one column family: the shared cache when
/// opted in, otherwise a private LRU of the configured (default 64 MiB) size.
fn block_cache_for_new_cf() -> Cache {
    static SHARED: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    if let Some(shared_mib) = env_mib(SHARED_BLOCK_CACHE_MIB_ENV) {
        return SHARED
            .get_or_init(|| Cache::new_lru_cache(shared_mib.saturating_mul(MIB)))
            .clone();
    }
    let per_cf =
        env_mib(BLOCK_CACHE_MIB_ENV).map_or(BLOCK_CACHE_BYTES, |mib| mib.saturating_mul(MIB));
    Cache::new_lru_cache(per_cf)
}

fn cf_handle<'a>(db: &'a DB, cf_name: &str) -> StorageResult<ColumnFamilyRef<'a>> {
    db.cf_handle(cf_name)
        .ok_or_else(|| StorageError::ReadFailed {
            cf_name: cf_name.to_owned(),
            detail: "column family handle missing".to_owned(),
        })
}

fn apply_block_cache(options: &mut Options) {
    let cache = block_cache_for_new_cf();
    let mut block_options = BlockBasedOptions::default();
    block_options.set_block_cache(&cache);
    options.set_block_based_table_factory(&block_options);
}

fn verify_schema_version(db: &DB, path: &Path, schema_version: u32) -> StorageResult<()> {
    db.get(SCHEMA_VERSION_KEY)
        .map_err(|source| open_failed(path, &source))?
        .map_or_else(
            || {
                db.put(SCHEMA_VERSION_KEY, schema_version.to_be_bytes())
                    .map_err(|source| open_failed(path, &source))
            },
            |value| {
                let actual = decode_schema_version(&value);
                if actual == Some(schema_version) {
                    Ok(())
                } else {
                    Err(StorageError::SchemaMismatch {
                        expected: schema_version,
                        actual: actual.unwrap_or_default(),
                    })
                }
            },
        )
}

fn decode_schema_version(value: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = value.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn open_failed(path: &Path, source: &rocksdb::Error) -> StorageError {
    open_failed_detail(path, source.to_string())
}

fn open_failed_detail(path: &Path, detail: String) -> StorageError {
    tracing::warn!(
        code = error_codes::STORAGE_OPEN_FAILED,
        storage_path = %path.display(),
        %detail,
        "storage open failed"
    );
    StorageError::OpenFailed {
        path: path.to_path_buf(),
        detail,
    }
}

#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod compaction_tests;
#[cfg(test)]
mod db_handle_tests_33;
#[cfg(test)]
mod gc_tests;
#[cfg(test)]
mod kv_quota_tests;
#[cfg(test)]
mod open_tests;
#[cfg(test)]
mod pressure_tests;
#[cfg(test)]
mod quota_reservation_tests;
#[cfg(all(test, windows))]
mod recovery_tests_33;
