//! CDP probe + attach for Chromium-family foregrounds.
//!
//! The diagnostic *types* ([`CdpDiagnostics`], [`CdpStatus`], [`CdpCapability`])
//! live in `synapse-core` because they are embedded in every `Observation`.
//! This module owns the *behaviour*: detecting a Chromium foreground, probing
//! for a reachable remote-debugging port, and attaching a `chromiumoxide`
//! client. It also owns the launched-port registry that ties
//! `act_launch` (#684) to the probe so a Synapse-launched browser is found
//! without the agent remembering manual flags.
//!
//! Background (research, 2026-06): since Chrome 136 the `--remote-debugging-port`
//! switch is ignored unless paired with a non-default `--user-data-dir`, so a
//! normally-launched Chrome on the user's primary profile can *never* expose a
//! debug port. That is why a normal launch probes `Unreachable` and why #684
//! must launch with a dedicated automation profile.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use synapse_core::error_codes;
use tokio::{net::TcpStream as TokioTcpStream, time::timeout};

pub use synapse_core::{CdpCapability, CdpDiagnostics, CdpStatus};

#[cfg(windows)]
use crate::{A11yError, A11yResult, EndpointOwner, ProcessStep};

#[cfg(windows)]
#[path = "cdp_webview2.rs"]
mod webview2;
#[cfg(windows)]
pub use webview2::{
    WebView2HostScope, WebView2ProcessRow, resolve_owned_webview2_page_target,
    webview2_browser_port_in_tree,
};

/// Default remote-debugging port probed when no launched port is registered for
/// the foreground process. 9222 is Chrome's conventional debug port.
pub const DEFAULT_CDP_PORT: u16 = 9222;

/// Environment override for the probed port list, e.g. `9222,9333`.
const CDP_PORTS_ENV: &str = "SYNAPSE_CDP_PORTS";
const CDP_UNREACHABLE_DETAIL: &str = "no reachable loopback CDP HTTP endpoint on checked ports; manual /json/version attach \
     requires Chrome/Edge to be started with --remote-debugging-port and, on Chrome 136+, \
     a non-default --user-data-dir. Chrome 144+ chrome://inspect/#remote-debugging is an \
     auto-connect permission flow, not a raw HTTP endpoint Synapse can probe through \
     SYNAPSE_CDP_PORTS";

#[must_use]
pub fn cdp_capabilities() -> Vec<CdpCapability> {
    vec![
        CdpCapability::DomSnapshot,
        CdpCapability::AccessibilityFullAxTree,
        CdpCapability::DomQuerySelector,
        CdpCapability::PageCaptureScreenshot,
        CdpCapability::PageFrameTree,
        CdpCapability::FlatIframeSessions,
        CdpCapability::PiercedShadowDom,
    ]
}

#[must_use]
pub fn is_chromium_family(process_name: &str) -> bool {
    // Iron Law 9: the Chromium image names live in `browsers/*.json`
    // (`legacy_chromium_process_names` + every Chromium row's `process_names`).
    synapse_browsers::BrowserRegistry::global().is_chromium_process(process_name)
}

// === Launched-port registry =================================================
//
// `act_launch` (#684) registers the ephemeral debug port it opened for a
// browser it launched, keyed by the browser's process id. The observe/find
// probe consults this registry first so a Synapse-launched browser is attached
// without any default-port collision or manual flag.

fn registry() -> &'static Mutex<HashMap<u32, u16>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u32, u16>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Records the CDP debug `port` that `act_launch` opened for browser process
/// `pid`, so a later `observe`/`find` can find it.
pub fn register_launched_port(pid: u32, port: u16) {
    if let Ok(mut map) = registry().lock() {
        map.insert(pid, port);
        tracing::info!(
            code = "A11Y_CDP_PORT_REGISTERED",
            pid,
            port,
            "registered Synapse-launched CDP debug port"
        );
    }
}

/// Removes a registered port (e.g. when the browser process exits).
pub fn forget_launched_port(pid: u32) {
    if let Ok(mut map) = registry().lock() {
        map.remove(&pid);
    }
}

/// The CDP debug port registered for `pid` by `act_launch`, if any.
#[must_use]
pub fn launched_port_for_pid(pid: u32) -> Option<u16> {
    registry()
        .lock()
        .ok()
        .and_then(|map| map.get(&pid).copied())
}

