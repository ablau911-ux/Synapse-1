use synapse_core::StoredReflexAudit;
use synapse_storage::{Db, StorageResult, cf, encode_json};

pub(crate) const AUDIT_QUEUE_CAPACITY: usize = 1024;

/// Writes one reflex audit row to `CF_REFLEX_AUDIT`.
///
/// # Errors
///
/// Returns a storage error when JSON encoding fails or the storage batcher
/// rejects the write.
#[tracing::instrument(
    skip_all,
    fields(
        reflex_id = %audit.reflex_id,
        audit_id = %audit.audit_id,
        ts_ns = audit.ts_ns
    )
)]
pub fn write_audit(db: &Db, audit: &StoredReflexAudit) -> StorageResult<()> {
    let key = audit_key(audit);
    let value = encode_json(audit)?;
    db.put_batch(cf::CF_REFLEX_AUDIT, [(key.into_bytes(), value)])
}

fn audit_key(audit: &StoredReflexAudit) -> String {
    format!("{}:{:020}:{}", audit.reflex_id, audit.ts_ns, audit.audit_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEnqueueError {
    Full,
    Disconnected,
}

impl std::fmt::Display for AuditEnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "audit enqueue: {self:?}")
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditProducerStats {
    pub overflow_count: u64,
    pub write_error_count: u64,
}

#[derive(Default)]
struct AuditCounters {
    overflow: std::sync::atomic::AtomicU64,
    write_error: std::sync::atomic::AtomicU64,
}

pub struct AuditProducer {
    sender: Option<std::sync::mpsc::SyncSender<StoredReflexAudit>>,
    worker: Option<std::thread::JoinHandle<()>>,
    counters: std::sync::Arc<AuditCounters>,
}

impl AuditProducer {
    pub fn with_writer(
        capacity: usize,
        writer: impl Fn(StoredReflexAudit) -> StorageResult<()> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<StoredReflexAudit>(capacity);
        let counters = std::sync::Arc::new(AuditCounters::default());
        let counts = std::sync::Arc::clone(&counters);
        let worker = std::thread::Builder::new().name("synapse-reflex-audit".to_owned()).spawn(move || {
            for audit in receiver {
                if let Err(error) = writer(audit) {
                    let count = counts.write_error.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::error!(%error, write_error_count = count.saturating_add(1), "asynchronous reflex audit storage write failed");
                }
            }
        }).map_err(|error| tracing::error!(%error, "asynchronous reflex audit worker could not start")).ok();
        Self {
            sender: Some(sender),
            worker,
            counters,
        }
    }

    pub fn try_enqueue(&self, audit: StoredReflexAudit) -> Result<(), AuditEnqueueError> {
        let Some(sender) = &self.sender else {
            return Err(AuditEnqueueError::Disconnected);
        };
        sender.try_send(audit).map_err(|error| match error {
            std::sync::mpsc::TrySendError::Full(audit) => {
                let count = self.counters.overflow.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(overflow_count = count.saturating_add(1), audit_id = %audit.audit_id, reflex_id = %audit.reflex_id, "asynchronous reflex audit queue full; row rejected");
                AuditEnqueueError::Full
            }
            std::sync::mpsc::TrySendError::Disconnected(_) => AuditEnqueueError::Disconnected,
        })
    }

    pub fn stats(&self) -> AuditProducerStats {
        AuditProducerStats {
            overflow_count: self
                .counters
                .overflow
                .load(std::sync::atomic::Ordering::Relaxed),
            write_error_count: self
                .counters
                .write_error
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

impl Drop for AuditProducer {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("asynchronous reflex audit worker panicked");
        }
    }
}

pub(crate) fn enqueue_audit(
    producer: &AuditProducer,
    audit: &StoredReflexAudit,
) -> Result<(), AuditEnqueueError> {
    producer.try_enqueue(audit.clone())
}

#[cfg(test)]
#[path = "audit_async_252.rs"]
mod audit_async_252;
