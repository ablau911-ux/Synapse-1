//! #147 rework: the seat-owner idle reclaim must treat a session as streaming
//! only while an SSE stream is attached NOW. A registered subscription row
//! survives a dropped stream (client crash), so it must not keep a browser
//! seat alive.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{num::NonZeroUsize, time::Duration};

use axum::{http::HeaderMap, response::Response};
use synapse_core::EventFilter;

use super::{EventsQuery, SseState};

fn state() -> SseState {
    SseState::with_limits(NonZeroUsize::new(8).unwrap(), Duration::from_secs(3600))
}

fn subscribe_as(state: &SseState, session: &str) -> String {
    state
        .subscribe(
            EventFilter::All,
            Vec::new(),
            false,
            Some(session.to_owned()),
        )
        .unwrap()
}

fn open_stream(state: &SseState, subscription_id: &str) -> Response {
    state.open(
        &HeaderMap::new(),
        EventsQuery {
            subscription_id: Some(subscription_id.to_owned()),
        },
    )
}

fn attached(state: &SseState) -> Vec<String> {
    state.attached_owner_session_ids().unwrap()
}

#[test]
fn must_fail_subscribed_owner_without_stream_is_not_attached() {
    let state = state();
    subscribe_as(&state, "crashed");
    assert!(attached(&state).is_empty());
}

#[test]
fn must_fail_subscribed_then_disconnected_owner_is_not_attached() {
    let state = state();
    let id = subscribe_as(&state, "crashed");
    let live = open_stream(&state, &id);
    assert_eq!(attached(&state), vec!["crashed".to_owned()]);
    drop(live);
    assert!(
        attached(&state).is_empty(),
        "the subscription row survives the dropped stream but is not activity"
    );
    assert_eq!(state.active_subscription_count(), 1);
}

#[test]
fn must_pass_owner_with_open_stream_is_attached_and_others_are_not() {
    let state = state();
    let live_id = subscribe_as(&state, "live");
    subscribe_as(&state, "silent");
    let _live = open_stream(&state, &live_id);
    assert_eq!(attached(&state), vec!["live".to_owned()]);
}

#[test]
fn must_pass_reconnected_stream_makes_the_owner_attached_again() {
    let state = state();
    let id = subscribe_as(&state, "flaky");
    drop(open_stream(&state, &id));
    assert!(attached(&state).is_empty());
    let _again = open_stream(&state, &id);
    assert_eq!(attached(&state), vec!["flaky".to_owned()]);
}

#[test]
fn must_pass_one_attached_subscription_is_enough_for_an_owner_with_several() {
    let state = state();
    let first = subscribe_as(&state, "multi");
    let second = subscribe_as(&state, "multi");
    let _live = open_stream(&state, &second);
    drop(open_stream(&state, &first));
    assert_eq!(attached(&state), vec!["multi".to_owned()]);
}
