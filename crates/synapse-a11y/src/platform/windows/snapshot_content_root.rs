//! #257: snapshot coverage bookkeeping and Gecko web-content root selection.
//!
//! Firefox exposes browser chrome before the web-content Document, so one
//! depth-first walk can spend its whole node budget / deadline on chrome and
//! never reach the page. For Gecko windows a unique visible (non-offscreen)
//! Document is collected first: root, ancestor path, Document subtree; the rest
//! of the window follows with the remaining budget. Every snapshot records the
//! first limit that made it incomplete and which subtrees were cut.

use std::{collections::HashMap, time::Instant};

use synapse_core::{
    AccessibleNode, AccessibleSubtree, ElementId, SnapshotScope, SnapshotStopReason,
};
use uiautomation::{UIElement, types::ControlType};

use crate::A11yResult;

use super::{
    SNAPSHOT_DEADLINE, SNAPSHOT_NODE_BUDGET, SnapshotWalk, collect_child_nodes, collect_walk_node,
    depth_cut_hides_children, has_child_elements, node_from_cached_element,
    snapshot_depth::SnapshotDepth, walker_reached_end,
};

/// Cap on reported `skipped_subtree_roots`.
const MAX_SKIPPED_SUBTREE_ROOTS: usize = 32;
/// Cap on elements visited while searching for the content Document.
const DISCOVERY_VISIT_BUDGET: usize = 256;
/// Discovery may use at most this share of `SNAPSHOT_DEADLINE` (r2): when it
/// runs out it selects nothing and the walk keeps the remaining main deadline.
const DISCOVERY_DEADLINE_DIVISOR: u32 = 4;
/// The selected Document subtree may use at most 3/4 of the snapshot budget,
/// so the rest of the window always keeps some coverage.
const CONTENT_ROOT_NODE_BUDGET: usize = SNAPSHOT_NODE_BUDGET / 4 * 3;
const GECKO_WINDOW_CLASS: &str = "MozillaWindowClass";
const GECKO_FRAMEWORK_ID: &str = "Gecko";

/// Snapshot entry scope. Content-root discovery runs only for `Window`
/// (`snapshot_window_from_hwnd`); an explicit element scope walks plainly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SnapshotRoot {
    Window,
    Element,
}

/// Whether a walked element was already collected by the content-root pass.
pub(super) enum PriorNode {
    New,
    /// Ancestor on the root -> Document path: reuse this node index.
    Ancestor(usize),
    /// The selected Document: its subtree is already collected.
    ContentRoot,
}

/// Mutable coverage state threaded through one snapshot walk.
#[derive(Default)]
pub(super) struct WalkState {
    pub(super) truncated: bool,
    stop_reason: Option<SnapshotStopReason>,
    skipped_roots: Vec<ElementId>,
    collected_ancestors: HashMap<ElementId, usize>,
    content_root: Option<ElementId>,
}

impl WalkState {
    /// Marks the tree incomplete; the first recorded reason wins.
    pub(super) fn record(&mut self, reason: SnapshotStopReason) {
        self.truncated = true;
        if self.stop_reason.is_none() {
            self.stop_reason = Some(reason);
        }
    }

    /// Records a limit that cut the children of `root` (deduplicated, capped).
    pub(super) fn record_skipped(&mut self, reason: SnapshotStopReason, root: Option<&ElementId>) {
        self.record(reason);
        let Some(root) = root else {
            return;
        };
        let full = self.skipped_roots.len() >= MAX_SKIPPED_SUBTREE_ROOTS;
        if !full && !self.skipped_roots.contains(root) {
            self.skipped_roots.push(root.clone());
        }
    }

    pub(super) fn prior(&self, id: &ElementId) -> PriorNode {
        if self.content_root.as_ref() == Some(id) {
            return PriorNode::ContentRoot;
        }
        self.collected_ancestors
            .get(id)
            .map_or(PriorNode::New, |index| PriorNode::Ancestor(*index))
    }

