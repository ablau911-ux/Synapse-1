use std::{
    collections::HashSet,
    ffi::c_void,
    mem,
    sync::Mutex,
    time::{Duration, Instant},
};

use synapse_core::{
    AccessibleNode, AccessibleSubtree, ElementId, Point, SnapshotStopReason, UiaPattern, element_id,
};
use uiautomation::{
    UIAutomation, UIElement,
    core::{UICacheRequest, UICondition, UITreeWalker},
    types::{
        ElementMode, Handle, Point as UiaPoint, PropertyConditionFlags, TreeScope, UIProperty,
    },
    variants::Variant,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{
        EnumChildWindows, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, GW_OWNER, GetAncestor,
        GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindow, GetWindowThreadProcessId,
        IsWindow,
    },
};

use crate::{A11yError, A11yResult, ElementSearchScope};

use super::common::{
    TreeView, cached_control_state, cached_hwnd, cached_patterns, cached_public_name, cached_rect,
    cached_role, cached_runtime_id_hex_or_fallback, cached_value, create_cache_request,
    map_uia_error, non_empty, pattern_property, with_automation, with_automation_operation,
};
use super::valid_hwnd;

#[path = "snapshot_content_root.rs"]
mod snapshot_content_root;

#[path = "snapshot_depth.rs"]
mod snapshot_depth;

#[cfg(test)]
use snapshot_content_root::collect_nodes;
use snapshot_content_root::{
    PriorNode, SnapshotRoot, WalkState, collection_limit, count_child, descend, stop_reason_label,
    walk_snapshot,
};

static SNAPSHOT_CACHE: Mutex<Option<SnapshotCache>> = Mutex::new(None);
const RAW_SUPPLEMENT_DEPTH: u32 = 2;
/// Hard ceiling on nodes collected in one snapshot. Cross-process trees (UWP,
/// browsers) can be large; this bounds worst-case work while preserving every
/// node collected up to the cap (the result is flagged `truncated`, never
/// silently collapsed).
const SNAPSHOT_NODE_BUDGET: usize = 4000;
/// Wall-clock budget for one snapshot walk. When exceeded the walk stops
/// descending further but KEEPS everything collected so far and flags
/// `truncated`. This replaces the previous behaviour, which discarded a
/// complete tree and re-ran at depth 1 whenever a walk took >25ms — fatal for
/// inherently slower cross-process UWP/ApplicationFrameHost trees.
const SNAPSHOT_DEADLINE: Duration = Duration::from_millis(400);
const SNAPSHOT_WORKER_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const RAW_SUPPLEMENT_NODE_BUDGET: usize = 60;
const CHROMIUM_RENDERER_CONTENT_TOP_INSET_PX: i32 = 96;
// Packaged Notepad exposes these top-level menu items through raw
// name+ExpandCollapse search even when RawView child walking omits them.
const RAW_MENU_SUPPLEMENT_NAMES: [&str; 3] = ["File", "Edit", "View"];

struct SnapshotCache {
    requested_depth: u32,
    root: ElementId,
    captured_at: Instant,
    tree: AccessibleSubtree,
}

struct SnapshotWalk<'a> {
    /// True condition so every child is enumerated (raw view equivalent).
    true_condition: &'a UICondition,
    cache: &'a UICacheRequest,
    raw_walker: &'a UITreeWalker,
    root_hwnd: i64,
    /// Stop collecting once this many nodes are collected.
    node_budget: usize,
    /// Stop descending once this instant is reached.
    deadline: Instant,
}

pub fn snapshot(root: &UIElement, depth: u32) -> A11yResult<AccessibleSubtree> {
    let _ = root;
    let _ = depth;
    Err(A11yError::internal(
        "direct UIElement snapshot is disabled; use snapshot_focused_window, snapshot_window_from_hwnd, or snapshot_element so UIA stays on the dedicated MTA worker",
    ))
}

pub fn snapshot_window_from_hwnd(hwnd: i64, depth: u32) -> A11yResult<AccessibleSubtree> {
    if hwnd == 0 {
        return Err(A11yError::NoForeground {
            detail: "HWND was null".to_owned(),
        });
    }
    let hwnd = isize::try_from(hwnd).map_err(|err| A11yError::InvalidElementId {
        detail: err.to_string(),
    })?;
    with_automation_operation(
        format!("snapshot_window_from_hwnd hwnd=0x{hwnd:x} depth={depth}"),
        SNAPSHOT_WORKER_REPLY_TIMEOUT,
        move |automation| {
            let root = automation
                .element_from_handle(Handle::from(hwnd))
                .map_err(map_uia_error)?;
            snapshot_rooted(automation, &root, depth, SnapshotRoot::Window)
        },
    )
}

pub fn snapshot_element(id: &ElementId, depth: u32) -> A11yResult<AccessibleSubtree> {
    let id = id.clone();
    with_automation_operation(
        format!("snapshot_element element_id={id} depth={depth}"),
        SNAPSHOT_WORKER_REPLY_TIMEOUT,
        move |automation| {
            let root = super::resolve::re_resolve_on_worker(automation, &id)?;
            snapshot_from_root(automation, &root, depth)
        },
    )
}

