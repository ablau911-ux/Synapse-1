use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};

use crate::{
    ActionDetail, ActionReceipt, BackendIdentity, BackendKind, BrowserRuntimeConfig,
    CaptureReceipt, Frame, FrameMetadata, OwnerId, SeatAction, SeatConfig, SeatError,
    SeatGeneration, SeatId, SeatState, SeatSupervisorConfig, SeatView, TeardownStatus, now_ms,
};

#[async_trait]
pub trait SeatBackend: Send + Sync {
    fn identity(&self) -> BackendIdentity;
    /// True only when successful close proves the isolated process has exited,
    /// ending every input it owns. Shared guests still need verified key-up.
    fn inputs_end_with_process(&self) -> bool {
        false
    }
    async fn act(&mut self, action: SeatAction) -> Result<ActionDetail, SeatError>;
    async fn capture(&mut self) -> Result<Frame, SeatError>;
    async fn release_inputs(&mut self) -> Result<(), SeatError>;
    async fn close(&mut self) -> Result<(), SeatError>;
}

#[async_trait]
pub trait SeatBackendFactory: Send + Sync {
    fn kind(&self) -> BackendKind;
    async fn create(
        &self,
        seat_id: SeatId,
        config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError>;
    /// Physical resources a starting seat may leave behind (private paths,
    /// child pid), known without awaiting. Named in every pending
    /// startup-teardown report (#73).
    fn startup_resources(&self, seat_id: SeatId) -> String {
        format!(
            "{:?} backend for seat {seat_id} reports no physical resources",
            self.kind()
        )
    }
    /// Physical readback after the supervisor dropped this seat's creation
    /// future at its cutoff, bounded by `allowance` (#73). `resources` is the
    /// `startup_resources` snapshot taken before that drop. The default cannot
    /// confirm anything, so it reports the teardown as pending.
    async fn abandoned_startup_teardown(
        &self,
        _seat_id: SeatId,
        resources: &str,
        _allowance: Duration,
    ) -> TeardownStatus {
        TeardownStatus::Pending(format!(
            "startup abandoned at the supervisor cutoff; teardown has no physical readback; {resources}"
        ))
    }
}

struct SeatEntry {
    owner: OwnerId,
    generation: SeatGeneration,
    state: SeatState,
    backend: Box<dyn SeatBackend>,
    sequence: u64,
    latest_receipt: Option<ActionReceipt>,
    latest_frame: Option<FrameMetadata>,
    live_permit: Option<OwnedSemaphorePermit>,
    last_owned_action: std::time::Instant,
}

struct SeatCell {
    owner: OwnerId,
    queue: Arc<Semaphore>,
    entry: Mutex<SeatEntry>,
}

#[derive(Clone)]
pub struct SeatSupervisor {
    inner: Arc<SupervisorInner>,
}

impl std::fmt::Debug for SeatSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SeatSupervisor")
            .finish_non_exhaustive()
    }
}

struct SupervisorInner {
    config: SeatSupervisorConfig,
    seats: RwLock<HashMap<SeatId, Arc<SeatCell>>>,
    factories: RwLock<HashMap<BackendKind, Arc<dyn SeatBackendFactory>>>,
    capacity: Arc<Semaphore>,
}

impl SeatSupervisor {
    pub fn new(config: SeatSupervisorConfig) -> Result<Self, SeatError> {
        Self::new_with_backends(config, Vec::new())
    }

    pub fn new_with_backends(
        config: SeatSupervisorConfig,
        backends: Vec<Arc<dyn SeatBackendFactory>>,
    ) -> Result<Self, SeatError> {
        let browser = Arc::new(crate::browser::BrowserBackendFactory::new(
            config.browser.clone(),
            config.startup_teardown_allowance(),
        ));
        Self::assemble(config, browser, backends)
    }

    /// `new_with_backends` with an explicit browser factory (test seam for a
    /// fake browser child; production always passes `BrowserBackendFactory`).
    pub(crate) fn assemble(
        config: SeatSupervisorConfig,
        browser: Arc<dyn SeatBackendFactory>,
        backends: Vec<Arc<dyn SeatBackendFactory>>,
    ) -> Result<Self, SeatError> {
        config.validate()?;
        let mut factories: HashMap<BackendKind, Arc<dyn SeatBackendFactory>> = HashMap::new();
        factories.insert(BackendKind::Browser, browser);
        for backend in backends {
            let kind = backend.kind();
            if factories.insert(kind, backend).is_some() {
                return Err(SeatError::BackendAlreadyRegistered(kind));
            }
        }
        Ok(Self {
            inner: Arc::new(SupervisorInner {
                capacity: Arc::new(Semaphore::new(config.max_seats)),
                config,
                seats: RwLock::new(HashMap::new()),
                factories: RwLock::new(factories),
            }),
        })
    }

