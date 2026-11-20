//! #2347 (roadmap item #571): how deep one window snapshot walks. The browser
//! frame is counted from the window, the page from its document node, and the
//! search for that document node has its own fixed reach. Pure value object;
//! the UIA walk stays in `snapshot_content_root`.

#[cfg(test)]
#[path = "snapshot_depth_table_2347.rs"]
mod snapshot_depth_table_2347;

/// Deepest level any snapshot walks: the depth `find` and `observe` with
/// `interactable` ask for (`MAX_PUBLIC_A11Y_SNAPSHOT_DEPTH` in `synapse-mcp`).
const MAX_WALK_DEPTH: u32 = 16;

/// The depth one snapshot was asked for, and the three walk depths it gives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SnapshotDepth {
    requested: u32,
}

impl SnapshotDepth {
    /// Takes the depth the caller asked for (any value; the methods cap it).
    pub(super) const fn new(requested: u32) -> Self {
        Self { requested }
    }

    /// Deepest level the walk from the window root reaches.
    pub(super) const fn for_window_walk(self) -> u32 {
        if self.requested < MAX_WALK_DEPTH {
            self.requested
        } else {
            MAX_WALK_DEPTH
        }
    }

    /// Levels the breadth-first search for the page's document node may go
    /// down, whatever depth the caller asked for (its own 256-visit and
    /// 100 ms limits still apply).
    #[allow(clippy::unused_self)] // the reach is fixed on purpose (#2347)
    pub(super) const fn for_content_root_search(self) -> u32 {
        MAX_WALK_DEPTH
    }

    /// Deepest level the page part is collected to, when the page's document
    /// node sits at level `node_depth`: the requested depth below the
    /// document, never deeper than `MAX_WALK_DEPTH` (so `find` is unchanged).
    pub(super) const fn below_content_root(self, node_depth: u32) -> u32 {
        let limit = node_depth.saturating_add(self.for_window_walk());
        if limit < MAX_WALK_DEPTH {
            limit
        } else {
            MAX_WALK_DEPTH
        }
    }
}
