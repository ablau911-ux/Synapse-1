//! Swappable `RocksDB` handle (#33).
//!
//! `RocksDB` latches a background error (for example a failed WAL switch) and
//! refuses every later write until `DB::Resume()` or a reopen. The C API that
//! `rust-rocksdb` wraps has no resume call, so recovery after the fault clears
//! is a controlled close and reopen of the same path. Every user takes a
//! short-lived `Arc<DB>` through [`DbHandle::get`]; [`DbHandle::reopen`] blocks
//! new users, waits for the outstanding ones to finish, drops the old database
//! (releasing its lock file) and opens a fresh one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use rocksdb::DB;

use crate::{StorageError, StorageResult};

/// How long a reopen waits for in-flight users of the old database to finish.
const QUIESCE_WAIT: Duration = Duration::from_secs(5);
const QUIESCE_POLL: Duration = Duration::from_millis(5);

pub(crate) struct DbHandle {
    slot: RwLock<Option<Arc<DB>>>,
    path: PathBuf,
    schema_version: u32,
}

impl DbHandle {
    pub(crate) fn new(db: DB, path: &Path, schema_version: u32) -> Self {
        Self {
            slot: RwLock::new(Some(Arc::new(db))),
            path: path.to_path_buf(),
            schema_version,
        }
    }

    /// Returns the current database. Fails only after a reopen lost the old
    /// database and could not open a new one; the next reopen retries.
    pub(crate) fn get(&self) -> StorageResult<Arc<DB>> {
        self.slot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .cloned()
            .ok_or_else(|| StorageError::OpenFailed {
                path: self.path.clone(),
                detail: "storage is reopening after a failed recovery attempt".to_owned(),
            })
    }

    /// Closes and reopens the database to clear a latched background error.
    ///
    /// # Errors
    ///
    /// Returns an error when in-flight users of the old database do not finish
    /// within [`QUIESCE_WAIT`] (nothing was changed) or when the fresh open
    /// fails (the handle then stays empty until the next reopen succeeds).
    pub(crate) fn reopen(&self) -> StorageResult<()> {
        let mut slot = self.slot.write().unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + QUIESCE_WAIT;
        while slot
            .as_ref()
            .is_some_and(|current| Arc::strong_count(current) > 1)
        {
            if Instant::now() >= deadline {
                return Err(StorageError::OpenFailed {
                    path: self.path.clone(),
                    detail: "storage reopen skipped: database still in use".to_owned(),
                });
            }
            thread::sleep(QUIESCE_POLL);
        }
        // Dropping the last reference closes the database and its lock file.
        drop(slot.take());
        let fresh = crate::open_rocksdb(&self.path, self.schema_version)?;
        *slot = Some(Arc::new(fresh));
        tracing::warn!(
            code = "STORAGE_REOPENED",
            storage_path = %self.path.display(),
            "storage reopened the database to clear a latched RocksDB error"
        );
        Ok(())
    }
}
