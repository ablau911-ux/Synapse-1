//! FROZEN behaviour table of Gitea #2347 (roadmap item #571), rows D1-D5.
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.

use super::SnapshotDepth;

/// One row: the depth the caller asked for, and what the snapshot must walk.
/// `None` = the row says nothing about that number.
struct Row {
    name: &'static str,
    requested: u32,
    /// Deepest level of the walk from the window root.
    window_walk: Option<u32>,
    /// Levels the search for the page's document node may go down.
    content_root_search: Option<u32>,
    /// (level of the document node, deepest level of the page part).
    page_part: Option<(u32, u32)>,
}

#[test]
fn snapshot_depth_table_2347() {
    let rows = [
        Row {
            name: "D1_plain_observe_depth_2",
            requested: 2,
            window_walk: Some(2),
            content_root_search: Some(16),
            page_part: None,
        },
        Row {
            // find / observe with interactable: as before #2347, the page
            // part also stops at level 16 (find is unchanged).
            name: "D2_find_depth_16",
            requested: 16,
            window_walk: Some(16),
            content_root_search: Some(16),
            page_part: Some((5, 16)),
        },
        Row {
            name: "D3_depth_0_root_only",
            requested: 0,
            window_walk: Some(0),
            content_root_search: Some(16),
            page_part: None,
        },
        Row {
            // Document at level 5: the page is collected down to 5 + 2 = 7.
            // The walk never descends below its deepest level, so a page node
            // at level 8 is cut.
            name: "D4_document_at_level_5_page_to_level_7",
            requested: 2,
            window_walk: None,
            content_root_search: None,
            page_part: Some((5, 7)),
        },
        Row {
            name: "D5_depth_40_above_the_public_maximum",
            requested: 40,
            window_walk: Some(16),
            content_root_search: Some(16),
            page_part: Some((5, 16)),
        },
    ];

    let mut failures = Vec::new();
    for row in &rows {
        let depth = SnapshotDepth::new(row.requested);
        if let Some(expected) = row.window_walk {
            let observed = depth.for_window_walk();
            if observed != expected {
                failures.push(format!(
                    "{}: window walk expected {expected}, observed {observed}",
                    row.name
                ));
            }
        }
        if let Some(expected) = row.content_root_search {
            let observed = depth.for_content_root_search();
            if observed != expected {
                failures.push(format!(
                    "{}: document search expected {expected}, observed {observed}",
                    row.name
                ));
            }
        }
        if let Some((document_level, expected)) = row.page_part {
            let observed = depth.below_content_root(document_level);
            if observed != expected {
                failures.push(format!(
                    "{}: page part below a document at level {document_level} expected down to {expected}, observed {observed}",
                    row.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} checks of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
