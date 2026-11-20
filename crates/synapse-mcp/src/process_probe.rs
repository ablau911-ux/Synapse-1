//! One shared Win32 process-liveness probe (#144 DRY).
//!
//! Callers that only need the creation time (`m2`) and callers that must tell
//! "gone" apart from "cannot be queried" (daemon lifecycle) share this code,
//! so the unsafe `OpenProcess`/`GetProcessTimes` sequence exists once.

/// Result of asking Windows about one PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProcessProbe {
    /// No such process, or it already exited.
    Gone,
    /// Process is running; `created_100ns` is its FILETIME creation tick count.
    Running { created_100ns: u64 },
    /// The process could not be queried (for example ACCESS_DENIED for a
    /// SYSTEM/protected process). This is not evidence of life or death.
    Unknown(String),
}

#[cfg(windows)]
pub(crate) fn probe_process(pid: u32) -> ProcessProbe {
    use windows::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, STILL_ACTIVE},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    // SAFETY: query-only handle to the requested PID; closed on every result.
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        Err(error)
            if error.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
        {
            return ProcessProbe::Gone;
        }
        Err(error) => return ProcessProbe::Unknown(format!("open process {pid}: {error}")),
    };
    let result = (|| -> windows::core::Result<ProcessProbe> {
        let mut exit_code = 0;
        // SAFETY: valid process handle and writable result pointer.
        unsafe { GetExitCodeProcess(handle, &raw mut exit_code) }?;
        if exit_code != STILL_ACTIVE.0 as u32 {
            return Ok(ProcessProbe::Gone);
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: same handle pins process identity; all outputs are writable.
        unsafe {
            GetProcessTimes(
                handle,
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        }?;
        Ok(ProcessProbe::Running {
            created_100ns: (u64::from(created.dwHighDateTime) << 32)
                | u64::from(created.dwLowDateTime),
        })
    })();
    // SAFETY: this function owns the handle and no longer uses it.
    let _ = unsafe { CloseHandle(handle) };
    result.unwrap_or_else(|error| ProcessProbe::Unknown(format!("query process {pid}: {error}")))
}

#[cfg(not(windows))]
pub(crate) fn probe_process(_pid: u32) -> ProcessProbe {
    // Creation-time identity checking is specific to Windows.
    ProcessProbe::Gone
}
