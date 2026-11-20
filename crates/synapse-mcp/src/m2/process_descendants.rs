//! Process-tree membership for related-window verification (#92).
//!
//! A Tauri/WebView2 app opens DevTools as a separate, unowned top-level window
//! that belongs to a child process (`msedgewebview2.exe`) of the target, so the
//! owner-chain rule cannot relate it. Descendant membership is computed from a
//! process snapshot with a PID-reuse guard (a child must not predate its
//! parent) and a bounded depth.

use std::collections::{HashMap, HashSet, VecDeque};

/// How many parent-to-child hops below the target still count as its tree.
pub(crate) const MAX_DESCENDANT_DEPTH: usize = 4;

/// Shared shell / broker hosts: nearly every user app descends from these, so
/// their process tree says nothing about whether a window belongs to the target.
const SHARED_HOST_IMAGES: &[&str] = &[
    "explorer.exe",
    "applicationframehost.exe",
    "svchost.exe",
    "dllhost.exe",
    "runtimebroker.exe",
    "sihost.exe",
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcessNode {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Process creation time (any consistent monotonic unit).
    pub start_time: u64,
}

/// True when `process_name` (image name or path) is a shared shell/broker host.
pub(crate) fn is_shared_host_process(process_name: &str) -> bool {
    let name = process_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(process_name)
        .trim();
    SHARED_HOST_IMAGES
        .iter()
        .any(|image| name.eq_ignore_ascii_case(image))
}

/// PIDs below `root` (excluding `root`) reachable within `max_depth` hops where
/// every child started no earlier than its parent. Cycle-safe.
pub(crate) fn descendant_pids(nodes: &[ProcessNode], root: u32, max_depth: usize) -> Vec<u32> {
    let by_pid: HashMap<u32, &ProcessNode> = nodes.iter().map(|node| (node.pid, node)).collect();
    let Some(root_node) = by_pid.get(&root) else {
        return Vec::new();
    };
    let mut children: HashMap<u32, Vec<&ProcessNode>> = HashMap::new();
    for node in nodes {
        if let Some(parent) = node.parent {
            children.entry(parent).or_default().push(node);
        }
    }
    let mut seen = HashSet::from([root]);
    let mut found = Vec::new();
    let mut queue = VecDeque::from([(*root_node, 0_usize)]);
    while let Some((parent, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for &child in children.get(&parent.pid).into_iter().flatten() {
            // A child older than its recorded parent means the parent PID was
            // reused: the real parent is gone, so this is not our descendant.
            if child.start_time < parent.start_time || !seen.insert(child.pid) {
                continue;
            }
            found.push(child.pid);
            queue.push_back((child, depth + 1));
        }
    }
    found
}

/// Live descendants of `root` from a fresh process snapshot. Empty off Windows.
#[cfg(windows)]
pub(crate) fn live_descendant_pids(root: u32) -> Vec<u32> {
    use sysinfo::{ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let nodes: Vec<ProcessNode> = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessNode {
            pid: pid.as_u32(),
            parent: process.parent().map(sysinfo::Pid::as_u32),
            start_time: process.start_time(),
        })
        .collect();
    descendant_pids(&nodes, root, MAX_DESCENDANT_DEPTH)
}

#[cfg(not(windows))]
pub(crate) fn live_descendant_pids(_root: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(test)]
#[path = "tests/process_descendants_92.rs"]
mod process_descendants_92;