/// Element-scoped snapshot: plain walk, never content-root discovery (#257).
pub(super) fn snapshot_from_root(
    automation: &UIAutomation,
    root: &UIElement,
    depth: u32,
) -> A11yResult<AccessibleSubtree> {
    snapshot_rooted(automation, root, depth, SnapshotRoot::Element)
}

fn snapshot_rooted(
    automation: &UIAutomation,
    root: &UIElement,
    depth: u32,
    kind: SnapshotRoot,
) -> A11yResult<AccessibleSubtree> {
    let started = Instant::now();
    let mut tree = snapshot_at_depth(automation, root, depth, kind)?;
    if depth >= RAW_SUPPLEMENT_DEPTH {
        if supplement_raw_pattern_nodes(automation, root, &mut tree.nodes)? {
            tree.truncated = true;
            if tree.stop_reason.is_none() {
                tree.stop_reason = Some(SnapshotStopReason::NodeBudget);
            }
        }
        tree.max_depth = tree.max_depth.max(RAW_SUPPLEMENT_DEPTH);
    }
    // Observability: `truncated` is also surfaced to callers (observe
    // diagnostics) so an incomplete tree is never mistaken for a complete
    // one. A truncated tree means the node budget/deadline was hit or a
    // subtree errored (the per-subtree error is logged at warn separately).
    tracing::debug!(
        code = "A11Y_SNAPSHOT_ASSEMBLED",
        requested_depth = depth,
        nodes = tree.nodes.len(),
        max_depth = tree.max_depth,
        truncated = tree.truncated,
        elapsed_ms = started.elapsed().as_millis(),
        "a11y snapshot assembled"
    );
    store_snapshot(depth, &tree);
    Ok(tree)
}

pub fn focused_element_node() -> A11yResult<AccessibleNode> {
    with_automation(|automation| {
        let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Control)?;
        let element = automation
            .get_focused_element_build_cache(&cache)
            .map_err(map_uia_error)?;
        let foreground_hwnd = unsafe { GetForegroundWindow() };
        let root_hwnd = cached_hwnd(&element)
            .filter(|value| *value != 0)
            .unwrap_or(foreground_hwnd.0 as isize as i64);
        node_from_cached_element(&element, None, 0, root_hwnd, 0)
    })
}

pub fn focused_hwnd_in_window(hwnd: i64) -> A11yResult<Option<i64>> {
    let target = valid_hwnd(hwnd)?;
    // SAFETY: target is a validated HWND; the call reads its GUI thread ID with no output pointer.
    let thread_id = unsafe { GetWindowThreadProcessId(target, None) };
    if thread_id == 0 {
        return Err(A11yError::NoForeground {
            detail: format!(
                "GetWindowThreadProcessId returned no GUI thread for hwnd 0x{:x}",
                target.0 as isize
            ),
        });
    }

    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(mem::size_of::<GUITHREADINFO>())
            .map_err(|_err| A11yError::internal("GUITHREADINFO size does not fit cbSize"))?,
        ..Default::default()
    };
    // SAFETY: info is a live, initialized GUITHREADINFO with the required cbSize.
    unsafe { GetGUIThreadInfo(thread_id, &raw mut info) }.map_err(|err| {
        let detail = format!(
            "GetGUIThreadInfo failed for hwnd 0x{:x} thread_id={thread_id}: {err}",
            target.0 as isize
        );
        if err.code().0 == 0x8007_0005_u32.cast_signed() {
            A11yError::NotAvailable { detail }
        } else {
            A11yError::internal(detail)
        }
    })?;

    let focus = info.hwndFocus;
    if focus.0.is_null() {
        return Ok(None);
    }
    // SAFETY: IsWindow accepts any handle value and only reads USER state.
    if !unsafe { IsWindow(Some(focus)) }.as_bool() {
        tracing::debug!(
            code = "A11Y_TARGET_FOCUS_HWND_STALE",
            target_hwnd = target.0 as isize,
            focus_hwnd = focus.0 as isize,
            "target GUI thread reported a stale focused HWND"
        );
        return Ok(None);
    }
    if !focus_belongs_to_target(target, focus) {
        tracing::debug!(
            code = "A11Y_TARGET_FOCUS_OUTSIDE_TARGET",
            target_hwnd = target.0 as isize,
            focus_hwnd = focus.0 as isize,
            "target GUI thread focus was outside the requested target root/owner chain"
        );
        return Ok(None);
    }

    Ok(Some(focus.0 as isize as i64))
}

