//! #2361: classic consoles only. All attach/detach/write calls run in the child.
use super::{SessionTarget, SynapseService};
use crate::console_type_policy::{
    ConsoleTypeOutcome, ConsoleTypePlan, ConsoleTypeRefusal, ReceiverIdentity,
};
use crate::m1::mcp_error;
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use std::{io::Read as _, os::windows::process::CommandExt as _, process::Stdio, time::Duration};
use synapse_core::error_codes;
use tokio::io::AsyncWriteExt as _;
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, HWND, WAIT_TIMEOUT},
        System::{
            Console::{
                AttachConsole, FreeConsole, GetConsoleProcessList, GetConsoleWindow, GetStdHandle,
                INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
                STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle, WriteConsoleInputW,
            },
            JobObjects::IsProcessInJob,
            Threading::{
                CREATE_NO_WINDOW, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, WaitForSingleObject,
            },
        },
        UI::WindowsAndMessaging::{GetClassNameW, GetWindowThreadProcessId},
    },
    core::BOOL,
};

pub(crate) struct ConsoleType {
    service: SynapseService,
    session_id: String,
    hwnd: i64,
    text: String,
    timeout_ms: u32,
    coordinates: bool,
}

impl ConsoleType {
    pub(crate) fn for_request(
        service: SynapseService,
        session_id: Option<String>,
        text: String,
        timeout_ms: u32,
        coordinates: bool,
    ) -> Result<Option<Self>, ErrorData> {
        let Some(SessionTarget::Window { hwnd }) = service.session_target(session_id.as_deref())?
        else {
            return Ok(None);
        };
        let remembered_console = if let Some(session_id) = session_id.as_deref() {
            let ledger = service.session_processes.lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "console launch ledger lock poisoned",
                )
            })?;
            ledger.get(session_id).is_some_and(|resources| {
                resources.values().any(|resource| {
                    resource
                        .process_job
                        .as_ref()
                        .and_then(crate::m4::OwnedProcessJob::console_receiver)
                        .is_some_and(|identity| identity.0 == hwnd)
                })
            })
        } else {
            false
        };
        // A recorded console whose HWND disappeared/changed class stays in this
        // refusing path. Never fall through to desktop typing on a reused HWND.
        if !remembered_console
            && ConsoleReceiver::class(hwnd).as_deref() != Some("ConsoleWindowClass")
        {
            return Ok(None);
        }
        let session_id = session_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_PROFILE_POLICY_DENIED,
                    "console typing requires an MCP session",
                )
            })?;
        Ok(Some(Self {
            service,
            session_id,
            hwnd,
            text,
            timeout_ms,
            coordinates,
        }))
    }

    // The launch wait already rejected pre-existing windows. Job membership is
    // read through the same live process handle as its creation identity.
    pub(crate) fn capture_launch(
        response: &crate::m4::ActLaunchResponse,
        job: &crate::m4::OwnedProcessJob,
    ) -> Option<ReceiverIdentity> {
        let hwnd = response.hwnd?;
        let receiver = ConsoleReceiver::open(hwnd, None, Some(job.raw_handle_value())).ok()?;
        (response.window_owner_pid == Some(receiver.identity.1)).then_some(receiver.identity)
    }

    pub(crate) async fn execute(self) -> Result<(bool, &'static str, Value), ErrorData> {
        self.service
            .ensure_supported_use_allows_action_for("act_type", None)?;
        let target = SessionTarget::Window { hwnd: self.hwnd };
        self.service
            .ensure_target_claim_allows_session("target_act", &self.session_id, &target)?;
        let claimed = self
            .service
            .target_claim_for_session(&self.session_id, &target)?
            .is_some();
        let saved = {
            let ledger = self.service.session_processes.lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "console launch ledger lock poisoned",
                )
            })?;
            ledger.get(&self.session_id).and_then(|resources| {
                resources
                    .values()
                    .filter_map(|resource| {
                        resource
                            .process_job
                            .as_ref()
                            .and_then(crate::m4::OwnedProcessJob::console_receiver)
                    })
                    .find(|identity| identity.0 == self.hwnd)
            })
        };
        let observed = ConsoleReceiver::open(self.hwnd, saved, None)
            .ok()
            .map(|r| r.identity);
        let selected = match self.service.session_target(Some(&self.session_id))? {
            Some(SessionTarget::Window { hwnd }) => Some(hwnd),
            _ => None,
        };
        let plan = ConsoleTypePlan::new(
            Some(&self.session_id),
            saved.map(|_| self.session_id.as_str()),
            selected,
            claimed,
            ConsoleReceiver::class(self.hwnd).as_deref().unwrap_or(""),
            saved,
            observed,
            &self.text,
            self.coordinates,
        )
        .map_err(Self::policy_error)?;
        let identity = plan.identity();
        let requested_records = plan.key_records().len();
        let payload = serde_json::to_vec(&(identity, &self.text)).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode console helper request: {error}"),
            )
        })?;
        // Re-read target, claim and resource immediately before transport. The
        // request binds to this identity; it never follows a later target change.
        self.service
            .ensure_target_claim_allows_session("target_act", &self.session_id, &target)?;
        if self.service.session_target(Some(&self.session_id))? != Some(target.clone())
            || self
                .service
                .target_claim_for_session(&self.session_id, &target)?
                .is_none()
        {
            return Err(Self::policy_error(ConsoleTypeRefusal::Claim));
        }
        {
            let ledger = self.service.session_processes.lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "console launch ledger lock poisoned",
                )
            })?;
            if !ledger.get(&self.session_id).is_some_and(|resources| {
                resources.values().any(|resource| {
                    resource
                        .process_job
                        .as_ref()
                        .and_then(crate::m4::OwnedProcessJob::console_receiver)
                        == Some(identity)
                })
            }) {
                return Err(Self::policy_error(ConsoleTypeRefusal::Ownership));
            }
        }
        let mut command =
            tokio::process::Command::new(std::env::current_exe().map_err(|error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    format!("resolve console helper executable: {error}"),
                )
            })?);
        command
            .arg("--console-type-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW.0);
        let mut child = command.spawn().map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("start console helper before delivery: {error}"),
            )
        })?;
        let helper_pid = child.id();
        let result =
            tokio::time::timeout(Duration::from_millis(u64::from(self.timeout_ms)), async {
                let Some(mut stdin) = child.stdin.take() else {
                    return Err(std::io::Error::other("console helper stdin missing"));
                };
                stdin.write_all(&payload).await?;
                drop(stdin);
                child.wait().await
            })
            .await;
        let exit = match result {
            Ok(Ok(exit)) => Some(exit),
            // Once the child exists an I/O/timeout error cannot prove zero
            // records. Kill and reap, report uncertain delivery, never retry.
            _ => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                None
            }
        };
        let code = exit.and_then(|status| status.code());
        let outcome = plan.helper_outcome(code);
        if outcome == ConsoleTypeOutcome::Refused {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "console helper refused identity, input or membership before delivery",
            ));
        }
        let complete = matches!(outcome, ConsoleTypeOutcome::Queued(_));
        Ok((
            complete,
            if complete { "ok" } else { "verify_needed" },
            json!({
                "ok": complete,
                "method": "classic_console_input_records",
                "window_hwnd": identity.0, "receiver_pid": identity.1,
                "receiver_created_filetime": identity.2, "helper_pid": helper_pid,
                "requested_records": requested_records,
                "accepted_records": if complete { Some(requested_records) } else { None },
                "required_foreground": false,
                "source_of_truth": format!("console input queue: HWND={} receiver PID={} creation FILETIME={}; WriteConsoleInputW count only; separate shell readback required", identity.0, identity.1, identity.2),
                "text_integrity": "dispatch_only_requires_target_readback",
                "retry_safe": false,
            }),
        ))
    }

    fn policy_error(reason: ConsoleTypeRefusal) -> ErrorData {
        let code = match reason {
            ConsoleTypeRefusal::Empty
            | ConsoleTypeRefusal::Control
            | ConsoleTypeRefusal::TooLarge
            | ConsoleTypeRefusal::Coordinates => error_codes::TOOL_PARAMS_INVALID,
            _ => error_codes::ACTION_TARGET_INVALID,
        };
        mcp_error(
            code,
            format!("classic console typing refused before delivery: {reason:?}"),
        )
    }

    // Called before Tokio, telemetry, DB locks, config and daemon setup.
    // 0=all records accepted; 2=refused before WriteConsoleInputW; 3=uncertain.
    pub(crate) fn helper_main() -> std::process::ExitCode {
        let limit = crate::m2::MAX_ACT_TYPE_UTF16_UNITS * 4 + 1024;
        let mut bytes = Vec::new();
        if std::io::stdin()
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > limit
        {
            return std::process::ExitCode::from(2);
        }
        let Ok((identity, text)) = serde_json::from_slice::<(ReceiverIdentity, String)>(&bytes)
        else {
            return std::process::ExitCode::from(2);
        };
        let Ok(writer) = ConsoleWriter::new(identity, &text) else {
            return std::process::ExitCode::from(2);
        };
        std::process::ExitCode::from(writer.write())
    }
}

