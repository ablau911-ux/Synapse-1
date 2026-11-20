//! #257 mutation-hardening (QA step 7): pins the content-root bounds that
//! cargo-mutants showed unguarded — discovery's own deadline and visit cap,
//! the two independent Gecko signals, and the stop-reason log spelling.
//! Reuses the synthetic UIA fixture (hidden window, real WM_GETOBJECT
//! provider; never shown, focused or sent input) and drives the real entry
//! point `snapshot_window_from_hwnd`. Expected orders come from the fixture
//! model, never from production code.

use std::sync::atomic::Ordering;

use serde_json::Value;
use synapse_core::{AccessibleSubtree, SnapshotStopReason, element_id};
use windows::Win32::UI::Accessibility::{UIA_ButtonControlTypeId, UIA_GroupControlTypeId};

use super::{
    firefox_content_budget_257::{Fixture, Model},
    firefox_content_root_cases_257::{
        ACTIVE, FULL_DEPTH, LONG_WALK_DEADLINE, assert_root_first, chrome, document, fixture_order,
        plain_window_order, preorder, spawn,
    },
    invalidate_snapshot_cache,
    snapshot_content_root::WalkState,
    snapshot_content_root::stop_reason_label,
    snapshot_deadline_seam_257::SnapshotDeadlineOverride,
    snapshot_element,
};

/// Criteria cap on reported `skipped_subtree_roots`.
const MAX_SKIPPED_ROOTS: usize = 32;

/// More elements than the 256-element discovery visit cap at BFS level 2.
const WIDE_CHILDREN: usize = 300;

/// Window snapshot whose main walk deadline is lengthened for this window only
/// (discovery keeps its own fixed SNAPSHOT_DEADLINE / 4 bound).
fn long_snapshot(fixture: &Fixture) -> AccessibleSubtree {
    let _deadline = SnapshotDeadlineOverride::for_window(fixture.model.hwnd, LONG_WALK_DEADLINE);
    fixture.snapshot(FULL_DEPTH)
}

/// Chrome toolbar, then the active Document, then a status bar; returns the
/// toolbar index (the first element discovery visits).
fn chrome_document_status(model: &mut Model) -> usize {
    let toolbar = chrome(model, 3);
    let active = document(model, 0, ACTIVE);
    model.active = active;
    model.push(Some(0), "257 status after", UIA_ButtonControlTypeId);
    toolbar
}

/// Contract of `stop_reason_label` (its doc comment): log label = wire spelling.
#[test]
fn must_pass_257_stop_reason_label_matches_wire_spelling_for_every_variant() {
    for reason in [
        SnapshotStopReason::NodeBudget,
        SnapshotStopReason::Deadline,
        SnapshotStopReason::DepthLimit,
        SnapshotStopReason::ProviderError,
    ] {
        let wire = serde_json::to_value(reason)
            .unwrap_or_else(|error| panic!("serialize {reason:?}: {error}"));
        assert_eq!(
            Value::from(stop_reason_label(reason)),
            wire,
            "#257: log label of {reason:?} differs from its wire spelling"
        );
    }
}

/// ADDENDUM r2 "Discovery has its own bound: at most SNAPSHOT_DEADLINE/4; when it
/// runs out it selects nothing and the normal walk continues with the
/// remaining main deadline". The fixture's 450 ms Name stall on the first
/// level-1 chrome element lands past the 100 ms discovery bound; the main walk
/// deadline is lengthened, so only discovery's own bound can stop it.
#[test]
fn must_fail_257_discovery_stall_past_its_own_bound_selects_nothing_and_walk_continues() {
    let fixture = spawn(true, |model| {
        let toolbar = chrome_document_status(model);
        model.slow = Some(toolbar);
    });
    let tree = long_snapshot(&fixture);
    assert!(
        fixture.model.delayed.load(Ordering::SeqCst),
        "#257: the discovery stall never ran"
    );
    assert_root_first(&tree);
    assert!(
        tree.selected_content_root.is_none(),
        "#257: discovery ran past its own deadline and selected {:?}",
        tree.selected_content_root
    );
    assert!(tree.completeness_scope.is_none());
    assert_eq!(
        fixture_order(&tree, &fixture.model),
        plain_window_order(&fixture.model),
        "#257: after giving up, the plain window walk must still collect everything"
    );
}

