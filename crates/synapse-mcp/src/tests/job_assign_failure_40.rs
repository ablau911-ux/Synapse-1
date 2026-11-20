//! #40 regression: a shell child never runs outside its kill-on-close Windows job.
//!
//! Children are created suspended, placed in their job, and only then resumed. A
//! failed assignment must terminate the still-suspended child before it executes
//! anything, return a precise error, and record that error in the durable job
//! status. The assignment step is injected through the `*_with` seams, so the
//! failure path runs without admin rights or a debugger harness.

#![cfg(windows)]

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        System::{
            JobObjects::IsProcessInJob,
            Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, WaitForSingleObject,
            },
        },
    },
    core::BOOL,
};

use super::*;

/// File the child creates in its working directory once it runs.
const MARKER: &str = "issue40-child-ran.txt";
/// Negative control: an unsuspended `cmd /c echo` child finishes well inside this window.
pub(super) const UNSUSPENDED_RUN_WINDOW: Duration = Duration::from_millis(750);
pub(super) const MARKER_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// SYNCHRONIZE + query handle that pins the child's process object, so its state is
/// read back from that exact process even after its pid is released.
pub(super) struct PinnedProcess(HANDLE);

impl PinnedProcess {
    pub(super) fn open(pid: u32) -> Self {
        // SAFETY: plain OpenProcess call with integer arguments; the handle is owned by the
        // returned value and closed exactly once in Drop.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .unwrap_or_else(|error| panic!("pin child pid {pid}: {error}"));
        Self(handle)
    }

