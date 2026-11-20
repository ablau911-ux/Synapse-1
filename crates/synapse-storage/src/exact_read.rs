//! Exact-key point reads (#60 item 7): one `RocksDB` `get` instead of a prefix
//! scan followed by a linear search for the matching key.

use crate::{Db, StorageError, StorageResult};

impl Db {
    /// Reads exactly one row by its full key.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::ReadFailed`] when the column family is missing
    /// or the `RocksDB` point read fails.
    #[tracing::instrument(skip_all, fields(cf_name, key_len = key.len()))]
    pub fn get_cf_exact(&self, cf_name: &str, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        #[cfg(any(test, feature = "test-support"))]
        self.check_read_fault_for_test(cf_name, key)?;
        let db = self.db()?;
        let handle = crate::cf_handle(&db, cf_name)?;
        db.get_cf(&handle, key)
            .map_err(|source| StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: source.to_string(),
            })
    }
}
