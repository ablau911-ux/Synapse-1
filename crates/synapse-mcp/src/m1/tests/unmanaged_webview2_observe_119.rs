//! #119: Unmanaged WebView2 host windows with incomplete UIA web content must
//! surface `A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE` and
//! `A11Y_CDP_UNREACHABLE` (`--remote-debugging-port` / `cdp_debug=true`) across
//! `enrich_input_with_cdp_for_target`, compact `observe`, and `find`.

use synapse_core::{CdpDiagnostics, CdpStatus, Rect, SensorStatus, WebPerceptionPath, error_codes};
use synapse_perception::{ObservationAssembler, ObservationInput, ObserveInclude};

use super::*;
use crate::m1::{
    FindParams, enrich_input_with_cdp_for_target, match_find_input, settle_cdp_snapshot_status,
};

fn webview_host_input(usable_controls: usize) -> ObservationInput {
    let bounds = Rect {
        x: 0,
        y: 0,
        w: 900,
        h: 700,
    };
    let mut fg = foreground_with_bounds(bounds);
    fg.process_name = "ContosoWebViewHost.exe".to_owned();
    fg.window_title = "Contoso Portal".to_owned();

    let mut root = node(0, 0, "Contoso Portal", "Window", false);
    root.bbox = bounds;
    let mut region = node(1, 1, "Contoso Portal - Web content", "region", false);
    region.bbox = Rect {
        x: 10,
        y: 40,
        w: 860,
        h: 620,
    };

    let mut elements = vec![root, region];
    for idx in 0..usable_controls {
        let seq = u32::try_from(idx + 2).unwrap_or(2);
        let mut ctrl = node(seq, 2, &format!("Action {idx}"), "button", false);
        ctrl.bbox = Rect {
            x: 24,
            y: 60 + i32::try_from(idx).unwrap_or(0) * 40,
            w: 120,
            h: 32,
        };
        elements.push(ctrl);
    }

    let mut input = ObservationInput::new(fg);
    input.elements = elements;
    input.a11y_status = SensorStatus::Healthy;
    input.capture_status = SensorStatus::Disabled;
    input
}

#[tokio::test]
async fn must_fail_unmanaged_webview2_host_degrades_observe_and_find_and_survives_compact_and_enrich()
 {
    let mut input = webview_host_input(1);
    mark_sparse_target_a11y(&mut input);
    input.cdp = Some(CdpDiagnostics::unreachable_with_probe(
        "ContosoWebViewHost.exe",
        error_codes::A11Y_CDP_UNREACHABLE,
        Vec::new(),
        "WebView2 browser child has no --remote-debugging-port; UIA may expose only part of the page. Launch this app through Synapse process launch with cdp_debug=true to enable complete page observation and background input",
    ));

    enrich_input_with_cdp_for_target(&mut input, 3, 300, None).await;

    let compact_include = ObserveInclude {
        diagnostics: false,
        ..ObserveInclude::default()
    };
    let obs = ObservationAssembler::new()
        .assemble(compact_include, input.clone())
        .expect("assemble compact observation");
    assert_eq!(
        obs.diagnostics.a11y_status,
        SensorStatus::DegradedSensorFailed {
            reason_code: A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE.to_owned(),
        }
    );
    assert_eq!(obs.diagnostics.web_path, Some(WebPerceptionPath::UiaOnly));
    let cdp = obs
        .diagnostics
        .cdp
        .expect("failed WebView2 CDP diagnostics must survive compact observe");
    assert_eq!(cdp.status, CdpStatus::Unreachable);
    assert_eq!(
        cdp.reason_code.as_deref(),
        Some(error_codes::A11Y_CDP_UNREACHABLE)
    );
    let detail = cdp.detail.unwrap_or_default();
    assert!(detail.contains("--remote-debugging-port"), "got: {detail}");
    assert!(detail.contains("cdp_debug=true"), "got: {detail}");

    let find = match_find_input(&input, &FindParams::default());
    assert_eq!(
        find.a11y_coverage.as_deref(),
        Some(A11Y_TARGET_WINDOW_WEBVIEW_CONTENT_INCOMPLETE)
    );
}

#[test]
fn must_fail_webview2_browser_debug_port_rejects_renderer_children_and_unflagged_browser_child() {
    let renderer_args = vec![
        "--type=renderer".to_owned(),
        "--remote-debugging-port=9229".to_owned(),
    ];
    assert_eq!(
        synapse_a11y::webview2_browser_debug_port("msedgewebview2.exe", &renderer_args),
        None
    );
    let unflagged_args = vec!["--no-first-run".to_owned()];
    assert_eq!(
        synapse_a11y::webview2_browser_debug_port("msedgewebview2.exe", &unflagged_args),
        None
    );
    let zero_port_args = vec!["--remote-debugging-port=0".to_owned()];
    assert_eq!(
        synapse_a11y::webview2_browser_debug_port("msedgewebview2.exe", &zero_port_args),
        None
    );
}

#[test]
fn must_pass_webview2_with_three_usable_controls_or_attached_cdp_tree_reports_healthy_coverage() {
    let browser_args = vec!["--remote-debugging-port=\"9333\"".to_owned()];
    assert_eq!(
        synapse_a11y::webview2_browser_debug_port("msedgewebview2.exe", &browser_args),
        Some(9333)
    );

    let mut rich_uia = webview_host_input(3);
    mark_sparse_target_a11y(&mut rich_uia);
    assert_eq!(rich_uia.a11y_status, SensorStatus::Healthy);
    assert_eq!(
        match_find_input(&rich_uia, &FindParams::default()).a11y_coverage,
        None
    );

    let mut sparse_then_cdp = webview_host_input(1);
    mark_sparse_target_a11y(&mut sparse_then_cdp);
    settle_cdp_snapshot_status(&mut sparse_then_cdp, 12, 12, 300, 0, 0);
    assert_eq!(sparse_then_cdp.a11y_status, SensorStatus::Healthy);
    assert_eq!(
        match_find_input(&sparse_then_cdp, &FindParams::default()).a11y_coverage,
        None
    );
}
