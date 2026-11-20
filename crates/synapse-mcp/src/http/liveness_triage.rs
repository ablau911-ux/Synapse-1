//! Pre-parse triage for connections that arrive while the normal admission budget is full.
//!
//! Only a connection whose complete request head is an authenticated-liveness-shaped
//! `GET /health` may take a reserve slot (#19). Slow or partial heads, and every other path,
//! never touch the reserve, so a slow-header flood cannot starve health checks.

#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsSocket;
use std::{io::ErrorKind, time::Duration};

use tokio::net::TcpStream;

const MAX_HEAD_BYTES: usize = 4096;
const LIVENESS_PREFIX: &[u8] = b"GET /health";
const HEAD_END: &[u8] = b"\r\n\r\n";
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HeadClass {
    /// Not enough bytes yet to decide.
    Incomplete,
    /// Complete head of a liveness request.
    Liveness,
    /// Cannot be a liveness request; reject immediately.
    Other,
}

pub(super) fn classify_head(head: &[u8]) -> HeadClass {
    let compared = head.len().min(LIVENESS_PREFIX.len());
    if head[..compared] != LIVENESS_PREFIX[..compared] {
        return HeadClass::Other;
    }
    if head.len() <= LIVENESS_PREFIX.len() {
        return HeadClass::Incomplete;
    }
    if !matches!(head[LIVENESS_PREFIX.len()], b' ' | b'?') {
        return HeadClass::Other;
    }
    if head
        .windows(HEAD_END.len())
        .any(|window| window == HEAD_END)
    {
        HeadClass::Liveness
    } else {
        HeadClass::Incomplete
    }
}

/// Classify, without waiting, the bytes already queued on `stream`. A complete liveness head that
/// arrived with the connection is thereby admitted even when the triage pool is saturated (#19).
pub(super) fn classify_ready_head(stream: &TcpStream) -> HeadClass {
    // Peek the OS socket directly: a freshly accepted tokio stream has no readiness event yet,
    // so `poll_peek` reports Pending even when the request head is already queued (#19).
    #[cfg(unix)]
    let socket = stream.as_fd().try_clone_to_owned();
    #[cfg(windows)]
    let socket = stream.as_socket().try_clone_to_owned();
    let Ok(socket) = socket else {
        return HeadClass::Other;
    };
    let mut storage = [0_u8; MAX_HEAD_BYTES];
    match std::net::TcpStream::from(socket).peek(&mut storage) {
        Ok(peeked) if peeked > 0 => match classify_head(&storage[..peeked]) {
            HeadClass::Incomplete if peeked == MAX_HEAD_BYTES => HeadClass::Other,
            class => class,
        },
        Err(error) if error.kind() == ErrorKind::WouldBlock => HeadClass::Incomplete,
        _ => HeadClass::Other,
    }
}

/// Wait up to `deadline` for the peeked (unconsumed) bytes to form a complete liveness head.
pub(super) async fn await_liveness_head(stream: &TcpStream, deadline: Duration) -> bool {
    let decided = tokio::time::timeout(deadline, async {
        let mut buffer = [0_u8; MAX_HEAD_BYTES];
        let mut seen = 0_usize;
        loop {
            let peeked = stream.peek(&mut buffer).await.ok()?;
            if peeked == 0 {
                return None;
            }
            match classify_head(&buffer[..peeked]) {
                HeadClass::Liveness => return Some(true),
                HeadClass::Other => return Some(false),
                HeadClass::Incomplete if peeked == buffer.len() => return Some(false),
                HeadClass::Incomplete => {}
            }
            if peeked == seen {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            seen = peeked;
        }
    })
    .await;
    matches!(decided, Ok(Some(true)))
}

#[cfg(test)]
#[path = "tests/liveness_triage_19.rs"]
mod liveness_triage_19;