/// The ordered list of ports to probe for `pid`: the registered launched port
/// (if any) first, then the env-configured / default port list. De-duplicated,
/// order-preserving.
#[must_use]
pub fn candidate_ports_for_pid(pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Some(port) = launched_port_for_pid(pid) {
        ports.push(port);
    }
    for port in configured_ports() {
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

fn configured_ports() -> Vec<u16> {
    std::env::var(CDP_PORTS_ENV).map_or_else(
        |_| vec![DEFAULT_CDP_PORT],
        |raw| {
            let parsed: Vec<u16> = raw
                .split(',')
                .filter_map(|token| token.trim().parse::<u16>().ok())
                .filter(|port| *port != 0)
                .collect();
            if parsed.is_empty() {
                vec![DEFAULT_CDP_PORT]
            } else {
                parsed
            }
        },
    )
}

// === Probing ================================================================

fn endpoint_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn endpoints_for_ports(ports: &[u16]) -> Vec<String> {
    ports.iter().map(|port| endpoint_for_port(*port)).collect()
}

fn ok_diagnostics(process_name: &str, port: u16, checked_ports: Vec<u16>) -> CdpDiagnostics {
    CdpDiagnostics {
        process_name: process_name.to_owned(),
        status: CdpStatus::Ok,
        endpoint: Some(endpoint_for_port(port)),
        checked_endpoints: endpoints_for_ports(&checked_ports),
        checked_ports,
        reason_code: None,
        detail: None,
        capabilities: cdp_capabilities(),
        attached_node_count: None,
        selected_target_id: None,
        selected_session_id: None,
        target_selection_reason: None,
        target_candidate_count: None,
        frame_tree_frame_count: None,
        attached_frame_target_count: None,
        blocked_frame_targets: Vec::new(),
        frame_snapshot_errors: Vec::new(),
    }
}

/// Synchronous CDP reachability probe.
///
/// Used from the perception `platform_input` path so both `observe` and `find`
/// surface `cdp.status` without an async runtime. Connection-refused on loopback
/// returns immediately, so the common "no debug port" case costs microseconds,
/// not the full `connect_timeout`.
#[must_use]
pub fn probe_chromium_cdp_blocking(
    process_name: &str,
    ports: &[u16],
    connect_timeout: Duration,
) -> CdpDiagnostics {
    if !is_chromium_family(process_name) {
        return CdpDiagnostics::not_chromium(process_name);
    }
    let mut checked_ports = Vec::new();
    for port in ports {
        checked_ports.push(*port);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), *port);
        if TcpStream::connect_timeout(&addr, connect_timeout).is_ok() {
            return ok_diagnostics(process_name, *port, checked_ports);
        }
    }
    CdpDiagnostics::unreachable_with_probe(
        process_name,
        error_codes::A11Y_CDP_UNREACHABLE,
        checked_ports,
        CDP_UNREACHABLE_DETAIL,
    )
}

/// Async CDP reachability probe (used by tests and the async attach path).
pub async fn probe_chromium_cdp(
    process_name: &str,
    ports: &[u16],
    connect_timeout: Duration,
) -> CdpDiagnostics {
    if !is_chromium_family(process_name) {
        return CdpDiagnostics::not_chromium(process_name);
    }

    let mut checked_ports = Vec::new();
    for port in ports {
        checked_ports.push(*port);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), *port);
        if timeout(connect_timeout, TokioTcpStream::connect(addr))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            return ok_diagnostics(process_name, *port, checked_ports);
        }
    }

    CdpDiagnostics::unreachable_with_probe(
        process_name,
        error_codes::A11Y_CDP_UNREACHABLE,
        checked_ports,
        CDP_UNREACHABLE_DETAIL,
    )
}

/// Resolves a reachable CDP endpoint for the browser window `hwnd`.
///
/// Used by action routing (#686). Looks up the window's pid, gathers its
/// candidate debug ports (launched-port registry first, then defaults), keeps
/// only the ports that serve this window (#2409, [`ports_serving_window`]),
/// and returns the first reachable `http://127.0.0.1:<port>`. `None` if the
/// window is gone, not a Chromium browser, or has no reachable debug port of
/// its own.
#[cfg(windows)]
#[must_use]
pub fn endpoint_for_window(hwnd: i64) -> Option<String> {
    let context = crate::foreground_context(hwnd).ok()?;
    if !is_chromium_family(&context.process_name)
        && let Some(port) = webview2_debug_port_for_host(context.pid)
        && !ports_serving_window(context.pid, &[port]).is_empty()
    {
        let endpoint = endpoint_for_port(port);
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            return Some(endpoint);
        }
    }
    if !is_chromium_family(&context.process_name) {
        return None;
    }
    let ports = ports_serving_window(context.pid, &candidate_ports_for_pid(context.pid));
    probe_chromium_cdp_blocking(&context.process_name, &ports, Duration::from_millis(250)).endpoint
}

