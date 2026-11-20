use std::{
    sync::{
        Arc, Mutex, PoisonError, mpsc,
        mpsc::{RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rocksdb::{DB, WriteBatch, WriteOptions};

use crate::{StorageError, StorageResult, db_handle::DbHandle};

/// Upper bound on how long shutdown waits for the worker's final flush. Normal
/// writes are never delayed by it: the worker commits as soon as its queue is
/// idle, so a lone writer pays one `RocksDB` write, not a coalescing timer.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
/// Pending bytes at which the worker commits mid-drain instead of coalescing more.
pub const FLUSH_BYTES: usize = 64 * 1024;
/// First wait before a failed batcher re-probes with a real write.
pub const RETRY_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
/// Ceiling of the exponential re-probe backoff.
pub const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long a writer waits for admission when the bounded queue is full.
const ADMISSION_WAIT: Duration = Duration::from_secs(5);
const ADMISSION_POLL: Duration = Duration::from_millis(1);

// Bound both queued requests and each request, including allocation overhead.
const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUEUED_COMMANDS: usize = 4;

/// Observable batcher health. `failed` stays set from the first failed commit
/// until a later real write succeeds again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatcherStatus {
    pub failed: bool,
    pub detail: Option<String>,
    /// Failed commits since the last success.
    pub consecutive_failures: u64,
    /// Times the batcher returned to healthy after a failure.
    pub recoveries: u64,
    /// Time until the next write is allowed through as a recovery probe.
    pub retry_in_ms: Option<u64>,
}

pub struct Batcher {
    sender: SyncSender<Command>,
    worker: Option<JoinHandle<()>>,
    health: Arc<Mutex<Health>>,
}

impl Batcher {
    pub fn spawn(db: Arc<DbHandle>) -> Self {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_COMMANDS);
        let health = Arc::new(Mutex::new(Health::default()));
        let worker_health = Arc::clone(&health);
        let worker = thread::spawn(move || {
            Worker {
                db,
                receiver,
                health: worker_health,
            }
            .run();
        });
        Self {
            sender,
            worker: Some(worker),
            health,
        }
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "used by tests/batch_failure_ownership.rs, which includes this file by path"
    )]
    pub fn put_batch(&self, cf_name: &str, kvs: Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<()> {
        self.submit(cf_name, kvs)?.wait()
    }

    pub fn status(&self) -> BatcherStatus {
        self.health
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .status(Instant::now())
    }

    /// Queues a write and returns without waiting for its commit. Once this
    /// returns `Ok`, the write is ahead of any later `flush` in queue order,
    /// which lets a caller reserve quota and enqueue as one atomic step and
    /// wait for the commit afterwards (#34, #33).
    pub fn submit(
        &self,
        cf_name: &str,
        kvs: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> StorageResult<WriteReceipt> {
        let charged_bytes = batch_charge(cf_name, &kvs)?;
        let (reply, wait) = mpsc::sync_channel(1);
        self.admit(
            Command::Write {
                cf_name: cf_name.to_owned(),
                kvs,
                charged_bytes,
                reply,
            },
            cf_name,
        )?;
        Ok(WriteReceipt {
            cf_name: cf_name.to_owned(),
            wait,
        })
    }

    // Bounded admission: concurrent writers wait briefly for a queue slot, but
    // a stuck worker fails them after ADMISSION_WAIT instead of hanging them.
    fn admit(&self, mut command: Command, cf_name: &str) -> StorageResult<()> {
        let deadline = Instant::now() + ADMISSION_WAIT;
        loop {
            match self.sender.try_send(command) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(returned)) if Instant::now() < deadline => {
                    command = returned;
                    thread::sleep(ADMISSION_POLL);
                }
                Err(error) => return Err(write_failed(cf_name, error.to_string())),
            }
        }
    }

    pub fn flush(&self) -> StorageResult<()> {
        let (reply, wait) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Flush { reply })
            .map_err(|error| write_failed("batcher", error.to_string()))?;
        wait.recv()
            .map_err(|error| write_failed("batcher", error.to_string()))?
    }
}

/// Handle for a queued write; `wait` blocks until the batcher commits it.
pub struct WriteReceipt {
    cf_name: String,
    wait: mpsc::Receiver<StorageResult<()>>,
}

impl WriteReceipt {
    pub fn wait(self) -> StorageResult<()> {
        self.wait
            .recv()
            .map_err(|error| write_failed(&self.cf_name, error.to_string()))?
    }
}

