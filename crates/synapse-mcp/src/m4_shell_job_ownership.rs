//! Retained shell Job Object identity; durable PIDs are readback, never kill authority (#59).
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

use super::{ActRunShellJobStatus, OwnedProcessJob, ShellJobTerminationReadback};

pub(super) type SharedShellJobHandle = Arc<Mutex<OwnedProcessJob>>;
type JobKey = (String, String);
type JobEntry = (u32, Weak<Mutex<OwnedProcessJob>>);
static JOBS: OnceLock<Mutex<HashMap<JobKey, JobEntry>>> = OnceLock::new();

fn key(session_id: Option<&str>, job_id: &str) -> JobKey {
    (session_id.unwrap_or_default().to_owned(), job_id.to_owned())
}

pub(super) struct ShellJobHandleRegistration {
    key: JobKey,
    handle: Weak<Mutex<OwnedProcessJob>>,
}

impl Drop for ShellJobHandleRegistration {
    fn drop(&mut self) {
        if let Some(registry) = JOBS.get()
            && let Ok(mut registry) = registry.lock()
            && registry
                .get(&self.key)
                .is_some_and(|(_, handle)| Weak::ptr_eq(handle, &self.handle))
        {
            registry.remove(&self.key);
        }
    }
}

pub(super) fn register_shell_job_handle(
    session_id: Option<&str>,
    job_id: &str,
    root_pid: u32,
    process_job: &SharedShellJobHandle,
) -> ShellJobHandleRegistration {
    let key = key(session_id, job_id);
    let handle = Arc::downgrade(process_job);
    if let Ok(mut registry) = JOBS.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        registry.insert(key.clone(), (root_pid, handle.clone()));
    }
    ShellJobHandleRegistration { key, handle }
}

pub(super) fn registered_shell_job_handle(
    session_id: Option<&str>,
    job_id: &str,
) -> Option<(u32, SharedShellJobHandle)> {
    let (pid, handle) = JOBS
        .get()?
        .lock()
        .ok()?
        .get(&key(session_id, job_id))?
        .clone();
    Some((pid, handle.upgrade()?))
}

pub(super) fn restore_registered_shell_job_root_pid(
    job: &mut ActRunShellJobStatus,
    session_id: Option<&str>,
    job_id: &str,
) {
    if let Some((pid, _)) = registered_shell_job_handle(session_id, job_id) {
        job.pid = Some(pid);
    }
}

pub(super) fn terminate_registered_shell_job(
    job: &ActRunShellJobStatus,
) -> ShellJobTerminationReadback {
    let Some((pid, handle)) = registered_shell_job_handle(job.session_id.as_deref(), &job.job_id)
    else {
        return unavailable("job_handle_unavailable".to_owned());
    };
    terminate_handle(&handle, pid)
}

fn unavailable(status: String) -> ShellJobTerminationReadback {
    ShellJobTerminationReadback {
        attempted: false,
        status,
        remaining_process_ids: Vec::new(),
    }
}

fn terminate_handle(handle: &SharedShellJobHandle, pid: u32) -> ShellJobTerminationReadback {
    match handle.lock() {
        Ok(job) => job.terminate(pid),
        Err(error) => unavailable(format!("job_object_lock_failed:{error}")),
    }
}

pub(super) async fn terminate_owned_process_job_offloaded(
    handle: SharedShellJobHandle,
    pid: u32,
) -> Result<ShellJobTerminationReadback, tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || terminate_handle(&handle, pid)).await
}

#[cfg(windows)]
impl OwnedProcessJob {
    pub(super) fn process_ids(&self) -> Result<Vec<u32>, String> {
        use windows::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_PROCESS_ID_LIST, JobObjectBasicProcessIdList, QueryInformationJobObject,
        };
        const MAX_JOB_PROCESS_IDS: usize = 4096;
        let buffer_size = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>()
            + (MAX_JOB_PROCESS_IDS - 1) * std::mem::size_of::<usize>();
        let mut buffer = vec![0_usize; buffer_size.div_ceil(std::mem::size_of::<usize>())];
        let length = u32::try_from(buffer.len() * std::mem::size_of::<usize>())
            .map_err(|e| e.to_string())?;
        // SAFETY: aligned, writable buffer contains room for header and MAX_JOB_PROCESS_IDS entries.
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicProcessIdList,
                buffer.as_mut_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        // SAFETY: successful query initialized the fixed header in the aligned buffer.
        let list = unsafe { &*buffer.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
        let count = usize::try_from(list.NumberOfProcessIdsInList).map_err(|e| e.to_string())?;
        if list.NumberOfAssignedProcesses != list.NumberOfProcessIdsInList
            || count > MAX_JOB_PROCESS_IDS
        {
            return Err("job_process_list_incomplete".to_owned());
        }
        // SAFETY: count validated above, and the query initialized those entries at the C field offset.
        let ids = unsafe {
            let start = buffer
                .as_ptr()
                .cast::<u8>()
                .add(std::mem::offset_of!(
                    JOBOBJECT_BASIC_PROCESS_ID_LIST,
                    ProcessIdList
                ))
                .cast::<usize>();
            std::slice::from_raw_parts(start, count)
        };
        ids.iter()
            .map(|pid| u32::try_from(*pid).map_err(|e| e.to_string()))
            .collect()
    }

    fn terminate(&self, _pid: u32) -> ShellJobTerminationReadback {
        use windows::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: retained handle belongs exclusively to this value; caller holds its mutex.
        if let Err(error) = unsafe { TerminateJobObject(self.handle, 1) } {
            let remaining = self.process_ids();
            return ShellJobTerminationReadback {
                attempted: true,
                status: match &remaining {
                    Ok(_) => format!("job_object_terminate_failed:{error}"),
                    Err(query) => format!(
                        "job_object_terminate_failed:{error}:membership_query_failed:{query}"
                    ),
                },
                remaining_process_ids: remaining.unwrap_or_default(),
            };
        }
        self.wait_for_empty_job()
    }

    fn wait_for_empty_job(&self) -> ShellJobTerminationReadback {
        let started = Instant::now();
        loop {
            let (status, remaining_process_ids) = match self.process_ids() {
                Ok(ids) if ids.is_empty() => ("terminated".to_owned(), ids),
                Ok(ids) if started.elapsed() >= Duration::from_secs(5) => {
                    ("termination_incomplete".to_owned(), ids)
                }
                Ok(_) => {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }
                Err(error) => (format!("job_membership_query_failed:{error}"), Vec::new()),
            };
            return ShellJobTerminationReadback {
                attempted: true,
                status,
                remaining_process_ids,
            };
        }
    }
}

#[cfg(not(windows))]
impl OwnedProcessJob {
    fn terminate(&self, pid: u32) -> ShellJobTerminationReadback {
        super::terminate_shell_job_process_tree(pid)
    }
}
