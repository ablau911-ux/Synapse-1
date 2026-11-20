use super::SynapseService;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio_util::sync::CancellationToken;

type Key = (usize, String);
type Pauses = BTreeMap<Key, Weak<PauseState>>;

#[derive(Default)]
struct PauseState {
    entered: CancellationToken,
    resumed: CancellationToken,
}

fn pauses() -> &'static Mutex<Pauses> {
    static PAUSES: OnceLock<Mutex<Pauses>> = OnceLock::new();
    PAUSES.get_or_init(Default::default)
}

fn key(service: &SynapseService, owner: &str) -> Key {
    (
        Arc::as_ptr(&service.target_claims) as usize,
        owner.to_owned(),
    )
}

pub(super) struct AdoptionHandoffPause {
    key: Key,
    state: Arc<PauseState>,
}

impl AdoptionHandoffPause {
    pub(super) async fn wait_until_entered(&self) {
        self.state.entered.cancelled().await;
    }

    pub(super) fn resume(&self) {
        self.state.resumed.cancel();
    }
}

impl Drop for AdoptionHandoffPause {
    fn drop(&mut self) {
        self.resume();
        let mut pauses = pauses()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pauses
            .get(&self.key)
            .is_some_and(|state| state.ptr_eq(&Arc::downgrade(&self.state)))
        {
            pauses.remove(&self.key);
        }
    }
}

pub(super) fn adoption_handoff_pause_for_test(
    service: &SynapseService,
    owner: &str,
) -> AdoptionHandoffPause {
    let key = key(service, owner);
    let state = Arc::new(PauseState::default());
    pauses()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key.clone(), Arc::downgrade(&state));
    AdoptionHandoffPause { key, state }
}

pub(super) async fn pause_adoption_handoff_for_test(service: &SynapseService, owner: &str) {
    let state = pauses()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key(service, owner))
        .and_then(Weak::upgrade);
    if let Some(state) = state {
        state.entered.cancel();
        state.resumed.cancelled().await;
    }
}