    pub(super) fn into_subtree(
        self,
        root: ElementId,
        nodes: Vec<AccessibleNode>,
        max_depth: u32,
    ) -> AccessibleSubtree {
        AccessibleSubtree {
            root,
            nodes,
            max_depth,
            truncated: self.truncated,
            completeness_scope: self.content_root.as_ref().map(|_| SnapshotScope::Document),
            selected_content_root: self.content_root,
            stop_reason: self.stop_reason,
            skipped_subtree_roots: self.skipped_roots,
        }
    }
}

/// Log label for a stop reason (matches the wire spelling).
pub(super) const fn stop_reason_label(reason: SnapshotStopReason) -> &'static str {
    match reason {
        SnapshotStopReason::NodeBudget => "node_budget",
        SnapshotStopReason::Deadline => "deadline",
        SnapshotStopReason::DepthLimit => "depth_limit",
        SnapshotStopReason::ProviderError => "provider_error",
    }
}

/// First collection limit reached, if any (node budget before deadline).
pub(super) fn collection_limit(
    nodes_len: usize,
    node_budget: usize,
    now: Instant,
    deadline: Instant,
) -> Option<SnapshotStopReason> {
    if nodes_len >= node_budget {
        Some(SnapshotStopReason::NodeBudget)
    } else if now >= deadline {
        Some(SnapshotStopReason::Deadline)
    } else {
        None
    }
}

/// Test entry: one standalone walk reporting coverage as a plain flag.
#[cfg(test)]
pub(super) fn collect_nodes(
    walk: &SnapshotWalk<'_>,
    element: &UIElement,
    parent: Option<ElementId>,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    truncated: &mut bool,
) -> A11yResult<()> {
    let mut state = WalkState::default();
    let collected = collect_walk_node(walk, element, parent, depth, max_depth, nodes, &mut state);
    *truncated |= state.truncated;
    collected.map(|_| ())
}

pub(super) fn count_child(nodes: &mut [AccessibleNode], parent_index: usize) {
    if let Some(parent) = nodes.get_mut(parent_index) {
        parent.children_count = parent.children_count.saturating_add(1);
    }
}

/// One snapshot walk. The root node is collected first and is never subject
/// to the entry limit (#257 r2), so a slow discovery or walk still returns
/// `nodes[0] = root`. Window scopes then try content-root selection.
/// #2347: the document search has its own reach, the page part is collected
/// `max_depth` levels below the document, the rest of the window `max_depth`
/// levels below the root (`SnapshotDepth`).
pub(super) fn walk_snapshot(
    walk: &SnapshotWalk<'_>,
    root: &UIElement,
    max_depth: u32,
    kind: SnapshotRoot,
) -> A11yResult<AccessibleSubtree> {
    let depth = SnapshotDepth::new(max_depth);
    let window_depth = depth.for_window_walk();
    let mut nodes = Vec::new();
    let mut state = WalkState::default();
    let root_node = node_from_cached_element(root, None, 0, walk.root_hwnd, 0)?;
    let (root_id, root_index) = push_ancestor(&mut nodes, &mut state, root_node);
    if kind == SnapshotRoot::Window
        && let Some(path) = find_content_root(walk, root, depth.for_content_root_search())
    {
        collect_content_root(walk, &path, depth, &mut nodes, &mut state);
    }
    descend(
        walk,
        root,
        &root_id,
        root_index,
        0,
        window_depth,
        &mut nodes,
        &mut state,
    );
    // #2347: the path to the page and the page itself may sit below the
    // requested depth. Report them at that depth, as the Chromium renderer
    // supplement and the CDP web nodes do, so the observe element depth filter
    // keeps them; parent links keep the true tree. No window-walk node is
    // deeper than `window_depth`, so this changes only page-part nodes.
    for node in &mut nodes {
        node.depth = node.depth.min(window_depth);
    }
    Ok(state.into_subtree(root_id, nodes, window_depth))
}