pub fn focused_element_node_in_window(hwnd: i64) -> A11yResult<Option<AccessibleNode>> {
    let target = valid_hwnd(hwnd)?;
    let Some(focus_raw) = focused_hwnd_in_window(hwnd)? else {
        return Ok(None);
    };
    let focus_raw = isize::try_from(focus_raw)
        .map_err(|_err| A11yError::internal("focused HWND does not fit isize"))?;
    let target_raw = target.0 as isize as i64;
    with_automation_operation(
        format!(
            "focused_element_node_in_window target=0x{:x} focus=0x{focus_raw:x}",
            target.0 as isize
        ),
        SNAPSHOT_WORKER_REPLY_TIMEOUT,
        move |automation| {
            let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Control)?;
            let element = automation
                .element_from_handle_build_cache(Handle::from(focus_raw), &cache)
                .map_err(map_uia_error)?;
            let mut node = node_from_cached_element(&element, None, 0, target_raw, 0)?;
            node.focused = true;
            Ok(Some(node))
        },
    )
}

pub fn element_node_from_point(point: Point) -> A11yResult<AccessibleNode> {
    with_automation(move |automation| {
        let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Control)?;
        let element = automation
            .element_from_point_build_cache(UiaPoint::new(point.x, point.y), &cache)
            .map_err(map_uia_error)?;
        let root_hwnd = cached_hwnd(&element).unwrap_or(0);
        node_from_cached_element(&element, None, 0, root_hwnd, 0)
    })
}

pub fn find_by_name_and_pattern(
    root: &UIElement,
    name: &str,
    pattern: UiaPattern,
    scope: ElementSearchScope,
) -> A11yResult<Option<AccessibleNode>> {
    if name.is_empty() {
        return Ok(None);
    }
    let _ = root;
    let _ = pattern;
    let _ = scope;

    Err(A11yError::internal(
        "direct UIElement search is disabled; use a data-returning worker API so UIA stays on the dedicated MTA worker",
    ))
}

pub fn find_by_name_and_pattern_in_window(
    hwnd: i64,
    name: String,
    pattern: UiaPattern,
    scope: ElementSearchScope,
) -> A11yResult<Option<AccessibleNode>> {
    if hwnd == 0 {
        return Err(A11yError::NoForeground {
            detail: "HWND was null".to_owned(),
        });
    }
    let hwnd = isize::try_from(hwnd).map_err(|err| A11yError::InvalidElementId {
        detail: err.to_string(),
    })?;
    with_automation_operation(
        format!(
            "find_by_name_and_pattern_in_window hwnd=0x{hwnd:x} scope={scope:?} pattern={pattern:?}"
        ),
        SNAPSHOT_WORKER_REPLY_TIMEOUT,
        move |automation| {
            let root = automation
                .element_from_handle(Handle::from(hwnd))
                .map_err(map_uia_error)?;
            find_by_name_and_pattern_from_root(automation, &root, &name, pattern, scope)
        },
    )
}

pub fn chromium_renderer_accessibility_nodes_from_window(
    hwnd: i64,
    depth: u32,
    max_nodes: usize,
) -> A11yResult<(Vec<AccessibleNode>, bool)> {
    if hwnd == 0 || max_nodes == 0 {
        return Ok((Vec::new(), false));
    }
    let hwnd = isize::try_from(hwnd).map_err(|err| A11yError::InvalidElementId {
        detail: err.to_string(),
    })?;
    let Some(root_hwnd) = chromium_accessibility_root_hwnd(HWND(hwnd as *mut c_void)) else {
        return Ok((Vec::new(), false));
    };
    let root_hwnd = root_hwnd.0 as isize;
    with_automation_operation(
        format!(
            "chromium_renderer_accessibility_nodes_from_window hwnd=0x{hwnd:x} accessibility_root=0x{root_hwnd:x} depth={depth} max_nodes={max_nodes}"
        ),
        SNAPSHOT_WORKER_REPLY_TIMEOUT,
        move |automation| {
            let root = automation
                .element_from_handle(Handle::from(root_hwnd))
                .map_err(map_uia_error)?;
            chromium_renderer_accessibility_nodes_from_root(automation, &root, depth, max_nodes)
        },
    )
}

fn chromium_renderer_accessibility_nodes_from_root(
    automation: &UIAutomation,
    root: &UIElement,
    depth: u32,
    max_nodes: usize,
) -> A11yResult<(Vec<AccessibleNode>, bool)> {
    let class_name = root.get_classname().unwrap_or_default();
    if !is_chromium_widget_window_class(&class_name) {
        return Ok((Vec::new(), false));
    }

    let root_cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
    let cached_root = root
        .build_updated_cache(&root_cache)
        .map_err(map_uia_error)?;
    let root_hwnd = cached_hwnd(&cached_root).unwrap_or(0);
    let root_id = element_id_from_cached_element(&cached_root, root_hwnd)?;
    let root_rect = cached_rect(&cached_root);

    let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
    let content_condition = automation
        .create_property_condition(UIProperty::IsContentElement, Variant::from(true), None)
        .map_err(map_uia_error)?;
    let elements = root
        .find_all_build_cache(TreeScope::Subtree, &content_condition, &cache)
        .map_err(map_uia_error)?;

    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    // Set when a content node was dropped (cap reached or read error): the
    // caller must report partial coverage (#77).
    let mut truncated = false;
    let node_depth = depth.max(1);
    for element in elements {
        let node = match node_from_cached_element(
            &element,
            Some(root_id.clone()),
            node_depth,
            root_hwnd,
            0,
        ) {
            Ok(node) => node,
            Err(error) => {
                tracing::warn!(
                    code = "A11Y_CHROMIUM_RENDERER_SUPPLEMENT_NODE_FAILED",
                    error = %error,
                    element_name = %cached_public_name(&element),
                    element_class = %element.get_cached_classname().unwrap_or_default(),
                    control_type = ?element.get_cached_control_type().ok(),
                    automation_id = %element.get_cached_automation_id().unwrap_or_default(),
                    process_id = element.get_cached_process_id().unwrap_or(-1),
                    "Chromium renderer UIA supplement node read failed; node omitted"
                );
                truncated = true;
                continue;
            }
        };
        if is_chromium_renderer_content_node(&node, root_rect)
            && seen.insert(node.element_id.clone())
        {
            if nodes.len() >= max_nodes {
                truncated = true;
                break;
            }
            nodes.push(node);
        }
    }
    Ok((nodes, truncated))
}

