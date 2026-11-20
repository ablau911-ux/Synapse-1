//! Shell children run only inside their kill-on-close Windows job (#40).
//!
//! Assigning a child that is already running leaves a window in which it, and
//! anything it spawns, runs outside the job; if the assignment then fails, a
//! durable child (`kill_on_drop(false)`) is left running unmonitored. Shell
//! children are therefore created with `CREATE_SUSPENDED`, placed in their job,
//! and only then resumed. If the assignment fails, the still-suspended child,
//! which has executed no code, is terminated and reaped before the error returns.
//! No path resumes a child that is not in its job.
//!
//! Stable std/tokio expose neither the primary-thread handle nor
//! `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so the resume calls `NtResumeProcess` on the
//! child's own process handle (the ntdll export the #906 resume path uses).
//!
//! `act_launch` creates its child through raw `CreateProcessW` and holds the
//! primary-thread handle, so [`SuspendedProcessHandles`] gives that child the same
//! confine-then-resume sequence through the shared [`SuspendedChild`] contract.

use std::time::{Duration, Instant};

use rmcp::ErrorData;
use serde_json::{Value, json};
use synapse_core::error_codes;
use tokio::process::{Child, Command as TokioCommand};

use super::{OwnedProcessJob, shell_tool_error};

/// Bound on reaping a terminated child that was never resumed. A suspended process
/// dies as soon as it is terminated; the bound only guards against a wedged handle.
const DISCARD_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const DISCARD_REAP_POLL: Duration = Duration::from_millis(5);

/// Sets a shell child's creation flags: no console window and, on Windows, created
/// suspended so it cannot run before [`confine_suspended_child`] resumes it. This
/// replaces earlier flags because `creation_flags` does not accumulate.
#[cfg(windows)]
pub(super) fn apply_suspended_no_window(command: &mut TokioCommand) {
    use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

    command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
}

#[cfg(not(windows))]
pub(super) fn apply_suspended_no_window(_command: &mut TokioCommand) {}

/// A child created suspended that has not executed any code yet.
pub(super) trait SuspendedChild {
    /// Requests termination (TerminateProcess on the child's own handle).
    fn start_kill(&mut self) -> std::io::Result<()>;
    /// Polls for exit: `Ok(None)` while live, `Ok(Some(code))` once reaped.
    fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>>;
    /// Lets the child run.
    fn resume(&self) -> Result<(), String>;
}

impl SuspendedChild for Child {
    fn start_kill(&mut self) -> std::io::Result<()> {
        Self::start_kill(self)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>> {
        Self::try_wait(self).map(|status| status.map(|status| status.code()))
    }

    fn resume(&self) -> Result<(), String> {
        resume_suspended_child(self)
    }
}

/// Places a child spawned with [`apply_suspended_no_window`] (or raw
/// `CREATE_SUSPENDED`) into its job through `assign` (production:
/// `assign_owned_process_job`; tests inject failures), and only then resumes it. If
/// assignment or resume fails, the never-resumed child is terminated and reaped
/// before the error is returned.
pub(super) fn confine_suspended_child<C, A>(
    child: &mut C,
    pid: u32,
    tool_name: &'static str,
    resource_id: Option<&str>,
    assign: A,
) -> Result<OwnedProcessJob, ErrorData>
where
    C: SuspendedChild,
    A: FnOnce(u32, &'static str, Option<&str>) -> Result<OwnedProcessJob, ErrorData>,
{
    let target = ConfinementTarget {
        tool_name,
        pid,
        resource_id,
    };
    let process_job = match assign(pid, tool_name, resource_id) {
        Ok(process_job) => process_job,
        Err(source) => {
            return Err(target.abort(
                child,
                "process_job_assign_failed",
                "could not be placed in its Windows job object",
                &source.message,
                source.data,
            ));
        }
    };
    if let Err(resume_error) = child.resume() {
        // Returning drops `process_job`; closing the kill-on-close job is the backstop
        // that terminates anything still inside it.
        return Err(target.abort(
            child,
            "process_job_resume_failed",
            "could not be resumed inside its Windows job object",
            &resume_error,
            None,
        ));
    }
    Ok(process_job)
}

/// The child being confined, named in the error readback.
struct ConfinementTarget<'a> {
    tool_name: &'static str,
    pid: u32,
    resource_id: Option<&'a str>,
}

impl ConfinementTarget<'_> {
    /// Terminates and reaps the never-resumed child, then builds an error naming the
    /// failed step, its source error, and the cleanup readback.
    fn abort(
        &self,
        child: &mut impl SuspendedChild,
        reason: &'static str,
        failure: &str,
        source_error: &str,
        source_error_data: Option<Value>,
    ) -> ErrorData {
        let discard = UnresumedChildDiscard::run(child);
        let (tool_name, pid) = (self.tool_name, self.pid);
        let outcome = discard.outcome();
        shell_tool_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!(
                "{tool_name} child pid {pid} {failure}; the never-resumed child {outcome}: {source_error}"
            ),
            json!({
                "code": error_codes::TOOL_INTERNAL_ERROR,
                "reason": reason,
                "pid": pid,
                "resource_id": self.resource_id,
                "source_error": source_error,
                "source_error_data": source_error_data,
                "child_resumed": false,
                "child_kill_error": discard.kill_error,
                "child_reaped": discard.reap_error.is_none(),
                "child_exit_code": discard.exit_code,
                "child_reap_error": discard.reap_error,
                "cleanup_elapsed_ms": discard.elapsed_ms,
            }),
        )
    }
}

