//! FROZEN behaviour table of Gitea #2346 (roadmap item #585), rows J1-J7.
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/tests/launch_tree_table_2346.rs, beside the
//! seam tests of #249 (registered by the `#[path]` line in m4_launch_tree.rs).
//!
//! One launch: process 200, created at 1000. `processes` lists the LIVE
//! processes as (pid, (parent pid, creation time)); a pid missing from it has
//! exited. `job` lists the pids in the launch's kill-on-close job (injected,
//! no real job). Every window is titled "Fixture Browser". Each row that
//! expects "not selected" also has a control case that must be selected, so
//! no row passes on an object that selects nothing.

use std::collections::{BTreeMap, HashSet};

use serde_json::json;
use synapse_core::ForegroundContext;

use super::LaunchedTree;

const LAUNCHED_PID: u32 = 200;
const LAUNCH_CREATED: u64 = 1000;

struct Case {
    name: &'static str,
    /// Visible windows: (hwnd, owning pid).
    windows: &'static [(i64, u32)],
    /// Live processes: (pid, (parent pid, creation time)).
    processes: &'static [(u32, (u32, u64))],
    /// Pids in the launch's job.
    job: &'static [u32],
    /// Hwnds that were visible before the launch.
    excluded: &'static [i64],
    /// The hwnd the launch must select, or `None`.
    selected: Option<i64>,
}

fn window(hwnd: i64, pid: u32) -> Result<ForegroundContext, String> {
    serde_json::from_value(json!({
        "hwnd": hwnd, "pid": pid, "process_name": "firefox.exe",
        "process_path": "C:\\fixture\\firefox.exe", "window_title": "Fixture Browser",
        "window_bounds": {"x": 0, "y": 0, "w": 200, "h": 100}, "monitor_index": 0,
        "dpi_scale": 1.0, "is_fullscreen": false, "is_dwm_composed": true
    }))
    .map_err(|error| format!("window fixture {hwnd}: {error}"))
}

#[test]
fn launch_tree_table_2346() -> Result<(), String> {
    let cases = [
        Case {
            name: "J1_window_of_the_launched_process",
            windows: &[(11, 200)],
            processes: &[(200, (1, 1000))],
            job: &[200],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            name: "J2_child_of_the_launched_process_both_alive",
            windows: &[(11, 201)],
            processes: &[(200, (1, 1000)), (201, (200, 1001))],
            job: &[200, 201],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            // As today for a launch outside a session (no job): the live
            // parent walk still proves the child.
            name: "J2_child_both_alive_launch_without_a_job",
            windows: &[(11, 201)],
            processes: &[(200, (1, 1000)), (201, (200, 1001))],
            job: &[],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            // The Firefox case: the launcher (200) has exited, its browser
            // process (201) is in the job and owns the window.
            name: "J3_job_member_whose_launcher_exited",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            // A same-titled window of a process outside the job (the owner's
            // own Firefox), even one started during the wait.
            name: "J4_window_of_a_process_outside_the_job",
            windows: &[(11, 300)],
            processes: &[(201, (200, 1001)), (300, (4, 1002))],
            job: &[201],
            excluded: &[],
            selected: None,
        },
        Case {
            name: "J4_control_the_job_member_window_is_selected",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001)), (300, (4, 1002))],
            job: &[201],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            name: "J5_job_member_created_before_the_launch",
            windows: &[(11, 201)],
            processes: &[(201, (200, 999))],
            job: &[201],
            excluded: &[],
            selected: None,
        },
        Case {
            name: "J5_control_the_same_member_created_after_the_launch",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            name: "J6_two_job_member_windows_match_ambiguous",
            windows: &[(11, 201), (12, 202)],
            processes: &[(201, (200, 1001)), (202, (200, 1002))],
            job: &[201, 202],
            excluded: &[],
            selected: None,
        },
        Case {
            name: "J6_control_one_of_them_alone_is_selected",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001)), (202, (200, 1002))],
            job: &[201, 202],
            excluded: &[],
            selected: Some(11),
        },
        Case {
            name: "J7_prelaunch_window_now_owned_by_a_job_member",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            excluded: &[11],
            selected: None,
        },
        Case {
            name: "J7_control_the_same_window_not_seen_before_is_selected",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            excluded: &[],
            selected: Some(11),
        },
    ];

    let title = regex::Regex::new("^Fixture Browser$").map_err(|error| error.to_string())?;
    let mut failures = Vec::new();
    for case in &cases {
        let contexts = case
            .windows
            .iter()
            .map(|&(hwnd, pid)| window(hwnd, pid))
            .collect::<Result<Vec<_>, _>>()?;
        let processes: BTreeMap<u32, (u32, u64)> = case.processes.iter().copied().collect();
        let excluded: HashSet<i64> = case.excluded.iter().copied().collect();
        let mut tree = LaunchedTree::new(
            LAUNCHED_PID,
            LAUNCH_CREATED,
            |pid| processes.get(&pid).copied(),
            |pid| case.job.contains(&pid),
        );
        let observed = tree
            .select_window(&contexts, &title, &excluded, "firefox.exe", &[])
            .map(|context| context.hwnd);
        if observed != case.selected {
            failures.push(format!(
                "{}: expected {:?}, observed {observed:?}",
                case.name, case.selected
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} of {} cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        ))
    }
}
