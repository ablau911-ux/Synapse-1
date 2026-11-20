use std::{collections::BTreeSet, path::PathBuf, process::Stdio, time::Duration};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt as _, StreamExt as _};
use image::ImageReader;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    net::TcpStream,
    process::{Child, Command},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::{
    ActionDetail, BackendIdentity, BackendKind, BrowserRuntimeConfig, Frame, PointerButton,
    SeatAction, SeatConfig, SeatError, SeatId, TeardownStatus, Viewport, now_ms,
    profile_guard::{ProfileGuard, cleanup_profile, detach_profile_cleanup},
    runtime::{SeatBackend, SeatBackendFactory},
};

#[path = "browser_startup.rs"]
mod startup;
use startup::{
    ChildSpawner, LaunchPolicy, SpawnLedger, StartupDeadline, abandoned_teardown, profile_root_for,
    resources_text, teardown_owned, with_teardown,
};

const HIDDEN_WINDOW: u32 = 0x0800_0000;

pub struct BrowserBackendFactory {
    config: BrowserRuntimeConfig,
    policy: LaunchPolicy,
    ledger: SpawnLedger,
}
impl BrowserBackendFactory {
    /// `teardown` bounds kill, exit wait and profile removal after a failed
    /// startup; the supervisor passes
    /// `SeatSupervisorConfig::startup_teardown_allowance` (#73).
    #[must_use]
    pub fn new(config: BrowserRuntimeConfig, teardown: Duration) -> Self {
        Self::with_spawner(config, teardown, spawn_chromium)
    }

    /// Test seam: the same factory with an injected child spawner.
    pub(crate) fn with_spawner(
        config: BrowserRuntimeConfig,
        teardown: Duration,
        spawner: ChildSpawner,
    ) -> Self {
        Self {
            config,
            policy: LaunchPolicy { spawner, teardown },
            ledger: SpawnLedger::default(),
        }
    }
}

#[async_trait]
impl SeatBackendFactory for BrowserBackendFactory {
    fn kind(&self) -> BackendKind {
        BackendKind::Browser
    }
    async fn create(
        &self,
        seat_id: SeatId,
        config: &SeatConfig,
    ) -> Result<Box<dyn SeatBackend>, SeatError> {
        let config = match config {
            SeatConfig::Browser(config) => config,
            #[cfg(any(test, feature = "test-support"))]
            SeatConfig::Mock(_) => {
                return Err(SeatError::InvalidConfig(
                    "browser factory received a mock config".to_owned(),
                ));
            }
        };
        let url = config.initial_url.clone();
        let launched = BrowserBackend::launch_with(
            seat_id,
            config.viewport,
            url,
            &self.config,
            self.policy,
            &self.ledger,
        )
        .await?;
        Ok(Box::new(launched))
    }

    fn startup_resources(&self, seat_id: SeatId) -> String {
        let profile_root = profile_root_for(&self.config, seat_id);
        resources_text(&profile_root, self.ledger.pid(seat_id))
    }

    async fn abandoned_startup_teardown(
        &self,
        seat_id: SeatId,
        resources: &str,
        allowance: Duration,
    ) -> TeardownStatus {
        abandoned_teardown(&self.config, seat_id, resources, allowance).await
    }
}

pub struct BrowserBackend {
    identity: BackendIdentity,
    process: Child,
    profile_root: PathBuf,
    runtime_root: PathBuf,
    cdp: CdpClient,
    viewport: Viewport,
    pointer_x: f64,
    pointer_y: f64,
    pressed_keys: BTreeSet<String>,
    pressed_buttons: BTreeSet<PointerButton>,
    /// Set once the child is dead and the profile removed; `Drop` finishes the
    /// teardown for any backend that never got that far (#73).
    torn_down: bool,
}

