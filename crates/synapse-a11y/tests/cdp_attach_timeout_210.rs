//! #210: attaching to a CDP endpoint that accepts TCP but never answers HTTP
//! (hung Chrome, suspended WebView2 host, reused port) must time out instead of
//! hanging the caller forever.
#![cfg(windows)]
#![allow(clippy::expect_used)]

use std::{
    io::{Read, Write},
    net::TcpListener,
    time::{Duration, Instant},
};

use synapse_a11y::{A11yError, attach_chromiumoxide_with_timeout};

/// Minimal responsive CDP endpoint: answers one `/json/version` request with a
/// websocket URL on the same port, then completes the websocket handshake and
/// holds the socket open until the client goes away.
fn spawn_responsive_cdp_endpoint() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake CDP endpoint");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        let (mut http, _) = listener.accept().expect("accept /json/version");
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = http.read(&mut chunk).expect("read /json/version request");
            if read == 0 {
                return;
            }
            request.extend_from_slice(&chunk[..read]);
        }
        let body = format!(
            r#"{{"Browser":"FakeChrome/210","Protocol-Version":"1.3","User-Agent":"fake-210","V8-Version":"0","WebKit-Version":"0","webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/fake-210"}}"#
        );
        write!(
            http,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write /json/version response");
        drop(http);

        let (ws_stream, _) = listener.accept().expect("accept websocket");
        let mut websocket =
            tokio_tungstenite::tungstenite::accept(ws_stream).expect("websocket handshake");
        while websocket.read().is_ok() {}
    });
    port
}

#[tokio::test]
async fn must_pass_responsive_endpoint_attaches_within_the_limit() {
    let port = spawn_responsive_cdp_endpoint();
    let endpoint = format!("http://127.0.0.1:{port}");

    let started = Instant::now();
    let result = attach_chromiumoxide_with_timeout(&endpoint, Duration::from_secs(5)).await;

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a responsive endpoint must attach before the limit, took {:?}",
        started.elapsed()
    );
    match result {
        Ok(attachment) => assert_eq!(attachment.endpoint, endpoint),
        Err(error) => panic!("responsive CDP endpoint must attach, got {error:?}"),
    }
}

#[tokio::test]
async fn must_fail_refused_port_is_unreachable_not_a_timeout() {
    // Bind then release: nothing listens, so the connect is refused at once.
    // A refused endpoint is `CdpUnreachable` (caller may drop its state), never
    // the #210 timeout `CdpAttachFailed` (caller keeps state and retries).
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe listener");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    let endpoint = format!("http://127.0.0.1:{port}");

    // Windows retries a refused loopback SYN for ~2 s; the limit stays well above.
    let result = attach_chromiumoxide_with_timeout(&endpoint, Duration::from_secs(8)).await;

    match result {
        Err(A11yError::CdpUnreachable { .. }) => {}
        Err(other) => panic!("expected CdpUnreachable for a refused port, got {other:?}"),
        Ok(_attachment) => panic!("attached to a refused port"),
    }
}

#[tokio::test]
async fn must_fail_silent_endpoint_times_out_with_attach_failed() {
    // Bound but never accepted/answered: the kernel completes the TCP handshake
    // from the backlog, so the HTTP `/json/version` request hangs.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind silent listener");
    let port = listener.local_addr().expect("local addr").port();
    let endpoint = format!("http://127.0.0.1:{port}");

    let started = Instant::now();
    let result = attach_chromiumoxide_with_timeout(&endpoint, Duration::from_millis(300)).await;

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "attach must be bounded, took {:?}",
        started.elapsed()
    );
    match result {
        Err(A11yError::CdpAttachFailed { detail }) => {
            assert!(detail.contains("timed out after 300 ms"), "{detail}");
        }
        Err(other) => panic!("expected CdpAttachFailed timeout, got {other:?}"),
        Ok(_attachment) => panic!("attached to a silent listener"),
    }
    drop(listener);
}