/// Readback of terminating a child that was never resumed.
struct UnresumedChildDiscard {
    kill_error: Option<String>,
    exit_code: Option<i32>,
    reap_error: Option<String>,
    elapsed_ms: u64,
}

impl UnresumedChildDiscard {
    /// `start_kill` (TerminateProcess on the child's own handle), then a bounded
    /// `try_wait` reap. The spawn paths are synchronous, so this polls instead of
    /// awaiting `Child::wait`.
    fn run(child: &mut impl SuspendedChild) -> Self {
        let started = Instant::now();
        let kill_error = child.start_kill().err().map(|error| error.to_string());
        let (exit_code, reap_error) = loop {
            match child.try_wait() {
                Ok(Some(exit_code)) => break (exit_code, None),
                Ok(None) if started.elapsed() < DISCARD_REAP_TIMEOUT => {
                    std::thread::sleep(DISCARD_REAP_POLL);
                }
                Ok(None) => {
                    let waited_ms = started.elapsed().as_millis();
                    break (None, Some(format!("child still live after {waited_ms} ms")));
                }
                Err(error) => break (None, Some(error.to_string())),
            }
        };
        Self {
            kill_error,
            exit_code,
            reap_error,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn outcome(&self) -> String {
        match (&self.kill_error, &self.reap_error) {
            (_, None) => "was terminated and reaped".to_owned(),
            (None, Some(reap_error)) => format!("was terminated but not reaped ({reap_error})"),
            (Some(kill_error), Some(_)) => format!("could not be terminated ({kill_error})"),
        }
    }
}

/// Owns the process and primary-thread handles `CreateProcessW` returned for a child
/// created with `CREATE_SUSPENDED`; closes both on drop.
#[cfg(windows)]
pub(super) struct SuspendedProcessHandles {
    process: windows::Win32::Foundation::HANDLE,
    thread: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl SuspendedProcessHandles {
    pub(super) fn new(info: &windows::Win32::System::Threading::PROCESS_INFORMATION) -> Self {
        Self {
            process: info.hProcess,
            thread: info.hThread,
        }
    }
}

#[cfg(windows)]
impl Drop for SuspendedProcessHandles {
    fn drop(&mut self) {
        // SAFETY: the process handle came from a successful CreateProcessW, is owned only by
        // this value, and is closed exactly once here.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.process) };
        // SAFETY: the thread handle came from the same CreateProcessW, is owned only by this
        // value, and is closed exactly once here.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.thread) };
    }
}

#[cfg(windows)]
impl SuspendedChild for SuspendedProcessHandles {
    fn start_kill(&mut self) -> std::io::Result<()> {
        use windows::Win32::System::Threading::TerminateProcess;

        // SAFETY: `self.process` is the live process handle owned by this value; CreateProcessW
        // returns it with PROCESS_ALL_ACCESS, which includes PROCESS_TERMINATE.
        unsafe { TerminateProcess(self.process, 1) }
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>> {
        use windows::Win32::{
            Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{GetExitCodeProcess, WaitForSingleObject},
        };

        // SAFETY: `self.process` is live with SYNCHRONIZE access; a zero timeout only polls.
        let wait = unsafe { WaitForSingleObject(self.process, 0) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != WAIT_OBJECT_0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut exit_code = 0_u32;
        // SAFETY: `self.process` is live with PROCESS_QUERY_LIMITED_INFORMATION and the
        // out-pointer refers to a writable local.
        unsafe { GetExitCodeProcess(self.process, &raw mut exit_code) }
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Some(Some(exit_code.cast_signed())))
    }

    fn resume(&self) -> Result<(), String> {
        use windows::Win32::System::Threading::ResumeThread;

        // SAFETY: `self.thread` is the live primary-thread handle owned by this value;
        // CreateProcessW returns it with THREAD_ALL_ACCESS, which includes THREAD_SUSPEND_RESUME.
        let previous_suspend_count = unsafe { ResumeThread(self.thread) };
        if previous_suspend_count == u32::MAX {
            return Err(format!(
                "ResumeThread failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Resumes a child created with `CREATE_SUSPENDED` through its own process handle.
#[cfg(windows)]
fn resume_suspended_child(child: &Child) -> Result<(), String> {
    use windows::Win32::Foundation::HANDLE;

    // SAFETY: plain ntdll export linked at load time with the stdcall signature
    // `NTSTATUS f(HANDLE)`, declared exactly as in the #906 resume path. The only call
    // below passes a live process handle that carries PROCESS_SUSPEND_RESUME.
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(handle: HANDLE) -> i32;
    }

    let raw = child
        .raw_handle()
        .ok_or_else(|| "child process handle is unavailable (already reaped)".to_owned())?;
    // SAFETY: `raw` is the process handle owned by `child`, which stays borrowed for this
    // call, so the handle cannot be closed underneath it. CreateProcess returns the handle
    // with PROCESS_ALL_ACCESS, which includes PROCESS_SUSPEND_RESUME.
    let status = unsafe { NtResumeProcess(HANDLE(raw)) };
    if status < 0 {
        return Err(format!("NtResumeProcess returned NTSTATUS 0x{status:08x}"));
    }
    Ok(())
}

/// Children are not created suspended off Windows, so there is nothing to resume.
#[cfg(not(windows))]
fn resume_suspended_child(_child: &Child) -> Result<(), String> {
    Ok(())
}
