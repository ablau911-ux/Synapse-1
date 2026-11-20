use super::*;
use crate::m3::subscribe::{SubscribeParams, subscribe_to_events};

fn params(capacity: u32) -> SubscribeParams {
    SubscribeParams {
        kinds: vec!["g7b".into()],
        filter: None,
        snapshot_first: false,
        buffer_size: capacity,
    }
}

fn event(seq: u64) -> synapse_core::Event {
    synapse_core::Event {
        seq,
        at: chrono::Utc::now(),
        source: synapse_core::EventSource::System,
        kind: "g7b".into(),
        data: serde_json::json!({"seq": seq}),
        correlations: Vec::new(),
    }
}

#[test]
fn must_pass_subscribe_entry_honors_small_queue_and_replay_capacity_26() {
    for size in [1, 2] {
        let state = SseState::with_max_subscriptions(NonZeroUsize::new(4).expect("cap"));
        assert_eq!(state.active_subscription_count(), 0);
        let response = subscribe_to_events(&state, &params(size), Some("owner".into()))
            .expect("bounded capacity");
        let subscription = state
            .inner
            .subscriptions
            .lock()
            .expect("map")
            .get(&response.subscription_id)
            .expect("physical subscription")
            .clone();
        assert_eq!(subscription.handle.len(), 0);
        for seq in 1..=u64::from(size) + 1 {
            state.event_bus().publish(event(seq));
        }
        assert_eq!(subscription.handle.len(), size as usize);
        let events = subscription.handle.drain();
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            (2..=u64::from(size) + 1).collect::<Vec<_>>()
        );
        assert_eq!(subscription.handle.take_dropped_since_read(), 1);
        // Feed replay one event at a time, proving its independent capacity too.
        for seq in 1..=u64::from(size) + 1 {
            subscription.push_events(vec![event(seq)]);
        }
        let (replay, gap) = subscription.events_after(Some(0));
        assert!(gap);
        assert_eq!(
            replay.iter().map(|e| e.event.seq).collect::<Vec<_>>(),
            (2..=u64::from(size) + 1).collect::<Vec<_>>()
        );
    }
}

#[test]
fn must_fail_invalid_capacity_never_allocates_registry_or_bus_rows_26() {
    let state = SseState::with_max_subscriptions(NonZeroUsize::new(4).expect("cap"));
    let live =
        subscribe_to_events(&state, &params(4096), Some("owner".into())).expect("upper boundary");
    let before = state.subscription_ids_for_session("owner").expect("owners");
    assert_eq!(before, vec![live.subscription_id]);
    for size in [0, 4097, u32::MAX] {
        let error = subscribe_to_events(&state, &params(size), Some("owner".into()))
            .expect_err("invalid capacity");
        assert_eq!(
            crate::server::g7b_support::code(&error),
            "TOOL_PARAMS_INVALID"
        );
        assert_eq!(state.active_subscription_count(), 1);
        assert_eq!(state.event_bus().subscriber_count(), 1);
        assert_eq!(
            state.subscription_ids_for_session("owner").expect("after"),
            before
        );
    }
}
