//! #73 test harness: an injected fake browser child (hidden `powershell
//! Start-Sleep`, never Chromium) and a scripted fake DevTools endpoint whose
//! phases (probe, handshake, first reply, initial navigate) can each be delayed
//! or stalled. Drives the production `SeatSupervisor` and `BrowserBackendFactory`;
//! only the child spawner is replaced.

use std::{
    collections::HashMap,
    os::windows::process::CommandExt as _,
    path::Path,
    sync::{Arc, LazyLock, Mutex as StdMutex},
    time::{Duration, Instant as StdInstant},
};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    sync::Mutex,
};

use super::*;
use crate::{BrowserSeatConfig, SeatSupervisor, SeatSupervisorConfig};

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) const STARTUP_MS: u64 = 1_500;
/// Request timeout; the browser teardown allowance is derived from it.
pub(crate) const REQUEST_MS: u64 = 1_000;
/// Guards every wait in the harness and the tests against a hang.
pub(crate) const TEST_BOUND: Duration = Duration::from_secs(10);
pub(crate) const BROWSER_PATH: &str = "/devtools/browser/fake-73";

#[derive(Clone, Copy)]
pub(crate) enum Reply {
    AfterMs(u64),
    Never,
}

#[derive(Clone, Copy)]
pub(crate) enum Handshake {
    AfterMs(u64),
    /// Accepts TCP, never answers the upgrade.
    Silent,
    /// Sends only the 101 status line, never the header terminator.
    Partial,
}

#[derive(Clone, Copy)]
pub(crate) struct Script {
    pub(crate) probe: Reply,
    pub(crate) handshake: Handshake,
    pub(crate) metrics: Reply,
    pub(crate) navigate: Reply,
}

pub(crate) const FAST: Script = Script {
    probe: Reply::AfterMs(0),
    handshake: Handshake::AfterMs(0),
    metrics: Reply::AfterMs(0),
    navigate: Reply::AfterMs(0),
};

/// Per-test fake DevTools endpoint plus its private runtime root.
pub(crate) struct Fake {
    pub(crate) script: Mutex<Script>,
    pub(crate) arrived: Mutex<Vec<&'static str>>,
    pub(crate) root: PathBuf,
    pub(crate) runtime_root: PathBuf,
}

/// Spawner state keyed by runtime root (the spawner is a plain `fn`).
pub(crate) struct FakeChild {
    pub(crate) http_port: u16,
    pub(crate) pid: Option<u32>,
}

pub(crate) static FAKE_CHILDREN: LazyLock<StdMutex<HashMap<PathBuf, FakeChild>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

impl Fake {
    pub(crate) async fn script(&self) -> Script {
        *self.script.lock().await
    }

    pub(crate) async fn record(&self, phase: &'static str) {
        self.arrived.lock().await.push(phase);
    }

    pub(crate) async fn wait_for(&self, phase: &'static str) -> TestResult {
        let deadline = StdInstant::now() + TEST_BOUND;
        while StdInstant::now() < deadline {
            if self.arrived.lock().await.contains(&phase) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(format!("startup never reached phase {phase}").into())
    }

    pub(crate) fn pid(&self) -> TestResult<u32> {
        let children = FAKE_CHILDREN
            .lock()
            .map_err(|_| "fake child registry poisoned")?;
        let child = children
            .get(&self.runtime_root)
            .ok_or("fake child was never spawned")?;
        child.pid.ok_or_else(|| "fake child has no pid".into())
    }

    pub(crate) async fn finish(&self) -> TestResult {
        FAKE_CHILDREN
            .lock()
            .map_err(|_| "fake child registry poisoned")?
            .remove(&self.runtime_root);
        tokio::fs::remove_dir_all(&self.root).await?;
        Ok(())
    }
}

/// `ChildSpawner` seam: a live hidden child that ignores Chromium flags, and the
/// DevToolsActivePort file Chromium would write.
pub(crate) fn fake_child(
    config: &BrowserRuntimeConfig,
    profile_dir: &Path,
    _viewport: Viewport,
) -> std::io::Result<Child> {
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
    let child = command.spawn()?;
    let mut children = FAKE_CHILDREN
        .lock()
        .map_err(|_| std::io::Error::other("fake registry poisoned"))?;
    let entry = children
        .get_mut(&config.runtime_root)
        .ok_or_else(|| std::io::Error::other("no fake endpoint for this runtime root"))?;
    entry.pid = child.id();
    let port_file = format!("{}\n{BROWSER_PATH}\n", entry.http_port);
    std::fs::write(profile_dir.join("DevToolsActivePort"), port_file)?;
    Ok(child)
}

pub(crate) async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1_024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => head.extend_from_slice(&buffer[..read]),
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

pub(crate) async fn pause(reply: Reply) -> bool {
    match reply {
        Reply::AfterMs(ms) => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            true
        }
        Reply::Never => false,
    }
}