/// Criteria "Discovery is bounded: breadth-first, at most 256 visited
/// elements". The only Document sits at BFS level 2 behind 300 level-2
/// siblings, so the visit cap ends discovery before it is reached.
#[test]
fn must_fail_257_document_beyond_discovery_visit_cap_is_not_selected() {
    let fixture = spawn(true, |model| {
        let wide = model.push(Some(0), "257 wide chrome", UIA_GroupControlTypeId);
        for index in 0..WIDE_CHILDREN {
            model.push(
                Some(wide),
                &format!("257 wide {index}"),
                UIA_ButtonControlTypeId,
            );
        }
        let container = model.push(Some(0), "257 content container", UIA_GroupControlTypeId);
        let active = document(model, container, ACTIVE);
        model.active = active;
    });
    let tree = long_snapshot(&fixture);
    assert_root_first(&tree);
    assert!(
        tree.selected_content_root.is_none(),
        "#257: Document beyond the 256-element visit cap was selected: {:?}",
        tree.selected_content_root
    );
    assert!(tree.completeness_scope.is_none());
    assert_eq!(
        fixture_order(&tree, &fixture.model),
        plain_window_order(&fixture.model),
        "#257: no selection must keep the plain window DFS order"
    );
}

/// Criteria "Gecko window (root class MozillaWindowClass OR root FrameworkId
/// Gecko)": the window class alone is enough.
#[test]
fn must_pass_257_mozilla_window_class_alone_selects_the_active_document() {
    let fixture = spawn(true, |model| {
        model.gecko_framework = false; // FrameworkId "Win32"
        chrome_document_status(model);
    });
    let selected = fixture.id(fixture.model.active);
    let tree = long_snapshot(&fixture);
    assert_root_first(&tree);
    assert_eq!(
        tree.selected_content_root.as_ref(),
        Some(&selected),
        "#257: class MozillaWindowClass alone must mark a Gecko window"
    );
}

/// Criteria "Gecko window (root class MozillaWindowClass OR root FrameworkId
/// Gecko)": the FrameworkId alone is enough.
#[test]
fn must_pass_257_gecko_framework_id_alone_selects_the_active_document() {
    let fixture = spawn(false, |model| {
        model.gecko_framework = true; // generic window class, FrameworkId "Gecko"
        chrome_document_status(model);
    });
    let selected = fixture.id(fixture.model.active);
    let tree = long_snapshot(&fixture);
    assert_root_first(&tree);
    assert_eq!(
        tree.selected_content_root.as_ref(),
        Some(&selected),
        "#257: FrameworkId Gecko alone must mark a Gecko window"
    );
}

/// Criteria "skipped_subtree_roots: dedup, capped at 32" and "stop_reason: first
/// limit that made the tree incomplete", on the real walk state.
#[test]
fn must_pass_257_skipped_subtree_roots_are_deduplicated_capped_and_first_reason_wins() {
    let id = |index: usize| element_id(0x257, &format!("{index:08x}"));
    let mut repeated = WalkState::default();
    repeated.record_skipped(SnapshotStopReason::Deadline, Some(&id(0)));
    repeated.record_skipped(SnapshotStopReason::NodeBudget, Some(&id(0)));
    let tree = repeated.into_subtree(id(999), Vec::new(), FULL_DEPTH);
    assert_eq!(
        tree.skipped_subtree_roots,
        vec![id(0)],
        "#257: a root recorded twice must be reported once"
    );
    assert_eq!(tree.stop_reason, Some(SnapshotStopReason::Deadline));
    assert!(tree.truncated);

    let mut many = WalkState::default();
    for index in 0..MAX_SKIPPED_ROOTS + 8 {
        many.record_skipped(SnapshotStopReason::ProviderError, Some(&id(index)));
    }
    let tree = many.into_subtree(id(999), Vec::new(), FULL_DEPTH);
    assert_eq!(
        tree.skipped_subtree_roots,
        (0..MAX_SKIPPED_ROOTS).map(id).collect::<Vec<_>>(),
        "#257: skipped_subtree_roots must keep the first 32 distinct roots"
    );
    assert_eq!(tree.stop_reason, Some(SnapshotStopReason::ProviderError));
}

/// Criteria (#77 behaviour kept by #257): a depth cut marks the tree incomplete
/// only when it hides real children. Element scope (no window proxy elements)
/// at depth 1 puts the Document's controls, which have no children, exactly at
/// max depth: the snapshot is complete.
#[test]
fn must_pass_257_complete_tree_with_leaves_at_max_depth_is_not_truncated() {
    let fixture = spawn(true, |model| {
        chrome_document_status(model);
    });
    let page = fixture.id(fixture.model.active);
    invalidate_snapshot_cache();
    let tree = snapshot_element(&page, 1)
        .unwrap_or_else(|error| panic!("#257 real element snapshot failed: {error}"));
    let names: Vec<String> = tree.nodes.iter().map(|node| node.name.clone()).collect();
    assert_eq!(
        names,
        preorder(&fixture.model, fixture.model.active),
        "#257: depth-1 element snapshot must hold the Document and its controls"
    );
    assert!(
        !tree.truncated,
        "#257: leaves at max depth hide nothing, yet truncated: {:?}",
        tree.stop_reason
    );
    assert!(tree.stop_reason.is_none());
}