    pub async fn register_backend(
        &self,
        factory: Arc<dyn SeatBackendFactory>,
    ) -> Result<(), SeatError> {
        let kind = factory.kind();
        let mut factories = self.inner.factories.write().await;
        if factories.contains_key(&kind) {
            return Err(SeatError::BackendAlreadyRegistered(kind));
        }
        factories.insert(kind, factory);
        Ok(())
    }

    pub async fn create(&self, owner: OwnerId, config: SeatConfig) -> Result<SeatView, SeatError> {
        config.validate()?;
        self.prune_closed().await;
        let permit = self
            .inner
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| SeatError::CapacityExhausted)?;
        let id = SeatId::new();
        let factory = match self
            .inner
            .factories
            .read()
            .await
            .get(&config.backend_kind())
            .cloned()
        {
            Some(factory) => factory,
            None => return Err(SeatError::BackendUnavailable(config.backend_kind())),
        };
        // `permit` is a local: every early return below, including the startup
        // deadline, drops it and returns the capacity slot (#73).
        let backend = self.start_backend(factory.as_ref(), id, &config).await?;
        let entry = SeatEntry {
            owner,
            generation: SeatGeneration(1),
            state: SeatState::Ready,
            backend,
            sequence: 0,
            latest_receipt: None,
            latest_frame: None,
            live_permit: Some(permit),
            last_owned_action: std::time::Instant::now(),
        };
        let cell = Arc::new(SeatCell {
            owner: cell_owner(&entry),
            queue: Arc::new(Semaphore::new(self.inner.config.per_seat_queue_capacity)),
            entry: Mutex::new(entry),
        });
        let entry = cell.entry.lock().await;
        let view = view_from_entry(id, &entry);
        drop(entry);
        self.inner.seats.write().await.insert(id, cell);
        Ok(view)
    }

    pub async fn list(&self, owner: &OwnerId) -> Vec<SeatView> {
        let seats: Vec<(SeatId, Arc<SeatCell>)> = self
            .inner
            .seats
            .read()
            .await
            .iter()
            .map(|(id, cell)| (*id, Arc::clone(cell)))
            .collect();
        let mut views = Vec::new();
        for (id, cell) in seats {
            if &cell.owner == owner
                && let Ok(entry) = tokio::time::timeout(
                    Duration::from_millis(self.inner.config.request_timeout_ms),
                    cell.entry.lock(),
                )
                .await
            {
                views.push(view_from_entry(id, &entry));
            }
        }
        views
    }

    /// Read-only fleet metadata for the daemon's authenticated supervision surface.
    /// This intentionally has no corresponding cross-owner control operation.
    pub async fn list_all_for_supervision(&self) -> Vec<SeatView> {
        let seats: Vec<(SeatId, Arc<SeatCell>)> = self
            .inner
            .seats
            .read()
            .await
            .iter()
            .map(|(id, cell)| (*id, Arc::clone(cell)))
            .collect();
        let mut views = Vec::with_capacity(seats.len());
        for (id, cell) in seats {
            if let Ok(entry) = tokio::time::timeout(
                Duration::from_millis(self.inner.config.request_timeout_ms),
                cell.entry.lock(),
            )
            .await
            {
                views.push(view_from_entry(id, &entry));
            }
        }
        views
    }

    /// Owners with entries in the authoritative seat registry.
    /// Used by daemon lifecycle cleanup to reap seats after an MCP transport
    /// disappears without completing its DELETE request.
    pub fn try_owner_ids(&self) -> Result<Vec<OwnerId>, SeatError> {
        let seats = self
            .inner
            .seats
            .try_read()
            .map_err(|_| SeatError::Backend("seat registry is busy".to_owned()))?;
        let mut owners: Vec<OwnerId> = seats
            .values()
            .filter_map(|cell| match cell.entry.try_lock() {
                Ok(entry) if entry.state == SeatState::Closed => None,
                Ok(_) | Err(_) => Some(cell.owner.clone()),
            })
            .collect();
        owners.sort();
        owners.dedup();
        Ok(owners)
    }

    pub async fn get(&self, owner: &OwnerId, id: SeatId) -> Result<SeatView, SeatError> {
        let cell = self.lookup(owner, id).await?;
        let entry = self.lock_entry(id, &cell).await?;
        Ok(view_from_entry(id, &entry))
    }

    pub async fn act(
        &self,
        owner: &OwnerId,
        id: SeatId,
        generation: SeatGeneration,
        action: SeatAction,
    ) -> Result<ActionReceipt, SeatError> {
        action.validate()?;
        let cell = self.lookup(owner, id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        validate_control(id, &entry, generation)?;
        let detail =
            match tokio::time::timeout(self.request_timeout(), entry.backend.act(action)).await {
                Ok(Ok(detail)) => detail,
                Ok(Err(error)) if is_uncertain(&error) => {
                    return Err(self.mark_failed(id, &mut entry, error).await);
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(self
                        .mark_failed(id, &mut entry, SeatError::Timeout(id))
                        .await);
                }
            };
        entry.sequence = entry
            .sequence
            .checked_add(1)
            .ok_or_else(|| SeatError::Backend("receipt sequence overflow".to_owned()))?;
        let receipt = ActionReceipt {
            seat_id: id,
            generation,
            sequence: entry.sequence,
            timestamp_ms: now_ms(),
            detail,
        };
        entry.latest_receipt = Some(receipt.clone());
        entry.last_owned_action = std::time::Instant::now();
        Ok(receipt)
    }

    pub async fn capture(
        &self,
        owner: &OwnerId,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<CaptureReceipt, SeatError> {
        let cell = self.lookup(owner, id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        validate_control(id, &entry, generation)?;
        let frame =
            match tokio::time::timeout(self.request_timeout(), entry.backend.capture()).await {
                Ok(Ok(frame)) => frame,
                Ok(Err(error)) if is_uncertain(&error) => {
                    return Err(self.mark_failed(id, &mut entry, error).await);
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(self
                        .mark_failed(id, &mut entry, SeatError::Timeout(id))
                        .await);
                }
            };
        entry.sequence = entry
            .sequence
            .checked_add(1)
            .ok_or_else(|| SeatError::Backend("receipt sequence overflow".to_owned()))?;
        entry.latest_frame = Some(FrameMetadata {
            sequence: entry.sequence,
            width: frame.width,
            height: frame.height,
            timestamp_ms: frame.timestamp_ms,
        });
        entry.last_owned_action = std::time::Instant::now();
        Ok(CaptureReceipt {
            seat_id: id,
            generation,
            sequence: entry.sequence,
            timestamp_ms: now_ms(),
            frame,
        })
    }

    pub async fn pause(
        &self,
        owner: &OwnerId,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let cell = self.lookup(owner, id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        validate_control(id, &entry, generation)?;
        match tokio::time::timeout(self.request_timeout(), entry.backend.release_inputs()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(self.mark_failed(id, &mut entry, error).await);
            }
            Err(_) => {
                return Err(self
                    .mark_failed(id, &mut entry, SeatError::Timeout(id))
                    .await);
            }
        }
        entry.state = SeatState::Paused;
        entry.last_owned_action = std::time::Instant::now();
        Ok(view_from_entry(id, &entry))
    }

    pub async fn resume(
        &self,
        owner: &OwnerId,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let cell = self.lookup(owner, id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        if entry.generation != generation {
            return Err(SeatError::StaleGeneration {
                id,
                provided: generation.0,
                current: entry.generation.0,
            });
        }
        if entry.state != SeatState::Paused {
            return Err(state_error(id, &entry));
        }
        entry.generation = SeatGeneration(
            entry
                .generation
                .0
                .checked_add(1)
                .ok_or_else(|| SeatError::Backend("seat generation overflow".to_owned()))?,
        );
        entry.state = SeatState::Ready;
        entry.last_owned_action = std::time::Instant::now();
        Ok(view_from_entry(id, &entry))
    }

    pub async fn close(
        &self,
        owner: &OwnerId,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let cell = self.lookup(owner, id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        if entry.generation != generation {
            return Err(SeatError::StaleGeneration {
                id,
                provided: generation.0,
                current: entry.generation.0,
            });
        }
        if entry.state == SeatState::Closed {
            return Ok(view_from_entry(id, &entry));
        }
        // A failed operation may already have released and closed this backend.
        // Do not send further input to a terminal backend merely to update its view.
        if entry.live_permit.is_none() {
            entry.state = SeatState::Closed;
            return Ok(view_from_entry(id, &entry));
        }
        if let Err(error) = self.cleanup_backend(&mut entry).await {
            entry.state = SeatState::Failed;
            entry.last_owned_action = std::time::Instant::now();
            return Err(SeatError::Failed(id, error.to_string()));
        }
        if let Err(error) = self.release_admission(&mut entry) {
            entry.state = SeatState::Failed;
            return Err(SeatError::Failed(id, error.to_string()));
        }
        entry.state = SeatState::Closed;
        Ok(view_from_entry(id, &entry))
    }

    pub async fn close_owner(&self, owner: &OwnerId) -> Vec<Result<SeatView, SeatError>> {
        // Do not derive teardown work from `list`: list intentionally omits a
        // contended entry to keep ordinary reads non-blocking.  Session teardown
        // must instead account for every owner cell, including an action that is
        // still holding its entry lock, so the lifecycle report can surface a
        // cleanup failure rather than falsely reporting no owned seats.
        let targets: Vec<SeatId> = self
            .inner
            .seats
            .read()
            .await
            .iter()
            .filter_map(|(id, cell)| (&cell.owner == owner).then_some(*id))
            .collect();
        let mut results = Vec::with_capacity(targets.len());
        for id in targets {
            match self.get(owner, id).await {
                Ok(view) if view.state == SeatState::Closed => {}
                Ok(view) => results.push(self.close(owner, id, view.generation).await),
                Err(error) => results.push(Err(error)),
            }
        }
        results
    }

    /// Privileged, local-dashboard capture. It intentionally does not update owner activity.
    pub async fn capture_for_supervision(
        &self,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<CaptureReceipt, SeatError> {
        let cell = self.lookup_supervision(id).await?;
        let _queued = self.acquire_queue(id, &cell).await?;
        let mut entry = self.lock_entry(id, &cell).await?;
        if entry.generation != generation {
            return Err(SeatError::StaleGeneration {
                id,
                provided: generation.0,
                current: entry.generation.0,
            });
        }
        if !matches!(entry.state, SeatState::Ready | SeatState::Paused) {
            return Err(state_error(id, &entry));
        }
        let frame =
            match tokio::time::timeout(self.request_timeout(), entry.backend.capture()).await {
                Ok(Ok(frame)) => frame,
                Ok(Err(error)) if is_uncertain(&error) => {
                    return Err(self.mark_failed(id, &mut entry, error).await);
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => {
                    return Err(self
                        .mark_failed(id, &mut entry, SeatError::Timeout(id))
                        .await);
                }
            };
        entry.sequence = entry
            .sequence
            .checked_add(1)
            .ok_or_else(|| SeatError::backend("receipt sequence overflow"))?;
        entry.latest_frame = Some(FrameMetadata {
            sequence: entry.sequence,
            width: frame.width,
            height: frame.height,
            timestamp_ms: frame.timestamp_ms,
        });
        Ok(CaptureReceipt {
            seat_id: id,
            generation,
            sequence: entry.sequence,
            timestamp_ms: now_ms(),
            frame,
        })
    }

    pub async fn pause_for_supervision(
        &self,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let owner = self.owner_for(id).await?;
        self.pause(&owner, id, generation).await
    }
    pub async fn resume_for_supervision(
        &self,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let owner = self.owner_for(id).await?;
        self.resume(&owner, id, generation).await
    }
    pub async fn close_for_supervision(
        &self,
        id: SeatId,
        generation: SeatGeneration,
    ) -> Result<SeatView, SeatError> {
        let owner = self.owner_for(id).await?;
        self.close(&owner, id, generation).await
    }

    /// Closes only idle, non-executing seats; a busy seat is left untouched for a later pass.
    pub async fn reap_idle(&self, max_idle: Duration) -> Vec<Result<SeatView, SeatError>> {
        let seats: Vec<(SeatId, Arc<SeatCell>)> = self
            .inner
            .seats
            .read()
            .await
            .iter()
            .map(|(id, cell)| (*id, Arc::clone(cell)))
            .collect();
        let mut results = Vec::new();
        for (id, cell) in seats {
            let Ok(_queued) = cell.queue.clone().try_acquire_owned() else {
                continue;
            };
            let Ok(mut entry) = cell.entry.try_lock() else {
                continue;
            };
            if entry.state == SeatState::Closed || entry.last_owned_action.elapsed() < max_idle {
                continue;
            }
            if entry.live_permit.is_none() {
                entry.state = SeatState::Closed;
                results.push(Ok(view_from_entry(id, &entry)));
                continue;
            }
            let outcome = match self.cleanup_backend(&mut entry).await {
                Ok(()) => match self.release_admission(&mut entry) {
                    Ok(()) => {
                        entry.state = SeatState::Closed;
                        Ok(view_from_entry(id, &entry))
                    }
                    Err(error) => {
                        entry.state = SeatState::Failed;
                        Err(SeatError::Failed(id, error.to_string()))
                    }
                },
                Err(error) => {
                    entry.state = SeatState::Failed;
                    entry.last_owned_action = std::time::Instant::now();
                    Err(SeatError::Failed(id, error.to_string()))
                }
            };
            results.push(outcome);
        }
        results
    }

    async fn lookup(&self, owner: &OwnerId, id: SeatId) -> Result<Arc<SeatCell>, SeatError> {
        let cell = self
            .inner
            .seats
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(SeatError::NotFound(id))?;
        if cell.owner != *owner {
            return Err(SeatError::NotOwner(id));
        }
        Ok(cell)
    }

    async fn lookup_supervision(&self, id: SeatId) -> Result<Arc<SeatCell>, SeatError> {
        self.inner
            .seats
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(SeatError::NotFound(id))
    }
    async fn owner_for(&self, id: SeatId) -> Result<OwnerId, SeatError> {
        Ok(self.lookup_supervision(id).await?.owner.clone())
    }

    async fn acquire_queue(
        &self,
        id: SeatId,
        cell: &Arc<SeatCell>,
    ) -> Result<OwnedSemaphorePermit, SeatError> {
        cell.queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| SeatError::QueueFull(id))
    }

    async fn lock_entry<'a>(
        &self,
        id: SeatId,
        cell: &'a Arc<SeatCell>,
    ) -> Result<tokio::sync::MutexGuard<'a, SeatEntry>, SeatError> {
        tokio::time::timeout(self.request_timeout(), cell.entry.lock())
            .await
            .map_err(|_| SeatError::Timeout(id))
    }

    fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.inner.config.request_timeout_ms)
    }

    /// C5 (#73): one overall deadline (startup + request timeout) covers the
    /// backend creation, its cancellation, the abandoned-startup readback and
    /// the return. Creation is cut off early enough to keep the backstop reserve
    /// for that readback; the cut-off future is dropped (its guards start the
    /// teardown). Covers every factory, not only browsers.
    async fn start_backend(
        &self,
        factory: &dyn SeatBackendFactory,
        id: SeatId,
        config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        let timeout_ms = self.inner.config.startup_budget_ms();
        let deadline = instant_after(Duration::from_millis(timeout_ms));
        let reserve = self.inner.config.startup_backstop_reserve();
        let cutoff = deadline.checked_sub(reserve).unwrap_or(deadline);
        let mut creation = factory.create(id, config);
        if let Ok(created) = tokio::time::timeout_at(cutoff, &mut creation).await {
            return created;
        }
        // Hand-over: snapshot the physical resources while the creation future
        // (and its ledger entries) is still alive, then drop it; its guards
        // start the teardown and remove the entries.
        let resources = factory.startup_resources(id);
        drop(creation);
        let teardown = abandoned_readback(factory, id, &resources, deadline).await;
        Err(SeatError::StartupTimeout {
            timeout_ms,
            teardown,
        })
    }

    /// Shared guests require verified input release before close. Isolated
    /// browser inputs end with confirmed process exit, so unresponsive CDP must
    /// not prevent their terminal teardown. Pause still uses `release_inputs`.
    async fn cleanup_backend(&self, entry: &mut SeatEntry) -> Result<(), SeatError> {
        if !entry.backend.inputs_end_with_process() {
            match tokio::time::timeout(self.request_timeout(), entry.backend.release_inputs()).await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(SeatError::backend(format!(
                        "input release during terminal cleanup failed: {error}"
                    )));
                }
                Err(_) => {
                    return Err(SeatError::backend(
                        "input release during terminal cleanup timed out",
                    ));
                }
            }
        }
        match tokio::time::timeout(self.request_timeout(), entry.backend.close()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(SeatError::backend(format!(
                "backend close during terminal cleanup failed: {error}"
            ))),
            Err(_) => Err(SeatError::backend(
                "backend close during terminal cleanup timed out",
            )),
        }
    }

    /// Return the concurrency permit only after cleanup has conclusively
    /// released input and closed the backend.
    fn release_admission(&self, entry: &mut SeatEntry) -> Result<(), SeatError> {
        let _ = entry.live_permit.take();
        Ok(())
    }

    async fn mark_failed(&self, id: SeatId, entry: &mut SeatEntry, error: SeatError) -> SeatError {
        entry.state = SeatState::Failed;
        entry.last_owned_action = std::time::Instant::now();
        match self.cleanup_backend(entry).await {
            Ok(()) => match self.release_admission(entry) {
                Ok(()) => SeatError::Failed(id, error.to_string()),
                Err(cleanup_error) => SeatError::Failed(
                    id,
                    format!(
                        "{error}; terminal cleanup could not release admission: {cleanup_error}"
                    ),
                ),
            },
            Err(cleanup_error) => SeatError::Failed(
                id,
                format!("{error}; terminal cleanup remains pending: {cleanup_error}"),
            ),
        }
    }

    async fn prune_closed(&self) {
        let candidates: Vec<(SeatId, Arc<SeatCell>)> = self
            .inner
            .seats
            .read()
            .await
            .iter()
            .map(|(id, cell)| (*id, Arc::clone(cell)))
            .collect();
        let closed: Vec<SeatId> = candidates
            .into_iter()
            .filter_map(|(id, cell)| {
                cell.entry
                    .try_lock()
                    .ok()
                    .and_then(|entry| (entry.state == SeatState::Closed).then_some(id))
            })
            .collect();
        if !closed.is_empty() {
            let mut seats = self.inner.seats.write().await;
            for id in closed {
                seats.remove(&id);
            }
        }
    }
}