fn find_by_name_and_pattern_from_root(
    automation: &UIAutomation,
    root: &UIElement,
    name: &str,
    pattern: UiaPattern,
    scope: ElementSearchScope,
) -> A11yResult<Option<AccessibleNode>> {
    if name.is_empty() {
        return Ok(None);
    }

    let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
    let name_condition = automation
        .create_property_condition(
            UIProperty::Name,
            Variant::from(name),
            Some(PropertyConditionFlags::IgnoreCase),
        )
        .map_err(map_uia_error)?;
    let pattern_condition = automation
        .create_property_condition(pattern_property(pattern), Variant::from(true), None)
        .map_err(map_uia_error)?;
    let condition = automation
        .create_and_condition(name_condition, pattern_condition)
        .map_err(map_uia_error)?;
    let elements = root
        .find_all_build_cache(scope.into(), &condition, &cache)
        .map_err(map_uia_error)?;
    let root_hwnd = root
        .build_updated_cache(&cache)
        .ok()
        .and_then(|cached_root| cached_hwnd(&cached_root))
        .unwrap_or(0);

    elements
        .into_iter()
        .filter(|element| element.is_cached_enabled().unwrap_or(true))
        .map(|element| node_from_cached_element(&element, None, 0, root_hwnd, 0))
        .next()
        .transpose()
}

fn focus_belongs_to_target(target: HWND, focus: HWND) -> bool {
    if target.0 == focus.0 {
        return true;
    }
    let target_root = ancestor_or_self(target, GA_ROOT);
    let focus_root = ancestor_or_self(focus, GA_ROOT);
    if focus_root.0 == target_root.0 {
        return true;
    }
    let target_owner = ancestor_or_self(target, GA_ROOTOWNER);
    let focus_owner = ancestor_or_self(focus, GA_ROOTOWNER);
    focus_owner.0 == target_root.0
        || focus_owner.0 == target_owner.0
        || owner_chain_contains(focus_root, target_root, target_owner)
}

fn ancestor_or_self(
    hwnd: HWND,
    flags: windows::Win32::UI::WindowsAndMessaging::GET_ANCESTOR_FLAGS,
) -> HWND {
    let ancestor = unsafe { GetAncestor(hwnd, flags) };
    if ancestor.0.is_null() { hwnd } else { ancestor }
}

fn owner_chain_contains(mut hwnd: HWND, target_root: HWND, target_owner: HWND) -> bool {
    for _ in 0..32 {
        let owner = match unsafe { GetWindow(hwnd, GW_OWNER) } {
            Ok(owner) => owner,
            Err(_) => return false,
        };
        if owner.0.is_null() {
            return false;
        }
        if owner.0 == target_root.0 || owner.0 == target_owner.0 {
            return true;
        }
        let owner_root = ancestor_or_self(owner, GA_ROOT);
        if owner_root.0 == target_root.0 || owner_root.0 == target_owner.0 {
            return true;
        }
        hwnd = owner;
    }
    false
}
fn snapshot_at_depth(
    automation: &UIAutomation,
    root: &UIElement,
    depth: u32,
    kind: SnapshotRoot,
) -> A11yResult<AccessibleSubtree> {
    let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
    let true_condition = automation.create_true_condition().map_err(map_uia_error)?;
    let raw_walker = automation.get_raw_view_walker().map_err(map_uia_error)?;
    let cached_root = root.build_updated_cache(&cache).map_err(map_uia_error)?;
    let root_hwnd = cached_hwnd(&cached_root).unwrap_or(0);
    let root_id = element_id_from_cached_element(&cached_root, root_hwnd)?;
    if let Some(tree) = cached_snapshot(depth, &root_id) {
        return Ok(tree);
    }
    let walk = SnapshotWalk {
        true_condition: &true_condition,
        cache: &cache,
        raw_walker: &raw_walker,
        root_hwnd,
        node_budget: SNAPSHOT_NODE_BUDGET,
        deadline: Instant::now() + snapshot_deadline(root_hwnd),
    };
    walk_snapshot(&walk, &cached_root, depth, kind)
}