impl BrowserBackend {
    /// Production launch policy with the startup timeout as teardown allowance;
    /// used by the launch tests that drive Chromium spawn failure/cancellation.
    #[cfg(test)]
    async fn launch(
        seat_id: SeatId,
        viewport: Viewport,
        initial_url: Option<String>,
        config: &BrowserRuntimeConfig,
    ) -> Result<Self, SeatError> {
        let policy = LaunchPolicy {
            spawner: spawn_chromium,
            teardown: Duration::from_millis(config.startup_timeout_ms),
        };
        let ledger = SpawnLedger::default();
        Self::launch_with(seat_id, viewport, initial_url, config, policy, &ledger).await
    }

    async fn launch_with(
        seat_id: SeatId,
        viewport: Viewport,
        initial_url: Option<String>,
        config: &BrowserRuntimeConfig,
        policy: LaunchPolicy,
        ledger: &SpawnLedger,
    ) -> Result<Self, SeatError> {
        ensure_executable(config).await?;
        let profile_root = profile_root_for(config, seat_id);
        let profile_dir = profile_root.join("profile");
        // Armed before the directory exists: every early return and any
        // cancellation of this future removes it (#73).
        let mut guard = ProfileGuard::new(config.runtime_root.clone(), profile_root.clone());
        if let Err(error) = tokio::fs::create_dir_all(&profile_dir).await {
            let _cleanup_result = guard.cleanup().await;
            return Err(io_error(error));
        }
        let mut process = match (policy.spawner)(config, &profile_dir, viewport) {
            Ok(process) => process,
            Err(error) => {
                let _cleanup_result = guard.cleanup().await;
                return Err(io_error(error));
            }
        };
        // Removed on every exit of this launch, including a dropped future (#73).
        let _ledger_entry = ledger.record(seat_id, process.id());
        // One deadline for every startup wait: probe, WebSocket handshake and
        // the initial CDP replies (#73).
        let deadline = StartupDeadline::start(config.startup_timeout_ms, policy.teardown);
        let connection = match deadline.bound(wait_for_cdp(&profile_dir, deadline)).await {
            Ok(connection) => connection,
            Err(error) => {
                let root = &config.runtime_root;
                let teardown =
                    teardown_owned(&mut process, root, &profile_root, policy.teardown).await;
                // Unconfirmed removal leaves the guard armed: Drop retries it.
                if teardown == TeardownStatus::Confirmed {
                    guard.disarm();
                }
                return Err(with_teardown(error, teardown));
            }
        };
        // The backend owns child and profile from here (its Drop covers
        // cancellation during startup). No await between disarm and `Self`.
        guard.disarm();
        let backend = Self::from_parts(process, connection, profile_root, config, viewport);
        backend.finish_startup(initial_url, deadline).await
    }

    fn from_parts(
        process: Child,
        connection: CdpConnection,
        profile_root: PathBuf,
        config: &BrowserRuntimeConfig,
        viewport: Viewport,
    ) -> Self {
        Self {
            identity: BackendIdentity {
                kind: BackendKind::Browser,
                target: connection.identity_target,
                process_id: process.id(),
            },
            process,
            profile_root,
            runtime_root: config.runtime_root.clone(),
            cdp: connection.cdp,
            viewport,
            pointer_x: 0.0,
            pointer_y: 0.0,
            pressed_keys: BTreeSet::new(),
            pressed_buttons: BTreeSet::new(),
            torn_down: false,
        }
    }

    /// Initial CDP commands under the shared startup deadline. Any failure,
    /// including a stalled reply, tears the backend down within the teardown
    /// allowance and reports the physical readback with the error (#73).
    async fn finish_startup(
        mut self,
        initial_url: Option<String>,
        deadline: StartupDeadline,
    ) -> Result<Self, SeatError> {
        let viewport = self.viewport;
        let result = deadline
            .bound(async {
                self.cdp("Emulation.setDeviceMetricsOverride", json!({ "width": viewport.width, "height": viewport.height, "deviceScaleFactor": 1, "mobile": false })).await?;
                if let Some(url) = initial_url {
                    self.act(SeatAction::Navigate { url }).await?;
                }
                Ok::<(), SeatError>(())
            })
            .await;
        match result {
            Ok(()) => Ok(self),
            Err(error) => {
                let (root, profile) = (&self.runtime_root, &self.profile_root);
                let teardown =
                    teardown_owned(&mut self.process, root, profile, deadline.teardown).await;
                // Unconfirmed teardown leaves `torn_down` false: Drop retries it.
                self.torn_down = teardown == TeardownStatus::Confirmed;
                Err(with_teardown(error, teardown))
            }
        }
    }

