//! Suspended ConPTY creation: assign an agent's job before its first instruction (#160).
use super::capture::CaptureSpec;
use crate::m4::OwnedProcessJob;
use portable_pty::{Child, ChildKiller, ExitStatus, MasterPty, PtySize};
use std::{
    io::{self, PipeReader, PipeWriter, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    sync::{Arc, Mutex},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole},
            Threading::{
                CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
                INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
                STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

type SpawnedPty = (
    Box<dyn MasterPty + Send>,
    Box<dyn Child + Send + Sync>,
    OwnedProcessJob,
);

struct Console(HPCON);
impl Drop for Console {
    fn drop(&mut self) {
        // SAFETY: this value owns the live pseudoconsole, closed exactly once.
        unsafe { ClosePseudoConsole(self.0) };
    }
}

struct NativeMaster {
    console: Console,
    output: PipeReader,
    input: Mutex<Option<PipeWriter>>,
    size: Mutex<PtySize>,
}

impl MasterPty for NativeMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        let mut current = self.size.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        // SAFETY: live owned pseudoconsole, dimensions checked before FFI.
        unsafe { ResizePseudoConsole(self.console.0, dimensions(size.cols, size.rows)?) }?;
        *current = size;
        Ok(())
    }
    fn get_size(&self) -> anyhow::Result<PtySize> {
        self.size
            .lock()
            .map(|size| *size)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }
    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.output.try_clone()?))
    }
    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        let input = self
            .input
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .take()
            .ok_or_else(|| anyhow::anyhow!("PTY_WRITER_ALREADY_TAKEN"))?;
        Ok(Box::new(input))
    }
}

#[derive(Debug)]
struct NativeChild {
    process: Arc<OwnedHandle>,
    pid: u32,
}

#[derive(Debug)]
struct NativeKiller(Arc<OwnedHandle>);

fn terminate(process: &OwnedHandle) -> io::Result<()> {
    // SAFETY: borrowed live process handle owned by NativeChild/NativeKiller.
    unsafe { TerminateProcess(HANDLE(process.as_raw_handle()), 1) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))
}

impl ChildKiller for NativeKiller {
    fn kill(&mut self) -> io::Result<()> {
        terminate(&self.0)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Self(Arc::clone(&self.0)))
    }
}

impl ChildKiller for NativeChild {
    fn kill(&mut self) -> io::Result<()> {
        terminate(&self.process)
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(NativeKiller(Arc::clone(&self.process)))
    }
}

impl Child for NativeChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: live process handle, zero timeout only reads its signaled state.
        if unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), 0) } != WAIT_OBJECT_0
        {
            return Ok(None);
        }
        self.exit_status().map(Some)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        // SAFETY: process handle remains alive throughout the blocking wait.
        if unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), INFINITE) }
            != WAIT_OBJECT_0
        {
            return Err(io::Error::last_os_error());
        }
        self.exit_status()
    }
    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        Some(self.process.as_raw_handle())
    }
}

impl NativeChild {
    fn exit_status(&self) -> io::Result<ExitStatus> {
        let mut code = 0;
        // SAFETY: process handle is live and code is a writable u32.
        unsafe { GetExitCodeProcess(HANDLE(self.process.as_raw_handle()), &raw mut code) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
        Ok(ExitStatus::with_exit_code(code))
    }
    fn discard(&mut self) {
        let _ = self.kill();
        // SAFETY: owned child was never resumed and has just received TerminateProcess.
        let _ = unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), 2_000) };
    }
}