/// Walk deadline for a snapshot rooted at `root_hwnd`: `SNAPSHOT_DEADLINE`,
/// unless a `#[cfg(test)]` override is active for that window (#257 seam).
fn snapshot_deadline(root_hwnd: i64) -> Duration {
    #[cfg(test)]
    if let Some(deadline) = snapshot_deadline_seam_257::override_for(root_hwnd) {
        return deadline;
    }
    #[cfg(not(test))]
    let _ = root_hwnd;
    SNAPSHOT_DEADLINE
}

fn supplement_raw_pattern_nodes(
    automation: &UIAutomation,
    root: &UIElement,
    nodes: &mut Vec<AccessibleNode>,
) -> A11yResult<bool> {
    let Some(root_id) = nodes.first().map(|node| node.element_id.clone()) else {
        return Ok(false);
    };
    if !should_supplement_raw_pattern_nodes(nodes.first().map(|node| node.name.as_str())) {
        return Ok(false);
    }
    let root_hwnd = root_id
        .parts()
        .map_err(|err| A11yError::InvalidElementId {
            detail: err.to_string(),
        })?
        .hwnd;
    let cache = create_cache_request(automation, 0, ElementMode::Full, TreeView::Raw)?;
    let mut seen: HashSet<ElementId> = nodes.iter().map(|node| node.element_id.clone()).collect();
    let mut truncated = false;
    for name in RAW_MENU_SUPPLEMENT_NAMES {
        let name_condition = automation
            .create_property_condition(
                UIProperty::Name,
                Variant::from(name),
                Some(PropertyConditionFlags::IgnoreCase),
            )
            .map_err(map_uia_error)?;
        let pattern_condition = automation
            .create_property_condition(
                UIProperty::IsExpandCollapsePatternAvailable,
                Variant::from(true),
                None,
            )
            .map_err(map_uia_error)?;
        let condition = automation
            .create_and_condition(name_condition, pattern_condition)
            .map_err(map_uia_error)?;
        let raw_elements = root
            .find_all_build_cache(TreeScope::Subtree, &condition, &cache)
            .map_err(map_uia_error)?;
        for element in raw_elements {
            if nodes.len() >= RAW_SUPPLEMENT_NODE_BUDGET {
                truncated = true;
                break;
            }
            let node = match node_from_cached_element(
                &element,
                Some(root_id.clone()),
                RAW_SUPPLEMENT_DEPTH,
                root_hwnd,
                0,
            ) {
                Ok(node) => node,
                Err(error) => {
                    truncated = true;
                    tracing::warn!(
                        code = "A11Y_RAW_SUPPLEMENT_NODE_FAILED",
                        error = %error,
                        element_name = %cached_public_name(&element),
                        element_class = %element.get_cached_classname().unwrap_or_default(),
                        control_type = ?element.get_cached_control_type().ok(),
                        automation_id = %element.get_cached_automation_id().unwrap_or_default(),
                        process_id = element.get_cached_process_id().unwrap_or(-1),
                        "UIA raw supplement node read failed; node omitted and snapshot flagged truncated"
                    );
                    continue;
                }
            };
            if seen.insert(node.element_id.clone()) {
                nodes.push(node);
            }
        }
        if truncated {
            break;
        }
    }
    Ok(truncated)
}

fn cached_snapshot(depth: u32, root: &ElementId) -> Option<AccessibleSubtree> {
    let guard = SNAPSHOT_CACHE.lock().ok()?;
    let cache = guard.as_ref()?;
    let is_fresh = cache.requested_depth == depth
        && cache.root == *root
        && cache.captured_at.elapsed() <= Duration::from_millis(50);
    let tree = is_fresh.then(|| cache.tree.clone());
    drop(guard);
    tree
}

fn store_snapshot(depth: u32, tree: &AccessibleSubtree) {
    if let Ok(mut guard) = SNAPSHOT_CACHE.lock() {
        *guard = Some(SnapshotCache {
            requested_depth: depth,
            root: tree.root.clone(),
            captured_at: Instant::now(),
            tree: tree.clone(),
        });
    }
}

pub(super) fn invalidate_snapshot_cache() {
    if let Ok(mut guard) = SNAPSHOT_CACHE.lock() {
        *guard = None;
    }
}

fn collect_walk_node(
    walk: &SnapshotWalk<'_>,
    element: &UIElement,
    parent: Option<ElementId>,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) -> A11yResult<bool> {
    let now = Instant::now();
    if let Some(reason) = collection_limit(nodes.len(), walk.node_budget, now, walk.deadline) {
        state.record_skipped(reason, parent.as_ref());
        return Err(A11yError::internal(format!(
            "UIA snapshot {} reached before collecting node",
            stop_reason_label(reason)
        )));
    }
    let node = node_from_cached_element(element, parent, depth, walk.root_hwnd, 0)?;
    let node_id = node.element_id.clone();
    let (node_index, fresh) = match state.prior(&node_id) {
        PriorNode::ContentRoot => return Ok(false),
        PriorNode::Ancestor(index) => (index, false),
        PriorNode::New => {
            nodes.push(node);
            (nodes.len().saturating_sub(1), true)
        }
    };
    descend(
        walk, element, &node_id, node_index, depth, max_depth, nodes, state,
    );
    Ok(fresh)
}

