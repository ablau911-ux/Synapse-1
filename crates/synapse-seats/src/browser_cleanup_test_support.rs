//! Shared helper for the browser seat launch/runtime tests.

use super::*;
use std::{error::Error, os::windows::process::CommandExt as _};
use tokio::net::TcpListener;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

// A real child and a real WebSocket peer that never replies. This exercises
// teardown without requiring an installed browser or simulating exit state.
pub(crate) async fn unresponsive_backend()
-> TestResult<(BrowserBackend, WebSocketStream<TcpStream>, PathBuf)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        tokio_tungstenite::accept_async(stream)
            .await
            .map_err(std::io::Error::other)
    });
    let (socket, _) = connect_async(format!("ws://{address}")).await?;
    let peer = peer.await??;
    let runtime_root =
        std::env::temp_dir().join(format!("synapse-seat-cleanup-{}", uuid::Uuid::new_v4()));
    let profile_root = runtime_root.join("browser-seat");
    tokio::fs::create_dir_all(&profile_root).await?;
    tokio::fs::write(profile_root.join("owned-marker"), b"cleanup-test").await?;
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 60",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command.as_std_mut().creation_flags(HIDDEN_WINDOW);
    let process = command.spawn()?;
    let backend = BrowserBackend {
        identity: BackendIdentity {
            kind: BackendKind::Browser,
            target: format!("ws://{address}"),
            process_id: process.id(),
        },
        process,
        profile_root,
        runtime_root: runtime_root.clone(),
        cdp: CdpClient { socket, next_id: 1 },
        viewport: Viewport::default(),
        pointer_x: 0.0,
        pointer_y: 0.0,
        pressed_keys: BTreeSet::from(["Control".to_owned()]),
        pressed_buttons: BTreeSet::from([PointerButton::Left]),
        torn_down: false,
    };
    Ok((backend, peer, runtime_root))
}

/// OS process-table readback (not the `Child` handle): true while `pid` is
/// listed. Fails closed: a tasklist that cannot run or exits non-zero is an
/// error, never "process gone". Awaited, not blocking: `tasklist` takes 100 ms
/// or more, and a blocking call would stall the runtime that drives the startup
/// deadline and the fake `DevTools` peer under test (#2399 G16).
pub(crate) async fn process_listed(pid: u32) -> TestResult<bool> {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .creation_flags(HIDDEN_WINDOW)
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!("tasklist failed with {}", output.status).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

/// Polls the process table until `pid` is gone (true) or 10 s pass (false).
/// A failed process-table read is an error.
pub(crate) async fn wait_process_gone(pid: u32) -> TestResult<bool> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if !process_listed(pid).await? {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(false)
}

/// Entry names in `root` right now (one filesystem read, no waiting).
pub(crate) async fn dir_entries(root: &std::path::Path) -> TestResult<Vec<String>> {
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Entry names left in `root` once it is empty or 10 s pass.
pub(crate) async fn wait_until_dir_empty(root: &std::path::Path) -> TestResult<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let names = dir_entries(root).await?;
        if names.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(names);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