impl Drop for Batcher {
    fn drop(&mut self) {
        let (reply, wait) = mpsc::sync_channel(1);
        let _ = self.sender.send(Command::Shutdown { reply });
        let _ = wait.recv_timeout(FLUSH_INTERVAL.saturating_mul(2));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum Command {
    Write {
        cf_name: String,
        kvs: Vec<(Vec<u8>, Vec<u8>)>,
        charged_bytes: usize,
        reply: mpsc::SyncSender<StorageResult<()>>,
    },
    Flush {
        reply: mpsc::SyncSender<StorageResult<()>>,
    },
    Shutdown {
        reply: mpsc::SyncSender<StorageResult<()>>,
    },
}

struct Worker {
    db: Arc<DbHandle>,
    receiver: mpsc::Receiver<Command>,
    health: Arc<Mutex<Health>>,
}

impl Worker {
    fn health_lock(&self) -> std::sync::MutexGuard<'_, Health> {
        self.health.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // The result of a commit that was really attempted: updates health and
    // returns the result unchanged for the caller to forward.
    fn record(&self, result: Result<(), String>) -> Result<(), String> {
        self.health_lock().record(&result, Instant::now());
        result
    }

    // Healthy -> commit. Failed and still backing off -> refuse with the
    // sticky detail. Failed and backoff elapsed -> the caller's work is the
    // recovery probe.
    fn refusal(&self) -> Option<String> {
        self.health_lock().refusal(Instant::now())
    }

    fn run(self) {
        let mut pending = PendingBatch::default();
        loop {
            match receive_next(&self.receiver, &pending) {
                Ok(Command::Write {
                    cf_name,
                    kvs,
                    charged_bytes,
                    reply,
                }) => {
                    if let Some(detail) = self.refusal() {
                        let _ = reply.send(Err(write_failed(&cf_name, detail)));
                        continue;
                    }
                    if pending.bytes.saturating_add(charged_bytes) > MAX_BATCH_BYTES
                        && let Err(detail) = self.finish(&mut pending, false)
                    {
                        let _ = reply.send(Err(write_failed(&cf_name, detail)));
                        continue;
                    }
                    if kvs.is_empty() {
                        let _ = reply.send(Ok(()));
                        continue;
                    }
                    enqueue(&mut pending, &cf_name, kvs);
                    pending.bytes += charged_bytes;
                    pending.replies.push((cf_name, reply));
                    if pending.bytes >= FLUSH_BYTES {
                        let _ = self.finish(&mut pending, false);
                    }
                }
                Ok(Command::Flush { reply }) => {
                    let result = self.sync_pending(&mut pending);
                    let _ = reply.send(result.map_err(|detail| write_failed("batcher", detail)));
                }
                Ok(Command::Shutdown { reply }) => {
                    let result = self.sync_pending(&mut pending);
                    if let Err(detail) = &result {
                        tracing::error!(code = "STORAGE_WRITE_FAILED", error = %detail,
                            "storage batcher shutdown could not confirm WAL durability");
                    }
                    let _ = reply.send(result.map_err(|detail| write_failed("batcher", detail)));
                    break;
                }
                // Queue idle with writes pending: commit now (group commit).
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.finish(&mut pending, false);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if let Err(detail) = self.sync_pending(&mut pending) {
                        tracing::error!(code = "STORAGE_WRITE_FAILED", error = %detail,
                            "storage batcher disconnected without confirmed WAL durability");
                    }
                    break;
                }
            }
        }
    }

    // Synchronous commit + WAL sync. Refused while a failure is backing off.
    fn sync_pending(&self, pending: &mut PendingBatch) -> Result<(), String> {
        if let Some(detail) = self.refusal() {
            return Err(detail);
        }
        self.finish(pending, true)
    }

    // While a failure is latched, RocksDB refuses writes until it is reopened
    // (it has no resume call), so every recovery probe reopens the database
    // first. A reopen that fails keeps the batcher failed and backing off.
    fn commit(&self, pending: &mut PendingBatch, sync: bool) -> StorageResult<()> {
        if self.health_lock().failure.is_some() {
            self.db.reopen()?;
        }
        let db = self.db.get()?;
        flush_pending(&db, pending, sync)
    }

    // Every owner receives the actual write result. Health is updated before
    // any owner is released, so a caller that sees an error also sees the
    // failed state. A failed batch is dropped, never retried silently or grown.
    fn finish(&self, pending: &mut PendingBatch, sync: bool) -> Result<(), String> {
        let result = self
            .commit(pending, sync)
            .map_err(|error| error.to_string());
        let result = self.record(result);
        for (cf_name, reply) in pending.replies.drain(..) {
            let _ = reply.send(
                result
                    .clone()
                    .map_err(|detail| write_failed(&cf_name, detail)),
            );
        }
        *pending = PendingBatch::default();
        result
    }
}

#[derive(Default)]
struct Health {
    failure: Option<Failure>,
    consecutive_failures: u64,
    recoveries: u64,
}

struct Failure {
    detail: String,
    next_probe: Instant,
    backoff: Duration,
}

impl Health {
    fn refusal(&self, now: Instant) -> Option<String> {
        self.failure
            .as_ref()
            .filter(|failure| now < failure.next_probe)
            .map(|failure| failure.detail.clone())
    }

