//! #1699: how a shell job's outcome is judged. Pure decisions, pinned by
//! `m4_shell_exit_verdict_table_1699.rs`; the OS work (process handle, timer,
//! termination, marker files) stays in `m4.rs`.

/// What the wait loop saw about one shell child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShellChildObservation {
    /// `Some(code)` when the child exited by itself (code `None` = no code
    /// reported); `None` when it was still running at the deadline and was
    /// terminated by us.
    pub exited: Option<Option<i32>>,
    /// The wait deadline fired before the exit was observed.
    pub timer_fired: bool,
    /// Kernel-recorded lifetime (exit time minus creation time), when the OS
    /// probe could read it.
    pub lifetime_ms: Option<u64>,
    /// Wall clock since spawn; the fallback when the lifetime is unknown.
    pub elapsed_ms: u64,
}

/// The judged outcome: the exit code to report and whether the budget expired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ShellExitVerdict {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// A child still running at the deadline was terminated: timeout, no exit
/// code. A child that exited by itself keeps its exit code, and timed out iff
/// it lived at least the budget — judged from the kernel lifetime, falling
/// back to the spawn-relative wall clock only when the lifetime is unknown.
/// When the monitor ran (or the timer fired) does not matter.
pub(crate) const fn classify_shell_child_exit(
    budget_ms: u64,
    observation: ShellChildObservation,
) -> ShellExitVerdict {
    let Some(exit_code) = observation.exited else {
        return ShellExitVerdict {
            exit_code: None,
            timed_out: true,
        };
    };
    let lived_ms = match observation.lifetime_ms {
        Some(lifetime_ms) => lifetime_ms,
        None => observation.elapsed_ms,
    };
    ShellExitVerdict {
        exit_code,
        timed_out: lived_ms >= budget_ms,
    }
}

/// The local side of a durable SSH job when a remote exit marker is read, in
/// precedence order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalShellOutcome {
    /// An operator cancel was requested.
    CancelRequested,
    /// The local wrapper (ssh.exe) outran its budget.
    TimedOut,
    /// The local wrapper already exited 0 and is no longer running.
    ExitedOk,
    /// Anything else (still running, exited nonzero, stale transport).
    Other,
}

/// What a remote exit marker does to the job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemoteExitMarkerVerdict {
    /// The marker changes nothing.
    Ignore,
    /// The marker's remote exit becomes the job outcome; when
    /// `overrides_local_timeout` the local budget expiry is cleared and kept
    /// only as evidence.
    Apply { overrides_local_timeout: bool },
}

/// A cancel always wins and a marker whose pid/process group differ from the
/// job's is foreign. A matching marker beats a local wrapper timeout whatever
/// the remote exit code (the command finished; the wrapper's cleanup is a
/// separate warning). Without a local timeout only a remote exit 0 is
/// honoured — a nonzero exit is already carried by ssh's own exit code — and a
/// job that already finished ok is left alone.
pub(crate) const fn remote_exit_marker_verdict(
    local: LocalShellOutcome,
    identity_matches: bool,
    remote_exit_code: i32,
) -> RemoteExitMarkerVerdict {
    match local {
        LocalShellOutcome::CancelRequested => RemoteExitMarkerVerdict::Ignore,
        _ if !identity_matches => RemoteExitMarkerVerdict::Ignore,
        LocalShellOutcome::TimedOut => RemoteExitMarkerVerdict::Apply {
            overrides_local_timeout: true,
        },
        LocalShellOutcome::ExitedOk => RemoteExitMarkerVerdict::Ignore,
        LocalShellOutcome::Other if remote_exit_code != 0 => RemoteExitMarkerVerdict::Ignore,
        LocalShellOutcome::Other => RemoteExitMarkerVerdict::Apply {
            overrides_local_timeout: false,
        },
    }
}

#[cfg(test)]
#[path = "m4_shell_exit_verdict_table_1699.rs"]
mod table_1699;