struct ConsoleReceiver {
    identity: ReceiverIdentity,
    process: HANDLE,
}

impl ConsoleReceiver {
    fn class(hwnd: i64) -> Option<String> {
        let mut buffer = [0_u16; 256];
        // SAFETY: class-name query writes only within the supplied buffer.
        let length = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buffer) };
        usize::try_from(length)
            .ok()
            .filter(|length| *length > 0)
            .map(|length| String::from_utf16_lossy(&buffer[..length]))
    }

    fn open(hwnd: i64, expected: Option<ReceiverIdentity>, job: Option<isize>) -> Result<Self, ()> {
        if hwnd <= 0 || Self::class(hwnd).as_deref() != Some("ConsoleWindowClass") {
            return Err(());
        }
        let mut pid = 0;
        // SAFETY: read-only HWND query, valid writable pid output.
        if unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&raw mut pid)) } == 0
            || pid == 0
        {
            return Err(());
        }
        // SAFETY: query/synchronize rights only; owned handle closed by Drop.
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .map_err(|_| ())?;
        let mut receiver = Self {
            identity: (hwnd, pid, 0),
            process,
        };
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: process is owned/live and all FILETIME outputs are writable.
        unsafe {
            GetProcessTimes(
                process,
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        }
        .map_err(|_| ())?;
        receiver.identity.2 =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        if receiver.identity.2 == 0
            || expected.is_some_and(|identity| identity != receiver.identity)
        {
            return Err(());
        }
        if let Some(job) = job {
            let mut member = BOOL::default();
            // SAFETY: caller keeps its owned job alive for this read; process is
            // the same handle used to read creation time, never a second PID open.
            unsafe { IsProcessInJob(process, Some(HANDLE(job as *mut _)), &raw mut member) }
                .map_err(|_| ())?;
            if !member.as_bool() {
                return Err(());
            }
        }
        receiver.observed().ok_or(())?;
        Ok(receiver)
    }

    fn observed(&self) -> Option<ReceiverIdentity> {
        let mut pid = 0;
        // SAFETY: nonblocking read on owned process handle; no desktop mutation.
        if unsafe { WaitForSingleObject(self.process, 0) } != WAIT_TIMEOUT {
            return None;
        }
        // SAFETY: read-only query, writable output.
        if unsafe { GetWindowThreadProcessId(HWND(self.identity.0 as *mut _), Some(&raw mut pid)) }
            == 0
            || pid != self.identity.1
            || Self::class(self.identity.0).as_deref() != Some("ConsoleWindowClass")
        {
            return None;
        }
        Some(self.identity)
    }
}

