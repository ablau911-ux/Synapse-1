//! #2346 (roadmap item #585): which processes belong to one `act_launch`. A
//! window counts as the launch's own when its process is the launched
//! process, a process in the launch's kill-on-close job (that membership
//! survives a launcher that has already exited, as Firefox's does), or a live
//! descendant of the launched process. A process created before the launch
//! never counts. The two OS reads (process record, job membership) are passed
//! in, so the rule is tested without a real job. #2401: the window comes with
//! the rule that accepted it (`WindowOwner`), and a wait that found no window
//! is explained by `LaunchWindowTimeout` (ambiguous, or refused as
//! stale/foreign).

use std::collections::HashSet;

use rmcp::schemars::JsonSchema;
use serde::Serialize;
use synapse_core::ForegroundContext;

use super::select_launch_window;

#[cfg(test)]
#[path = "tests/launch_tree_table_2346.rs"]
mod launch_tree_table_2346;

#[cfg(test)]
#[path = "tests/launch_tree_table_2401.rs"]
mod launch_tree_table_2401;

/// Deepest parent chain the live-parent walk follows (as before #2346).
const MAX_PARENT_WALK: usize = 64;

/// One launch: its process, that process's creation time, and the two OS
/// reads the window choice needs.
pub(super) struct LaunchedTree<P, J> {
    pid: u32,
    created: u64,
    /// (parent pid, creation time) of a live process; `None` when it has
    /// exited or cannot be read.
    process: P,
    /// Whether a process is in the launch's job (always `false` for a launch
    /// without one).
    in_job: J,
}

impl<P, J> LaunchedTree<P, J>
where
    P: FnMut(u32) -> Option<(u32, u64)>,
    J: FnMut(u32) -> bool,
{
    pub(super) const fn new(pid: u32, created: u64, process: P, in_job: J) -> Self {
        Self {
            pid,
            created,
            process,
            in_job,
        }
    }

    /// The one visible window, not seen before the launch and matching the
    /// title, whose process belongs to this launch; `None` when there is none
    /// or more than one. Production asks `select_window_with_rule`; this
    /// window-only form is kept for the #2346 / #249 tables.
    #[cfg(test)]
    pub(super) fn select_window<'a>(
        &mut self,
        contexts: &'a [ForegroundContext],
        title_regex: &regex::Regex,
        excluded: &HashSet<i64>,
        launch_target_name: &str,
        launch_args: &[String],
    ) -> Option<&'a ForegroundContext> {
        self.select_window_with_rule(
            contexts,
            title_regex,
            excluded,
            launch_target_name,
            launch_args,
        )
        .map(|(context, _)| context)
    }

    /// #2401: the one eligible window (as `select_window`) and the rule that
    /// made it this launch's own; `None` when there is none or more than one.
    pub(super) fn select_window_with_rule<'a>(
        &mut self,
        contexts: &'a [ForegroundContext],
        title_regex: &regex::Regex,
        excluded: &HashSet<i64>,
        launch_target_name: &str,
        launch_args: &[String],
    ) -> Option<(&'a ForegroundContext, WindowOwner)> {
        match self
            .eligible_windows(
                contexts,
                title_regex,
                excluded,
                launch_target_name,
                launch_args,
            )
            .as_slice()
        {
            [only] => Some(*only),
            _ => None,
        }
    }

    /// #2401: what the windows of a wait that ends now say: how many windows
    /// of this launch match (two or more: ambiguous) and how many other title
    /// matches the ownership rule refused (seen before the launch, or not a
    /// process of this launch).
    pub(super) fn at_timeout(
        &mut self,
        contexts: &[ForegroundContext],
        title_regex: &regex::Regex,
        excluded: &HashSet<i64>,
        launch_target_name: &str,
        launch_args: &[String],
    ) -> LaunchWindowTimeout {
        let eligible = self.eligible_windows(
            contexts,
            title_regex,
            excluded,
            launch_target_name,
            launch_args,
        );
        let refused = contexts
            .iter()
            .filter(|context| {
                context.pid != self.pid
                    && title_regex.is_match(&context.window_title)
                    && !eligible
                        .iter()
                        .any(|(window, _)| window.hwnd == context.hwnd)
            })
            .count();
        LaunchWindowTimeout::new(self.pid, eligible.len(), refused)
    }

    /// Every visible window, not seen before the launch and matching the
    /// title, whose process belongs to this launch, with the rule that
    /// accepted it: the one rule behind `select_window`,
    /// `select_window_with_rule` and `at_timeout`.
    fn eligible_windows<'a>(
        &mut self,
        contexts: &'a [ForegroundContext],
        title_regex: &regex::Regex,
        excluded: &HashSet<i64>,
        launch_target_name: &str,
        launch_args: &[String],
    ) -> Vec<(&'a ForegroundContext, WindowOwner)> {
        contexts
            .iter()
            .filter(|context| {
                // Both visible and hidden-desktop waits use the same tested HWND/title
                // selector. The process proof below must hold for this candidate's PID;
                // executable-family or shell-activation names never replace that proof.
                select_launch_window(
                    std::slice::from_ref(*context),
                    context.pid,
                    title_regex,
                    excluded,
                    launch_target_name,
                    launch_args,
                )
                .is_some()
            })
            .filter_map(|context| self.owner(context.pid).map(|owner| (context, owner)))
            .collect()
    }

    /// The rule that makes `pid` this launch's own, if any. A job member counts
    /// when it was created at or after the launch (the job is asked first); any
    /// other process (the launched one included) needs the live-parent walk.
    fn owner(&mut self, pid: u32) -> Option<WindowOwner> {
        if pid != self.pid && (self.in_job)(pid) {
            return (self.process)(pid)
                .is_some_and(|(_, created)| created >= self.created)
                .then_some(WindowOwner::JobMember);
        }
        if !self.descends_from_launch(pid) {
            return None;
        }
        Some(if pid == self.pid {
            WindowOwner::LaunchedProcess
        } else {
            WindowOwner::LiveDescendant
        })
    }

    /// The live-parent walk of before #2346, unchanged: every process on the
    /// way up is alive, created at or after the launch and no later than its
    /// child, and the walk reaches the launched process with its exact
    /// creation time.
    fn descends_from_launch(&mut self, pid: u32) -> bool {
        let mut current = pid;
        let mut child_created = u64::MAX;
        let mut seen = HashSet::new();
        for _ in 0..MAX_PARENT_WALK {
            if !seen.insert(current) {
                return false;
            }
            let Some((parent, created)) = (self.process)(current) else {
                return false;
            };
            if created < self.created || created > child_created {
                return false;
            }
            if current == self.pid {
                return created == self.created;
            }
            current = parent;
            child_created = created;
        }
        false
    }
}

