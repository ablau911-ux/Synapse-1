//! Per-connection idle deadline (#17).
//!
//! hyper's HTTP/1 `header_read_timeout` only starts once the auto builder has detected the
//! protocol, and HTTP/2 has no header deadline at all. A silent socket, a partial HTTP/2 preface,
//! an HTTP/2 connection that never completes a request, or one that stalls mid-HEADERS therefore
//! kept its admission slot forever. This tracks request exchanges per connection: a connection
//! with no exchange in flight for the whole deadline is closed. An exchange lasts until its
//! response body is finished, so SSE streams are never cut. Upgraded connections (WebSocket)
//! leave the policy.

use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    http::{Response, StatusCode},
};
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use tokio::{sync::Notify, time::Instant};
use tower::Service;

pub(super) struct ConnectionActivity {
    deadline: Duration,
    active: AtomicUsize,
    upgraded: AtomicBool,
    idle_since: Mutex<Instant>,
    changed: Notify,
}

impl ConnectionActivity {
    pub(super) fn new(deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            deadline,
            active: AtomicUsize::new(0),
            upgraded: AtomicBool::new(false),
            idle_since: Mutex::new(Instant::now()),
            changed: Notify::new(),
        })
    }

    fn begin(self: &Arc<Self>) -> ActivityGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_one();
        ActivityGuard {
            activity: Arc::clone(self),
        }
    }

    fn mark_upgraded(&self) {
        self.upgraded.store(true, Ordering::Release);
        self.changed.notify_one();
    }

    fn idle_since(&self) -> Instant {
        *self
            .idle_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn idle_for_full_deadline(&self) -> bool {
        !self.upgraded.load(Ordering::Acquire)
            && self.active.load(Ordering::Acquire) == 0
            && Instant::now() >= self.idle_since() + self.deadline
    }

    /// Resolves once no exchange has been in flight for the whole deadline. Never resolves for
    /// an upgraded connection.
    pub(super) async fn expired(&self) {
        loop {
            if self.upgraded.load(Ordering::Acquire) {
                std::future::pending::<()>().await;
            }
            if self.active.load(Ordering::Acquire) > 0 {
                self.changed.notified().await;
                continue;
            }
            let due = self.idle_since() + self.deadline;
            tokio::select! {
                () = tokio::time::sleep_until(due) => {
                    if self.idle_for_full_deadline() {
                        return;
                    }
                }
                () = self.changed.notified() => {}
            }
        }
    }
}

/// Marks one request/response exchange as in flight until it is dropped.
struct ActivityGuard {
    activity: Arc<ConnectionActivity>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        // Stamp the idle start before the counter can read zero, so the watchdog never pairs a
        // zero counter with a stale idle start.
        *self
            .activity
            .idle_since
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.activity.active.fetch_sub(1, Ordering::AcqRel);
        self.activity.changed.notify_one();
    }
}

/// Response body that keeps its exchange in flight until hyper drops it.
struct ActivityBody {
    inner: Body,
    _guard: ActivityGuard,
}

impl HttpBody for ActivityBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// The router, instrumented with a [`ConnectionActivity`].
#[derive(Clone)]
pub(super) struct ActivityService {
    app: Router,
    activity: Arc<ConnectionActivity>,
}

impl ActivityService {
    pub(super) const fn new(app: Router, activity: Arc<ConnectionActivity>) -> Self {
        Self { app, activity }
    }
}

impl Service<hyper::Request<hyper::body::Incoming>> for ActivityService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: hyper::Request<hyper::body::Incoming>) -> Self::Future {
        let guard = self.activity.begin();
        // HTTP/2 extended CONNECT (WebSocket): the exchange outlives its response body.
        if request.extensions().get::<hyper::ext::Protocol>().is_some() {
            self.activity.mark_upgraded();
        }
        let activity = Arc::clone(&self.activity);
        let mut app = self.app.clone();
        Box::pin(async move {
            let Ok(response) = app.call(request.map(Body::new)).await;
            if response.status() == StatusCode::SWITCHING_PROTOCOLS {
                activity.mark_upgraded();
            }
            Ok(response.map(|inner| {
                Body::new(ActivityBody {
                    inner,
                    _guard: guard,
                })
            }))
        })
    }
}

#[cfg(test)]
#[path = "tests/connection_activity_17.rs"]
mod connection_activity_17;