fn instant_after(duration: Duration) -> tokio::time::Instant {
    const FAR_FUTURE: Duration = Duration::from_secs(60 * 60 * 24 * 365);
    let now = tokio::time::Instant::now();
    now.checked_add(duration)
        .unwrap_or_else(|| now + FAR_FUTURE)
}

/// The factory's physical readback of an abandoned startup, bounded by the C5
/// deadline. If it cannot finish, the report still names the physical
/// resources (paths, child pid) the factory recorded for this seat (#73).
async fn abandoned_readback(
    factory: &dyn SeatBackendFactory,
    id: SeatId,
    resources: &str,
    deadline: tokio::time::Instant,
) -> TeardownStatus {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let readback = factory.abandoned_startup_teardown(id, resources, remaining / 2);
    tokio::time::timeout_at(deadline, readback)
        .await
        .unwrap_or_else(|_| {
            TeardownStatus::Pending(format!(
                "abandoned-startup readback did not finish inside the startup budget; {resources}"
            ))
        })
}

fn validate_control(
    id: SeatId,
    entry: &SeatEntry,
    generation: SeatGeneration,
) -> Result<(), SeatError> {
    if entry.generation != generation {
        return Err(SeatError::StaleGeneration {
            id,
            provided: generation.0,
            current: entry.generation.0,
        });
    }
    if entry.state != SeatState::Ready {
        return Err(state_error(id, entry));
    }
    Ok(())
}
fn state_error(id: SeatId, entry: &SeatEntry) -> SeatError {
    match entry.state {
        SeatState::Paused => SeatError::Paused(id),
        SeatState::Closed => SeatError::Closed(id),
        SeatState::Failed => SeatError::Failed(id, "backend previously failed".to_owned()),
        SeatState::Starting | SeatState::Ready => {
            SeatError::Backend("invalid control state".to_owned())
        }
    }
}
fn is_uncertain(error: &SeatError) -> bool {
    matches!(
        error,
        SeatError::Backend(_) | SeatError::Timeout(_) | SeatError::Failed(_, _)
    )
}
fn view_from_entry(id: SeatId, entry: &SeatEntry) -> SeatView {
    SeatView {
        id,
        owner: entry.owner.clone(),
        generation: entry.generation,
        backend: entry.backend.identity(),
        state: entry.state.clone(),
        latest_receipt: entry.latest_receipt.clone(),
        latest_frame: entry.latest_frame.clone(),
    }
}
fn cell_owner(entry: &SeatEntry) -> OwnerId {
    entry.owner.clone()
}
#[cfg(test)]
#[path = "browser_runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runtime_startup_deadline_73.rs"]
mod startup_deadline_73_tests;

#[allow(dead_code)]
fn _browser_config_is_daemon_owned(_config: &BrowserRuntimeConfig) {}