impl Drop for ConsoleReceiver {
    fn drop(&mut self) {
        // SAFETY: exactly the handle opened and owned by this object.
        let _ = unsafe { CloseHandle(self.process) };
    }
}

struct ConsoleWriter {
    receiver: ConsoleReceiver,
    plan: ConsoleTypePlan,
    attached: bool,
}

impl ConsoleWriter {
    fn new(identity: ReceiverIdentity, text: &str) -> Result<Self, ()> {
        let receiver = ConsoleReceiver::open(identity.0, Some(identity), None)?;
        // Parent already authorized the real session/launch/claim. Child repeats
        // input/identity validation; these labels confer no external authority.
        let plan = ConsoleTypePlan::new(
            Some("helper"),
            Some("helper"),
            Some(identity.0),
            true,
            "ConsoleWindowClass",
            Some(identity),
            receiver.observed(),
            text,
            false,
        )
        .map_err(|_| ())?;
        // SAFETY: helper process only. No attachment is expected with
        // CREATE_NO_WINDOW; FreeConsole failure here means already detached.
        let _ = unsafe { FreeConsole() };
        // stdin was read completely first. Clear redirected standard handles
        // BEFORE attach; otherwise GetStdHandle can still return a pipe (TP2).
        // SAFETY: helper-local standard handle slots; a null handle is valid.
        unsafe { SetStdHandle(STD_INPUT_HANDLE, HANDLE::default()) }.map_err(|_| ())?;
        // SAFETY: as above, for the output slot.
        unsafe { SetStdHandle(STD_OUTPUT_HANDLE, HANDLE::default()) }.map_err(|_| ())?;
        // SAFETY: the helper has no console now; it attaches to the receiver's.
        unsafe { AttachConsole(identity.1) }.map_err(|_| ())?;
        let writer = Self {
            receiver,
            plan,
            attached: true,
        };
        writer.confirm()?;
        Ok(writer)
    }

