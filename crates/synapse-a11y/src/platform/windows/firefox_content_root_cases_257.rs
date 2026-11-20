//! #257 round 2: content-root cases the r1 review found untested (criteria
//! ADDENDUM r2, frozen 2026-10-16 20:10 +03:00). Reuses the synthetic UIA
//! fixture of `firefox_content_budget_257` (hidden window, real WM_GETOBJECT
//! provider; never shown, activated, focused or sent input). Every test drives
//! the real entry point `snapshot_window_from_hwnd`, or `snapshot_element` for
//! the explicit-scope case.
//!
//! Expected "plain DFS order" comes from the fixture model's own child order,
//! never from production code. UIA may add HWND-proxy elements (for example a
//! non-client title bar) to the raw window tree, so window order checks compare
//! only the fixture's own elements.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use serde_json::Value;
use synapse_core::{AccessibleSubtree, ElementId};
use windows::Win32::UI::Accessibility::{
    UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId, UIA_DocumentControlTypeId,
    UIA_GroupControlTypeId, UIA_ToolBarControlTypeId,
};

use super::{
    SNAPSHOT_NODE_BUDGET, SNAPSHOT_WORKER_REPLY_TIMEOUT,
    firefox_content_budget_257::{CONTROLS, Fixture, Model},
    invalidate_snapshot_cache,
    snapshot_deadline_seam_257::SnapshotDeadlineOverride,
    snapshot_element, snapshot_window_from_hwnd,
};

/// Deep enough that no fixture tree below is cut by the requested depth.
pub(super) const FULL_DEPTH: u32 = 8;
pub(super) const ACTIVE: &str = "257 active content";
const CHROME_BUTTONS: usize = 5;
/// Walk deadline (via the #257 deadline seam) for cases where the slow
/// synthetic provider must not hit `SNAPSHOT_DEADLINE`; kept below the 10 s
/// `SNAPSHOT_WORKER_REPLY_TIMEOUT`, which would retire the shared UIA worker.
pub(super) const LONG_WALK_DEADLINE: Duration = Duration::from_secs(8);

/// Window with Gecko (`MozillaWindowClass` / `Gecko`) or plain Win32 identity;
/// `build` adds the tree below the window root (model index 0).
pub(super) fn spawn(gecko: bool, build: impl FnOnce(&mut Model) + Send + 'static) -> Fixture {
    Fixture::spawn_with(gecko, move |hwnd| {
        let mut model = Model::root(hwnd, gecko);
        build(&mut model);
        model
    })
}

/// Browser-chrome toolbar with `buttons` children; returns the toolbar index.
pub(super) fn chrome(model: &mut Model, buttons: usize) -> usize {
    let toolbar = model.push(Some(0), "257 browser chrome", UIA_ToolBarControlTypeId);
    for index in 0..buttons {
        model.push(
            Some(toolbar),
            &format!("257 chrome {index}"),
            UIA_ButtonControlTypeId,
        );
    }
    toolbar
}

/// Visible Document `name` under `parent` holding the three fixture controls.
pub(super) fn document(model: &mut Model, parent: usize, name: &str) -> usize {
    let document = model.push(Some(parent), name, UIA_DocumentControlTypeId);
    for (control, role) in CONTROLS {
        model.push(Some(document), &format!("{name} / {control}"), role);
    }
    document
}

/// Depth-first pre-order of the model subtree at `from`, in the model's own
/// child order (independent of production code).
pub(super) fn preorder(model: &Model, from: usize) -> Vec<String> {
    let mut order = Vec::new();
    let mut stack = vec![from];
    while let Some(index) = stack.pop() {
        let node = &model.nodes[index];
        order.push(node.name.clone());
        stack.extend(node.children.iter().rev().copied());
    }
    order
}

/// Plain window DFS order of every fixture element below the root.
pub(super) fn plain_window_order(model: &Model) -> Vec<String> {
    preorder(model, 0).into_iter().skip(1).collect()
}

