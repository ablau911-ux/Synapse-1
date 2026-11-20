//! #302 frozen behaviour table (manager-owned, work method v3). Codex and the
//! review agent never edit this file. One case per L row of the table in Gitea
//! issue #302.

use super::{PerceptionPurpose, capture_baseline_required, webview2_probe_required};

#[test]
fn capture_baseline_required_table_302() {
    // (row, purpose, a11y_only, pixels_needed_for_detection, expected)
    let rows: [(&str, PerceptionPurpose, bool, bool, bool); 6] = [
        (
            "302a_find_auto_needs_no_capture",
            PerceptionPurpose::Find,
            false,
            false,
            false,
        ),
        (
            "302b_observe_auto_captures",
            PerceptionPurpose::Observe,
            false,
            false,
            true,
        ),
        (
            "302c_find_a11y_only_no_capture",
            PerceptionPurpose::Find,
            true,
            false,
            false,
        ),
        (
            "302c_observe_a11y_only_no_capture",
            PerceptionPurpose::Observe,
            true,
            false,
            false,
        ),
        (
            "302d_find_pixel_mode_still_captures",
            PerceptionPurpose::Find,
            false,
            true,
            true,
        ),
        (
            "302d_hostile_a11y_only_wins_over_pixels",
            PerceptionPurpose::Find,
            true,
            true,
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (row, purpose, a11y_only, pixels, expected) in rows {
        let got = capture_baseline_required(purpose, a11y_only, pixels);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#302 table rows failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn webview2_probe_required_table_302() {
    // (row, process_is_chromium_family, has_chromium_child_window, expected)
    let rows: [(&str, bool, bool, bool); 4] = [
        (
            "302e_plain_native_window_is_not_probed",
            false,
            false,
            false,
        ),
        (
            "302f_native_host_with_chromium_child_is_probed",
            false,
            true,
            true,
        ),
        (
            "302g_chromium_process_is_not_a_webview2_host_probe",
            true,
            false,
            false,
        ),
        (
            "302g_chromium_process_with_child_not_probed",
            true,
            true,
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (row, chromium, child, expected) in rows {
        let got = webview2_probe_required(chromium, child);
        if got != expected {
            failures.push(format!("row {row}: {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#302 table rows failed:\n{}",
        failures.join("\n")
    );
}