/// #2409: the ports of `ports`, in their order, that serve the window
/// `window_pid`: the process listening on the port is the window's own
/// process or one it started ([`EndpointOwner`]). A port held by another
/// browser is dropped, so a window never gets another browser's pages.
#[cfg(windows)]
#[must_use]
pub fn ports_serving_window(window_pid: u32, ports: &[u16]) -> Vec<u16> {
    let candidates: Vec<EndpointOwner> = ports
        .iter()
        .map(|port| {
            let port_owner = listening_port_owner(*port);
            let chain = port_owner.map_or_else(Vec::new, |owner| process_chain(owner, window_pid));
            EndpointOwner::new(*port, window_pid, port_owner, chain)
        })
        .collect();
    EndpointOwner::serving_ports(&candidates)
}

/// #2409: the one process listening for TCP on IPv4 `port` at 127.0.0.1 or
/// 0.0.0.0 (the listeners a probe of 127.0.0.1 reaches). `None` when nothing
/// listens there, when two processes do, or when the table cannot be read.
#[cfg(windows)]
fn listening_port_owner(port: u16) -> Option<u32> {
    use windows::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR},
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
        },
        Networking::WinSock::AF_INET,
    };

    // MIB_TCPTABLE_OWNER_PID: the row count, then rows of six u32 fields.
    const ROW_WORDS: usize = size_of::<MIB_TCPROW_OWNER_PID>() / 4;

    let mut size = 0_u32;
    let mut table: Vec<u32> = Vec::new();
    // The first call asks for the size; the table may grow before the next.
    for _ in 0..3 {
        let buffer = (!table.is_empty()).then_some(table.as_mut_ptr().cast::<std::ffi::c_void>());
        // SAFETY: `buffer` is None (size query) or points to `table`, which is
        // 4-byte aligned and holds at least `size` bytes; the call writes at
        // most `size` bytes there and sets `size` to the bytes it needs.
        let status = unsafe {
            GetExtendedTcpTable(
                buffer,
                &raw mut size,
                false,
                u32::from(AF_INET.0),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if status == NO_ERROR.0 {
            let entries = usize::try_from(*table.first()?).ok()?;
            let mut owners: Vec<u32> = table
                .get(1..)?
                .chunks_exact(ROW_WORDS)
                .take(entries)
                .filter_map(|row| {
                    let &[
                        _state,
                        address,
                        local_port,
                        _remote_address,
                        _remote_port,
                        pid,
                    ] = row
                    else {
                        return None;
                    };
                    let address = Ipv4Addr::from(address.to_ne_bytes());
                    // The port is in network byte order in the low 16 bits.
                    let [high, low, ..] = local_port.to_ne_bytes();
                    (u16::from_be_bytes([high, low]) == port
                        && (address == Ipv4Addr::LOCALHOST || address.is_unspecified()))
                    .then_some(pid)
                })
                .collect();
            owners.sort_unstable();
            owners.dedup();
            return match owners.as_slice() {
                [owner] => Some(*owner),
                _ => None,
            };
        }
        if status != ERROR_INSUFFICIENT_BUFFER.0 {
            return None;
        }
        table = vec![0_u32; usize::try_from(size).ok()?.div_ceil(4)];
    }
    None
}

/// #2409: the port owner and its ancestors, nearest first, each with its
/// parent and exact creation time. Stops after the window's own process, at a
/// process that cannot be read, or after [`EndpointOwner::MAX_CHAIN_STEPS`]
/// steps, so a cycle in the parent table cannot hang it.
#[cfg(windows)]
fn process_chain(owner_pid: u32, window_pid: u32) -> Vec<ProcessStep> {
    let mut chain = Vec::new();
    let mut pid = owner_pid;
    while chain.len() < EndpointOwner::MAX_CHAIN_STEPS {
        let Some((parent_pid, created)) = crate::launch_process_record(pid) else {
            break;
        };
        chain.push(ProcessStep::new(pid, parent_pid, created));
        if pid == window_pid {
            break;
        }
        pid = parent_pid;
    }
    chain
}

/// Probe only the debug endpoint owned by a WebView2 host window. The host
/// image itself is not Chromium, so the ordinary browser-name gate cannot be
/// used for perception diagnostics.
#[cfg(windows)]
#[must_use]
pub fn webview2_cdp_diagnostics_for_window(hwnd: i64) -> Option<CdpDiagnostics> {
    let context = crate::foreground_context(hwnd).ok()?;
    if is_chromium_family(&context.process_name) {
        return None;
    }
    let Some(port) =
        webview2_browser_port_for_host(context.pid, WebView2HostScope::DirectChildren)?
    else {
        return Some(CdpDiagnostics::unreachable_with_probe(
            context.process_name,
            error_codes::A11Y_CDP_UNREACHABLE,
            Vec::new(),
            "WebView2 browser child has no --remote-debugging-port; UIA may expose only part of the page. Launch this app through Synapse process launch with cdp_debug=true to enable complete page observation and background input",
        ));
    };
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    if !ports_serving_window(context.pid, &[port]).is_empty()
        && TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
    {
        Some(ok_diagnostics(&context.process_name, port, vec![port]))
    } else {
        Some(CdpDiagnostics::unreachable_with_probe(
            context.process_name,
            error_codes::A11Y_CDP_UNREACHABLE,
            vec![port],
            CDP_UNREACHABLE_DETAIL,
        ))
    }
}

/// Extract the loopback CDP port from the browser process directly owned by a
/// WebView2 host. Child renderer/utility processes are excluded by their type.
#[cfg(windows)]
#[must_use]
pub fn webview2_browser_debug_port(process_name: &str, arguments: &[String]) -> Option<u16> {
    if !process_name.eq_ignore_ascii_case("msedgewebview2.exe")
        || arguments.iter().any(|arg| arg.starts_with("--type="))
    {
        return None;
    }
    arguments.iter().find_map(|arg| {
        arg.strip_prefix("--remote-debugging-port=")
            .and_then(|port| port.trim_matches('"').parse::<u16>().ok())
            .filter(|port| *port != 0)
    })
}

#[cfg(windows)]
#[must_use]
pub fn webview2_debug_port_for_host(host_pid: u32) -> Option<u16> {
    webview2_browser_port_for_host(host_pid, WebView2HostScope::DirectChildren).flatten()
}

/// Debug port of the WebView2 browser anywhere in the process tree of a process
/// Synapse launched itself (a wrapper launcher may start the real host).
#[cfg(windows)]
#[must_use]
pub fn webview2_debug_port_for_launched_tree(launched_pid: u32) -> Option<u16> {
    webview2_browser_port_for_host(launched_pid, WebView2HostScope::LaunchedTree).flatten()
}

// `None` = no WebView2 browser child, `Some(None)` = child without a debug port,
// `Some(Some(port))` = child with a port; callers distinguish all three cases.
#[cfg(windows)]
#[allow(clippy::option_option)]
fn webview2_browser_port_for_host(host_pid: u32, scope: WebView2HostScope) -> Option<Option<u16>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let rows = system
        .processes()
        .values()
        .map(|process| {
            let name = process.name().to_string_lossy().into_owned();
            let arguments = if name.eq_ignore_ascii_case("msedgewebview2.exe") {
                process
                    .cmd()
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect()
            } else {
                Vec::new()
            };
            WebView2ProcessRow {
                pid: process.pid().as_u32(),
                parent_pid: process.parent().map(Pid::as_u32),
                name,
                arguments,
            }
        })
        .collect::<Vec<_>>();
    webview2_browser_port_in_tree(host_pid, scope, &rows)
}

