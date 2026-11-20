//! Regression tests for #17 (re-review 2026-10-14): sockets that never complete protocol
//! detection or an HTTP/2 preface/settings/headers exchange must be closed at the header
//! deadline instead of holding an admission slot forever. The HTTP/1 header timer only starts
//! after hyper-util's auto builder has detected the protocol, so these paths had no deadline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use super::*;
use crate::http::connection_admission::HttpConnectionAdmission;

const DEADLINE: Duration = Duration::from_secs(1);
const GENEROUS: Duration = Duration::from_secs(4);
/// Lower bound: well inside DEADLINE, so an immediate close is caught.
const EARLY: Duration = Duration::from_millis(500);
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
const H2_EMPTY_SETTINGS: &[u8] = &[0, 0, 0, 4, 0, 0, 0, 0, 0];
/// HEADERS frame (stream 1, END_HEADERS|END_STREAM) that declares 64 payload bytes.
const H2_HEADERS_FRAME_START: &[u8] = &[0, 0, 64, 1, 5, 0, 0, 0, 1, 0x82, 0x84, 0x86];

struct TestServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<io::Result<()>>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route("/ping", get(|| async { "pong" }));
        let cancel = CancellationToken::new();
        let admission = HttpConnectionAdmission::with_limits(8, DEADLINE, 2, DEADLINE, 8);
        let task = spawn_server(
            listener,
            app,
            cancel.clone(),
            ActiveHttpSockets::default(),
            admission,
        );
        Self { addr, cancel, task }
    }

    async fn connect_and_send(&self, bytes: &[u8]) -> TcpStream {
        let mut stream = TcpStream::connect(self.addr).await.expect("connect");
        if !bytes.is_empty() {
            stream.write_all(bytes).await.expect("write");
            stream.flush().await.expect("flush");
        }
        stream
    }

    async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.task).await;
    }
}

/// True when the server closes (EOF or reset) within `limit`; server bytes (h2 SETTINGS) are drained.
async fn closed_within(stream: &mut TcpStream, limit: Duration) -> bool {
    let mut buffer = [0_u8; 512];
    tokio::time::timeout(limit, async {
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await
    .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_silent_socket_is_closed_at_the_deadline() {
    let server = TestServer::start().await;
    let mut silent = server.connect_and_send(b"").await;
    assert!(
        !closed_within(&mut silent, EARLY).await,
        "a silent socket was closed before the deadline (accept-drop regression)"
    );
    assert!(
        closed_within(&mut silent, GENEROUS).await,
        "a socket that sends nothing kept its admission slot past the deadline"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_partial_h2_preface_is_closed_at_the_deadline() {
    let server = TestServer::start().await;
    let mut partial = server.connect_and_send(&H2_PREFACE[..20]).await;
    assert!(
        !closed_within(&mut partial, EARLY).await,
        "a partial HTTP/2 preface was closed before the deadline"
    );
    assert!(
        closed_within(&mut partial, GENEROUS).await,
        "a partial HTTP/2 preface kept its admission slot past the deadline"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_h2_connection_that_never_sends_a_request_is_closed_at_the_deadline() {
    let server = TestServer::start().await;
    let mut bytes = H2_PREFACE.to_vec();
    bytes.extend_from_slice(H2_EMPTY_SETTINGS);
    let mut idle = server.connect_and_send(&bytes).await;
    assert!(
        !closed_within(&mut idle, EARLY).await,
        "an idle HTTP/2 connection was closed before the deadline"
    );
    assert!(
        closed_within(&mut idle, GENEROUS).await,
        "an HTTP/2 connection with no request kept its admission slot past the deadline"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_partial_h2_headers_are_closed_at_the_deadline() {
    let server = TestServer::start().await;
    let mut bytes = H2_PREFACE.to_vec();
    bytes.extend_from_slice(H2_EMPTY_SETTINGS);
    bytes.extend_from_slice(H2_HEADERS_FRAME_START);
    let mut partial = server.connect_and_send(&bytes).await;
    assert!(
        !closed_within(&mut partial, EARLY).await,
        "partial HTTP/2 headers were closed before the deadline"
    );
    assert!(
        closed_within(&mut partial, GENEROUS).await,
        "an HTTP/2 request whose headers never complete kept its admission slot"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_a_normal_http1_request_still_completes() {
    let server = TestServer::start().await;
    let mut stream = server
        .connect_and_send(b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await;
    let mut response = Vec::new();
    tokio::time::timeout(GENEROUS, stream.read_to_end(&mut response))
        .await
        .expect("response within deadline")
        .expect("read response");
    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 200"),
        "unexpected response: {text}"
    );
    assert!(text.ends_with("pong"), "unexpected body: {text}");
    server.stop().await;
}

/// Reads h2 frames until a HEADERS frame on stream 1; returns its payload.
async fn read_stream1_headers(stream: &mut TcpStream) -> Vec<u8> {
    loop {
        let mut head = [0_u8; 9];
        stream.read_exact(&mut head).await.expect("frame header");
        let len = (usize::from(head[0]) << 16) | (usize::from(head[1]) << 8) | usize::from(head[2]);
        let kind = head[3];
        let stream_id = u32::from_be_bytes([head[5] & 0x7f, head[6], head[7], head[8]]);
        let mut payload = vec![0_u8; len];
        stream
            .read_exact(&mut payload)
            .await
            .expect("frame payload");
        if kind == 1 && stream_id == 1 {
            return payload;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_complete_h2_request_still_completes() {
    let server = TestServer::start().await;
    // HPACK: :method GET, :scheme http, :path "/ping", :authority "127.0.0.1".
    let mut block = vec![0x82, 0x86, 0x04, 0x05];
    block.extend_from_slice(b"/ping");
    block.extend_from_slice(&[0x01, 0x09]);
    block.extend_from_slice(b"127.0.0.1");
    let len = u8::try_from(block.len()).expect("small block");
    let mut bytes = H2_PREFACE.to_vec();
    bytes.extend_from_slice(H2_EMPTY_SETTINGS);
    // HEADERS, END_STREAM|END_HEADERS, stream 1.
    bytes.extend_from_slice(&[0, 0, len, 1, 5, 0, 0, 0, 1]);
    bytes.extend_from_slice(&block);
    let mut stream = server.connect_and_send(&bytes).await;
    let headers = tokio::time::timeout(GENEROUS, read_stream1_headers(&mut stream))
        .await
        .expect("h2 response HEADERS within deadline");
    // HPACK static index 8 = ":status: 200".
    assert_eq!(
        headers.first(),
        Some(&0x88),
        "h2 response not 200: {headers:?}"
    );
    server.stop().await;
}
