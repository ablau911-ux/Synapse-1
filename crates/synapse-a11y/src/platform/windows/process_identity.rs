use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

#[cfg(all(test, windows))]
#[path = "process_identity_native_249.rs"]
mod process_identity_native_249;

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a successful Win32 handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Reads parent PID and exact creation FILETIME while retaining the process handle.
pub fn launch_process_record(pid: u32) -> Option<(u32, u64)> {
    // SAFETY: read-only process query; successful handles are closed by RAII.
    let process =
        ProcessHandle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    let created = creation_time(process.0)?;
    // SAFETY: read-only process snapshot, owned until enumeration completes.
    let snapshot = ProcessHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?);
    let mut row = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).ok()?,
        ..Default::default()
    };
    // SAFETY: initialized structure advertises the correct size; snapshot is live.
    unsafe { Process32FirstW(snapshot.0, &mut row) }.ok()?;
    loop {
        if row.th32ProcessID == pid {
            return (creation_time(process.0)? == created)
                .then_some((row.th32ParentProcessID, created));
        }
        // SAFETY: same live snapshot and initialized output structure.
        if unsafe { Process32NextW(snapshot.0, &mut row) }.is_err() {
            return None;
        }
    }
}

fn creation_time(handle: HANDLE) -> Option<u64> {
    let mut created = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: valid retained process handle and writable FILETIME outputs.
    unsafe { GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user) }.ok()?;
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    (ticks != 0).then_some(ticks)
}