    async fn ensure_alive(&mut self) -> Result<(), SeatError> {
        if let Some(status) = self.process.try_wait().map_err(io_error)? {
            return Err(SeatError::Backend(format!(
                "owned Chromium exited with {status}"
            )));
        }
        Ok(())
    }

    async fn cdp(&mut self, method: &str, params: Value) -> Result<Value, SeatError> {
        self.ensure_alive().await?;
        self.cdp.call(method, params).await
    }

    async fn key(&mut self, key: String, down: bool) -> Result<(), SeatError> {
        let descriptor = key_descriptor(&key);
        let mut held = self.pressed_keys.clone();
        if down {
            // Retain a possible down event even if the reply is lost: cleanup must
            // attempt its key-up rather than leave the owned page/guest stuck.
            self.pressed_keys.insert(key.clone());
            held = self.pressed_keys.clone();
        } else {
            held.remove(&key);
        }
        let suppress_text = held.contains("Control")
            || held.contains("Ctrl")
            || held.contains("Alt")
            || held.contains("Meta");
        let text = down
            .then_some(descriptor.text.as_deref())
            .flatten()
            .filter(|_| !suppress_text)
            .map(|text| shifted_text(text, &held));
        let event_key = text
            .as_ref()
            .filter(|_| descriptor.ascii_letter)
            .map_or_else(|| descriptor.key.clone(), Clone::clone);
        let event_type = if down && text.is_some() {
            "keyDown"
        } else if down {
            "rawKeyDown"
        } else {
            "keyUp"
        };
        let mut params = json!({ "type": event_type, "key": event_key, "code": descriptor.code, "windowsVirtualKeyCode": descriptor.virtual_key_code, "nativeVirtualKeyCode": descriptor.virtual_key_code, "modifiers": modifier_mask(&held) });
        if let Some(text) = text {
            params["text"] = Value::String(text.clone());
            params["unmodifiedText"] = Value::String(text);
        }
        self.cdp("Input.dispatchKeyEvent", params).await?;
        if !down {
            self.pressed_keys = held;
        }
        Ok(())
    }

    async fn mouse(
        &mut self,
        event_type: &str,
        button: Option<PointerButton>,
    ) -> Result<(), SeatError> {
        let buttons = self.pressed_buttons.clone();
        self.mouse_with_buttons(event_type, button, &buttons).await
    }

    async fn mouse_with_buttons(
        &mut self,
        event_type: &str,
        button: Option<PointerButton>,
        buttons: &BTreeSet<PointerButton>,
    ) -> Result<(), SeatError> {
        let button_name = button.map_or("none", button_name);
        let buttons = button_bits(buttons);
        self.cdp("Input.dispatchMouseEvent", json!({ "type": event_type, "x": self.pointer_x, "y": self.pointer_y, "button": button_name, "buttons": buttons, "modifiers": modifier_mask(&self.pressed_keys), "clickCount": if matches!(event_type, "mousePressed" | "mouseReleased") { 1 } else { 0 } })).await?;
        Ok(())
    }
}

#[async_trait]
impl SeatBackend for BrowserBackend {
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn inputs_end_with_process(&self) -> bool {
        true
    }