/// Aligned backing storage retained until the attribute list is deleted.
struct Attributes {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl Attributes {
    fn new(console: HPCON) -> anyhow::Result<Self> {
        let mut bytes = 0;
        // SAFETY: null list requests required allocation size only.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &raw mut bytes) };
        anyhow::ensure!(bytes > 0, "PTY_ATTRIBUTE_SIZE_UNAVAILABLE");
        let mut storage = vec![0_usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        // SAFETY: aligned allocation has the byte count returned by the sizing call.
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &raw mut bytes) }?;
        let attributes = Self {
            _storage: storage,
            list,
        };
        // SAFETY: documented pseudoconsole attribute takes the HPCON value, not its address.
        unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(console.0 as *const std::ffi::c_void),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
        }?;
        Ok(attributes)
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized list still backed by _storage, deleted once before allocation drops.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

fn dimensions(cols: u16, rows: u16) -> anyhow::Result<COORD> {
    anyhow::ensure!(cols > 0 && rows > 0, "PTY_SIZE_EMPTY");
    Ok(COORD {
        X: i16::try_from(cols)?,
        Y: i16::try_from(rows)?,
    })
}

fn wide(value: &std::ffi::OsStr) -> anyhow::Result<Vec<u16>> {
    let mut units: Vec<u16> = value.encode_wide().collect();
    anyhow::ensure!(!units.contains(&0), "PTY_STRING_CONTAINS_NUL");
    units.push(0);
    Ok(units)
}

fn create_suspended(
    spec: &CaptureSpec,
    console: HPCON,
) -> anyhow::Result<(NativeChild, OwnedHandle)> {
    let attributes = Attributes::new(console)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>())?;
    startup.lpAttributeList = attributes.list;
    let line = crate::m4::shell_command_line_from_parts(&spec.program, &spec.args);
    let mut command = wide(std::ffi::OsStr::new(&line))?;
    let cwd = spec.cwd.as_ref().map(|p| wide(p.as_os_str())).transpose()?;
    let mut title = spec
        .title
        .as_ref()
        .map(|s| wide(std::ffi::OsStr::new(s)))
        .transpose()?;
    if let Some(title) = title.as_mut() {
        startup.StartupInfo.lpTitle = PWSTR(title.as_mut_ptr());
    }
    let env = spec
        .env
        .iter()
        .map(|(k, v)| (k.to_uppercase(), (k.clone(), v.clone())))
        .collect();
    let environment = crate::m4::child_environment::environment_block(env);
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all UTF-16 buffers and the initialized attribute list live through this call.
    // EXTENDED_STARTUPINFO_PRESENT tells Windows to read startup as STARTUPINFOEXW.
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            Some(environment.as_ptr().cast()),
            cwd.as_ref().map_or(PCWSTR::null(), |s| PCWSTR(s.as_ptr())),
            &raw const startup.StartupInfo,
            &raw mut info,
        )
    }?;
    // SAFETY: successful CreateProcessW transfers both unique handles to these RAII owners.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess.0) };
    // SAFETY: same ownership transfer for the primary thread.
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread.0) };
    Ok((
        NativeChild {
            process: Arc::new(process),
            pid: info.dwProcessId,
        },
        thread,
    ))
}

pub(super) fn spawn<A>(
    spec: &CaptureSpec,
    resource_id: &str,
    assign: A,
) -> anyhow::Result<SpawnedPty>
where
    A: FnOnce(u32, &'static str, Option<&str>) -> Result<OwnedProcessJob, rmcp::ErrorData>,
{
    let (input_read, input_write) = io::pipe()?;
    let (output_read, output_write) = io::pipe()?;
    // SAFETY: pipe ends are valid, remain live through creation, and are not inherited.
    let console = Console(unsafe {
        CreatePseudoConsole(
            dimensions(spec.cols, spec.rows)?,
            HANDLE(input_read.as_raw_handle()),
            HANDLE(output_write.as_raw_handle()),
            0,
        )
    }?);
    drop(input_read);
    drop(output_write);
    let (mut child, thread) = create_suspended(spec, console.0)?;
    let job = match assign(child.pid, "act_spawn_agent", Some(resource_id)) {
        Ok(job) => job,
        Err(error) => {
            child.discard();
            return Err(anyhow::anyhow!("PTY_JOB_ASSIGN_FAILED: {error}"));
        }
    };
    // SAFETY: owned primary thread of the still-suspended child, now inside its owned job.
    if unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } == u32::MAX {
        let error = io::Error::last_os_error();
        child.discard();
        return Err(anyhow::anyhow!("PTY_JOB_RESUME_FAILED: {error}"));
    }
    let master = NativeMaster {
        console,
        output: output_read,
        input: Mutex::new(Some(input_write)),
        size: Mutex::new(PtySize {
            cols: spec.cols,
            rows: spec.rows,
            pixel_width: 0,
            pixel_height: 0,
        }),
    };
    Ok((Box::new(master), Box::new(child), job))
}