    fn record(&mut self, result: &Result<(), String>, now: Instant) {
        match result {
            Ok(()) => {
                if self.failure.take().is_some() {
                    self.recoveries = self.recoveries.saturating_add(1);
                    tracing::warn!(
                        code = "STORAGE_BATCHER_RECOVERED",
                        recoveries = self.recoveries,
                        "storage batcher write succeeded again after a failure"
                    );
                }
                self.consecutive_failures = 0;
            }
            Err(detail) => {
                let backoff = self
                    .failure
                    .as_ref()
                    .map_or(RETRY_BACKOFF_INITIAL, |failure| {
                        failure.backoff.saturating_mul(2).min(RETRY_BACKOFF_MAX)
                    });
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.failure = Some(Failure {
                    detail: detail.clone(),
                    next_probe: now + backoff,
                    backoff,
                });
            }
        }
    }

    fn status(&self, now: Instant) -> BatcherStatus {
        BatcherStatus {
            failed: self.failure.is_some(),
            detail: self.failure.as_ref().map(|failure| failure.detail.clone()),
            consecutive_failures: self.consecutive_failures,
            recoveries: self.recoveries,
            retry_in_ms: self.failure.as_ref().map(|failure| {
                u64::try_from(
                    failure
                        .next_probe
                        .saturating_duration_since(now)
                        .as_millis(),
                )
                .unwrap_or(u64::MAX)
            }),
        }
    }
}

fn batch_charge(cf_name: &str, kvs: &Vec<(Vec<u8>, Vec<u8>)>) -> StorageResult<usize> {
    let mut bytes = kvs.capacity().saturating_mul(
        2 * (std::mem::size_of::<PendingWrite>()
            + std::mem::size_of::<(String, mpsc::SyncSender<StorageResult<()>>)>()),
    );
    for (key, value) in kvs {
        bytes = bytes
            .saturating_add(key.capacity())
            .saturating_add(value.capacity())
            .saturating_add(cf_name.len());
    }
    if bytes > MAX_BATCH_BYTES {
        return Err(write_failed(
            cf_name,
            format!("batch allocation exceeds {MAX_BATCH_BYTES} byte limit"),
        ));
    }
    Ok(bytes)
}

#[derive(Default)]
struct PendingBatch {
    writes: Vec<PendingWrite>,
    replies: Vec<(String, mpsc::SyncSender<StorageResult<()>>)>,
    bytes: usize,
}

struct PendingWrite {
    cf_name: String,
    key: Vec<u8>,
    value: Vec<u8>,
}

// Blocks while nothing is pending. With writes pending it only drains what is
// already queued, reporting `Timeout` the moment the queue is idle so the
// caller commits immediately; waiters coalesce naturally while a commit runs.
fn receive_next(
    receiver: &mpsc::Receiver<Command>,
    pending: &PendingBatch,
) -> Result<Command, RecvTimeoutError> {
    if pending.writes.is_empty() {
        return receiver
            .recv()
            .map_err(|_error| RecvTimeoutError::Disconnected);
    }
    receiver.try_recv().map_err(|error| match error {
        TryRecvError::Empty => RecvTimeoutError::Timeout,
        TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
    })
}

fn enqueue(pending: &mut PendingBatch, cf_name: &str, kvs: Vec<(Vec<u8>, Vec<u8>)>) {
    if kvs.is_empty() {
        return;
    }
    for (key, value) in kvs {
        pending.writes.push(PendingWrite {
            cf_name: cf_name.to_owned(),
            key,
            value,
        });
    }
}

fn flush_pending(db: &DB, pending: &mut PendingBatch, sync: bool) -> StorageResult<()> {
    if pending.writes.is_empty() {
        if sync {
            db.flush_wal(true)
                .map_err(|error| write_failed("batcher", error.to_string()))?;
        }
        return Ok(());
    }

    let mut batch = WriteBatch::default();
    for write in &pending.writes {
        let cf = db.cf_handle(&write.cf_name).ok_or_else(|| {
            write_failed(
                &write.cf_name,
                "column family handle missing while flushing batch".to_owned(),
            )
        })?;
        batch.put_cf(&cf, &write.key, &write.value);
    }

    let mut options = WriteOptions::default();
    options.set_sync(sync);
    db.write_opt(batch, &options)
        .map_err(|error| write_failed("batcher", error.to_string()))?;
    if sync {
        db.flush_wal(true)
            .map_err(|error| write_failed("batcher", error.to_string()))?;
    }

    pending.writes.clear();
    pending.bytes = 0;
    Ok(())
}

fn write_failed(cf_name: &str, detail: String) -> StorageError {
    StorageError::WriteFailed {
        cf_name: cf_name.to_owned(),
        detail,
    }
}
