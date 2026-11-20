//! #17 (raid 2026-10-15): connection admission must COUNT what it does, so
//! `/health` can report active connections, the cap, pre-parse rejections and
//! header/idle-deadline closures. Before the fix deadline closures were only
//! logged (`MCP_HTTP_HEADER_READ_TIMEOUT` / `MCP_HTTP_CONNECTION_IDLE_DEADLINE`).
//! Drives the real `spawn_server` accept loop.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use super::*;
use crate::http::connection_admission::HttpConnectionAdmission;

const DEADLINE: Duration = Duration::from_secs(2);
const GENEROUS: Duration = Duration::from_secs(6);

struct TestServer {
    addr: SocketAddr,
    admission: HttpConnectionAdmission,
    cancel: CancellationToken,
    task: JoinHandle<io::Result<()>>,
}

impl TestServer {
    /// normal=`max`, reserve=1, triage=1; the test keeps the admission for readback.
    async fn start(max: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route("/ping", get(|| async { "pong" }));
        let cancel = CancellationToken::new();
        let admission = HttpConnectionAdmission::with_limits(max, DEADLINE, 1, DEADLINE, 1);
        let task = spawn_server(
            listener,
            app,
            cancel.clone(),
            ActiveHttpSockets::default(),
            admission.clone(),
        );
        assert_eq!(admission.active_connections(), 0, "precondition");
        assert_eq!(admission.header_read_timeouts_total(), 0, "precondition");
        assert_eq!(admission.rejected_total(), 0, "precondition");
        Self {
            addr,
            admission,
            cancel,
            task,
        }
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

/// Polls `read` until it equals `want`, failing after GENEROUS.
async fn poll_eq<T: PartialEq + std::fmt::Debug>(what: &str, want: T, read: impl Fn() -> T) {
    let reached = tokio::time::timeout(GENEROUS, async {
        while read() != want {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "{what}: expected {want:?}, saw {:?}",
        read()
    );
}

/// True when the server closes (EOF or reset) within `limit`.
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

async fn assert_deadline_close_is_counted(head: &[u8]) {
    let server = TestServer::start(4).await;
    let mut stream = server.connect_and_send(head).await;
    poll_eq("active after accept", 1, || {
        server.admission.active_connections()
    })
    .await;
    assert!(
        closed_within(&mut stream, GENEROUS).await,
        "precondition: the deadline closes the connection"
    );
    poll_eq("header timeouts", 1, || {
        server.admission.header_read_timeouts_total()
    })
    .await;
    poll_eq("slot released", 0, || server.admission.active_connections()).await;
    assert_eq!(
        server.admission.rejected_total(),
        0,
        "a timeout is not a rejection"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_silent_socket_closed_at_the_deadline_is_counted_as_header_timeout() {
    assert_deadline_close_is_counted(b"").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_partial_http1_head_closed_at_the_deadline_is_counted_as_header_timeout() {
    assert_deadline_close_is_counted(b"GET /ping HTTP/1.1\r\nHost: x\r\n").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_pass_completed_request_is_neither_a_timeout_nor_a_rejection() {
    let server = TestServer::start(4).await;
    let mut stream = server
        .connect_and_send(b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await;
    let mut response = Vec::new();
    tokio::time::timeout(GENEROUS, stream.read_to_end(&mut response))
        .await
        .expect("response within deadline")
        .expect("read response");
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
    poll_eq("slot released", 0, || server.admission.active_connections()).await;
    assert_eq!(server.admission.header_read_timeouts_total(), 0);
    assert_eq!(server.admission.rejected_total(), 0);
    assert_eq!(server.admission.max_connections(), 4);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_over_cap_connection_is_counted_as_rejection_not_timeout() {
    let server = TestServer::start(1).await;
    let _normal = server.connect_and_send(b"").await;
    poll_eq("normal slot taken", 1, || {
        server.admission.active_connections()
    })
    .await;
    let _triaged = server.connect_and_send(b"GET /heal").await;
    let _overflow = server.connect_and_send(b"POST /mcp HTTP/1.1\r\n").await;
    // Well inside DEADLINE: the overflow is refused at once, nothing timed out yet.
    poll_eq("rejections", 1_u64, || server.admission.rejected_total()).await;
    assert_eq!(server.admission.header_read_timeouts_total(), 0);
    assert_eq!(
        server.admission.active_connections(),
        1,
        "reserve untouched"
    );
    server.stop().await;
}