    async fn act(&mut self, action: SeatAction) -> Result<ActionDetail, SeatError> {
        match action {
            SeatAction::Navigate { url } => {
                let result = self.cdp("Page.navigate", json!({ "url": url })).await?;
                Ok(ActionDetail::Navigated {
                    frame_id: result
                        .get("frameId")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                })
            }
            SeatAction::TypeText { text } => {
                let byte_count = u32::try_from(text.len())
                    .map_err(|_| SeatError::backend("text byte count overflow"))?;
                self.cdp("Input.insertText", json!({ "text": text }))
                    .await?;
                Ok(ActionDetail::TextInserted { byte_count })
            }
            SeatAction::KeyDown { key } => {
                self.key(key.clone(), true).await?;
                Ok(ActionDetail::Key { key, pressed: true })
            }
            SeatAction::KeyUp { key } => {
                self.key(key.clone(), false).await?;
                Ok(ActionDetail::Key {
                    key,
                    pressed: false,
                })
            }
            SeatAction::PointerMove { x, y } => {
                self.pointer_x = x;
                self.pointer_y = y;
                self.mouse("mouseMoved", None).await?;
                Ok(ActionDetail::PointerMoved { x, y })
            }
            SeatAction::PointerDown { button } => {
                self.pressed_buttons.insert(button);
                self.mouse("mousePressed", Some(button)).await?;
                Ok(ActionDetail::PointerButton {
                    button,
                    pressed: true,
                })
            }
            SeatAction::PointerUp { button } => {
                let mut remaining = self.pressed_buttons.clone();
                remaining.remove(&button);
                self.mouse_with_buttons("mouseReleased", Some(button), &remaining)
                    .await?;
                self.pressed_buttons = remaining;
                Ok(ActionDetail::PointerButton {
                    button,
                    pressed: false,
                })
            }
            SeatAction::Scroll { delta_x, delta_y } => {
                self.cdp("Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": self.pointer_x, "y": self.pointer_y, "deltaX": delta_x, "deltaY": delta_y, "buttons": button_bits(&self.pressed_buttons), "modifiers": modifier_mask(&self.pressed_keys) })).await?;
                Ok(ActionDetail::Scrolled { delta_x, delta_y })
            }
            SeatAction::Resize { viewport } => {
                self.cdp("Emulation.setDeviceMetricsOverride", json!({ "width": viewport.width, "height": viewport.height, "deviceScaleFactor": 1, "mobile": false })).await?;
                self.viewport = viewport;
                Ok(ActionDetail::Resized { viewport })
            }
            SeatAction::Evaluate { expression } => {
                let response = self.cdp("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": false })).await?;
                if let Some(exception) = response.get("exceptionDetails") {
                    return Err(SeatError::Backend(format!(
                        "owned-page evaluation threw: {exception}"
                    )));
                }
                let result = response.get("result").cloned().unwrap_or(Value::Null);
                let value_json = serde_json::to_string(result.get("value").unwrap_or(&Value::Null))
                    .map_err(|error| {
                        SeatError::backend(format!("evaluation result cannot be encoded: {error}"))
                    })?;
                if value_json.len() > crate::MAX_EVALUATE_BYTES {
                    return Err(SeatError::backend(format!(
                        "evaluation result exceeded {} bytes",
                        crate::MAX_EVALUATE_BYTES
                    )));
                }
                let result_type = result
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let description = result
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .filter(|text| text.len() <= 4_096);
                Ok(ActionDetail::Evaluated {
                    result_type,
                    value_json,
                    description,
                })
            }
        }
    }

    async fn capture(&mut self) -> Result<Frame, SeatError> {
        let response = self
            .cdp(
                "Page.captureScreenshot",
                json!({ "format": "png", "fromSurface": true }),
            )
            .await?;
        let image_base64 = response
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                SeatError::Backend("CDP screenshot response had no PNG data".to_owned())
            })?
            .to_owned();
        let bytes = STANDARD.decode(&image_base64).map_err(|error| {
            SeatError::Backend(format!("CDP screenshot base64 was invalid: {error}"))
        })?;
        if bytes.len() > crate::MAX_SCREENSHOT_BYTES {
            return Err(SeatError::FrameTooLarge);
        }
        let image = ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| {
                SeatError::backend(format!("CDP screenshot format was invalid: {error}"))
            })?
            .decode()
            .map_err(|error| {
                SeatError::backend(format!("CDP screenshot PNG was invalid: {error}"))
            })?;
        Ok(Frame {
            width: image.width(),
            height: image.height(),
            timestamp_ms: now_ms(),
            mime_type: "image/png".to_owned(),
            image_base64,
        })
    }

    async fn release_inputs(&mut self) -> Result<(), SeatError> {
        let keys: Vec<String> = self.pressed_keys.iter().cloned().collect();
        let buttons: Vec<PointerButton> = self.pressed_buttons.iter().copied().collect();
        let mut failures = Vec::new();
        for key in keys {
            if let Err(error) = self.key(key, false).await {
                failures.push(error.to_string());
            }
        }
        for button in buttons {
            let mut remaining = self.pressed_buttons.clone();
            remaining.remove(&button);
            match self
                .mouse_with_buttons("mouseReleased", Some(button), &remaining)
                .await
            {
                Ok(()) => self.pressed_buttons = remaining,
                Err(error) => failures.push(error.to_string()),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(SeatError::Backend(format!(
                "failed to release one or more CDP inputs: {}",
                failures.join("; ")
            )))
        }
    }

    async fn close(&mut self) -> Result<(), SeatError> {
        // Seats own disposable Chromium children. Never wait for a CDP reply
        // before initiating physical teardown: the socket may be stalled, and
        // the supervisor can cancel this future at its cleanup deadline.
        if self.process.try_wait().map_err(io_error)?.is_none() {
            self.process.start_kill().map_err(io_error)?;
            let _ = self.process.wait().await.map_err(io_error)?;
        }
        cleanup_profile(&self.runtime_root, &self.profile_root).await?;
        self.torn_down = true;
        Ok(())
    }
}

