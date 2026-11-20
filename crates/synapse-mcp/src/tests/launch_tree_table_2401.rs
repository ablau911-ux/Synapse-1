//! FROZEN behaviour table of Gitea #2401 (review of build 085eafa2), rows
//! J8-J12 and T1-T3. Written from the issue table before the code. Coders
//! never edit this file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/tests/launch_tree_table_2401.rs, beside
//! `launch_tree_table_2346.rs` (registered by the `#[path]` line of the #2401
//! stub in m4_launch_tree.rs). Table 2346 (J1-J7, `select_window`) stays.
//!
//! One launch: process 200, created at 1000 (as in table 2346). `processes`
//! lists the LIVE processes as (pid, (parent pid, creation time)); a pid
//! missing from it has exited. `job` lists the pids in the launch's
//! kill-on-close job (injected, no real job). Every window is titled
//! "Fixture Browser"; no window was seen before the launch.
//! J rows: the window `select_window_with_rule` selects and the rule it names.
//! T rows: the `rejection_explanation` of a wait that timed out on these
//! windows (`at_timeout(..).explanation()`).
//! J12 would pass on a stub that selects nothing, so it has a control case
//! that must be selected. T3 would pass on an explanation that is always
//! `None`; T1 and T2 fail on such a stub.

use std::collections::{BTreeMap, HashSet};

use serde_json::json;
use synapse_core::ForegroundContext;

use super::{LaunchedTree, WindowOwner};

const LAUNCHED_PID: u32 = 200;
const LAUNCH_CREATED: u64 = 1000;
const TITLE: &str = "^Fixture Browser$";

/// The `rejection_explanation` of build 085eafa2 (m4.rs `launch_window_error`)
/// for one refused title match of launch pid 200, word for word: T2 keeps it.
const STALE_OR_FOREIGN_ONE_WINDOW: &str = "1 visible window(s) match the title regex but are owned by a different pid than the launched process (pid 200). act_launch only accepts a title match owned by the launched process, a process of its launch job created at or after the launch, or a live descendant of the launched process, so these were rejected as stale/foreign instances. Close the stale window(s) and retry, wait for the launched pid's own window, or — if the target intentionally re-execs into another process — match on that pid instead.";