pub(crate) fn devtools_body(request: &str, http_port: u16, ws_port: u16) -> String {
    if request.contains("/json/version") {
        format!(
            r#"{{"Browser":"HeadlessChrome/140.0.0.0","webSocketDebuggerUrl":"ws://127.0.0.1:{http_port}{BROWSER_PATH}"}}"#
        )
    } else {
        format!(
            r#"[{{"id":"T73","url":"about:blank","type":"page","webSocketDebuggerUrl":"ws://127.0.0.1:{ws_port}/devtools/page/T73"}}]"#
        )
    }
}

pub(crate) async fn serve_http(
    fake: Arc<Fake>,
    listener: TcpListener,
    http_port: u16,
    ws_port: u16,
) {
    let mut held = Vec::new();
    while let Ok((mut stream, _)) = listener.accept().await {
        let request = read_head(&mut stream).await;
        if request.contains("/json/version") {
            fake.record("probe").await;
            if !pause(fake.script().await.probe).await {
                held.push(stream);
                continue;
            }
        }
        let body = devtools_body(&request, http_port, ws_port);
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}

pub(crate) async fn serve_ws(fake: Arc<Fake>, listener: TcpListener) {
    let mut held = Vec::new();
    while let Ok((mut stream, _)) = listener.accept().await {
        fake.record("handshake").await;
        match fake.script().await.handshake {
            Handshake::Silent => held.push(stream),
            Handshake::Partial => {
                let _request = read_head(&mut stream).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 101 Switching Protocols\r\n")
                    .await;
                held.push(stream);
            }
            Handshake::AfterMs(ms) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let fake = Arc::clone(&fake);
                tokio::spawn(async move {
                    if let Ok(socket) = tokio_tungstenite::accept_async(stream).await {
                        serve_cdp(fake, socket).await;
                    }
                });
            }
        }
    }
}

pub(crate) async fn serve_cdp(fake: Arc<Fake>, mut socket: WebSocketStream<TcpStream>) {
    while let Some(Ok(Message::Text(text))) = socket.next().await {
        let Ok(request) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let script = fake.script().await;
        let (phase, reply, result) = match request.get("method").and_then(Value::as_str) {
            Some("Emulation.setDeviceMetricsOverride") => ("metrics", script.metrics, json!({})),
            Some("Page.navigate") => ("navigate", script.navigate, json!({ "frameId": "F73" })),
            _ => ("other", Reply::AfterMs(0), json!({})),
        };
        fake.record(phase).await;
        if !pause(reply).await {
            // Keep the socket open and silent: the stalled reply.
            std::future::pending::<()>().await;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = json!({ "id": id, "result": result }).to_string();
        if socket.send(Message::Text(reply.into())).await.is_err() {
            return;
        }
    }
}

pub(crate) async fn fake_devtools(script: Script) -> TestResult<Arc<Fake>> {
    let root =
        std::env::temp_dir().join(format!("synapse-seat-startup-73-{}", uuid::Uuid::new_v4()));
    let runtime_root = root.join("runtime");
    tokio::fs::create_dir_all(&runtime_root).await?;
    let http = TcpListener::bind("127.0.0.1:0").await?;
    let ws = TcpListener::bind("127.0.0.1:0").await?;
    let (http_port, ws_port) = (http.local_addr()?.port(), ws.local_addr()?.port());
    FAKE_CHILDREN
        .lock()
        .map_err(|_| "fake child registry poisoned")?
        .insert(
            runtime_root.clone(),
            FakeChild {
                http_port,
                pid: None,
            },
        );
    let fake = Arc::new(Fake {
        script: Mutex::new(script),
        arrived: Mutex::new(Vec::new()),
        root,
        runtime_root,
    });
    tokio::spawn(serve_http(Arc::clone(&fake), http, http_port, ws_port));
    tokio::spawn(serve_ws(Arc::clone(&fake), ws));
    Ok(fake)
}

/// The production supervisor and browser factory; only the child spawner is fake.
pub(crate) fn supervisor_config(fake: &Fake) -> TestResult<SeatSupervisorConfig> {
    Ok(SeatSupervisorConfig {
        browser: BrowserRuntimeConfig {
            chromium_executable: PathBuf::from(std::env::var("SystemRoot")?)
                .join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
            runtime_root: fake.runtime_root.clone(),
            startup_timeout_ms: STARTUP_MS,
        },
        max_seats: 1,
        per_seat_queue_capacity: 1,
        request_timeout_ms: REQUEST_MS,
    })
}

/// The production browser factory as the supervisor builds it; only the child
/// spawner is fake.
pub(crate) fn browser_factory(config: &SeatSupervisorConfig) -> BrowserBackendFactory {
    let teardown = config.startup_teardown_allowance();
    BrowserBackendFactory::with_spawner(config.browser.clone(), teardown, fake_child)
}

pub(crate) fn supervisor(fake: &Fake) -> TestResult<SeatSupervisor> {
    let config = supervisor_config(fake)?;
    let factory = browser_factory(&config);
    SeatSupervisor::assemble(config, Arc::new(factory), Vec::new()).map_err(Into::into)
}

pub(crate) fn browser_seat(initial_url: Option<&str>) -> SeatConfig {
    SeatConfig::Browser(BrowserSeatConfig {
        initial_url: initial_url.map(ToOwned::to_owned),
        viewport: Viewport::default(),
    })
}