    /// Exit code once the process object is signaled; `None` while it still runs.
    pub(super) fn exit_code(&self) -> Option<u32> {
        // SAFETY: `self.0` is a live handle with SYNCHRONIZE access; a zero timeout only polls.
        if unsafe { WaitForSingleObject(self.0, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut exit_code = 0_u32;
        // SAFETY: `self.0` is live with PROCESS_QUERY_LIMITED_INFORMATION and the out-pointer
        // refers to a writable local.
        unsafe { GetExitCodeProcess(self.0, &raw mut exit_code) }
            .unwrap_or_else(|error| panic!("read child exit code: {error}"));
        Some(exit_code)
    }

    pub(super) fn is_in_job(&self, job: &OwnedProcessJob) -> bool {
        let mut in_job = BOOL::default();
        // SAFETY: `self.0` is live with PROCESS_QUERY_LIMITED_INFORMATION, `job.handle` is the live
        // job handle owned by `job` (created with full access), and the out-pointer refers to a
        // writable local.
        unsafe { IsProcessInJob(self.0, Some(job.handle), &raw mut in_job) }
            .unwrap_or_else(|error| panic!("IsProcessInJob: {error}"));
        in_job.as_bool()
    }
}

impl Drop for PinnedProcess {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from OpenProcess in `open` and is closed exactly once here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// What the injected assignment step saw: the pinned child and its physical thread
/// suspend state at the moment it was about to be assigned.
pub(super) struct AssignmentObservation {
    pub(super) pid: u32,
    pub(super) pinned: PinnedProcess,
    pub(super) suspend_state: Vec<ProcessSuspendState>,
}

impl AssignmentObservation {
    pub(super) fn capture(pid: u32) -> Self {
        Self {
            pid,
            pinned: PinnedProcess::open(pid),
            suspend_state: process_tree_suspend_state(&[pid]),
        }
    }

    pub(super) fn assert_fully_suspended(&self) {
        assert!(
            matches!(
                self.suspend_state.as_slice(),
                [state] if state.pid == self.pid && state.fully_suspended
            ),
            "the child must still be suspended when it is assigned: {:?}",
            self.suspend_state
        );
    }
}

fn start_params(dir: &Path, job_id: &str, command: &str, args: &[&str]) -> ActRunShellStartParams {
    ActRunShellStartParams {
        command: command.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        working_dir: Some(dir.to_string_lossy().into_owned()),
        env: BTreeMap::new(),
        timeout_ms: None,
        job_id: Some(job_id.to_owned()),
    }
}

pub(super) fn synthetic_assignment_failure() -> ErrorData {
    shell_tool_error(
        error_codes::TOOL_INTERNAL_ERROR,
        "synthetic job assignment failure",
        json!({
            "code": error_codes::TOOL_INTERNAL_ERROR,
            "reason": "synthetic_job_assign_failure",
        }),
    )
}

async fn wait_for_file(path: &Path) -> bool {
    let started = Instant::now();
    while started.elapsed() < MARKER_TIMEOUT {
        if path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test]
async fn must_fail_durable_start_assignment_failure_kills_suspended_child_40() {
    let _root = ShellJobRootGuard::new();
    let dir = tempfile::tempdir().expect("working dir");
    let job_id = "issue40-assign-failure";
    let echo = format!("echo ran>{MARKER}");
    let params = start_params(dir.path(), job_id, "cmd.exe", &["/d", "/c", &echo]);
    let authorization = RunShellAuthorization {
        command_line: format!("cmd.exe /d /c {echo}"),
        matched_pattern: "synthetic-test".to_owned(),
    };

    let mut observed = None;
    let error = start_authorized_shell_job_with(
        params,
        &authorization,
        None,
        |pid, _tool_name, _resource_id| {
            observed = Some(AssignmentObservation::capture(pid));
            std::thread::sleep(UNSUSPENDED_RUN_WINDOW);
            Err(synthetic_assignment_failure())
        },
    )
    .expect_err("an injected assignment failure must fail the durable start");
    let observed = observed.expect("the assignment step must run");
    observed.assert_fully_suspended();

    let data = error.data.as_ref().expect("the error must carry readback");
    assert_eq!(data["reason"], "process_job_assign_failed");
    assert_eq!(data["pid"], observed.pid);
    assert_eq!(data["resource_id"], job_id);
    assert_eq!(data["source_error"], "synthetic job assignment failure");
    assert_eq!(
        data["source_error_data"]["reason"],
        "synthetic_job_assign_failure"
    );
    assert_eq!(data["child_resumed"], false);
    assert_eq!(data["child_reaped"], true, "{data}");
    assert_eq!(data["child_exit_code"], 1, "{data}");

    // The same process object, read through the pinned handle: terminated by the cleanup
    // (TerminateProcess exit code 1) without ever running its command.
    assert_eq!(
        observed.pinned.exit_code(),
        Some(1),
        "the unassigned child must be terminated"
    );
    let leftovers = fs::read_dir(dir.path())
        .expect("working dir readback")
        .count();
    assert_eq!(
        leftovers, 0,
        "the unassigned child must never run its command"
    );

    let root = shell_durable_job_root_dir().expect("durable job root");
    let status_path = shell_job_paths_from_root(&root, job_id).status_path;
    let status = read_shell_job_status(&status_path, job_id).expect("job status readback");
    assert_eq!(status.status, "spawn_failed");
    assert_eq!(status.pid, None);
    assert_eq!(
        status.error_code.as_deref(),
        Some(error_codes::TOOL_INTERNAL_ERROR)
    );
    assert_eq!(status.error_message.as_deref(), Some(&*error.message));
    assert!(status.completed_at.is_some());
}

#[tokio::test]
async fn must_pass_durable_child_stays_suspended_until_inside_its_job_40() {
    let dir = tempfile::tempdir().expect("working dir");
    let job_id = "issue40-in-job";
    let script = format!("Set-Content -LiteralPath {MARKER} -Value ran; Start-Sleep -Seconds 120");
    let params = start_params(
        dir.path(),
        job_id,
        "powershell.exe",
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ],
    );
    let logs = tempfile::tempdir().expect("log dir");
    let stdout = fs::File::create(logs.path().join("stdout.log")).expect("stdout log");
    let stderr = fs::File::create(logs.path().join("stderr.log")).expect("stderr log");

    let mut observed = None;
    let mut spawned = spawn_shell_job_child_with(
        &params,
        job_id,
        stdout,
        stderr,
        None,
        |pid, tool_name, resource_id| {
            observed = Some(AssignmentObservation::capture(pid));
            assign_owned_process_job(pid, tool_name, resource_id)
        },
    )
    .expect("the durable child must start inside its job");
    let observed = observed.expect("the assignment step must run");
    observed.assert_fully_suspended();
    assert_eq!(spawned.child.id(), Some(observed.pid));
    assert!(
        observed
            .pinned
            .is_in_job(&spawned.process_job.lock().expect("process job lock")),
        "the child must be a member of its own job"
    );
    assert!(
        wait_for_file(&dir.path().join(MARKER)).await,
        "the child must be resumed and run its command"
    );
    assert_eq!(
        observed.pinned.exit_code(),
        None,
        "the child must still be running"
    );

    // Closing the kill-on-close job terminates its members: physical proof of membership.
    drop(spawned.process_job);
    let exit = tokio::time::timeout(EXIT_TIMEOUT, spawned.child.wait())
        .await
        .expect("closing the job must end the child")
        .expect("child exit status");
    assert_eq!(
        observed.pinned.exit_code(),
        exit.code().map(i32::cast_unsigned),
        "the pinned process must be the child the job terminated"
    );
    for pump in spawned.pumps {
        tokio::time::timeout(EXIT_TIMEOUT, pump)
            .await
            .expect("output pump must finish once the child exits")
            .expect("output pump task");
    }
}
