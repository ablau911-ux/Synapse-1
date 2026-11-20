//! NEEDS-SEAM in m4.rs:
//! snapshot_visible_window_hwnds_with(read:impl FnOnce()->Result<Vec<ForegroundContext>,ErrorData>)
//! -> Result<HashSet<i64>,ErrorData>;
//! select_launch_window_with_process_times<'a>(contexts:&'a [ForegroundContext],pid:u32,
//! regex:&regex::Regex,excluded:&HashSet<i64>,name:&str,args:&[String],launch_created:u64,
//! process:impl FnMut(u32)->Option<(u32,u64)>)->Option<&'a ForegroundContext>;
//! process returns (parent_pid, creation_time) in one generation-consistent read.
//! Real selector must use this, including broker routes. No replacement algorithm here.
use super::*;

fn window(pid: u32) -> ForegroundContext {
    serde_json::from_value(json!({"hwnd":249,"pid":pid,"process_name":"calculator.exe",
        "process_path":"C:\\fixture\\calculator.exe", "window_title":"Calculator fixture",
        "window_bounds":{"x":0,"y":0,"w":200,"h":100},"monitor_index":0,
        "dpi_scale":1.0,"is_fullscreen":false,"is_dwm_composed":true}))
    .expect("fixture")
}

#[test]
fn must_fail_prelaunch_enumeration_error_is_not_an_empty_successful_snapshot() {
    let mut calls = 0;
    let result = snapshot_visible_window_hwnds_with(|| {
        calls += 1;
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "synthetic enumeration failure",
        ))
    });
    let error = result.expect_err("failed snapshot must abort launch attribution");
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|v| v.get("code"))
            .and_then(serde_json::Value::as_str),
        Some(error_codes::A11Y_NOT_AVAILABLE)
    );
    assert_eq!(calls, 1);
}

#[test]
fn must_pass_new_descendant_with_verified_creation_order_can_supply_window() {
    let contexts = [window(101)];
    let regex = regex::Regex::new("^Calculator fixture$").expect("regex");
    let processes = std::collections::BTreeMap::from([(100, (1, 1000)), (101, (100, 1001))]);
    assert_eq!(processes[&101], (100, 1001));
    let selected = LaunchedTree::new(100, 1000, |pid| processes.get(&pid).copied(), |_| false)
        .select_window(&contexts, &regex, &HashSet::new(), "calc.exe", &[]);
    assert_eq!(selected.map(|c| (c.hwnd, c.pid)), Some((249, 101)));
}

#[test]
fn must_fail_preexisting_hwnd_with_matching_title_and_verified_pid_is_not_selected()
-> Result<(), regex::Error> {
    let regex = regex::Regex::new("^Calculator fixture$")?;
    let processes = std::collections::BTreeMap::from([(100, (1, 1000)), (101, (100, 1001))]);
    let excluded = HashSet::from([249]);

    // Neither a matching root PID nor valid descendant ancestry overrides the
    // prelaunch HWND snapshot. Only exclusion changes between these calls.
    for candidate_pid in [100, 101] {
        let contexts = [window(candidate_pid)];
        let eligible = LaunchedTree::new(100, 1000, |pid| processes.get(&pid).copied(), |_| false)
            .select_window(&contexts, &regex, &HashSet::new(), "calc.exe", &[]);
        assert_eq!(
            eligible.map(|c| (c.hwnd, c.pid)),
            Some((249, candidate_pid))
        );

        let selected = LaunchedTree::new(100, 1000, |pid| processes.get(&pid).copied(), |_| false)
            .select_window(&contexts, &regex, &excluded, "calc.exe", &[]);
        assert!(
            selected.is_none(),
            "prelaunch HWND 249 must remain excluded even for verified PID {candidate_pid}"
        );
    }
    Ok(())
}

#[test]
fn must_fail_old_unreadable_cyclic_or_reused_process_ancestry_cannot_attribute_window() {
    let regex = regex::Regex::new("^Calculator fixture$").expect("regex");
    for rows in [
        vec![(100, (1, 1000)), (101, (100, 999))],
        vec![(100, (1, 1000))],
        vec![(100, (1, 1000)), (101, (101, 1001))],
        vec![(100, (1, 2000)), (101, (100, 1001))],
    ] {
        let contexts = [window(101)];
        let processes = rows
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        let before = processes.clone();
        let selected = LaunchedTree::new(100, 1000, |pid| processes.get(&pid).copied(), |_| false)
            .select_window(&contexts, &regex, &HashSet::new(), "calc.exe", &[]);
        assert_eq!(selected.map(|c| c.hwnd), None);
        assert_eq!(processes, before);
    }
}
