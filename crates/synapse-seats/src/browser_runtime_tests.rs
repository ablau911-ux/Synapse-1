use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;

use super::{SeatBackend, SeatBackendFactory, SeatSupervisor};
use crate::{
    ActionDetail, BackendIdentity, BackendKind, BrowserRuntimeConfig, Frame, MockSeatConfig,
    OwnerId, SeatAction, SeatConfig, SeatError, SeatId, SeatState, SeatSupervisorConfig,
};

struct MemoryBackend {
    releases: Arc<AtomicUsize>,
}

#[async_trait]
impl SeatBackend for MemoryBackend {
    fn identity(&self) -> BackendIdentity {
        BackendIdentity {
            kind: BackendKind::Mock,
            target: "memory-browser".to_owned(),
            process_id: None,
        }
    }

    async fn act(&mut self, _action: SeatAction) -> Result<ActionDetail, SeatError> {
        Ok(ActionDetail::TextInserted { byte_count: 1 })
    }

    async fn capture(&mut self) -> Result<Frame, SeatError> {
        Ok(Frame {
            width: 1,
            height: 1,
            timestamp_ms: 1,
            mime_type: "image/png".to_owned(),
            image_base64: "iVBORw0KGgo=".to_owned(),
        })
    }

    async fn release_inputs(&mut self) -> Result<(), SeatError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn close(&mut self) -> Result<(), SeatError> {
        Ok(())
    }
}

struct MemoryFactory {
    releases: Arc<AtomicUsize>,
}

#[async_trait]
impl SeatBackendFactory for MemoryFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }

    async fn create(
        &self,
        _seat_id: SeatId,
        _config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        Ok(Box::new(MemoryBackend {
            releases: Arc::clone(&self.releases),
        }))
    }
}

fn supervisor_config() -> SeatSupervisorConfig {
    SeatSupervisorConfig {
        browser: BrowserRuntimeConfig {
            chromium_executable: PathBuf::from("unused-in-memory-test.exe"),
            runtime_root: PathBuf::from("unused-in-memory-test"),
            startup_timeout_ms: 1_000,
        },
        max_seats: 2,
        per_seat_queue_capacity: 1,
        request_timeout_ms: 1_000,
    }
}

fn mock_config() -> SeatConfig {
    SeatConfig::Mock(MockSeatConfig {})
}

#[cfg(windows)]
#[tokio::test]
async fn terminal_browser_cleanup_ends_held_inputs_without_cdp()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (backend, _peer, root) = crate::browser::cleanup_tests::unresponsive_backend().await?;
    assert!(backend.inputs_end_with_process());
    let profile_root = root.join("browser-seat");
    let supervisor = SeatSupervisor::new(supervisor_config())?;
    let mut entry = super::SeatEntry {
        owner: OwnerId::new("terminal-cleanup-test"),
        generation: crate::SeatGeneration(1),
        state: SeatState::Failed,
        backend: Box::new(backend),
        sequence: 0,
        latest_receipt: None,
        latest_frame: None,
        live_permit: None,
        last_owned_action: std::time::Instant::now(),
    };
    supervisor.cleanup_backend(&mut entry).await?;
    drop(entry);
    assert!(!tokio::fs::try_exists(profile_root).await?);
    // If release_inputs had been attempted, the silent peer would time out
    // before close, preserving this marker and returning an error.
    tokio::fs::remove_dir(root).await?;
    Ok(())
}

#[tokio::test]
async fn owner_generation_pause_and_close_are_enforced() {
    let supervisor = SeatSupervisor::new(supervisor_config())
        .unwrap_or_else(|error| panic!("supervisor config failed: {error}"));
    let releases = Arc::new(AtomicUsize::new(0));
    supervisor
        .register_backend(Arc::new(MemoryFactory {
            releases: Arc::clone(&releases),
        }))
        .await
        .unwrap_or_else(|error| panic!("mock backend registration failed: {error}"));

    let owner = OwnerId::new("session-a");
    let other = OwnerId::new("session-b");
    let created = supervisor
        .create(owner.clone(), mock_config())
        .await
        .unwrap_or_else(|error| panic!("seat create failed: {error}"));
    assert!(matches!(
        supervisor.get(&other, created.id).await,
        Err(SeatError::NotOwner(_))
    ));

    let paused = supervisor
        .pause(&owner, created.id, created.generation)
        .await
        .unwrap_or_else(|error| panic!("seat pause failed: {error}"));
    assert_eq!(paused.state, SeatState::Paused);
    assert_eq!(releases.load(Ordering::SeqCst), 1);

    let resumed = supervisor
        .resume(&owner, created.id, created.generation)
        .await
        .unwrap_or_else(|error| panic!("seat resume failed: {error}"));
    assert!(resumed.generation.0 > created.generation.0);
    assert!(matches!(
        supervisor
            .close(&owner, created.id, created.generation)
            .await,
        Err(SeatError::StaleGeneration { .. })
    ));
    let closed = supervisor
        .close(&owner, created.id, resumed.generation)
        .await
        .unwrap_or_else(|error| panic!("seat close failed: {error}"));
    assert_eq!(closed.state, SeatState::Closed);
}

#[tokio::test]
async fn owner_teardown_closes_every_owned_seat() {
    let supervisor = SeatSupervisor::new(supervisor_config())
        .unwrap_or_else(|error| panic!("supervisor config failed: {error}"));
    let releases = Arc::new(AtomicUsize::new(0));
    supervisor
        .register_backend(Arc::new(MemoryFactory {
            releases: Arc::clone(&releases),
        }))
        .await
        .unwrap_or_else(|error| panic!("mock backend registration failed: {error}"));
    let owner = OwnerId::new("session-a");
    for _ in 0..2 {
        supervisor
            .create(owner.clone(), mock_config())
            .await
            .unwrap_or_else(|error| panic!("seat create failed: {error}"));
    }
    assert_eq!(
        supervisor
            .try_owner_ids()
            .unwrap_or_else(|error| panic!("owner scan failed: {error}")),
        vec![owner.clone()]
    );
    let results = supervisor.close_owner(&owner).await;
    assert_eq!(results.len(), 2);
    assert!(results.into_iter().all(|result| result.is_ok()));
    assert_eq!(releases.load(Ordering::SeqCst), 2);
    assert!(
        supervisor
            .try_owner_ids()
            .unwrap_or_else(|error| panic!("owner scan failed: {error}"))
            .is_empty()
    );
}