/// True when stopping at `depth` leaves real children of the node unvisited.
const fn depth_cut_hides_children(depth: u32, max_depth: u32, has_children: bool) -> bool {
    depth >= max_depth && has_children
}

/// Cheap existence probe for children below the depth limit. An enumeration
/// error counts as "has children": coverage is unknown, so never claim it.
fn has_child_elements(walk: &SnapshotWalk<'_>, element: &UIElement) -> bool {
    if should_use_bulk_child_fallback(element) {
        return match element.find_all_build_cache(
            TreeScope::Children,
            walk.true_condition,
            walk.cache,
        ) {
            Ok(children) => !children.is_empty(),
            Err(_) => true,
        };
    }
    has_children_after_probe(
        walk.raw_walker
            .get_first_child_build_cache(element, walk.cache)
            .map(|_| ()),
    )
}

/// True when a tree-walker error is the genuine end-of-list signal: either
/// `ERR_NOTFOUND` or the empty error windows-rs produces for a NULL element
/// out-param (HRESULT 0). Any other failure (provider gone, COM/RPC error,
/// timeout) leaves coverage unknown and must never read as "no more nodes".
fn walker_reached_end(error: &uiautomation::Error) -> bool {
    matches!(error.code(), 0 | uiautomation::errors::ERR_NOTFOUND)
}

/// Child-existence decision for a first-child probe. A non-end error counts as
/// "has children": coverage is unknown, so never claim the node is a leaf.
fn has_children_after_probe(probe: Result<(), uiautomation::Error>) -> bool {
    match probe {
        Ok(()) => true,
        Err(error) => !walker_reached_end(&error),
    }
}

/// Geometry a caller may treat as on-screen. A control UIA reports as
/// offscreen (scrolled out of view, collapsed, minimized) keeps its origin but
/// has no visible extent, so the shared `is_usable_element` visibility model
/// excludes it from find and interactable observe (#101).
const fn bbox_for_visibility(rect: synapse_core::Rect, offscreen: bool) -> synapse_core::Rect {
    if offscreen {
        synapse_core::Rect { w: 0, h: 0, ..rect }
    } else {
        rect
    }
}

// UIA tree-walk recursion threads the full walk context (depth/max_depth, parent id
// and index, output sink, coverage state); a wrapper struct would not improve clarity.
#[allow(clippy::too_many_arguments)]
fn collect_child_nodes(
    walk: &SnapshotWalk<'_>,
    element: &UIElement,
    node_id: &ElementId,
    node_index: usize,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) {
    if should_use_bulk_child_fallback(element) {
        collect_child_nodes_from_bulk(
            walk, element, node_id, node_index, depth, max_depth, nodes, state,
        );
        return;
    }

    let mut child = match walk
        .raw_walker
        .get_first_child_build_cache(element, walk.cache)
    {
        Ok(child) => child,
        Err(error) => {
            note_walker_error(element, node_id, depth, "first child", &error, state);
            return;
        }
    };
    loop {
        if !collect_one_child(
            walk, &child, node_id, node_index, depth, max_depth, nodes, state,
        ) {
            break;
        }
        match walk
            .raw_walker
            .get_next_sibling_build_cache(&child, walk.cache)
        {
            Ok(next) => child = next,
            Err(error) => {
                note_walker_error(&child, node_id, depth, "next sibling", &error, state);
                break;
            }
        }
    }
}

/// Collects one child of `node_id` (shared by the streaming and bulk walks).
/// Returns false when a collection limit stops the remaining siblings.
#[allow(clippy::too_many_arguments)]
fn collect_one_child(
    walk: &SnapshotWalk<'_>,
    child: &UIElement,
    node_id: &ElementId,
    node_index: usize,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) -> bool {
    let now = Instant::now();
    if let Some(reason) = collection_limit(nodes.len(), walk.node_budget, now, walk.deadline) {
        state.record_skipped(reason, Some(node_id));
        tracing::debug!(
            code = "A11Y_SNAPSHOT_WALK_TRUNCATED",
            reason = stop_reason_label(reason),
            nodes = nodes.len(),
            node_budget = walk.node_budget,
            depth,
            "UIA snapshot collection stopped before remaining siblings"
        );
        return false;
    }
    let collected = collect_walk_node(
        walk,
        child,
        Some(node_id.clone()),
        depth + 1,
        max_depth,
        nodes,
        state,
    );
    match collected {
        Ok(true) => count_child(nodes, node_index),
        Ok(false) => {}
        Err(error) => {
            state.record_skipped(SnapshotStopReason::ProviderError, Some(node_id));
            tracing::warn!(
                code = "A11Y_CHILD_NODE_FAILED",
                error = %error,
                depth = depth + 1,
                element_name = %cached_public_name(child),
                element_class = %child.get_cached_classname().unwrap_or_default(),
                control_type = ?child.get_cached_control_type().ok(),
                automation_id = %child.get_cached_automation_id().unwrap_or_default(),
                process_id = child.get_cached_process_id().unwrap_or(-1),
                "UIA child node read failed; node omitted and snapshot flagged truncated"
            );
        }
    }
    true
}

