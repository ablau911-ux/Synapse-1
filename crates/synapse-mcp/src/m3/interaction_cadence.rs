//! Counts-only low-level input hook for timeline interaction cadence (#838).
//!
//! The hook never records key names, characters, window text, or clipboard
//! content. It only emits event class counters plus the OS-injected flag so the
//! activity recorder can keep human cadence separate from Synapse-generated
//! input. Mouse events also carry their screen point, used transiently to find
//! the window under the pointer (#106) and never stored.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::sync::mpsc;

/// Owner-away mode: wall-clock ns of the last counted physical mouse move
/// (0 = none). Moves are coalesced in the hook and never become cadence events.
static LAST_PHYSICAL_MOVE_NS: AtomicU64 = AtomicU64::new(0);

/// The last counted physical (non-injected) mouse move, if any.
pub fn last_physical_move_ns() -> Option<u64> {
    match LAST_PHYSICAL_MOVE_NS.load(Ordering::Acquire) {
        0 => None,
        ns => Some(ns),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionEvent {
    pub ts_ns: u64,
    pub kind: InteractionEventKind,
    pub injected: bool,
    pub key_signal: Option<InteractionKeySignal>,
    /// Screen position of a mouse event (click/wheel), used only to resolve the
    /// window under the pointer; never stored or recorded.
    pub screen_point: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionEventKind {
    Keystroke,
    Click,
    VerticalScroll { delta: i32 },
    HorizontalScroll { delta: i32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionKeySignal {
    UndoCommand,
    DeleteCommand,
    TextLikeKey,
    OtherKey,
}

pub struct InteractionHook {
    inner: platform::InteractionHook,
}

impl InteractionHook {
    /// Starts the platform low-level input hook.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform hook cannot be installed. The daemon
    /// must fail closed rather than silently run without cadence rows.
    pub fn start(sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
        Ok(Self {
            inner: platform::InteractionHook::start(sender)?,
        })
    }

    #[must_use]
    pub const fn readback(&self) -> &InteractionHookReadback {
        self.inner.readback()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionHookReadback {
    pub thread_id: u32,
    pub keyboard_hook_installed: bool,
    pub mouse_hook_installed: bool,
}

#[cfg(windows)]
mod platform {
    use std::{
        sync::{Mutex, OnceLock, mpsc as std_mpsc},
        thread,
    };

    use anyhow::{Context, Result, bail};
    use tokio::sync::mpsc;
    use windows::Win32::{
        Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::Input::KeyboardAndMouse::GetAsyncKeyState,
        UI::WindowsAndMessaging::{
            CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
            PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
            WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
            WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_XBUTTONDOWN,
        },
    };

    use super::{
        InteractionEvent, InteractionEventKind, InteractionHookReadback, InteractionKeySignal,
    };
    use crate::server::user_active_gate::owner_presence;

    const LLKHF_INJECTED_MASK: u32 = 0x12;
    const LLMHF_INJECTED_MASK: u32 = 0x03;
    const KEY_DOWN_MASK: u16 = 0x8000;
    const VK_BACK_CODE: u32 = 0x08;
    const VK_SPACE_CODE: u32 = 0x20;
    const VK_DELETE_CODE: u32 = 0x2e;
    const VK_CONTROL_CODE: u32 = 0x11;
    const VK_LCONTROL_CODE: u32 = 0xa2;
    const VK_RCONTROL_CODE: u32 = 0xa3;
    const VK_Z_CODE: u32 = 0x5a;
    const VK_PACKET_CODE: u32 = 0xe7;

    static HOOK_SENDER: OnceLock<Mutex<Option<mpsc::UnboundedSender<InteractionEvent>>>> =
        OnceLock::new();

    pub struct InteractionHook {
        readback: InteractionHookReadback,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl InteractionHook {
        pub fn start(sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
            {
                let mut slot = hook_sender()
                    .lock()
                    .map_err(|_| anyhow::anyhow!("interaction hook sender lock poisoned"))?;
                if slot.is_some() {
                    bail!("interaction cadence hook is already installed in this process");
                }
                *slot = Some(sender);
            }

            let (ready_tx, ready_rx) = std_mpsc::channel();
            let thread = thread::Builder::new()
                .name("synapse-interaction-cadence-hook".to_owned())
                .spawn(move || run_hook_thread(ready_tx))
                .context("spawn interaction cadence hook thread")?;
            let readback = match ready_rx.recv() {
                Ok(Ok(readback)) => readback,
                Ok(Err(error)) => {
                    clear_sender();
                    let _ = thread.join();
                    bail!(error);
                }
                Err(error) => {
                    clear_sender();
                    let _ = thread.join();
                    bail!("interaction cadence hook thread exited before readiness: {error}");
                }
            };
            Ok(Self {
                readback,
                thread: Some(thread),
            })
        }

        pub const fn readback(&self) -> &InteractionHookReadback {
            &self.readback
        }
    }

    impl Drop for InteractionHook {
        fn drop(&mut self) {
            // SAFETY: PostThreadMessageW takes only plain integers; thread_id is the hook thread's
            // id. If that thread already exited the call just fails and the result is ignored.
            let _ = unsafe {
                PostThreadMessageW(self.readback.thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
            };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            clear_sender();
        }
    }

    struct HookGuard(HHOOK);

    impl Drop for HookGuard {
        fn drop(&mut self) {
            // SAFETY: self.0 is the HHOOK from SetWindowsHookExW, owned by this guard and unhooked
            // exactly once here on the hook thread; failure is ignored.
            let _ = unsafe { UnhookWindowsHookEx(self.0) };
        }
    }

    fn hook_sender() -> &'static Mutex<Option<mpsc::UnboundedSender<InteractionEvent>>> {
        HOOK_SENDER.get_or_init(|| Mutex::new(None))
    }

    fn clear_sender() {
        if let Ok(mut guard) = hook_sender().lock() {
            *guard = None;
        }
    }

    fn run_hook_thread(ready_tx: std_mpsc::Sender<Result<InteractionHookReadback, String>>) {
        // SAFETY: GetCurrentThreadId has no preconditions.
        let thread_id = unsafe { GetCurrentThreadId() };
        // SAFETY: a None module name returns the calling process's own handle; no pointers.
        let module = match unsafe { GetModuleHandleW(None) } {
            Ok(module) => module,
            Err(error) => {
                let _ = ready_tx.send(Err(format!(
                    "GetModuleHandleW failed for interaction cadence hook: {error}"
                )));
                return;
            }
        };
        // SAFETY: keyboard_hook_proc is a 'static extern "system" fn matching HOOKPROC, module is
        // the valid process HINSTANCE, and thread id 0 makes a global LL hook; this thread pumps
        // messages in the GetMessageW loop below, which low-level hooks require.
        let keyboard_hook = match unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                Some(HINSTANCE(module.0)),
                0,
            )
        } {
            Ok(hook) => hook,
            Err(error) => {
                let _ = ready_tx.send(Err(format!(
                    "SetWindowsHookExW(WH_KEYBOARD_LL) failed: {error}"
                )));
                return;
            }
        };
        // SAFETY: mouse_hook_proc is a 'static extern "system" fn matching HOOKPROC, module is
        // the valid process HINSTANCE, and thread id 0 makes a global LL hook; this thread pumps
        // messages in the GetMessageW loop below, which low-level hooks require.
        let mouse_hook = match unsafe {
            SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook_proc),
                Some(HINSTANCE(module.0)),
                0,
            )
        } {
            Ok(hook) => hook,
            Err(error) => {
                let _keyboard_guard = HookGuard(keyboard_hook);
                let _ = ready_tx.send(Err(format!(
                    "SetWindowsHookExW(WH_MOUSE_LL) failed: {error}"
                )));
                return;
            }
        };
        let _keyboard_guard = HookGuard(keyboard_hook);
        let _mouse_guard = HookGuard(mouse_hook);
        let _ = ready_tx.send(Ok(InteractionHookReadback {
            thread_id,
            keyboard_hook_installed: true,
            mouse_hook_installed: true,
        }));

        let mut msg = MSG::default();
        loop {
            // SAFETY: &raw mut msg points to a live, initialized local MSG; a None HWND receives
            // all messages for this thread, which also drives the low-level hook callbacks.
            let result = unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.0;
            match classify_get_message(result) {
                MessagePump::Continue => {}
                MessagePump::Quit => break,
                MessagePump::Failed => {
                    // GetMessageW returned -1: the pump can no longer run, so stop instead of
                    // busy-spinning; the guards unhook on return.
                    tracing::warn!(
                        code = "MCP_INTERACTION_CADENCE_GETMESSAGE_FAILED",
                        result,
                        os_error = %std::io::Error::last_os_error(),
                        "interaction cadence hook message pump failed; exiting hook thread"
                    );
                    break;
                }
            }
        }
    }

    /// What the hook thread does after one `GetMessageW` call.
    #[derive(Debug, PartialEq, Eq)]
    enum MessagePump {
        /// `> 0`: a message was retrieved; keep pumping.
        Continue,
        /// `0`: `WM_QUIT`; exit normally.
        Quit,
        /// `-1` (or any other negative value): error; `BOOL::as_bool` would wrongly treat this
        /// as true and busy-loop forever.
        Failed,
    }

    const fn classify_get_message(result: i32) -> MessagePump {
        if result > 0 {
            MessagePump::Continue
        } else if result == 0 {
            MessagePump::Quit
        } else {
            MessagePump::Failed
        }
    }

    // SAFETY: contract - Windows invokes this only as the WH_KEYBOARD_LL HOOKPROC on the hook
    // thread; when code >= 0, lparam points to a KBDLLHOOKSTRUCT valid for the callback.
    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = u32::try_from(wparam.0).unwrap_or(0);
            if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                // SAFETY: code >= 0, WM_KEY* message: lparam is a valid, aligned pointer to a
                // KBDLLHOOKSTRUCT living for this callback; it is Copy, read by value.
                let data = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
                emit(
                    InteractionEventKind::Keystroke,
                    data.flags.0 & LLKHF_INJECTED_MASK != 0,
                    Some(key_signal(data.vkCode)),
                    None,
                );
            } else if matches!(message, WM_KEYUP | WM_SYSKEYUP) {
                // Key-up confirms release state but is not an interaction
                // count. Counting key-down only avoids doubling keystrokes.
            }
        }
        // SAFETY: forwards the hook arguments unchanged down the chain; hook handle may be None.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    // SAFETY: contract - Windows invokes this only as the WH_MOUSE_LL HOOKPROC on the hook
    // thread; when code >= 0, lparam points to a MSLLHOOKSTRUCT valid for the callback.
    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 {
            let message = u32::try_from(wparam.0).unwrap_or(0);
            // SAFETY: code >= 0, so lparam is a valid, aligned pointer to a MSLLHOOKSTRUCT that
            // lives for this callback; it is Copy and read by value here.
            let data = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
            let injected = data.flags & LLMHF_INJECTED_MASK != 0;
            let point = (data.pt.x, data.pt.y);
            match message {
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                    emit(InteractionEventKind::Click, injected, None, Some(point));
                }
                WM_MOUSEWHEEL => {
                    emit(
                        InteractionEventKind::VerticalScroll {
                            delta: wheel_delta(data.mouseData),
                        },
                        injected,
                        None,
                        Some(point),
                    );
                }
                WM_MOUSEHWHEEL => {
                    emit(
                        InteractionEventKind::HorizontalScroll {
                            delta: wheel_delta(data.mouseData),
                        },
                        injected,
                        None,
                        Some(point),
                    );
                }
                WM_MOUSEMOVE if !injected => note_physical_move(),
                _ => {}
            }
        }
        // SAFETY: forwards the hook arguments unchanged down the chain; hook handle may be None.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn emit(
        kind: InteractionEventKind,
        injected: bool,
        key_signal: Option<InteractionKeySignal>,
        screen_point: Option<(i32, i32)>,
    ) {
        let event = InteractionEvent {
            ts_ns: super::super_now_ts_ns(),
            kind,
            injected,
            key_signal,
            screen_point,
        };
        owner_presence::observe_input_event(injected, event.ts_ns);
        if let Ok(guard) = hook_sender().lock()
            && let Some(sender) = guard.as_ref()
        {
            let _ = sender.send(event);
        }
    }

    /// Owner-away mode: count a physical mouse move at most once per
    /// [`owner_presence::PHYSICAL_MOVE_COALESCE_MS`] and treat it like any
    /// other physical input for the owner-return stop.
    fn note_physical_move() {
        let now_ns = super::super_now_ts_ns();
        let last_ns = super::LAST_PHYSICAL_MOVE_NS.load(super::Ordering::Acquire);
        if owner_presence::physical_move_due(
            now_ns,
            last_ns,
            owner_presence::PHYSICAL_MOVE_COALESCE_MS,
        ) {
            super::LAST_PHYSICAL_MOVE_NS.store(now_ns, super::Ordering::Release);
            owner_presence::observe_input_event(false, now_ns);
        }
    }

    fn wheel_delta(mouse_data: u32) -> i32 {
        i32::from(((mouse_data >> 16) as u16) as i16)
    }

    fn key_signal(vk_code: u32) -> InteractionKeySignal {
        key_signal_with_ctrl(vk_code, ctrl_down())
    }

    fn key_signal_with_ctrl(vk_code: u32, ctrl_down: bool) -> InteractionKeySignal {
        if vk_code == VK_Z_CODE && ctrl_down {
            return InteractionKeySignal::UndoCommand;
        }
        if matches!(vk_code, VK_BACK_CODE | VK_DELETE_CODE) {
            return InteractionKeySignal::DeleteCommand;
        }
        if text_like_key(vk_code) {
            InteractionKeySignal::TextLikeKey
        } else {
            InteractionKeySignal::OtherKey
        }
    }

    fn ctrl_down() -> bool {
        key_down(VK_CONTROL_CODE) || key_down(VK_LCONTROL_CODE) || key_down(VK_RCONTROL_CODE)
    }

    fn key_down(vk_code: u32) -> bool {
        // SAFETY: GetAsyncKeyState takes a plain integer virtual-key code; no pointers involved.
        let state = unsafe { GetAsyncKeyState(i32::try_from(vk_code).unwrap_or(0)) };
        (state as u16 & KEY_DOWN_MASK) != 0
    }

    const fn text_like_key(vk_code: u32) -> bool {
        matches!(
            vk_code,
            VK_SPACE_CODE
                | VK_PACKET_CODE
                | 0x30..=0x39
                | 0x41..=0x5a
                | 0x60..=0x6f
                | 0xba..=0xc0
                | 0xdb..=0xdf
        )
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Result, bail};
    use tokio::sync::mpsc;

    use super::{InteractionEvent, InteractionHookReadback};

    pub struct InteractionHook {
        readback: InteractionHookReadback,
    }

    impl InteractionHook {
        pub fn start(_sender: mpsc::UnboundedSender<InteractionEvent>) -> Result<Self> {
            bail!("interaction cadence hook requires Windows")
        }

        pub const fn readback(&self) -> &InteractionHookReadback {
            &self.readback
        }
    }
}

fn super_now_ts_ns() -> u64 {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    u64::try_from(nanos).unwrap_or(0)
}
