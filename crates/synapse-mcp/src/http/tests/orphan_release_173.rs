//! #173: dashboard SSE subscriptions must be released when their stream closes
//! (page reload) and orphans reaped, so reloads never exhaust the shared
//! subscription cap and make the MCP `subscribe` tool fail.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{num::NonZeroUsize, time::Duration};

use axum::{http::HeaderMap, response::Response};
use synapse_core::EventFilter;

use super::{EventsQuery, SUBSCRIPTION_ID_HEADER, SseState, SseSubscribeError};

fn state(cap: usize, grace: Duration) -> SseState {
    SseState::with_limits(NonZeroUsize::new(cap).unwrap(), grace)
}

fn dashboard_subscription(state: &SseState) -> Result<String, SseSubscribeError> {
    state.subscribe_dashboard(EventFilter::All, Vec::new(), false)
}

fn open_stream(state: &SseState, subscription_id: Option<&str>) -> Response {
    state.open(
        &HeaderMap::new(),
        EventsQuery {
            subscription_id: subscription_id.map(str::to_owned),
        },
    )
}

#[test]
fn must_fail_reload_cycles_do_not_exhaust_the_subscription_cap() {
    let state = state(3, Duration::ZERO);
    for reload in 0..20 {
        let id = dashboard_subscription(&state)
            .unwrap_or_else(|error| panic!("reload {reload} hit the cap: {error:?}"));
        // The page reload drops the EventSource stream.
        drop(open_stream(&state, Some(&id)));
    }
    // Cap pressure reaps every detached orphan, so the live set cycles 1,2,3,1,..:
    // after reload 19 exactly two dashboard subscriptions remain.
    assert_eq!(state.active_subscription_count(), 2);
    state
        .subscribe(
            EventFilter::All,
            Vec::new(),
            false,
            Some("session-1".to_owned()),
        )
        .expect("MCP subscribe must still succeed after many dashboard reloads");
    assert_eq!(state.active_subscription_count(), 3);
}

#[test]
fn must_pass_dropped_stream_stays_resumable_until_grace_expires() {
    let state = state(8, Duration::from_secs(3600));
    let id = dashboard_subscription(&state).unwrap();
    let live = open_stream(&state, Some(&id));
    assert!(
        state.reap_orphans(Duration::ZERO).is_empty(),
        "a subscription with an attached stream is never an orphan"
    );
    drop(live);
    assert!(
        state.reap_orphans(Duration::from_secs(3600)).is_empty(),
        "EventSource reconnect (Last-Event-ID) must still find the subscription"
    );
    assert_eq!(state.active_subscription_count(), 1);
    assert_eq!(state.reap_orphans(Duration::ZERO), vec![id]);
    assert_eq!(state.active_subscription_count(), 0);
}

#[test]
fn must_pass_session_owned_subscription_survives_stream_drop() {
    let state = state(8, Duration::ZERO);
    let id = state
        .subscribe(
            EventFilter::All,
            Vec::new(),
            false,
            Some("session-1".to_owned()),
        )
        .unwrap();
    drop(open_stream(&state, Some(&id)));
    assert!(state.reap_orphans(Duration::ZERO).is_empty());
    assert_eq!(state.active_subscription_count(), 1);
}

#[test]
fn must_fail_mcp_subscribe_without_owner_is_never_reaped() {
    let state = state(8, Duration::ZERO);
    state
        .subscribe(EventFilter::All, Vec::new(), false, None)
        .unwrap();
    assert!(state.reap_orphans(Duration::ZERO).is_empty());
    assert_eq!(state.active_subscription_count(), 1);
}

#[test]
fn must_pass_never_opened_dashboard_subscription_is_reaped() {
    let state = state(8, Duration::from_secs(3600));
    let id = dashboard_subscription(&state).unwrap();
    assert_eq!(state.reap_orphans(Duration::ZERO), vec![id]);
    assert_eq!(state.active_subscription_count(), 0);
}

#[test]
fn must_pass_bare_events_stream_subscription_is_released_after_drop() {
    let state = state(8, Duration::from_secs(3600));
    let response = open_stream(&state, None);
    assert_eq!(state.active_subscription_count(), 1);
    let id = header_id(&response);
    drop(response);
    assert_eq!(state.reap_orphans(Duration::ZERO), vec![id]);
    assert_eq!(state.active_subscription_count(), 0);
}

fn header_id(response: &Response) -> String {
    response
        .headers()
        .get(SUBSCRIPTION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("subscription id header")
        .to_owned()
}

#[test]
fn must_fail_cap_pressure_does_not_reap_a_subscription_that_just_detached() {
    let state = state(1, Duration::from_secs(3600));
    let id = dashboard_subscription(&state).unwrap();
    drop(open_stream(&state, Some(&id)));
    let second = dashboard_subscription(&state);
    assert!(
        matches!(second, Err(SseSubscribeError::CapReached { .. })),
        "a stream that closed moments ago keeps its resume window: {second:?}"
    );
    assert_eq!(state.active_subscription_count(), 1);
    // The detached subscription is still resumable under its own id.
    let resumed = open_stream(&state, Some(&id));
    assert_eq!(header_id(&resumed), id);
    assert_eq!(state.active_subscription_count(), 1);
}

#[tokio::test]
async fn must_pass_dropped_stream_subscription_is_released_by_the_grace_timer() {
    let state = state(8, Duration::from_millis(50));
    let id = dashboard_subscription(&state).unwrap();
    drop(open_stream(&state, Some(&id)));
    // Not released synchronously by the drop: only the timer may release it.
    assert_eq!(state.active_subscription_count(), 1);
    let released = tokio::time::timeout(Duration::from_secs(2), async {
        while state.active_subscription_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        released.is_ok(),
        "grace timer never released the detached subscription"
    );
}
