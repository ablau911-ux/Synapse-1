//! #79 / #293 frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row of the WebView2 owned-CDP
//! table: the host's debug port and the claimed window's page target.

use synapse_core::error_codes::ACTION_TARGET_INVALID;

use super::{
    WebView2HostScope, WebView2ProcessRow, resolve_owned_webview2_page_target,
    webview2_browser_port_in_tree,
};
use crate::CdpTargetSummary;

const HOST: u32 = 100;
const WEBVIEW: &str = "msedgewebview2.exe";
const DIRECT: WebView2HostScope = WebView2HostScope::DirectChildren;
const LAUNCHED: WebView2HostScope = WebView2HostScope::LaunchedTree;

fn process(pid: u32, parent_pid: u32, name: &str, arguments: &[&str]) -> WebView2ProcessRow {
    WebView2ProcessRow {
        pid,
        parent_pid: Some(parent_pid),
        name: name.to_owned(),
        arguments: arguments.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

fn browser(pid: u32, parent_pid: u32, port: u16) -> WebView2ProcessRow {
    let flag = format!("--remote-debugging-port={port}");
    process(pid, parent_pid, WEBVIEW, &[flag.as_str()])
}

/// A wrapper chain HOST -> 1001 -> 1002 ... with the browser at `depth`.
fn chain_with_browser_at(depth: u32, port: u16) -> Vec<WebView2ProcessRow> {
    let mut rows = Vec::new();
    let mut parent = HOST;
    for level in 1..depth {
        let pid = 1000 + level;
        rows.push(process(pid, parent, "wrapper.exe", &[]));
        parent = pid;
    }
    rows.push(browser(2000, parent, port));
    rows
}

#[test]
#[allow(clippy::option_option)]
fn webview2_host_port_table_79() {
    // (row, host pid, scope, process table, expected)
    let rows: Vec<(
        &str,
        u32,
        WebView2HostScope,
        Vec<WebView2ProcessRow>,
        Option<Option<u16>>,
    )> = vec![
        (
            "79a_neighbour_direct_child_browser_window_scope",
            HOST,
            DIRECT,
            vec![browser(200, HOST, 9227)],
            Some(Some(9227)),
        ),
        (
            "79a_neighbour_direct_child_browser_launched_scope",
            HOST,
            LAUNCHED,
            vec![browser(200, HOST, 9227)],
            Some(Some(9227)),
        ),
        (
            "79b_wrapper_launched_grandchild_browser_found",
            HOST,
            LAUNCHED,
            vec![process(200, HOST, "app.exe", &[]), browser(300, 200, 9228)],
            Some(Some(9228)),
        ),
        (
            "79b_boundary_launched_browser_at_depth_8_found",
            HOST,
            LAUNCHED,
            chain_with_browser_at(8, 9234),
            Some(Some(9234)),
        ),
        (
            "79b_boundary_launched_browser_at_depth_9_not_found",
            HOST,
            LAUNCHED,
            chain_with_browser_at(9, 9235),
            None,
        ),
        (
            "79c_someone_else_window_scope_never_walks_into_child_apps",
            HOST,
            DIRECT,
            vec![
                process(200, HOST, "teams.exe", &[]),
                browser(300, 200, 9229),
            ],
            None,
        ),
        (
            "79c_someone_else_unrelated_tree_with_port_ignored",
            HOST,
            LAUNCHED,
            vec![process(500, 1, "other.exe", &[]), browser(600, 500, 9230)],
            None,
        ),
        ("79d_empty_process_table", HOST, LAUNCHED, Vec::new(), None),
        (
            "79e_unmanaged_browser_child_without_port_is_unreachable",
            HOST,
            DIRECT,
            vec![process(
                200,
                HOST,
                WEBVIEW,
                &["--embedded-browser-webview=1"],
            )],
            Some(None),
        ),
        (
            "79e_renderer_only_with_port_is_not_a_browser",
            HOST,
            LAUNCHED,
            vec![process(
                200,
                HOST,
                WEBVIEW,
                &["--type=renderer", "--remote-debugging-port=9231"],
            )],
            None,
        ),
        (
            "79f_hostile_host_pid_zero_window_scope_matches_nothing",
            0,
            DIRECT,
            vec![browser(200, 0, 9232)],
            None,
        ),
        (
            "79f_hostile_host_pid_zero_launched_scope_matches_nothing",
            0,
            LAUNCHED,
            vec![browser(200, 0, 9232)],
            None,
        ),
        (
            "79f_hostile_parent_cycle_terminates_and_finds_browser",
            HOST,
            LAUNCHED,
            vec![
                process(200, HOST, "a.exe", &[]),
                process(HOST, 200, "host.exe", &[]),
                browser(300, 200, 9233),
            ],
            Some(Some(9233)),
        ),
        (
            "79g_second_actor_other_host_gets_only_its_own_port",
            110,
            DIRECT,
            vec![browser(200, HOST, 9301), browser(210, 110, 9302)],
            Some(Some(9302)),
        ),
        (
            "79h_two_browsers_nearest_then_lowest_pid_wins",
            HOST,
            LAUNCHED,
            vec![
                process(250, HOST, "wrap.exe", &[]),
                browser(300, 250, 9403),
                browser(220, HOST, 9402),
                browser(210, HOST, 9401),
            ],
            Some(Some(9401)),
        ),
    ];
    let mut failures = Vec::new();
    for (row, host_pid, scope, processes, expected) in rows {
        let got = webview2_browser_port_in_tree(host_pid, scope, &processes);
        if got != expected {
            failures.push(format!("row {row}: port {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#79 host port table rows failed:\n{}",
        failures.join("\n")
    );
}

fn target(target_id: &str, target_type: &str) -> CdpTargetSummary {
    CdpTargetSummary {
        target_id: target_id.to_owned(),
        target_type: target_type.to_owned(),
        title: String::new(),
        url: String::new(),
        attached: false,
    }
}

#[test]
fn webview2_owned_page_target_table_79() {
    const CLAIMED: i64 = 10;
    let none: &[CdpTargetSummary] = &[];
    let one_page: &[CdpTargetSummary] = &[target("T1", "page")];
    let two_pages: &[CdpTargetSummary] = &[target("T1", "page"), target("T2", "page")];
    let mixed: &[CdpTargetSummary] = &[
        target("S1", "service_worker"),
        target("T1", "page"),
        target("F1", "iframe"),
    ];
    let service_worker_only = &mixed[..1];
    // (row, claimed hwnd, requested hwnd, requested target id, targets, expected)
    let rows: [(
        &str,
        i64,
        Option<i64>,
        Option<&str>,
        &[CdpTargetSummary],
        Result<&str, &str>,
    ); 13] = [
        (
            "79i_single_page_no_request",
            CLAIMED,
            None,
            None,
            one_page,
            Ok("T1"),
        ),
        (
            "79i_single_page_requested_same",
            CLAIMED,
            None,
            Some("T1"),
            one_page,
            Ok("T1"),
        ),
        (
            "79i_page_among_service_worker_and_iframe",
            CLAIMED,
            None,
            None,
            mixed,
            Ok("T1"),
        ),
        (
            "79j_empty_targets",
            CLAIMED,
            None,
            None,
            none,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79j_no_page_type_target",
            CLAIMED,
            None,
            None,
            service_worker_only,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79k_two_pages_no_request_is_ambiguous",
            CLAIMED,
            None,
            None,
            two_pages,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79k_two_pages_requested_second",
            CLAIMED,
            None,
            Some("T2"),
            two_pages,
            Ok("T2"),
        ),
        (
            "79l_hostile_requested_unknown_target",
            CLAIMED,
            None,
            Some("T9"),
            two_pages,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79l_hostile_requested_non_page_target",
            CLAIMED,
            None,
            Some("F1"),
            mixed,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79l_empty_requested_target_id",
            CLAIMED,
            None,
            Some(""),
            one_page,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79m_someone_else_requested_hwnd_differs_from_claim",
            CLAIMED,
            Some(11),
            None,
            one_page,
            Err(ACTION_TARGET_INVALID),
        ),
        (
            "79m_requested_hwnd_matches_claim",
            CLAIMED,
            Some(CLAIMED),
            None,
            one_page,
            Ok("T1"),
        ),
        (
            "79n_boundary_null_claimed_hwnd",
            0,
            None,
            None,
            one_page,
            Err(ACTION_TARGET_INVALID),
        ),
    ];
    let mut failures = Vec::new();
    for (row, claimed, requested_hwnd, requested_target_id, targets, expected) in rows {
        let got = resolve_owned_webview2_page_target(
            claimed,
            requested_hwnd,
            requested_target_id,
            targets,
        );
        if got.as_deref().map_err(|error| *error) != expected {
            failures.push(format!("row {row}: {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#79 owned page target table rows failed:\n{}",
        failures.join("\n")
    );
}