    fn confirm(&self) -> Result<(), ()> {
        let mut clients = vec![0_u32; 16];
        // SAFETY: writable bounded vector, only the helper's attached console.
        let mut count = unsafe { GetConsoleProcessList(&mut clients) } as usize;
        if count > clients.len() {
            clients.resize(count, 0);
            // SAFETY: as above, with the vector grown to the reported count.
            count = unsafe { GetConsoleProcessList(&mut clients) } as usize;
        }
        if count == 0 || count > clients.len() {
            return Err(());
        }
        clients.truncate(count);
        // SAFETY: read-only helper-local attached-console query. Membership in
        // the shell's current console is insufficient if that shell reattached
        // elsewhere after launch: require the recorded classic HWND as well.
        let attached_hwnd = unsafe { GetConsoleWindow() };
        let observed = self
            .receiver
            .observed()
            .filter(|identity| attached_hwnd == HWND(identity.0 as *mut _));
        self.plan
            .confirm_receiver(observed, &clients)
            .map_err(|_| ())
    }

    fn write(self) -> u8 {
        // SAFETY: helper attached successfully, redirected stdin was cleared.
        let Ok(input) = (unsafe { GetStdHandle(STD_INPUT_HANDLE) }) else {
            return 2;
        };
        let records = self
            .plan
            .key_records()
            .into_iter()
            .map(|(down, unit)| INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    KeyEvent: KEY_EVENT_RECORD {
                        bKeyDown: BOOL::from(down),
                        wRepeatCount: 1,
                        wVirtualKeyCode: 0,
                        wVirtualScanCode: 0,
                        uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                        dwControlKeyState: 0,
                    },
                },
            })
            .collect::<Vec<_>>();
        // Last identity + membership read, after all preparation, before the
        // first and only write. Never split, replay or append Enter.
        if self.confirm().is_err() {
            return 2;
        }
        let mut accepted = 0;
        // SAFETY: input belongs to this helper's exact attached console;
        // records are initialized KEY_EVENTs and accepted is writable.
        let result = unsafe { WriteConsoleInputW(input, &records, &raw mut accepted) };
        if result.is_ok() && accepted as usize == records.len() {
            0
        } else {
            3
        }
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        if self.attached {
            // SAFETY: only this short-lived helper attached; daemon never does.
            let _ = unsafe { FreeConsole() };
        }
    }
}
