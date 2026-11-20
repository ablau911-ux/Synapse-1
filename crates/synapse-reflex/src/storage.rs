use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use synapse_core::{StoredReflexAudit, error_codes};
use synapse_storage::{
    DiskPressureLevel, GcReport, PressureReport, StorageResult, cf, decode_json,
};

use crate::{ReflexResult, ReflexRuntime};

/// Rows read per page when counting recursion-guard clamps.
const RECURSION_CLAMP_SCAN_WINDOW: usize = 1_000;

/// How long a recursion-clamp count is reused before the audit CF is rescanned.
/// `health` is polled far more often than clamps change, and a full audit scan
/// runs under the reflex runtime lock (#60).
const RECURSION_CLAMP_CACHE_TTL: Duration = Duration::from_secs(30);

/// Last recursion-clamp count and when it was computed.
#[derive(Debug, Default)]
pub(crate) struct ClampCountCache(Mutex<Option<(Instant, u64)>>);

impl ClampCountCache {
    fn fresh(&self, now: Instant) -> Option<u64> {
        let cached = *self.0.lock().unwrap_or_else(PoisonError::into_inner);
        cached.and_then(|(at, total)| {
            (now.saturating_duration_since(at) < RECURSION_CLAMP_CACHE_TTL).then_some(total)
        })
    }

    fn store(&self, now: Instant, total: u64) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((now, total));
    }
}

impl ReflexRuntime {
    /// Returns the storage path backing this runtime.
    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_path(&self) -> &Path {
        &self.db.path
    }

    /// Returns a shared handle to the backing storage so callers can write
    /// after releasing the runtime lock instead of holding it across disk I/O.
    #[must_use]
    pub fn storage_db(&self) -> std::sync::Arc<synapse_storage::Db> {
        std::sync::Arc::clone(&self.db)
    }

    /// Returns the storage schema version backing this runtime.
    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn schema_version(&self) -> u32 {
        self.db.schema_version
    }

    /// Returns the current storage pressure level backing reflex persistence.
    #[must_use]
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_pressure_level(&self) -> DiskPressureLevel {
        self.db.pressure_level()
    }

    /// Returns whether storage currently accepts writes to one column family.
    #[must_use]
    pub fn storage_pressure_permits_write(&self, cf_name: &str) -> bool {
        self.db.pressure_permits_write(cf_name)
    }