/// Criteria content-first order below the root: ancestor path root->Document,
/// the Document subtree, then the rest of the window in plain DFS order with
/// path and subtree not repeated.
fn content_first_order(model: &Model, document: usize) -> Vec<String> {
    let mut path = Vec::new();
    let mut up = model.nodes[document].parent;
    while let Some(index) = up.filter(|index| *index != 0) {
        path.push(model.nodes[index].name.clone());
        up = model.nodes[index].parent;
    }
    path.reverse();
    let subtree = preorder(model, document);
    let rest: Vec<String> = plain_window_order(model)
        .into_iter()
        .filter(|name| !path.contains(name) && !subtree.contains(name))
        .collect();
    path.into_iter().chain(subtree).chain(rest).collect()
}

/// Snapshot order of the fixture elements below the root (proxy elements and
/// the root itself are left out; the root is checked by `assert_root_first`).
pub(super) fn fixture_order(tree: &AccessibleSubtree, model: &Model) -> Vec<String> {
    let names: HashSet<&str> = model.nodes[1..]
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    tree.nodes
        .iter()
        .filter(|node| names.contains(node.name.as_str()))
        .map(|node| node.name.clone())
        .collect()
}

pub(super) fn wire(tree: &AccessibleSubtree) -> Value {
    serde_json::to_value(tree).unwrap_or_else(|error| panic!("serialize snapshot: {error}"))
}

/// Criteria "nodes[0] is the root": the window root leads the node list.
pub(super) fn assert_root_first(tree: &AccessibleSubtree) {
    let first = tree
        .nodes
        .first()
        .unwrap_or_else(|| panic!("#257: snapshot returned no nodes"));
    assert_eq!(
        first.element_id, tree.root,
        "#257: nodes[0] must be the snapshot root"
    );
    assert!(
        first.parent.is_none() && first.depth == 0,
        "#257: nodes[0] is not the window root: {first:?}"
    );
}

/// ADDENDUM r2 "Document nested >=3 levels: unique ids, every parent present in
/// nodes, ancestor children_count = real collected children" plus criteria
/// "root, ancestor path, Document subtree, rest of the window; never duplicated".
#[test]
fn must_pass_257_nested_document_three_levels_deep_has_unique_ids_present_parents_and_exact_children_count()
 {
    let fixture = spawn(true, |model| {
        chrome(model, CHROME_BUTTONS);
        let frame0 = model.push(Some(0), "257 frame 0", UIA_GroupControlTypeId);
        let frame1 = model.push(Some(frame0), "257 frame 1", UIA_GroupControlTypeId);
        model.push(Some(frame1), "257 sidebar before", UIA_ButtonControlTypeId);
        let frame2 = model.push(Some(frame1), "257 frame 2", UIA_GroupControlTypeId);
        let active = document(model, frame2, ACTIVE);
        model.active = active;
        model.push(Some(frame2), "257 status after", UIA_ButtonControlTypeId);
        model.push(Some(frame0), "257 sidebar after", UIA_ButtonControlTypeId);
    });
    let selected = fixture.nested_id(fixture.model.active);
    let tree = fixture.snapshot(FULL_DEPTH);
    assert_root_first(&tree);
    assert_eq!(
        tree.selected_content_root.as_ref(),
        Some(&selected),
        "#257: the Document under 3 group ancestors must be selected"
    );
    assert_eq!(wire(&tree)["completeness_scope"], "document");
    let ids: HashSet<&ElementId> = tree.nodes.iter().map(|node| &node.element_id).collect();
    assert_eq!(
        ids.len(),
        tree.nodes.len(),
        "#257: ancestor path or Document collected twice"
    );
    let mut children: HashMap<&ElementId, u32> = HashMap::new();
    for node in &tree.nodes[1..] {
        let parent = node
            .parent
            .as_ref()
            .unwrap_or_else(|| panic!("#257: non-root node without parent: {}", node.name));
        assert!(
            ids.contains(parent),
            "#257: parent of {} missing from nodes",
            node.name
        );
        *children.entry(parent).or_default() += 1;
    }
    for node in &tree.nodes {
        assert_eq!(
            node.children_count,
            children.get(&node.element_id).copied().unwrap_or(0),
            "#257: children_count of {} differs from its collected children",
            node.name
        );
    }
    for name in plain_window_order(&fixture.model) {
        let copies = tree.nodes.iter().filter(|node| node.name == name).count();
        assert_eq!(
            copies, 1,
            "#257: fixture element {name} collected {copies} times"
        );
    }
    // Content pass runs before the window walk, so no proxy element can sit
    // between the root and the Document: nodes[1..=4] is the exact path.
    let expected = content_first_order(&fixture.model, fixture.model.active);
    let leading: Vec<&str> = tree.nodes[1..]
        .iter()
        .take(4)
        .map(|node| node.name.as_str())
        .collect();
    assert_eq!(
        leading,
        expected[..4].iter().map(String::as_str).collect::<Vec<_>>(),
        "#257: root must be followed by the 3 ancestors and then the Document"
    );
    assert_eq!(
        fixture_order(&tree, &fixture.model),
        expected,
        "#257: order must be ancestor path, Document subtree, then the rest"
    );
}

