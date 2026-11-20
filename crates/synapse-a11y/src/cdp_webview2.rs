//! WebView2 pure decisions (#79, #293): which debug port belongs to a host
//! process, and which page target a claimed WebView2 window drives. The OS
//! process read and the CDP calls stay in `cdp`.

use synapse_core::error_codes;

use super::{CdpTargetSummary, webview2_browser_debug_port};

#[cfg(test)]
#[path = "cdp_webview2_table_79.rs"]
// The frozen table's row tuples are its own; it is never edited.
#[allow(clippy::type_complexity)]
mod cdp_webview2_table_79;

/// One process from the OS process table. `arguments` may be left empty for
/// anything that is not `msedgewebview2.exe`.
#[derive(Clone, Debug)]
pub struct WebView2ProcessRow {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub arguments: Vec<String>,
}

/// Which processes count as the host's own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebView2HostScope {
    /// The direct children of a window's owner process.
    DirectChildren,
    /// The process tree of a process Synapse launched itself (wrapper launchers).
    LaunchedTree,
}

/// How deep a launched process tree is searched; bounds a corrupt parent table.
const MAX_LAUNCHED_TREE_DEPTH: usize = 8;

/// Debug port of the WebView2 browser process owned by `host_pid`.
/// `None` = no WebView2 browser process, `Some(None)` = browser process without
/// a debug port, `Some(Some(port))` = browser process with a port. The nearest
/// browser wins, then the lowest pid. Pid 0 is never a host: it would match
/// every top-level process on the machine.
#[must_use]
#[allow(clippy::option_option)]
pub fn webview2_browser_port_in_tree(
    host_pid: u32,
    scope: WebView2HostScope,
    processes: &[WebView2ProcessRow],
) -> Option<Option<u16>> {
    if host_pid == 0 {
        return None;
    }
    let max_depth = match scope {
        WebView2HostScope::DirectChildren => 1,
        WebView2HostScope::LaunchedTree => MAX_LAUNCHED_TREE_DEPTH,
    };
    let mut visited = std::collections::HashSet::from([host_pid]);
    let mut level = vec![host_pid];
    let mut saw_browser = false;
    for _ in 0..max_depth {
        let mut children: Vec<&WebView2ProcessRow> = Vec::new();
        for process in processes {
            if process
                .parent_pid
                .is_some_and(|parent| level.contains(&parent))
                && visited.insert(process.pid)
            {
                children.push(process);
            }
        }
        children.sort_by_key(|process| process.pid);
        for process in &children {
            if !process.name.eq_ignore_ascii_case("msedgewebview2.exe")
                || process
                    .arguments
                    .iter()
                    .any(|arg| arg.starts_with("--type="))
            {
                continue;
            }
            saw_browser = true;
            if let Some(port) = webview2_browser_debug_port(&process.name, &process.arguments) {
                return Some(Some(port));
            }
        }
        if children.is_empty() {
            break;
        }
        level = children.iter().map(|process| process.pid).collect();
    }
    saw_browser.then_some(None)
}

/// The one CDP page target a claimed WebView2 window drives: the requested
/// target when it is a page, else the only page. A request for another window
/// is refused.
///
/// # Errors
///
/// `ACTION_TARGET_INVALID` for a null claim, a different requested window, an
/// unknown or non-page requested target, no page, or several pages and no
/// request.
pub fn resolve_owned_webview2_page_target(
    claimed_hwnd: i64,
    requested_hwnd: Option<i64>,
    requested_target_id: Option<&str>,
    targets: &[CdpTargetSummary],
) -> Result<String, &'static str> {
    if claimed_hwnd == 0 || requested_hwnd.is_some_and(|hwnd| hwnd != claimed_hwnd) {
        return Err(error_codes::ACTION_TARGET_INVALID);
    }
    let mut pages = targets.iter().filter(|target| target.target_type == "page");
    let page = match requested_target_id {
        Some(target_id) => pages.find(|target| target.target_id == target_id),
        None => pages.next().filter(|_| pages.next().is_none()),
    };
    page.map(|target| target.target_id.clone())
        .ok_or(error_codes::ACTION_TARGET_INVALID)
}
