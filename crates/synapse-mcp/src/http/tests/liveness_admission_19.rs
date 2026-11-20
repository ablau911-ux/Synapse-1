//! Regression tests for #19 (re-review 2026-10-14): a complete authenticated-liveness head must
//! be admitted even when the normal pool AND the triage pool are saturated by partial requests.
//! Before the fix the over-budget connection was rejected at `try_triage` before any byte of it
//! was inspected, so `/health` was refused although the liveness reserve was free.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

use super::*;
use crate::http::connection_admission::HttpConnectionAdmission;

const PARTIAL_HEAD: &[u8] = b"GET /heal";
const HEALTH_HEAD: &[u8] = b"GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t\r\n\r\n";

async fn client(addr: SocketAddr, bytes: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(bytes).await.expect("write");
    stream.flush().await.expect("flush");
    stream
}

/// Listener with normal=1, reserve=1, triage=1.
async fn tiny_listener() -> (TrackedTcpListener, SocketAddr) {
    let inner = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = inner.local_addr().expect("addr");
    let admission = HttpConnectionAdmission::with_limits(
        1,
        Duration::from_secs(10),
        1,
        Duration::from_secs(1),
        1,
    );
    let listener = TrackedTcpListener::new(
        inner,
        admission,
        #[cfg(windows)]
        ActiveHttpSockets::default(),
    );
    (listener, addr)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_complete_health_head_is_admitted_when_normal_and_triage_pools_are_full() {
    let (mut listener, addr) = tiny_listener().await;
    let _normal = client(addr, PARTIAL_HEAD).await;
    let _triaged = client(addr, PARTIAL_HEAD).await;
    let _rejected = client(addr, PARTIAL_HEAD).await;
    let health = client(addr, HEALTH_HEAD).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let first = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("normal slot admits the first connection");
    let second = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("complete health head must be admitted past saturated triage");

    assert_eq!(
        second.1,
        health.local_addr().expect("health local addr"),
        "the admitted connection must be the complete liveness request"
    );
    drop(first);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_incomplete_health_head_is_not_admitted_past_saturated_pools() {
    let (mut listener, addr) = tiny_listener().await;
    let _normal = client(addr, PARTIAL_HEAD).await;
    let _triaged = client(addr, PARTIAL_HEAD).await;
    let _flood = client(addr, b"GET /health HTTP/1.1\r\nHost: x\r\n").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let _first = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("normal slot admits the first connection");
    let second = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;

    assert!(
        second.is_err(),
        "a partial head must never reach the liveness reserve"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_health_head_is_peeked_before_rejection_when_idle_sockets_fill_both_pools() {
    let (mut listener, addr) = tiny_listener().await;
    let _idle_normal = client(addr, b"").await;
    let _idle_triaged = client(addr, b"").await;
    let _idle_overflow = client(addr, b"").await;
    let health = client(addr, HEALTH_HEAD).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let _first = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("normal slot admits the first connection");
    let second = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("health head must be peeked and admitted although both pools hold idle sockets");

    assert_eq!(second.1, health.local_addr().expect("health local addr"));
}