impl Drop for BrowserBackend {
    fn drop(&mut self) {
        if self.torn_down {
            return;
        }
        // Dropped without a completed close (cancelled startup, panic, failed
        // removal): never leave the child or the private profile behind.
        let _ = self.process.start_kill();
        detach_profile_cleanup(self.runtime_root.clone(), self.profile_root.clone());
    }
}

#[derive(Deserialize)]
struct CdpVersion {
    #[serde(rename = "Browser")]
    browser: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: String,
}
#[derive(Deserialize)]
struct CdpTarget {
    id: String,
    url: String,
    #[serde(rename = "type")]
    target_type: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: Option<String>,
}

struct CdpClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}
impl CdpClient {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, SeatError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| SeatError::Backend("CDP message sequence overflow".to_owned()))?;
        self.socket
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params })
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(ws_error)?;
        loop {
            let message = self
                .socket
                .next()
                .await
                .ok_or_else(|| SeatError::Backend("CDP socket closed".to_owned()))?
                .map_err(ws_error)?;
            match message {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).map_err(|error| {
                        SeatError::Backend(format!("invalid CDP response JSON: {error}"))
                    })?;
                    if value.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(error) = value.get("error") {
                            return Err(SeatError::Backend(format!(
                                "CDP {method} failed: {error}"
                            )));
                        }
                        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
                Message::Ping(payload) => self
                    .socket
                    .send(Message::Pong(payload))
                    .await
                    .map_err(ws_error)?,
                Message::Close(_) => {
                    return Err(SeatError::Backend(
                        "CDP socket closed by Chromium".to_owned(),
                    ));
                }
                _ => {}
            }
        }
    }
}

struct CdpConnection {
    cdp: CdpClient,
    identity_target: String,
}
async fn ensure_executable(config: &BrowserRuntimeConfig) -> Result<(), SeatError> {
    if tokio::fs::try_exists(&config.chromium_executable)
        .await
        .map_err(io_error)?
    {
        return Ok(());
    }
    Err(SeatError::InvalidConfig(format!(
        "configured Chromium executable does not exist: {}",
        config.chromium_executable.display()
    )))
}

