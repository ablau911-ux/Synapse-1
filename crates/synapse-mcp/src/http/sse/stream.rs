use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    http::HeaderValue,
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, Sse},
    },
};
use futures_util::stream;
use synapse_core::Event;

use super::{
    SUBSCRIPTION_ID_HEADER, SseState, replay,
    ring::{BufferedEvent, Subscription},
};

const SSE_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub(super) enum SseFrame {
    SubscriptionStarted {
        subscription_id: String,
        lossy: bool,
        buffer_capacity: usize,
    },
    Event {
        subscription_id: String,
        stream_seq: u64,
        event: Event,
        lossy: bool,
    },
}

/// #173: held by the live stream; dropping the stream (client disconnect,
/// page reload, server shutdown) detaches it so orphaned subscriptions are
/// released instead of leaking toward the subscription cap.
#[derive(Debug)]
pub(super) struct StreamGuard {
    subscription: Arc<Subscription>,
    state: SseState,
}

impl StreamGuard {
    pub(super) fn attach(state: &SseState, subscription: &Arc<Subscription>) -> Self {
        subscription.attachment().attach();
        Self {
            subscription: Arc::clone(subscription),
            state: state.clone(),
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if self.subscription.attachment().detach() {
            self.state.schedule_orphan_reap();
        }
    }
}

#[derive(Debug)]
struct LiveStreamState {
    _guard: StreamGuard,
    subscription: Arc<Subscription>,
    pending: VecDeque<SseFrame>,
    last_sent_seq: Option<u64>,
}

pub(super) fn sse_response(
    subscription: Arc<Subscription>,
    frames: VecDeque<SseFrame>,
    last_sent_seq: Option<u64>,
    guard: StreamGuard,
) -> Response {
    let subscription_id = subscription.id().to_owned();
    let stream = live_stream(subscription, frames, last_sent_seq, guard);
    let mut response = Sse::new(stream).into_response();
    if let Ok(header_value) = HeaderValue::from_str(&subscription_id) {
        response
            .headers_mut()
            .insert(SUBSCRIPTION_ID_HEADER, header_value);
    }
    response
}

fn live_stream(
    subscription: Arc<Subscription>,
    pending: VecDeque<SseFrame>,
    last_sent_seq: Option<u64>,
    guard: StreamGuard,
) -> impl futures_util::Stream<Item = Result<SseEvent, Infallible>> + Send + 'static {
    stream::unfold(
        LiveStreamState {
            _guard: guard,
            subscription,
            pending,
            last_sent_seq,
        },
        |mut state| async move {
            loop {
                if let Some(frame) = state.pending.pop_front() {
                    if let Some(seq) = frame.seq() {
                        state.last_sent_seq = Some(seq);
                    }
                    return Some((Ok(frame.into_event()), state));
                }
                state.pending.extend(replay::frames_after(
                    &state.subscription,
                    state.last_sent_seq,
                ));
                if state.pending.is_empty() {
                    tokio::time::sleep(SSE_POLL_INTERVAL).await;
                }
            }
        },
    )
}

impl SseFrame {
    pub(super) fn subscription_started(
        subscription_id: &str,
        lossy: bool,
        buffer_capacity: usize,
    ) -> Self {
        Self::SubscriptionStarted {
            subscription_id: subscription_id.to_owned(),
            lossy,
            buffer_capacity,
        }
    }

    pub(super) fn event(subscription_id: &str, buffered: BufferedEvent, lossy: bool) -> Self {
        Self::Event {
            subscription_id: subscription_id.to_owned(),
            stream_seq: buffered.stream_seq,
            event: buffered.event,
            lossy,
        }
    }

    pub(super) const fn seq(&self) -> Option<u64> {
        match self {
            Self::SubscriptionStarted { .. } => None,
            Self::Event { stream_seq, .. } => Some(*stream_seq),
        }
    }

    fn into_event(self) -> SseEvent {
        match self {
            Self::SubscriptionStarted {
                subscription_id,
                lossy,
                buffer_capacity,
            } => SseEvent::default().event("subscription_started").data(
                subscription_started_data(&subscription_id, lossy, buffer_capacity).to_string(),
            ),
            Self::Event {
                subscription_id,
                stream_seq,
                event,
                lossy,
            } => {
                let id = stream_seq.to_string();
                SseEvent::default()
                    .id(id)
                    .event("synapse/event")
                    .data(event_data(&subscription_id, stream_seq, &event, lossy).to_string())
            }
        }
    }
}

fn subscription_started_data(
    subscription_id: &str,
    lossy: bool,
    buffer_capacity: usize,
) -> serde_json::Value {
    serde_json::json!({
        "subscription_id": subscription_id,
        "lossy": lossy,
        "buffer_capacity": buffer_capacity,
    })
}

pub(super) fn event_data(
    subscription_id: &str,
    stream_seq: u64,
    event: &Event,
    lossy: bool,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "synapse/event",
        "params": {
            "subscription_id": subscription_id,
            "stream_seq": stream_seq,
            "lossy": lossy,
            "event": event,
        }
    })
}