/// Ends a streaming child walk. The genuine end-of-list signal is silent; any
/// other walker failure drops the rest of `node_id`'s children, so record a
/// provider error (#77, #257) instead of presenting a partial tree as complete.
fn note_walker_error(
    element: &UIElement,
    node_id: &ElementId,
    depth: u32,
    step: &str,
    error: &uiautomation::Error,
    state: &mut WalkState,
) {
    if walker_reached_end(error) {
        return;
    }
    state.record_skipped(SnapshotStopReason::ProviderError, Some(node_id));
    tracing::warn!(
        code = "A11Y_CHILD_WALK_FAILED",
        error = %error,
        step,
        depth,
        element_name = %cached_public_name(element),
        element_class = %element.get_cached_classname().unwrap_or_default(),
        control_type = ?element.get_cached_control_type().ok(),
        process_id = element.get_cached_process_id().unwrap_or(-1),
        "UIA child walk failed (not end-of-list); remaining nodes omitted and snapshot flagged truncated"
    );
}

// Mirrors `collect_child_nodes`' walk context (see above); same justified arg count.
#[allow(clippy::too_many_arguments)]
fn collect_child_nodes_from_bulk(
    walk: &SnapshotWalk<'_>,
    element: &UIElement,
    node_id: &ElementId,
    node_index: usize,
    depth: u32,
    max_depth: u32,
    nodes: &mut Vec<AccessibleNode>,
    state: &mut WalkState,
) {
    // `find_all_build_cache(Children, true)` reliably crosses the
    // cross-process `Windows.UI.Core.CoreWindow` boundary for UWP apps. Keep it
    // only for those known app-frame nodes; normal high-fanout trees use the
    // streaming walker above so a single child enumeration cannot monopolize
    // the snapshot deadline.
    let children =
        match element.find_all_build_cache(TreeScope::Children, walk.true_condition, walk.cache) {
            Ok(children) => children,
            Err(err) => {
                state.record_skipped(SnapshotStopReason::ProviderError, Some(node_id));
                tracing::warn!(
                    code = "A11Y_CHILD_ENUM_FAILED",
                    error = %err,
                    depth,
                    element_name = %cached_public_name(element),
                    element_class = %element.get_cached_classname().unwrap_or_default(),
                    control_type = ?element.get_cached_control_type().ok(),
                    automation_id = %element.get_cached_automation_id().unwrap_or_default(),
                    process_id = element.get_cached_process_id().unwrap_or(-1),
                    "UIA child enumeration failed; subtree omitted and snapshot flagged truncated"
                );
                return;
            }
        };
    for child in children {
        if !collect_one_child(
            walk, &child, node_id, node_index, depth, max_depth, nodes, state,
        ) {
            break;
        }
    }
}

fn should_use_bulk_child_fallback(element: &UIElement) -> bool {
    let class_name = element.get_cached_classname().unwrap_or_default();
    let class_name = class_name.as_str();
    class_name == "ApplicationFrameWindow"
        || class_name == "Windows.UI.Core.CoreWindow"
        || class_name == "ApplicationFrameInputSinkWindow"
}

fn should_supplement_raw_pattern_nodes(root_name: Option<&str>) -> bool {
    let Some(root_name) = root_name else {
        return false;
    };
    root_name == "Notepad" || root_name.ends_with(" - Notepad")
}

fn is_chromium_widget_window_class(class_name: &str) -> bool {
    class_name.starts_with("Chrome_WidgetWin_")
}

fn chromium_accessibility_root_hwnd(root: HWND) -> Option<HWND> {
    if is_chromium_widget_window_class(&window_class_name(root)) {
        return Some(root);
    }
    chromium_accessibility_child_hwnd(root)
}

/// Whether this window contains a Chromium widget HWND in its child tree.
#[must_use]
pub fn has_chromium_child_window(hwnd: i64) -> bool {
    isize::try_from(hwnd).is_ok_and(|hwnd| {
        hwnd != 0 && chromium_accessibility_child_hwnd(HWND(hwnd as *mut c_void)).is_some()
    })
}

fn chromium_accessibility_child_hwnd(root: HWND) -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // SAFETY: EnumChildWindows is synchronous; callback receives a live local Option.
    let _ = unsafe {
        EnumChildWindows(
            Some(root),
            Some(find_chromium_widget_child),
            LPARAM((&raw mut found) as isize),
        )
    };
    found
}