fn spawn_chromium(
    config: &BrowserRuntimeConfig,
    profile_dir: &std::path::Path,
    viewport: Viewport,
) -> std::io::Result<Child> {
    let mut command = Command::new(&config.chromium_executable);
    command
        .arg("--headless=new")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--remote-debugging-port=0")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(format!(
            "--window-size={},{}",
            viewport.width, viewport.height
        ))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(not(windows))]
    command.arg("--no-sandbox");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.as_std_mut().creation_flags(HIDDEN_WINDOW);
    }
    command.spawn()
}

async fn wait_for_cdp(
    profile_dir: &std::path::Path,
    deadline: StartupDeadline,
) -> Result<CdpConnection, SeatError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .map_err(|error| {
            SeatError::Backend(format!("cannot construct CDP probe client: {error}"))
        })?;
    loop {
        let port_file = profile_dir.join("DevToolsActivePort");
        if let Ok(contents) = tokio::fs::read_to_string(&port_file).await
            && let Some((port, browser_path)) = parse_devtools_active_port(&contents)
        {
            let endpoint = format!("http://127.0.0.1:{port}");
            let expected_browser_ws = format!("ws://127.0.0.1:{port}{browser_path}");
            if let Ok(response) = client.get(format!("{endpoint}/json/version")).send().await {
                if let Ok(body) = response.text().await
                    && let Ok(version) = serde_json::from_str::<CdpVersion>(&body)
                {
                    // Iron Law 9: product tokens live in browsers/*.json
                    // (`cdp_product_tokens` of every Chromium row).
                    if !synapse_browsers::BrowserRegistry::global()
                        .cdp_product_is_chromium(&version.browser)
                    {
                        return Err(SeatError::Backend(format!(
                            "owned process did not identify as Chromium: {}",
                            version.browser
                        )));
                    }
                    if version.websocket_debugger_url != expected_browser_ws {
                        return Err(SeatError::backend(
                            "DevToolsActivePort browser websocket did not match Chromium endpoint",
                        ));
                    }
                    if let Ok(target_response) =
                        client.get(format!("{endpoint}/json/list")).send().await
                    {
                        if let Ok(body) = target_response.text().await
                            && let Ok(targets) = serde_json::from_str::<Vec<CdpTarget>>(&body)
                        {
                            if let Some(target) = targets
                                .into_iter()
                                .find(|target| target.target_type == "page")
                                && let Some(url) = target.websocket_debugger_url
                            {
                                let (socket, _) = connect_async(url).await.map_err(ws_error)?;
                                return Ok(CdpConnection {
                                    cdp: CdpClient { socket, next_id: 1 },
                                    identity_target: format!(
                                        "{expected_browser_ws};target_id={};target_url={}",
                                        target.id,
                                        truncate_identity(&target.url)
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        if deadline.has_passed() {
            return Err(deadline.expired());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn parse_devtools_active_port(contents: &str) -> Option<(&str, &str)> {
    let mut lines = contents.lines();
    let port = lines.next()?;
    let path = lines.next()?;
    if port.parse::<u16>().ok()? == 0 || !path.starts_with("/devtools/browser/") {
        return None;
    }
    Some((port, path))
}
fn truncate_identity(value: &str) -> String {
    const MAX_IDENTITY_BYTES: usize = 2_048;
    if value.len() <= MAX_IDENTITY_BYTES {
        value.to_owned()
    } else {
        format!("{}…", &value[..MAX_IDENTITY_BYTES])
    }
}
fn io_error(error: std::io::Error) -> SeatError {
    SeatError::backend(error.to_string())
}
fn ws_error(error: impl std::fmt::Display) -> SeatError {
    SeatError::backend(format!("CDP transport error: {error}"))
}
const fn button_name(button: PointerButton) -> &'static str {
    match button {
        PointerButton::Left => "left",
        PointerButton::Middle => "middle",
        PointerButton::Right => "right",
        PointerButton::Back => "back",
        PointerButton::Forward => "forward",
    }
}
fn button_bits(buttons: &BTreeSet<PointerButton>) -> u8 {
    buttons.iter().fold(0, |bits, button| {
        bits | match button {
            PointerButton::Left => 1,
            PointerButton::Right => 2,
            PointerButton::Middle => 4,
            PointerButton::Back => 8,
            PointerButton::Forward => 16,
        }
    })
}

struct KeyDescriptor {
    key: String,
    code: String,
    virtual_key_code: u32,
    text: Option<String>,
    ascii_letter: bool,
}
fn key_descriptor(key: &str) -> KeyDescriptor {
    let (code, virtual_key_code, text, ascii_letter) = match key {
        "Alt" => ("AltLeft".to_owned(), 18, None, false),
        "Control" | "Ctrl" => ("ControlLeft".to_owned(), 17, None, false),
        "Meta" => ("MetaLeft".to_owned(), 91, None, false),
        "Shift" => ("ShiftLeft".to_owned(), 16, None, false),
        "Tab" => ("Tab".to_owned(), 9, Some("\t".to_owned()), false),
        "Enter" => ("Enter".to_owned(), 13, Some("\r".to_owned()), false),
        "Backspace" => ("Backspace".to_owned(), 8, None, false),
        "Escape" | "Esc" => ("Escape".to_owned(), 27, None, false),
        "ArrowLeft" => ("ArrowLeft".to_owned(), 37, None, false),
        "ArrowUp" => ("ArrowUp".to_owned(), 38, None, false),
        "ArrowRight" => ("ArrowRight".to_owned(), 39, None, false),
        "ArrowDown" => ("ArrowDown".to_owned(), 40, None, false),
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() => {
            let letter = value.to_ascii_uppercase();
            (
                format!("Key{letter}"),
                u32::from(letter.as_bytes()[0]),
                Some(value.to_owned()),
                true,
            )
        }
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_digit() => (
            format!("Digit{value}"),
            u32::from(value.as_bytes()[0]),
            Some(value.to_owned()),
            false,
        ),
        value if value.len() == 1 => ("Unidentified".to_owned(), 0, Some(value.to_owned()), false),
        value => (value.to_owned(), 0, None, false),
    };
    KeyDescriptor {
        key: key.to_owned(),
        code,
        virtual_key_code,
        text,
        ascii_letter,
    }
}
fn modifier_mask(keys: &BTreeSet<String>) -> u8 {
    let mut mask = 0;
    if keys.contains("Alt") {
        mask |= 1;
    }
    if keys.contains("Control") || keys.contains("Ctrl") {
        mask |= 2;
    }
    if keys.contains("Meta") {
        mask |= 4;
    }
    if keys.contains("Shift") {
        mask |= 8;
    }
    mask
}
fn shifted_text(text: &str, held: &BTreeSet<String>) -> String {
    if held.contains("Shift") && text.len() == 1 && text.as_bytes()[0].is_ascii_alphabetic() {
        text.to_ascii_uppercase()
    } else {
        text.to_owned()
    }
}

#[cfg(all(test, windows))]
#[path = "browser_cleanup_test_support.rs"]
pub(crate) mod cleanup_tests;

#[cfg(all(test, windows))]
#[path = "browser_close_stall_139.rs"]
mod close_stall_139_tests;

#[cfg(test)]
#[path = "browser_launch_tests.rs"]
mod launch_tests;

#[cfg(all(test, windows))]
#[path = "browser_startup_stall_73.rs"]
mod startup_stall_73_tests;

#[cfg(all(test, windows))]
#[path = "browser_fake_devtools_73.rs"]
mod fake_devtools_73;