/// Native client-area origin in physical screen pixels. For WebView2 hosts the
/// page viewport starts at the client area, below the non-client title bar.
#[cfg(windows)]
#[must_use]
pub fn client_origin_screen(hwnd: i64) -> Option<synapse_core::Point> {
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::ClientToScreen,
    };

    let mut origin = POINT { x: 0, y: 0 };
    // SAFETY: the Win32 API writes one caller-owned POINT, and an invalid HWND
    // returns false without dereferencing an invalid pointer.
    if unsafe { ClientToScreen(HWND(hwnd as *mut c_void), &raw mut origin) }.as_bool() {
        Some(synapse_core::Point {
            x: origin.x,
            y: origin.y,
        })
    } else {
        None
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub struct CdpAttachment {
    pub browser: chromiumoxide::Browser,
    pub handler: chromiumoxide::Handler,
    pub endpoint: String,
}

/// Drives a chromiumoxide connection handler and aborts it when dropped, so a
/// caller cancelled by a timeout (or an early `?`) never leaks the handler task
/// and its websocket (#210). `abort` stays available for explicit early stops.
pub struct CdpHandlerTask(tokio::task::JoinHandle<()>);

impl CdpHandlerTask {
    #[must_use]
    pub fn spawn(mut handler: chromiumoxide::Handler) -> Self {
        use futures_util::StreamExt as _;
        Self(tokio::spawn(async move {
            while handler.next().await.is_some() {}
        }))
    }

    pub fn abort(&self) {
        self.0.abort();
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    /// Test hook: guards an arbitrary task the way `spawn` guards a handler.
    #[cfg(test)]
    pub(crate) const fn from_task(task: tokio::task::JoinHandle<()>) -> Self {
        Self(task)
    }
}

#[cfg(test)]
#[path = "tests/cdp_handler_task_210.rs"]
mod cdp_handler_task_210_tests;

#[cfg(all(test, windows))]
#[path = "tests/endpoint_owner_os_2409.rs"]
mod endpoint_owner_os_2409_tests;

impl Drop for CdpHandlerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Attaches a `chromiumoxide` browser client to a reachable CDP endpoint.
///
/// # Errors
///
/// Returns `A11Y_CDP_UNREACHABLE` when `chromiumoxide` cannot connect to the
/// supplied endpoint and `A11Y_CDP_ATTACH_FAILED` when the endpoint accepts the
/// connection but does not finish the handshake within [`CDP_ATTACH_TIMEOUT`].
#[cfg(windows)]
pub async fn attach_chromiumoxide(endpoint: &str) -> A11yResult<CdpAttachment> {
    attach_chromiumoxide_with_timeout(endpoint, CDP_ATTACH_TIMEOUT).await
}

/// Upper bound for the `/json/version` request plus websocket handshake of one
/// CDP attach (#210). A hung browser must not hang its callers forever.
#[cfg(windows)]
pub const CDP_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// [`attach_chromiumoxide`] with an explicit handshake bound.
///
/// # Errors
///
/// As [`attach_chromiumoxide`]; the timeout is `A11Y_CDP_ATTACH_FAILED`, not
/// "unreachable", so cleanup callers keep their state and retry.
#[cfg(windows)]
pub async fn attach_chromiumoxide_with_timeout(
    endpoint: &str,
    limit: Duration,
) -> A11yResult<CdpAttachment> {
    let (browser, handler) = match timeout(limit, chromiumoxide::Browser::connect(endpoint)).await {
        Ok(Ok(connected)) => connected,
        Ok(Err(err)) => {
            return Err(A11yError::CdpUnreachable {
                detail: err.to_string(),
            });
        }
        Err(_elapsed) => {
            return Err(A11yError::CdpAttachFailed {
                detail: format!(
                    "CDP connect to {endpoint:?} timed out after {} ms",
                    limit.as_millis()
                ),
            });
        }
    };
    Ok(CdpAttachment {
        browser,
        handler,
        endpoint: endpoint.to_owned(),
    })
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct CdpTargetSummary {
    pub target_id: String,
    pub target_type: String,
    pub title: String,
    pub url: String,
    pub attached: bool,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct CdpOpenTabResult {
    pub target: CdpTargetSummary,
    pub target_count_before: u32,
    pub target_count_after: u32,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct CdpCloseTabResult {
    pub target_id: String,
    pub target_count_before: u32,
    pub target_count_after: u32,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct CdpActivateTabResult {
    pub target_id: String,
    pub title: String,
    pub url: String,
}

/// Reads the current CDP `Target.getTargets` table for `endpoint`.
///
/// This is the physical Source of Truth for tab-target lifecycle checks; callers
/// should inspect it separately from any create/close return value.
///
/// # Errors
///
/// Returns `A11Y_CDP_UNREACHABLE` when the endpoint cannot be connected and
/// `A11Y_CDP_ATTACH_FAILED` when `Target.getTargets` itself fails.
#[cfg(windows)]
pub async fn cdp_list_targets(endpoint: &str) -> A11yResult<Vec<CdpTargetSummary>> {
    let CdpAttachment {
        browser, handler, ..
    } = attach_chromiumoxide(endpoint).await?;
    let handler_task = crate::cdp::CdpHandlerTask::spawn(handler);
    let result = cdp_list_targets_with_browser(&browser).await;
    handler_task.abort();
    result
}

/// Opens a visible background tab with `Target.createTarget(background=true)`,
/// then reads `Target.getTargets` until the returned target id is present.
///
/// # Errors
///
/// Returns fail-loud CDP errors when the endpoint is unreachable, the protocol
/// command fails, or the target does not appear in the target table.
#[cfg(windows)]
pub async fn cdp_open_background_tab(endpoint: &str, url: &str) -> A11yResult<CdpOpenTabResult> {
    use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;

    let CdpAttachment {
        browser, handler, ..
    } = attach_chromiumoxide(endpoint).await?;
    let handler_task = crate::cdp::CdpHandlerTask::spawn(handler);

    let result = async {
        let before = cdp_list_targets_with_browser(&browser).await?;
        let params = CreateTargetParams::builder()
            .url(url)
            .new_window(false)
            .background(true)
            .build()
            .map_err(|error| A11yError::CdpAxtreeFailed {
                detail: format!("Target.createTarget params: {error}"),
            })?;
        let created =
            browser
                .execute(params)
                .await
                .map_err(|error| A11yError::CdpAxtreeFailed {
                    detail: format!("Target.createTarget(background=true): {error}"),
                })?;
        let target_id = created.result.target_id.inner().clone();
        let after = wait_for_target_present(&browser, &target_id).await?;
        let Some(target) = after
            .iter()
            .find(|target| target.target_id == target_id)
            .cloned()
        else {
            return Err(A11yError::CdpAxtreeFailed {
                detail: format!(
                    "Target.createTarget returned {target_id:?}, but Target.getTargets readback did not contain it"
                ),
            });
        };
        Ok(CdpOpenTabResult {
            target,
            target_count_before: u32::try_from(before.len()).unwrap_or(u32::MAX),
            target_count_after: u32::try_from(after.len()).unwrap_or(u32::MAX),
        })
    }
    .await;

    handler_task.abort();
    result
}

#[cfg(windows)]
const CLOSE_TARGET_ABSENT_DETAIL_PREFIX: &str =
    "Target.closeTarget refused: Target.getTargets readback did not contain target_id";

/// The error [`cdp_close_target`] returns when `Target.getTargets` does not
/// list `target_id` before the close (the tab is already gone, e.g. the user
/// closed it). Callers that only need the tab gone use
/// [`is_cdp_close_target_absent`] to treat this as success (#211).
#[cfg(windows)]
#[must_use]
pub fn cdp_close_target_absent_error(target_id: &str) -> A11yError {
    A11yError::CdpAxtreeFailed {
        detail: format!("{CLOSE_TARGET_ABSENT_DETAIL_PREFIX} {target_id:?} before close"),
    }
}

/// Whether `error` is the "target absent before close" failure from
/// [`cdp_close_target`].
#[cfg(windows)]
#[must_use]
pub fn is_cdp_close_target_absent(error: &A11yError) -> bool {
    matches!(
        error,
        A11yError::CdpAxtreeFailed { detail }
            if detail.starts_with(CLOSE_TARGET_ABSENT_DETAIL_PREFIX)
    )
}

/// Closes `target_id` with `Target.closeTarget`, then reads `Target.getTargets`
/// until the target is absent.
///
/// # Errors
///
/// Returns fail-loud CDP errors when the endpoint is unreachable, the target was
/// absent before close, the protocol command fails, or the target remains after
/// the close command.
#[cfg(windows)]
pub async fn cdp_close_target(endpoint: &str, target_id: &str) -> A11yResult<CdpCloseTabResult> {
    use chromiumoxide::cdp::browser_protocol::target::CloseTargetParams;

    let CdpAttachment {
        browser, handler, ..
    } = attach_chromiumoxide(endpoint).await?;
    let handler_task = crate::cdp::CdpHandlerTask::spawn(handler);

    let result = async {
        let before = cdp_list_targets_with_browser(&browser).await?;
        if !before.iter().any(|target| target.target_id == target_id) {
            return Err(cdp_close_target_absent_error(target_id));
        }
        browser
            .execute(CloseTargetParams::new(target_id.to_owned()))
            .await
            .map_err(|error| A11yError::CdpAxtreeFailed {
                detail: format!("Target.closeTarget({target_id:?}): {error}"),
            })?;
        let after = wait_for_target_absent(&browser, target_id).await?;
        Ok(CdpCloseTabResult {
            target_id: target_id.to_owned(),
            target_count_before: u32::try_from(before.len()).unwrap_or(u32::MAX),
            target_count_after: u32::try_from(after.len()).unwrap_or(u32::MAX),
        })
    }
    .await;

    handler_task.abort();
    result
}

/// Brings a raw-CDP page target to the front of its automation browser via
/// `Target.activateTarget` (the CDP-level analogue of selecting the tab). This
/// is for Synapse-launched automation profiles; it does not seize the human OS
/// foreground. Background-safe tab activation (#1189).
#[cfg(windows)]
pub async fn cdp_activate_target(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpActivateTabResult> {
    use chromiumoxide::cdp::browser_protocol::target::ActivateTargetParams;

    let CdpAttachment {
        browser, handler, ..
    } = attach_chromiumoxide(endpoint).await?;
    let handler_task = crate::cdp::CdpHandlerTask::spawn(handler);

    let result = async {
        let targets = cdp_list_targets_with_browser(&browser).await?;
        let Some(summary) = targets
            .into_iter()
            .find(|target| target.target_id == target_id)
        else {
            return Err(A11yError::CdpAxtreeFailed {
                detail: format!(
                    "Target.activateTarget refused: Target.getTargets readback did not contain target_id {target_id:?}"
                ),
            });
        };
        browser
            .execute(ActivateTargetParams::new(target_id.to_owned()))
            .await
            .map_err(|error| A11yError::CdpAxtreeFailed {
                detail: format!("Target.activateTarget({target_id:?}): {error}"),
            })?;
        Ok(CdpActivateTabResult {
            target_id: target_id.to_owned(),
            title: summary.title,
            url: summary.url,
        })
    }
    .await;

    handler_task.abort();
    result
}

#[cfg(windows)]
async fn cdp_list_targets_with_browser(
    browser: &chromiumoxide::Browser,
) -> A11yResult<Vec<CdpTargetSummary>> {
    use chromiumoxide::cdp::browser_protocol::target::GetTargetsParams;

    let targets = browser
        .execute(GetTargetsParams::default())
        .await
        .map_err(|error| A11yError::CdpAttachFailed {
            detail: format!("Target.getTargets: {error}"),
        })?
        .result
        .target_infos
        .into_iter()
        .map(|target| CdpTargetSummary {
            target_id: target.target_id.inner().clone(),
            target_type: target.r#type,
            title: target.title,
            url: target.url,
            attached: target.attached,
        })
        .collect();
    Ok(targets)
}

#[cfg(windows)]
async fn wait_for_target_present(
    browser: &chromiumoxide::Browser,
    target_id: &str,
) -> A11yResult<Vec<CdpTargetSummary>> {
    let mut last = Vec::new();
    for _ in 0..30 {
        last = cdp_list_targets_with_browser(browser).await?;
        if last.iter().any(|target| target.target_id == target_id) {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(A11yError::CdpAxtreeFailed {
        detail: format!(
            "target_id {target_id:?} did not appear in Target.getTargets within 3s; last target ids: {}",
            target_ids_for_error(&last)
        ),
    })
}

#[cfg(windows)]
async fn wait_for_target_absent(
    browser: &chromiumoxide::Browser,
    target_id: &str,
) -> A11yResult<Vec<CdpTargetSummary>> {
    let mut last = Vec::new();
    for _ in 0..30 {
        last = cdp_list_targets_with_browser(browser).await?;
        if !last.iter().any(|target| target.target_id == target_id) {
            return Ok(last);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(A11yError::CdpAxtreeFailed {
        detail: format!(
            "target_id {target_id:?} remained in Target.getTargets after close for 3s; last target ids: {}",
            target_ids_for_error(&last)
        ),
    })
}

#[cfg(windows)]
fn target_ids_for_error(targets: &[CdpTargetSummary]) -> String {
    targets
        .iter()
        .map(|target| target.target_id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