unsafe extern "system" fn find_chromium_widget_child(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::core::BOOL {
    if is_chromium_widget_window_class(&window_class_name(hwnd)) {
        // SAFETY: caller passes a valid Option<HWND> for this synchronous enumeration.
        unsafe { *(lparam.0 as *mut Option<HWND>) = Some(hwnd) };
        return windows::core::BOOL(0);
    }
    windows::core::BOOL(1)
}

fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0_u16; 256];
    // SAFETY: buffer is writable and GetClassNameW accepts stale HWNDs by returning zero.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
}

fn is_chromium_renderer_content_node(node: &AccessibleNode, root_rect: synapse_core::Rect) -> bool {
    if node.bbox.w <= 0 || node.bbox.h <= 0 {
        return false;
    }
    if node.bbox.y < chromium_renderer_content_top(root_rect) {
        return false;
    }
    if node
        .automation_id
        .as_deref()
        .is_some_and(|automation_id| automation_id.starts_with("view_"))
    {
        return false;
    }

    let role = node.role.to_ascii_lowercase();
    let content_role = matches!(
        role.as_str(),
        "button"
            | "check box"
            | "combo box"
            | "document"
            | "edit"
            | "group"
            | "heading"
            | "hyperlink"
            | "image"
            | "link"
            | "list"
            | "list item"
            | "menu item"
            | "pane"
            | "radio button"
            | "table"
            | "text"
    );
    if !content_role {
        return false;
    }
    role == "document"
        || !node.name.trim().is_empty()
        || node
            .value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || !node.patterns.is_empty()
}

fn chromium_renderer_content_top(root_rect: synapse_core::Rect) -> i32 {
    if root_rect.h <= 240 {
        return root_rect.y;
    }
    root_rect
        .y
        .saturating_add(CHROMIUM_RENDERER_CONTENT_TOP_INSET_PX.min(root_rect.h / 3))
}

fn node_from_cached_element(
    element: &UIElement,
    parent: Option<ElementId>,
    depth: u32,
    root_hwnd: i64,
    children_count: usize,
) -> A11yResult<AccessibleNode> {
    let hwnd = cached_hwnd(element)
        .filter(|value| *value != 0)
        .unwrap_or(root_hwnd);
    Ok(AccessibleNode {
        element_id: element_id_from_cached_element(element, hwnd)?,
        parent,
        name: cached_public_name(element),
        role: cached_role(element),
        automation_id: non_empty(element.get_cached_automation_id().unwrap_or_default()),
        value: cached_value(element),
        bbox: bbox_for_visibility(
            cached_rect(element),
            // The snapshot root is the window itself (minimized windows report
            // offscreen); keep its geometry.
            depth > 0 && element.is_cached_offscreen().unwrap_or(false),
        ),
        enabled: element.is_cached_enabled().unwrap_or(false),
        focused: element.has_cached_keyboard_focus().unwrap_or(false),
        patterns: cached_patterns(element),
        children_count: u32::try_from(children_count).unwrap_or(u32::MAX),
        depth,
        control_state: cached_control_state(element),
    })
}

fn element_id_from_cached_element(element: &UIElement, hwnd: i64) -> A11yResult<ElementId> {
    let readback = cached_runtime_id_hex_or_fallback(element, hwnd)?;
    if readback.used_fallback {
        tracing::warn!(
            code = "A11Y_RUNTIME_ID_UNAVAILABLE",
            hwnd,
            fallback_runtime_id_hex = %readback.hex,
            element_name = %cached_public_name(element),
            element_class = %element.get_cached_classname().unwrap_or_default(),
            control_type = ?element.get_cached_control_type().ok(),
            automation_id = %element.get_cached_automation_id().unwrap_or_default(),
            process_id = element.get_cached_process_id().unwrap_or(-1),
            "UIA RuntimeId was unavailable; generated a process-local fallback element id"
        );
    }
    Ok(element_id(hwnd, &readback.hex))
}

#[cfg(test)]
#[path = "stale_element_124.rs"]
mod stale_element_124;

#[cfg(test)]
#[path = "snapshot_coverage_77_101.rs"]
mod snapshot_coverage_77_101;

#[cfg(test)]
#[path = "snapshot_walker_errors_77.rs"]
mod snapshot_walker_errors_77;

#[cfg(test)]
#[path = "password_call_sites_32.rs"]
mod password_call_sites_32;

#[cfg(test)]
#[path = "subtree_refresh_stale_124.rs"]
mod subtree_refresh_stale_124;

#[cfg(test)]
#[path = "walker_streaming_61.rs"]
mod walker_streaming_61;

#[cfg(test)]
#[path = "firefox_content_budget_257.rs"]
mod firefox_content_budget_257;

#[cfg(test)]
#[path = "firefox_content_root_cases_257.rs"]
mod firefox_content_root_cases_257;

#[cfg(test)]
#[path = "firefox_content_root_bounds_257.rs"]
mod firefox_content_root_bounds_257;

#[cfg(test)]
#[path = "snapshot_deadline_seam_257.rs"]
mod snapshot_deadline_seam_257;

#[cfg(test)]
#[path = "embedded_renderer_variants_77.rs"]
mod embedded_renderer_variants_77;