#[derive(Clone, Copy)]
enum Then {
    /// `select_window_with_rule` selects this hwnd and names this rule.
    Selected(i64, WindowOwner),
    /// `select_window_with_rule` selects nothing (and so names no rule).
    NotSelected,
    /// The explanation says "ambiguous: <count> windows of this launch match"
    /// and contains neither "stale" nor "foreign".
    Ambiguous { count: usize },
    /// The explanation is exactly this text.
    Explains(&'static str),
    /// No explanation (`None`), as today when no window matches the title.
    NoExplanation,
}

struct Case {
    name: &'static str,
    /// Visible windows: (hwnd, owning pid).
    windows: &'static [(i64, u32)],
    /// Live processes: (pid, (parent pid, creation time)).
    processes: &'static [(u32, (u32, u64))],
    /// Pids in the launch's job.
    job: &'static [u32],
    title_regex: &'static str,
    then: Then,
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
fn launch_tree_table_2401() -> Result<(), String> {
    let cases = [
        Case {
            name: "J8_window_of_the_launched_process",
            windows: &[(11, 200)],
            processes: &[(200, (1, 1000))],
            job: &[200],
            title_regex: TITLE,
            then: Then::Selected(11, WindowOwner::LaunchedProcess),
        },
        Case {
            // The Firefox case: the launcher (200) has exited.
            name: "J9_job_member_whose_parent_exited",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            title_regex: TITLE,
            then: Then::Selected(11, WindowOwner::JobMember),
        },
        Case {
            name: "J10_live_child_no_job",
            windows: &[(11, 201)],
            processes: &[(200, (1, 1000)), (201, (200, 1001))],
            job: &[],
            title_regex: TITLE,
            then: Then::Selected(11, WindowOwner::LiveDescendant),
        },
        Case {
            // The job is asked first, as today.
            name: "J11_job_member_that_is_also_a_live_child",
            windows: &[(11, 201)],
            processes: &[(200, (1, 1000)), (201, (200, 1001))],
            job: &[200, 201],
            title_regex: TITLE,
            then: Then::Selected(11, WindowOwner::JobMember),
        },
        Case {
            name: "J12_no_window_at_all",
            windows: &[],
            processes: &[(200, (1, 1000))],
            job: &[200],
            title_regex: TITLE,
            then: Then::NotSelected,
        },
        Case {
            name: "J12_only_a_window_outside_the_launch",
            windows: &[(11, 300)],
            processes: &[(200, (1, 1000)), (300, (4, 1002))],
            job: &[200],
            title_regex: TITLE,
            then: Then::NotSelected,
        },
        Case {
            name: "J12_control_the_launched_process_window_is_selected",
            windows: &[(11, 200)],
            processes: &[(200, (1, 1000)), (300, (4, 1002))],
            job: &[200],
            title_regex: TITLE,
            then: Then::Selected(11, WindowOwner::LaunchedProcess),
        },
        Case {
            name: "T1_two_eligible_windows_ambiguous",
            windows: &[(11, 201), (12, 202)],
            processes: &[(201, (200, 1001)), (202, (200, 1002))],
            job: &[201, 202],
            title_regex: TITLE,
            then: Then::Ambiguous { count: 2 },
        },
        Case {
            name: "T2_one_title_match_outside_the_launch",
            windows: &[(11, 300)],
            processes: &[(200, (1, 1000)), (300, (4, 1002))],
            job: &[200],
            title_regex: TITLE,
            then: Then::Explains(STALE_OR_FOREIGN_ONE_WINDOW),
        },
        Case {
            name: "T3_no_title_match_at_all",
            windows: &[(11, 201)],
            processes: &[(201, (200, 1001))],
            job: &[201],
            title_regex: "^No Such Window$",
            then: Then::NoExplanation,
        },
    ];

    let excluded = HashSet::new();
    let mut failures = Vec::new();
    for case in &cases {
        let contexts = case
            .windows
            .iter()
            .map(|&(hwnd, pid)| window(hwnd, pid))
            .collect::<Result<Vec<_>, _>>()?;
        let processes: BTreeMap<u32, (u32, u64)> = case.processes.iter().copied().collect();
        let title = regex::Regex::new(case.title_regex).map_err(|error| error.to_string())?;
        let mut tree = LaunchedTree::new(
            LAUNCHED_PID,
            LAUNCH_CREATED,
            |pid| processes.get(&pid).copied(),
            |pid| case.job.contains(&pid),
        );
        let failure = match case.then {
            Then::Selected(hwnd, rule) => {
                let observed = tree
                    .select_window_with_rule(&contexts, &title, &excluded, "firefox.exe", &[])
                    .map(|(context, rule)| (context.hwnd, rule));
                (observed != Some((hwnd, rule))).then(|| {
                    format!("expected window {hwnd} by rule {rule:?}, observed {observed:?}")
                })
            }
            Then::NotSelected => tree
                .select_window_with_rule(&contexts, &title, &excluded, "firefox.exe", &[])
                .map(|(context, rule)| {
                    format!(
                        "expected nothing selected and no rule, observed window {} by rule {rule:?}",
                        context.hwnd
                    )
                }),
            Then::Ambiguous { count } => {
                let text = tree
                    .at_timeout(&contexts, &title, &excluded, "firefox.exe", &[])
                    .explanation();
                let wanted = format!("ambiguous: {count} windows of this launch match");
                match &text {
                    Some(text)
                        if text.contains(&wanted)
                            && !text.contains("stale")
                            && !text.contains("foreign") =>
                    {
                        None
                    }
                    _ => Some(format!(
                        "expected an explanation with \"{wanted}\" and no stale/foreign sentence, observed {text:?}"
                    )),
                }
            }
            Then::Explains(expected) => {
                let text = tree
                    .at_timeout(&contexts, &title, &excluded, "firefox.exe", &[])
                    .explanation();
                (text.as_deref() != Some(expected))
                    .then(|| format!("expected {expected:?}, observed {text:?}"))
            }
            Then::NoExplanation => tree
                .at_timeout(&contexts, &title, &excluded, "firefox.exe", &[])
                .explanation()
                .map(|text| format!("expected no explanation, observed {text:?}")),
        };
        if let Some(failure) = failure {
            failures.push(format!("{}: {failure}", case.name));
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
