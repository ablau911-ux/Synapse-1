//! #2409: whether a debug port belongs to a window. A port serves a window
//! only when the process listening on it is the window's own process or one
//! that process started (a WebView2 app's browser process is its child).
//! Pure value objects: the OS reads (who listens on the port, the parent
//! chain with creation times) are done once by `cdp` and passed in.

#[cfg(test)]
#[path = "endpoint_owner_table_2409.rs"]
mod endpoint_owner_table_2409;

/// One process of a parent chain, as the OS layer read it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessStep {
    pid: u32,
    parent_pid: u32,
    /// Creation time (Windows FILETIME ticks); a parent is never created
    /// after its child.
    created: u64,
}

impl ProcessStep {
    #[must_use]
    pub const fn new(pid: u32, parent_pid: u32, created: u64) -> Self {
        Self {
            pid,
            parent_pid,
            created,
        }
    }
}

/// One candidate debug port of a window, with who listens on it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointOwner {
    port: u16,
    window_pid: u32,
    /// The process listening on `port`; `None` when it cannot be read.
    port_owner: Option<u32>,
    /// The port owner first, then its parent, its grandparent, ...
    chain: Vec<ProcessStep>,
}

impl EndpointOwner {
    /// The window's process must be one of the first this many steps of the
    /// chain (row P7); the OS reader reads no more than this.
    pub const MAX_CHAIN_STEPS: usize = 64;

    #[must_use]
    pub const fn new(
        port: u16,
        window_pid: u32,
        port_owner: Option<u32>,
        chain: Vec<ProcessStep>,
    ) -> Self {
        Self {
            port,
            window_pid,
            port_owner,
            chain,
        }
    }

    /// True only when the port owner is the window's process or a descendant
    /// of it, each step created no later than its child.
    #[must_use]
    pub fn serves_window(&self) -> bool {
        let Some(owner) = self.port_owner else {
            return false;
        };
        if owner == 0 || self.window_pid == 0 {
            return false;
        }
        if owner == self.window_pid {
            return true;
        }
        let mut expected_pid = owner;
        let mut child_created: Option<u64> = None;
        for step in self.chain.iter().take(Self::MAX_CHAIN_STEPS) {
            if step.pid != expected_pid
                || child_created.is_some_and(|created| step.created > created)
            {
                return false;
            }
            if step.pid == self.window_pid {
                return true;
            }
            expected_pid = step.parent_pid;
            child_created = Some(step.created);
        }
        false
    }

    /// The ports of `candidates` that serve the window, in the given order.
    #[must_use]
    pub fn serving_ports(candidates: &[Self]) -> Vec<u16> {
        candidates
            .iter()
            .filter(|candidate| candidate.serves_window())
            .map(|candidate| candidate.port)
            .collect()
    }
}