/// #2401: the rule that made a window this launch's own (the launch reply's
/// `window_owner_rule`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowOwner {
    /// The window's process is the launched process.
    LaunchedProcess,
    /// A process of the launch's kill-on-close job created at or after the
    /// launch (its parent may have exited, as Firefox's launcher does).
    JobMember,
    /// A live descendant of the launched process.
    LiveDescendant,
}

/// #2401: why a launch's window wait ended without a window, for the error's
/// `rejection_explanation`: two or more windows of this launch matched the
/// title (ambiguous), and/or title matches of other processes were refused by
/// the ownership rule (stale / foreign).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LaunchWindowTimeout {
    pid: u32,
    eligible: usize,
    refused: usize,
}

impl LaunchWindowTimeout {
    /// `pid` is the launched process; `eligible` the windows of this launch
    /// that matched the title; `refused` the title matches of other processes
    /// that are not this launch's.
    pub(super) const fn new(pid: u32, eligible: usize, refused: usize) -> Self {
        Self {
            pid,
            eligible,
            refused,
        }
    }

    /// The explanation; `None` when no window matched the title at all.
    pub(super) fn explanation(&self) -> Option<String> {
        let pid = self.pid;
        let ambiguous = (self.eligible > 1).then(|| {
            format!(
                "ambiguous: {} windows of this launch match the title regex (launched pid {pid}), and act_launch accepts exactly one. Narrow the title regex so it matches one window of the launch.",
                self.eligible
            )
        });
        let refused = (self.refused > 0).then(|| {
            format!(
                "{} visible window(s) match the title regex but are owned by a different pid than the launched process (pid {pid}). act_launch only accepts a title match owned by the launched process, a process of its launch job created at or after the launch, or a live descendant of the launched process, so these were rejected as stale/foreign instances. Close the stale window(s) and retry, wait for the launched pid's own window, or — if the target intentionally re-execs into another process — match on that pid instead.",
                self.refused
            )
        });
        match (ambiguous, refused) {
            (Some(ambiguous), Some(refused)) => Some(format!("{ambiguous} {refused}")),
            (ambiguous, refused) => ambiguous.or(refused),
        }
    }
}
