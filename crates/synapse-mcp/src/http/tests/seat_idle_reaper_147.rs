//! #147: drive the REAL idle seat-owner reaper (`reclaim_idle_seat_owners`) on a service
//! whose SSE state this test owns, then read the seat registry and the session registry
//! back separately.
//!
//! The crash case from the first-review PARTIAL: a client that called `subscribe`,
//! opened its stream and then died. The subscription row survives; only the dropped
//! stream proves the client is gone. Its seat must close, its session must stay.

use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{http::HeaderMap, response::Response};
use synapse_core::EventFilter;
use synapse_seats::{
    ActionDetail, BackendIdentity, BackendKind, Frame, MockSeatConfig, OwnerId, SeatAction,
    SeatBackend, SeatBackendFactory, SeatConfig, SeatError, SeatId, SeatState,
};
use tokio_util::sync::CancellationToken;

use super::{EventsQuery, SseState};
use crate::server::SynapseService;

const LIMIT_MS: u64 = 15 * 60_000;
const IDLE_MS: u64 = 45 * 60_000;

struct CountingBackend(Arc<AtomicUsize>);

#[async_trait]
impl SeatBackend for CountingBackend {
    fn identity(&self) -> BackendIdentity {
        BackendIdentity {
            kind: BackendKind::Mock,
            target: "reaper-147".to_owned(),
            process_id: None,
        }
    }
    async fn act(&mut self, _action: SeatAction) -> Result<ActionDetail, SeatError> {
        Ok(ActionDetail::TextInserted { byte_count: 0 })
    }
    async fn capture(&mut self) -> Result<Frame, SeatError> {
        Err(SeatError::Backend("capture unused in #147 test".to_owned()))
    }
    async fn release_inputs(&mut self) -> Result<(), SeatError> {
        Ok(())
    }
    async fn close(&mut self) -> Result<(), SeatError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct CountingFactory(Arc<AtomicUsize>);

#[async_trait]
impl SeatBackendFactory for CountingFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Mock
    }
    async fn create(
        &self,
        _id: SeatId,
        _c: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        Ok(Box::new(CountingBackend(Arc::clone(&self.0))))
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    sse: SseState,
    service: SynapseService,
    closes: Arc<AtomicUsize>,
}

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("root: {error}"));
    let max = NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN);
    let sse = SseState::with_max_subscriptions(max);
    let m3 = crate::m3::M3ServiceConfig::from_cli_parts(
        Some(root.path().join("db")),
        Some(root.path().to_path_buf()),
        false,
        "127.0.0.1:0".to_owned(),
        max,
        false,
        true,
        None,
        false,
        None,
    );
    let service = SynapseService::try_with_m2_shutdown_reason_and_sse_state_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        sse.clone(),
        &crate::m2::M2ServiceConfig::default(),
        m3,
        crate::m4::M4ServiceConfig::default(),
    )
    .unwrap_or_else(|error| panic!("service: {error}"));
    let closes = Arc::new(AtomicUsize::new(0));
    service
        .seat_supervisor_handle()
        .register_backend(Arc::new(CountingFactory(Arc::clone(&closes))))
        .await
        .unwrap_or_else(|error| panic!("register mock backend: {error}"));
    Fixture {
        _root: root,
        sse,
        service,
        closes,
    }
}

impl Fixture {
    async fn owner_with_seat(&self, session: &str, idle_ms: u64) -> OwnerId {
        let owner = OwnerId::new(session);
        self.service
            .seat_supervisor_handle()
            .create(owner.clone(), SeatConfig::Mock(MockSeatConfig {}))
            .await
            .unwrap_or_else(|error| panic!("seat create: {error}"));
        let now = crate::server::session_registry::unix_time_ms_now();
        let registry = self.service.session_registry_handle();
        let mut registry = registry.lock().unwrap_or_else(|_| panic!("registry lock"));
        registry.record_seen(
            session,
            Some("seat_create".to_owned()),
            now.saturating_sub(idle_ms),
        );
        owner
    }

