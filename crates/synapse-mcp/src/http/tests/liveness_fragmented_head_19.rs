//! #19 (raid 2026-10-15): a legitimate health request whose head is NOT yet
//! complete when the listener accepts the socket (normal connect-then-write
//! timing, or a fragmented head) must still reach the free liveness reserve
//! while the normal and triage pools are saturated. Before the fix
//! `admit_over_budget` classified it `Incomplete` and `defer_to_liveness_triage`
//! rejected it at `try_triage`, although the reserve slot was free.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

use super::*;
use crate::http::connection_admission::HttpConnectionAdmission;

const PARTIAL_HEAD: &[u8] = b"GET /heal";
const HEALTH_FIRST: &[u8] = b"GET /health HTTP/1.1\r\nHost: x\r\n";
const HEALTH_REST: &[u8] = b"Authorization: Bearer t\r\n\r\n";
const RESERVE_DEADLINE: Duration = Duration::from_secs(3);
/// Max wait for the server's first look at the fragment before the rest is sent.
const FIRST_LOOK: Duration = Duration::from_millis(300);

async fn client(addr: SocketAddr, bytes: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(bytes).await.expect("write");
    stream.flush().await.expect("flush");
    stream
}

struct Saturated {
    listener: TrackedTcpListener,
    admission: HttpConnectionAdmission,
    addr: SocketAddr,
    _normal_client: TcpStream,
    _normal_admitted: (TrackedTcpStream, SocketAddr),
    _triage_client: TcpStream,
}

/// normal=1 (taken by a partial head), reserve=1 (free), triage=1 (a second
/// partial head is queued and takes it on the next accept pass).
async fn saturated() -> Saturated {
    let inner = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = inner.local_addr().expect("addr");
    let admission =
        HttpConnectionAdmission::with_limits(1, Duration::from_secs(10), 1, RESERVE_DEADLINE, 1);
    let mut listener = TrackedTcpListener::new(
        inner,
        admission.clone(),
        #[cfg(windows)]
        ActiveHttpSockets::default(),
    );
    let normal_client = client(addr, PARTIAL_HEAD).await;
    let normal_admitted = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("normal slot admits the first connection");
    assert_eq!(
        admission.active_connections(),
        1,
        "precondition: normal full, reserve free"
    );
    assert_eq!(admission.rejected_total(), 0, "precondition");
    let triage_client = client(addr, PARTIAL_HEAD).await;
    Saturated {
        listener,
        admission,
        addr,
        _normal_client: normal_client,
        _normal_admitted: normal_admitted,
        _triage_client: triage_client,
    }
}

/// Polls until the server rejected something or `FIRST_LOOK` passed.
async fn await_first_look(admission: &HttpConnectionAdmission) {
    let _ = tokio::time::timeout(FIRST_LOOK, async {
        while admission.rejected_total() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_fragmented_health_head_reaches_the_free_reserve_under_saturation() {
    // Bind first: a partial move out of a named value keeps the other fields
    // (the saturating sockets) alive until the end of the test.
    let sat = saturated().await;
    let Saturated {
        mut listener,
        admission,
        addr,
        ..
    } = sat;
    let mut health = client(addr, HEALTH_FIRST).await;
    let health_addr = health.local_addr().expect("health local addr");
    let accepting = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(2500), listener.accept())
            .await
            .map(|(stream, peer)| {
                drop(stream);
                peer
            })
    });
    await_first_look(&admission).await;
    // The write may fail on the unfixed code (socket already reset); the
    // admission assertion below is the verdict.
    let _ = health.write_all(HEALTH_REST).await;
    let admitted = accepting.await.expect("accept task");
    assert_eq!(
        admitted.ok(),
        Some(health_addr),
        "a fragmented authenticated /health must be admitted to the free liveness reserve; \
         rejections so far: {}",
        admission.rejected_total()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_fragmented_non_liveness_head_never_takes_the_reserve() {
    // Bind first: a partial move out of a named value keeps the other fields
    // (the saturating sockets) alive until the end of the test.
    let sat = saturated().await;
    let Saturated {
        mut listener,
        admission,
        addr,
        ..
    } = sat;
    let mut other = client(addr, b"GET /health").await;
    let accepting = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(1500), listener.accept())
            .await
            .map(|(stream, peer)| {
                drop(stream);
                peer
            })
    });
    await_first_look(&admission).await;
    let _ = other.write_all(b"z HTTP/1.1\r\nHost: x\r\n\r\n").await;
    let admitted = accepting.await.expect("accept task");
    assert!(
        admitted.is_err(),
        "GET /healthz is not liveness and must never be admitted: {admitted:?}"
    );
    assert_eq!(admission.active_connections(), 1, "reserve must stay free");
    assert_eq!(
        admission.rejected_total(),
        1,
        "the /healthz socket rejected exactly once"
    );
}