/// Visits the children of an already collected node: depth cut, collection
/// limits, then the child walk. Shared by the root and every walked node.
// Same walk context as `collect_child_nodes`; same justified arg count.
#[allow(clippy::too_many_arguments)]
pub(super) fn descend(
    walk: &SnapshotWalk<'_>,
    element: &UIElement,
    node_id: &ElementId,
    node_index: usize,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) {
    if depth >= max_depth {
        // The requested depth hides this node's children: say so instead of
        // reporting an untruncated tree (#77).
        if !state.truncated
            && depth_cut_hides_children(depth, max_depth, has_child_elements(walk, element))
        {
            state.record(SnapshotStopReason::DepthLimit);
        }
        return;
    }
    let now = Instant::now();
    if let Some(reason) = collection_limit(nodes.len(), walk.node_budget, now, walk.deadline) {
        state.record_skipped(reason, Some(node_id));
        return;
    }
    collect_child_nodes(
        walk, element, node_id, node_index, depth, max_depth, nodes, state,
    );
}

/// Root -> Document path selected by discovery (root excluded).
pub(super) struct ContentRootPath {
    ancestors: Vec<UIElement>,
    document: UIElement,
}

/// Visited discovery element plus the index of its parent in the same list.
type Visited = Vec<(UIElement, Option<usize>)>;

/// Breadth-first search for the shallowest visible Document of a Gecko
/// window. Never descends into a Document; stops after the first level that
/// holds a visible one. Two or more at that level is ambiguous: no selection.
fn find_content_root(
    walk: &SnapshotWalk<'_>,
    root: &UIElement,
    max_depth: u32,
) -> Option<ContentRootPath> {
    if !is_gecko_root(root) {
        return None;
    }
    let budget = SNAPSHOT_DEADLINE / DISCOVERY_DEADLINE_DIVISOR;
    let deadline = walk.deadline.min(Instant::now() + budget);
    let mut visited: Visited = vec![(root.clone(), None)];
    let mut level = vec![0_usize];
    for _ in 0..max_depth {
        let mut next = Vec::new();
        let mut documents = Vec::new();
        for parent in level {
            discover_children(
                walk,
                deadline,
                &mut visited,
                parent,
                &mut next,
                &mut documents,
            )?;
        }
        match documents.as_slice() {
            [document] => return path_to(&visited, *document),
            [] if !next.is_empty() => level = next,
            _ => return None,
        }
    }
    None
}

fn is_gecko_root(root: &UIElement) -> bool {
    root.get_cached_classname()
        .is_ok_and(|class| class == GECKO_WINDOW_CLASS)
        || root
            .get_framework_id()
            .is_ok_and(|framework| framework == GECKO_FRAMEWORK_ID)
}

/// Enumerates one element's children. `None` aborts discovery (visit cap,
/// discovery deadline or a walker failure that leaves the level unknown).
fn discover_children(
    walk: &SnapshotWalk<'_>,
    deadline: Instant,
    visited: &mut Visited,
    parent: usize,
    next: &mut Vec<usize>,
    documents: &mut Vec<usize>,
) -> Option<()> {
    let mut child = {
        let (element, _) = visited.get(parent)?;
        walk.raw_walker
            .get_first_child_build_cache(element, walk.cache)
    };
    loop {
        let element = match child {
            Ok(element) => element,
            Err(error) => return walker_reached_end(&error).then_some(()),
        };
        if visited.len() >= DISCOVERY_VISIT_BUDGET || Instant::now() >= deadline {
            return None;
        }
        let index = visited.len();
        if matches!(element.get_cached_control_type(), Ok(ControlType::Document)) {
            if !element.is_cached_offscreen().unwrap_or(false) {
                documents.push(index);
            }
        } else {
            next.push(index);
        }
        child = walk
            .raw_walker
            .get_next_sibling_build_cache(&element, walk.cache);
        visited.push((element, Some(parent)));
    }
}

fn path_to(visited: &Visited, document: usize) -> Option<ContentRootPath> {
    let (document, mut parent) = visited.get(document)?.clone();
    let mut ancestors = Vec::new();
    while let Some(index) = parent {
        let (element, up) = visited.get(index)?;
        if up.is_none() {
            break; // the window root itself is collected separately
        }
        ancestors.push(element.clone());
        parent = *up;
    }
    ancestors.reverse();
    Some(ContentRootPath {
        ancestors,
        document,
    })
}

