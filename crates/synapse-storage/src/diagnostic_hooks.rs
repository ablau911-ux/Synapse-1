//! Opt-in per-DB fault injection at real storage entry points.
use crate::{Db, StorageError, StorageResult};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

type Observer = Arc<dyn Fn(&'static str) + Send + Sync>;
type ReadFault = (String, Vec<u8>);

#[derive(Default)]
pub(crate) struct DbTestSupport {
    flush_fault: AtomicBool,
    read_fault: Mutex<Option<ReadFault>>,
    delete_batch_fault: Mutex<Option<ReadFault>>,
    maintenance_observer: Mutex<Option<Observer>>,
}

impl Db {
    /// Fails one matching delete batch on this DB before its atomic commit.
    #[doc(hidden)]
    pub fn fail_next_delete_batch_before_commit_for_test(&self, cf_name: &str, key_prefix: &[u8]) {
        *self
            .test_support
            .delete_batch_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((cf_name.to_owned(), key_prefix.to_vec()));
    }

    pub(crate) fn check_delete_batch_fault_for_test(
        &self,
        cf_name: &str,
        keys: &[Vec<u8>],
    ) -> StorageResult<()> {
        let mut fault = self
            .test_support
            .delete_batch_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fault.as_ref().is_some_and(|(target_cf, prefix)| {
            target_cf == cf_name && keys.iter().any(|key| key.starts_with(prefix))
        }) {
            fault.take();
            return Err(StorageError::WriteFailed {
                cf_name: cf_name.to_owned(),
                detail: "injected delete batch failure before commit".to_owned(),
            });
        }
        Ok(())
    }

    /// Fails the next explicit flush on this DB, before acknowledging durability.
    #[doc(hidden)]
    pub fn fail_next_flush_for_test(&self) {
        self.test_support.flush_fault.store(true, Ordering::Release);
    }

    /// Fails one matching point read or prefix scan on this DB only.
    #[doc(hidden)]
    pub fn fail_next_read_for_test(&self, cf_name: &str, key_prefix: &[u8]) {
        *self
            .test_support
            .read_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((cf_name.to_owned(), key_prefix.to_vec()));
    }

    pub(crate) fn check_flush_fault_for_test(&self) -> StorageResult<()> {
        if self.test_support.flush_fault.swap(false, Ordering::AcqRel) {
            return Err(StorageError::WriteFailed {
                cf_name: "storage_flush".to_owned(),
                detail: "injected explicit WAL flush failure".to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn check_read_fault_for_test(&self, cf_name: &str, key: &[u8]) -> StorageResult<()> {
        let mut fault = self
            .test_support
            .read_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fault
            .as_ref()
            .is_some_and(|(target_cf, prefix)| target_cf == cf_name && key.starts_with(prefix))
        {
            fault.take();
            return Err(StorageError::ReadFailed {
                cf_name: cf_name.to_owned(),
                detail: "injected storage read IO failure".to_owned(),
            });
        }
        Ok(())
    }

    /// Observes synchronous maintenance entry for this DB; clones share it.
    #[doc(hidden)]
    pub fn set_storage_maintenance_observer_for_test(&self, observer: Observer) {
        *self
            .test_support
            .maintenance_observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observer);
    }

    #[doc(hidden)]
    pub fn observe_storage_maintenance_for_test(&self, operation: &'static str) {
        let observer = self
            .test_support
            .maintenance_observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(observer) = observer {
            observer(operation);
        }
    }
}
