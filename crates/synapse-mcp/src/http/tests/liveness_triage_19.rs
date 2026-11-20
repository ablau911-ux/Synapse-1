#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{HeadClass, classify_head, classify_ready_head};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
};

#[test]
fn must_pass_complete_health_head_is_liveness() {
    let head = b"GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer t\r\n\r\n";
    assert_eq!(classify_head(head), HeadClass::Liveness);
}

#[test]
fn must_pass_health_with_query_is_liveness() {
    assert_eq!(
        classify_head(b"GET /health?verbose=1 HTTP/1.1\r\nHost: x\r\n\r\n"),
        HeadClass::Liveness
    );
}

#[test]
fn must_fail_partial_health_head_stays_incomplete() {
    assert_eq!(
        classify_head(b"GET /health HTTP/1.1\r\nHost: x\r\n"),
        HeadClass::Incomplete
    );
    assert_eq!(classify_head(b"GET /hea"), HeadClass::Incomplete);
    assert_eq!(classify_head(b""), HeadClass::Incomplete);
}

#[test]
fn must_fail_other_paths_and_methods_are_rejected_early() {
    assert_eq!(classify_head(b"GET /mcp"), HeadClass::Other);
    assert_eq!(
        classify_head(b"POST /health HTTP/1.1\r\n\r\n"),
        HeadClass::Other
    );
    assert_eq!(
        classify_head(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n"),
        HeadClass::Other
    );
    assert_eq!(
        classify_head(b"GET /health/../mcp HTTP/1.1\r\nHost: x\r\n\r\n"),
        HeadClass::Other
    );
    assert_eq!(classify_head(b"\x16\x03\x01garbage"), HeadClass::Other);
}

#[tokio::test]
async fn must_pass_ready_head_classification_never_waits_and_sees_queued_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    assert_eq!(classify_ready_head(&server), HeadClass::Incomplete);

    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    server.readable().await.unwrap();
    assert_eq!(classify_ready_head(&server), HeadClass::Incomplete);

    client.write_all(b"\r\n").await.unwrap();
    // Poll (bounded) until the queued terminator is visible; until then the
    // head may only ever read as Incomplete, never Other.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match classify_ready_head(&server) {
            HeadClass::Liveness => break,
            HeadClass::Incomplete => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "complete /health head never classified as Liveness"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            HeadClass::Other => panic!("complete /health head classified as Other"),
        }
    }
}

#[tokio::test]
async fn must_fail_ready_head_classification_rejects_other_paths_and_closed_peers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    client
        .write_all(b"GET /mcp HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    server.readable().await.unwrap();
    assert_eq!(classify_ready_head(&server), HeadClass::Other);

    let closed_client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (closed_server, _) = listener.accept().await.unwrap();
    drop(closed_client);
    closed_server.readable().await.unwrap();
    assert_eq!(classify_ready_head(&closed_server), HeadClass::Other);
}