/// ADDENDUM r2 "two equally shallow visible Documents -> no selection + plain
/// DFS order" (criteria: ambiguous -> no selection, old walk order).
#[test]
fn must_fail_257_two_visible_documents_select_nothing_and_keep_plain_dfs_order() {
    let fixture = spawn(true, |model| {
        chrome(model, CHROME_BUTTONS);
        document(model, 0, "257 first content");
        document(model, 0, "257 second content");
        model.push(Some(0), "257 status after", UIA_ButtonControlTypeId);
    });
    let tree = fixture.snapshot(FULL_DEPTH);
    assert_root_first(&tree);
    assert!(
        tree.selected_content_root.is_none(),
        "#257: ambiguous Documents must not be selected: {:?}",
        tree.selected_content_root
    );
    assert!(
        tree.completeness_scope.is_none(),
        "#257: no completeness_scope without a selection"
    );
    assert_eq!(
        fixture_order(&tree, &fixture.model),
        plain_window_order(&fixture.model),
        "#257: ambiguous Documents must keep the plain window DFS order"
    );
}

/// ADDENDUM r2 "window root collected BEFORE discovery, never subject to the
/// entry limit: a slow/exhausted discovery still returns Ok(tree) with
/// nodes[0] = root" and "discovery has its own bound". Review r1 D1.
#[test]
fn must_fail_257_slow_chrome_before_documents_still_returns_root_first() {
    let fixture = spawn(true, |model| {
        let toolbar = chrome(model, 3);
        // 450 ms Name stall (> SNAPSHOT_DEADLINE) on the first level-1 element
        // discovery visits, before it can reach the Document.
        model.slow = Some(toolbar);
        let active = document(model, 0, ACTIVE);
        model.active = active;
    });
    invalidate_snapshot_cache();
    let started = Instant::now();
    let result = snapshot_window_from_hwnd(fixture.model.hwnd, 4);
    let elapsed = started.elapsed();
    assert!(
        fixture.model.delayed.load(Ordering::SeqCst),
        "#257: the chrome stall never ran"
    );
    let tree = result.unwrap_or_else(|error| {
        panic!("#257 D1: slow chrome before the Documents lost the window root: {error}")
    });
    assert!(
        elapsed < SNAPSHOT_WORKER_REPLY_TIMEOUT,
        "#257: a finite stall must not exhaust the worker reply timeout"
    );
    assert_root_first(&tree);
    assert!(
        tree.truncated,
        "#257: a walk past its deadline is incomplete"
    );
    assert_eq!(
        wire(&tree)["stop_reason"],
        "deadline",
        "#257: the stall must be reported as the deadline"
    );
}