    /// Returns logical byte sizes for each storage column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a column family scan fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_cf_sizes(&self) -> StorageResult<BTreeMap<String, u64>> {
        self.db.cf_sizes()
    }

    /// Returns `RocksDB`'s live-data-size estimate for each storage column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a column family property cannot be read.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_cf_live_data_size_estimates(
        &self,
    ) -> StorageResult<synapse_storage::CfEstimateMap> {
        self.db.cf_live_data_size_estimates()
    }

    /// Returns exact row counts for each storage column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a column family cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_cf_row_counts(&self) -> StorageResult<BTreeMap<String, u64>> {
        self.db.cf_row_counts()
    }

    /// Returns `RocksDB`'s estimated row count for each storage column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when a column family property cannot be read.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_cf_estimated_row_counts(&self) -> StorageResult<synapse_storage::CfEstimateMap> {
        self.db.cf_estimated_row_counts()
    }

    /// Returns the newest rows in one storage column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, limit))]
    pub fn storage_cf_tail_rows(
        &self,
        cf_name: &str,
        limit: usize,
    ) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.db.scan_cf_tail(cf_name, limit)
    }

    /// Returns rows in one storage column family whose keys start with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, prefix_len = prefix.len(), limit))]
    pub fn storage_cf_prefix_rows(
        &self,
        cf_name: &str,
        prefix: &[u8],
        limit: usize,
    ) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut rows = self.db.scan_cf_prefix(cf_name, prefix)?;
        if rows.len() > limit {
            rows.truncate(limit);
        }
        Ok(rows)
    }

    /// Returns rows in one storage column family starting at `start_key` whose
    /// keys still match `prefix`.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, prefix_len = prefix.len(), start_key_len = start_key.len(), limit))]
    pub fn storage_cf_prefix_rows_from(
        &self,
        cf_name: &str,
        prefix: &[u8],
        start_key: &[u8],
        limit: usize,
    ) -> StorageResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut rows = self.db.scan_cf_prefix_from(cf_name, prefix, start_key)?;
        if rows.len() > limit {
            rows.truncate(limit);
        }
        Ok(rows)
    }

    /// Returns up to `max_rows` rows starting at `start_key` (inclusive) and
    /// whether more rows remain, without materializing the whole column family.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, start_key_len = start_key.len(), max_rows))]
    pub fn storage_cf_rows_from(
        &self,
        cf_name: &str,
        start_key: &[u8],
        max_rows: usize,
    ) -> StorageResult<synapse_storage::ScanWindow> {
        self.db.scan_cf_from(cf_name, start_key, max_rows)
    }

    /// Writes a bounded diagnostic batch to storage and flushes it immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name))]
    pub fn storage_put_probe_rows(
        &self,
        cf_name: &str,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<()> {
        self.storage_put_rows(cf_name, rows)
    }

    /// Writes rows to one storage column family and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, row_count = rows.len()))]
    pub fn storage_put_rows(
        &self,
        cf_name: &str,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<()> {
        self.db.put_batch(cf_name, rows)?;
        self.db.flush()
    }

    /// Writes storage-maintenance rows while bypassing the pressure ingestion gate.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, row_count = rows.len()))]
    pub fn storage_put_rows_pressure_bypass(
        &self,
        cf_name: &str,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<()> {
        self.db.put_batch_pressure_bypass(cf_name, rows)
    }

    /// Deletes rows from one storage column family and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the delete or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, row_count = keys.len()))]
    pub fn storage_delete_rows(&self, cf_name: &str, keys: Vec<Vec<u8>>) -> StorageResult<()> {
        self.db.delete_batch(cf_name, keys)
    }

    /// Flushes pending batched storage writes to disk.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_flush(&self) -> StorageResult<()> {
        self.db.flush()
    }

    /// Atomically replaces rows in one column family: deletes plus puts in a
    /// single synchronous flushed batch. For bounded derived-state rewrites
    /// (episode re-segmentation #846) where readers must never observe a
    /// half-replaced range; callers gate on disk pressure themselves before
    /// invoking.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family is missing or the
    /// write batch fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name, delete_count = deletes.len(), put_count = puts.len()))]
    pub fn storage_replace_rows(
        &self,
        cf_name: &str,
        deletes: Vec<Vec<u8>>,
        puts: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<()> {
        self.db.mutate_batch_pressure_bypass(cf_name, deletes, puts)
    }

    /// Compacts one key range of a column family (tombstone reclamation after
    /// a bulk delete, per the timeline ADR purge mechanics).
    ///
    /// # Errors
    ///
    /// Returns a storage error when the column family is missing.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name))]
    pub fn storage_compact_cf_range(
        &self,
        cf_name: &str,
        start: &[u8],
        end: &[u8],
    ) -> StorageResult<()> {
        self.db.compact_cf_range(cf_name, start, end)
    }

    /// Writes action audit rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_action_log_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_ACTION_LOG, rows)
    }

    /// Writes process start/exit history rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_process_history_rows(
        &self,
        rows: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_PROCESS_HISTORY, rows)
    }

    /// Writes profile-linked event rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_event_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_EVENTS, rows)
    }

    /// Writes observation rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_observation_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_OBSERVATIONS, rows)
    }

    /// Writes session rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_session_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_SESSIONS, rows)
    }

    /// Writes profile-registry rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_profile_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_PROFILES, rows)
    }

    /// Writes local registry key-value rows and flushes them immediately.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the write or flush fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", row_count = rows.len()))]
    pub fn storage_put_kv_rows(&self, rows: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.storage_put_rows(cf::CF_KV, rows)
    }

    /// Returns one exact profile-registry row by key.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the profile CF cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", key_len = key.len()))]
    pub fn storage_profile_row(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        Ok(self
            .db
            .scan_cf(cf::CF_PROFILES)?
            .into_iter()
            .find_map(|(row_key, value)| (row_key == key).then_some(value)))
    }

    /// Returns one exact local registry key-value row by key.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the key-value CF cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", key_len = key.len()))]
    pub fn storage_kv_row(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        Ok(self
            .db
            .scan_cf(cf::CF_KV)?
            .into_iter()
            .find_map(|(row_key, value)| (row_key == key).then_some(value)))
    }

    /// Runs one row-cap storage GC pass for operator diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the GC pass fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", cf_name))]
    pub fn storage_run_gc_once_with_row_caps(
        &self,
        cf_name: &'static str,
        soft_cap_rows: u64,
        hard_cap_rows: u64,
    ) -> StorageResult<GcReport> {
        self.db
            .run_gc_once_with_row_caps(cf_name, soft_cap_rows, hard_cap_rows)
    }

    /// Applies one synthetic free-space sample through the production pressure responder.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the pressure check fails.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime", free_bytes))]
    pub fn storage_run_pressure_sample(&self, free_bytes: u64) -> StorageResult<PressureReport> {
        self.db
            .run_pressure_check_with_free_bytes_sample(free_bytes)
    }

    /// Returns the in-process storage pressure transition code history.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the pressure state cannot be read.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn storage_pressure_transition_codes(&self) -> StorageResult<Vec<&'static str>> {
        self.db.pressure_transition_codes()
    }

    /// Counts persisted recursion-guard clamp audit rows, reusing a count
    /// younger than 30 seconds so frequent `health` polls do not rescan the
    /// whole audit CF. Use [`Self::recursion_clamps_total_fresh`] to force a rescan.
    ///
    /// # Errors
    ///
    /// Returns a reflex error when the audit CF cannot be scanned.
    pub fn recursion_clamps_total(&self) -> ReflexResult<u64> {
        if let Some(total) = self.clamp_count_cache.fresh(Instant::now()) {
            return Ok(total);
        }
        self.recursion_clamps_total_fresh()
    }

    /// Counts persisted recursion-guard clamp audit rows with a full rescan
    /// and refreshes the cached count.
    ///
    /// Pages through the audit CF in bounded windows instead of materializing
    /// it, and skips (and logs) rows that cannot be decoded: one corrupt audit
    /// row must not take the whole count, or `health`, down.
    ///
    /// # Errors
    ///
    /// Returns a reflex error when the audit CF cannot be scanned.
    #[tracing::instrument(skip_all, fields(component = "reflex_runtime"))]
    pub fn recursion_clamps_total_fresh(&self) -> ReflexResult<u64> {
        let mut total = 0_u64;
        let mut start_key: Vec<u8> = Vec::new();
        loop {
            let (rows, more) = self
                .db
                .scan_cf_from(cf::CF_REFLEX_AUDIT, &start_key, RECURSION_CLAMP_SCAN_WINDOW)
                .map_err(|error| {
                    crate::audit_state::audit_storage_error(&error, "reflex audit scan")
                })?;
            let Some((last_key, _)) = rows.last() else {
                self.clamp_count_cache.store(Instant::now(), total);
                return Ok(total);
            };
            start_key.clone_from(last_key);
            start_key.push(0);
            for (key, value) in &rows {
                match decode_json::<StoredReflexAudit>(value) {
                    Ok(audit)
                        if audit.error_code.as_deref()
                            == Some(error_codes::REFLEX_RECURSION_LIMIT) =>
                    {
                        total = total.saturating_add(1);
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        code = "REFLEX_AUDIT_ROW_UNDECODABLE",
                        key_len = key.len(),
                        error = %error,
                        "skipping undecodable reflex audit row while counting recursion clamps"
                    ),
                }
            }
            if !more {
                self.clamp_count_cache.store(Instant::now(), total);
                return Ok(total);
            }
        }
    }
}