/// Collects the ancestor path and the selected Document subtree (bounded by
/// `CONTENT_ROOT_NODE_BUDGET`, shared main deadline) after the root. Ancestors
/// are remembered so the following window walk descends into them without
/// duplicating them, and skips the Document subtree. An unreadable ancestor or
/// an exhausted limit drops the selection: the plain window walk follows.
fn collect_content_root(
    walk: &SnapshotWalk<'_>,
    path: &ContentRootPath,
    depth: SnapshotDepth,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) {
    let now = Instant::now();
    if collection_limit(nodes.len(), walk.node_budget, now, walk.deadline).is_some() {
        return;
    }
    let Some(root_id) = nodes.first().map(|node| node.element_id.clone()) else {
        return;
    };
    let Some(ancestors) = read_ancestors(walk, &path.ancestors, root_id) else {
        return;
    };
    let mut parent_index = 0_usize;
    for ancestor in ancestors {
        (_, parent_index) = push_ancestor(nodes, state, ancestor);
    }
    collect_document(walk, path, parent_index, depth, nodes, state);
}

/// Collects the selected Document subtree under its (collected) parent and
/// marks it as the content root when it was pushed.
fn collect_document(
    walk: &SnapshotWalk<'_>,
    path: &ContentRootPath,
    parent_index: usize,
    depth: SnapshotDepth,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) {
    let Some(parent_id) = nodes.get(parent_index).map(|node| node.element_id.clone()) else {
        return;
    };
    let content_walk = SnapshotWalk {
        node_budget: nodes
            .len()
            .saturating_add(CONTENT_ROOT_NODE_BUDGET)
            .min(walk.node_budget),
        ..*walk
    };
    let document_index = nodes.len();
    let document_depth = u32::try_from(path.ancestors.len().saturating_add(1)).unwrap_or(u32::MAX);
    let collected = collect_walk_node(
        &content_walk,
        &path.document,
        Some(parent_id),
        document_depth,
        depth.below_content_root(document_depth),
        nodes,
        state,
    );
    match collected {
        Ok(true) => {
            count_child(nodes, parent_index);
            state.content_root = nodes
                .get(document_index)
                .map(|node| node.element_id.clone());
        }
        Ok(false) => {}
        // Limits were already recorded; a failed Document read leaves no
        // selection and the window walk retries the Document in place.
        Err(error) => tracing::debug!(
            code = "A11Y_CONTENT_ROOT_NOT_COLLECTED",
            error = %error,
            "Gecko content Document was not collected first; falling back to window order"
        ),
    }
}

/// Reads every ancestor node on the root -> Document path before any is
/// pushed, so one unreadable ancestor leaves the node list untouched.
fn read_ancestors(
    walk: &SnapshotWalk<'_>,
    ancestors: &[UIElement],
    root_id: ElementId,
) -> Option<Vec<AccessibleNode>> {
    let mut parent = root_id;
    let mut depth = 0_u32;
    let mut nodes = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors {
        depth = depth.saturating_add(1);
        match node_from_cached_element(ancestor, Some(parent), depth, walk.root_hwnd, 0) {
            Ok(node) => {
                parent = node.element_id.clone();
                nodes.push(node);
            }
            Err(error) => {
                tracing::debug!(
                    code = "A11Y_CONTENT_ROOT_ANCESTOR_FAILED",
                    error = %error,
                    depth,
                    "Gecko content-root ancestor unreadable; selection dropped"
                );
                return None;
            }
        }
    }
    Some(nodes)
}

fn push_ancestor(
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
    node: AccessibleNode,
) -> (ElementId, usize) {
    let index = nodes.len();
    let id = node.element_id.clone();
    let parent_index = node
        .parent
        .as_ref()
        .and_then(|parent| state.collected_ancestors.get(parent))
        .copied();
    if let Some(parent_index) = parent_index {
        count_child(nodes, parent_index);
    }
    state.collected_ancestors.insert(id.clone(), index);
    nodes.push(node);
    (id, index)
}