    fn open_stream(&self, session: &str) -> Response {
        let id = self
            .sse
            .subscribe(
                EventFilter::All,
                Vec::new(),
                false,
                Some(session.to_owned()),
            )
            .unwrap_or_else(|error| panic!("subscribe: {error:?}"));
        self.sse.open(
            &HeaderMap::new(),
            EventsQuery {
                subscription_id: Some(id),
            },
        )
    }

    async fn seats(&self, owner: &OwnerId) -> usize {
        self.service
            .seat_supervisor_handle()
            .list(owner)
            .await
            .into_iter()
            .filter(|seat| seat.state != SeatState::Closed)
            .count()
    }

    fn lifecycle(&self, session: &str) -> Option<String> {
        let now = crate::server::session_registry::unix_time_ms_now();
        let registry = self.service.session_registry_handle();
        let registry = registry.lock().unwrap_or_else(|_| panic!("registry lock"));
        registry
            .reads(now)
            .into_iter()
            .find(|read| read.session_id == session)
            .map(|read| read.lifecycle)
    }

    async fn reap(&self) -> usize {
        let state = self
            .service
            .session_lifecycle_state()
            .unwrap_or_else(|e| panic!("{e:?}"));
        state.reclaim_idle_seat_owners(LIMIT_MS).await
    }
}

#[tokio::test]
async fn must_fail_crashed_subscriber_seat_is_reclaimed_but_session_kept_147() {
    let fixture = fixture().await;
    let owner = fixture.owner_with_seat("issue147-crashed", IDLE_MS).await;
    drop(fixture.open_stream("issue147-crashed")); // the client's stream died with it
    assert_eq!(fixture.seats(&owner).await, 1, "precondition: seat owned");
    // The registry label is derived from idle age (`session_registry.rs`
    // `entry_read`): a session last seen 45 min ago reads "stale" (> 5 min),
    // never "live". "Kept" means the entry stays registered and is not closed.
    assert_eq!(
        fixture.lifecycle("issue147-crashed").as_deref(),
        Some("stale"),
        "precondition: idle session registered and open"
    );

    let closed = fixture.reap().await;

    assert_eq!(closed, 1);
    assert_eq!(
        fixture.seats(&owner).await,
        0,
        "the crashed owner's seat must close"
    );
    assert_eq!(
        fixture.closes.load(Ordering::SeqCst),
        1,
        "backend close must run"
    );
    let retained = fixture.service.seat_supervisor_handle().list(&owner).await;
    assert_eq!(retained.len(), 1, "closed seat keeps its terminal readback");
    assert_eq!(retained[0].state, SeatState::Closed);
    assert_eq!(
        fixture.lifecycle("issue147-crashed").as_deref(),
        Some("stale"),
        "session kept: still registered, never closed by the seat reaper"
    );
}

#[tokio::test]
async fn must_pass_attached_stream_keeps_the_idle_owners_seat_147() {
    let fixture = fixture().await;
    let owner = fixture.owner_with_seat("issue147-streaming", IDLE_MS).await;
    let live = fixture.open_stream("issue147-streaming");

    let closed = fixture.reap().await;

    assert_eq!(closed, 0);
    assert_eq!(
        fixture.seats(&owner).await,
        1,
        "an attached stream is activity"
    );
    assert_eq!(fixture.closes.load(Ordering::SeqCst), 0);
    drop(live);
}

#[tokio::test]
async fn must_pass_recently_active_owner_keeps_its_seat_147() {
    let fixture = fixture().await;
    let owner = fixture.owner_with_seat("issue147-active", 60_000).await;

    assert_eq!(fixture.reap().await, 0);
    assert_eq!(fixture.seats(&owner).await, 1);
    assert_eq!(
        fixture.lifecycle("issue147-active").as_deref(),
        Some("live")
    );
}