/// ADDENDUM r2 "content > 3000 nodes -> chrome still collected after the content
/// cap" (criteria: Document subtree at most 3/4 of SNAPSHOT_NODE_BUDGET, then
/// the rest of the window with the remaining budget).
#[test]
fn must_pass_257_content_over_cap_still_collects_chrome_after_content() {
    let fixture = spawn(true, |model| {
        chrome(model, CHROME_BUTTONS);
        let active = model.push(Some(0), ACTIVE, UIA_DocumentControlTypeId);
        for index in 0..=SNAPSHOT_NODE_BUDGET {
            model.push(
                Some(active),
                &format!("257 content {index}"),
                UIA_ButtonControlTypeId,
            );
        }
        model.active = active;
    });
    let selected = fixture.id(fixture.model.active);
    // The synthetic provider cannot walk ~3000 nodes in SNAPSHOT_DEADLINE
    // (red run: stop_reason "deadline"); lengthen it for this window only, so
    // the node budget is the measured limit. Kept under the 10 s worker reply
    // timeout, which would otherwise retire the shared UIA worker.
    let tree = {
        let _deadline =
            SnapshotDeadlineOverride::for_window(fixture.model.hwnd, LONG_WALK_DEADLINE);
        fixture.snapshot(4)
    };
    assert_root_first(&tree);
    assert!(
        tree.nodes.len() <= SNAPSHOT_NODE_BUDGET,
        "#257: node ceiling exceeded: {}",
        tree.nodes.len()
    );
    assert_eq!(
        tree.selected_content_root.as_ref(),
        Some(&selected),
        "#257: the oversized active Document must stay selected"
    );
    assert_eq!(
        wire(&tree)["stop_reason"],
        "node_budget",
        "#257: the content cap must be the first limit (deadline = fixture too slow to measure)"
    );
    let content = 1 + tree
        .nodes
        .iter()
        .filter(|node| node.parent.as_ref() == Some(&selected))
        .count();
    // Oversized Document => exactly the cap: 3/4 of SNAPSHOT_NODE_BUDGET.
    assert_eq!(
        content,
        SNAPSHOT_NODE_BUDGET * 3 / 4,
        "#257: oversized Document subtree must fill exactly 3/4 of the node budget"
    );
    let toolbar = std::iter::once("257 browser chrome".to_owned())
        .chain((0..CHROME_BUTTONS).map(|index| format!("257 chrome {index}")));
    for name in toolbar {
        assert!(
            tree.nodes.iter().any(|node| node.name == name),
            "#257: chrome {name} starved by the capped content"
        );
    }
}

/// ADDENDUM r2 "non-Gecko window containing a Document -> no selection + plain
/// DFS order" (criteria Must NOT change: non-Gecko windows keep nodes/order).
#[test]
fn must_fail_257_non_gecko_window_with_document_selects_nothing() {
    let fixture = spawn(false, |model| {
        chrome(model, CHROME_BUTTONS);
        let active = document(model, 0, ACTIVE);
        model.active = active;
        model.push(Some(0), "257 status after", UIA_ButtonControlTypeId);
    });
    let tree = fixture.snapshot(FULL_DEPTH);
    assert_root_first(&tree);
    assert!(
        tree.selected_content_root.is_none(),
        "#257: a Win32 window must never get a content root: {:?}",
        tree.selected_content_root
    );
    assert!(
        tree.completeness_scope.is_none(),
        "#257: a Win32 window must never get a completeness_scope"
    );
    assert_eq!(
        fixture_order(&tree, &fixture.model),
        plain_window_order(&fixture.model),
        "#257: non-Gecko window order changed"
    );
}

/// ADDENDUM r2 "Discovery runs ONLY for window snapshots; never for
/// snapshot_element (an explicit element scope, e.g. the selected Document
/// itself)". Review r1 D2: a visible iframe Document inside the scoped page
/// Document must not be selected or moved first.
#[test]
fn must_fail_257_element_scoped_snapshot_never_runs_discovery() {
    let fixture = spawn(true, |model| {
        chrome(model, 3);
        let page = model.push(Some(0), ACTIVE, UIA_DocumentControlTypeId);
        model.push(Some(page), "257 page consent", UIA_CheckBoxControlTypeId);
        let frame = model.push(Some(page), "257 iframe content", UIA_DocumentControlTypeId);
        model.push(Some(frame), "257 iframe button", UIA_ButtonControlTypeId);
        model.push(Some(page), "257 page submit", UIA_ButtonControlTypeId);
        model.active = page;
    });
    let page = fixture.id(fixture.model.active);
    invalidate_snapshot_cache();
    let tree = snapshot_element(&page, FULL_DEPTH)
        .unwrap_or_else(|error| panic!("#257 real element snapshot failed: {error}"));
    assert!(
        tree.selected_content_root.is_none(),
        "#257 D2: element-scoped snapshot ran Document discovery and selected {:?}",
        tree.selected_content_root
    );
    assert!(
        tree.completeness_scope.is_none(),
        "#257 D2: element-scoped snapshot claimed document scope"
    );
    let order: Vec<String> = tree.nodes.iter().map(|node| node.name.clone()).collect();
    assert_eq!(
        order,
        preorder(&fixture.model, fixture.model.active),
        "#257 D2: element-scoped snapshot must keep the plain DFS order"
    );
}
