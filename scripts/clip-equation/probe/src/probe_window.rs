//! The probe's own window, on its own thread. Nothing here touches another
//! application's window: it is created topmost and "no activate", so it never
//! takes the keyboard focus from the owner's foreground window, and clicks on
//! it do not activate it either.
//!
//! `Content::Counter` repaints once per desktop composition (`DwmFlush`). A row
//! of 12 black-or-white blocks of 16 px at the top left holds the low 12 bits
//! of the paint counter, so a captured frame tells which paint it shows; the
//! counter is also drawn as text.
//!
//! `Content::Text` holds an edit control for the caret and typing phases. The
//! typing phase posts `WM_CHAR` to that edit control only.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, FillRect, GdiFlush, GetDC, GetStockObject, HBRUSH, ReleaseDC, TextOutW,
    WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, ES_MULTILINE, GW_CHILD, GetMessageW,
    GetWindow, GetWindowRect, IDC_ARROW, LoadCursorW, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW,
    PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_MOUSEACTIVATE,
    WM_QUIT, WNDCLASSW, WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

use crate::{Result, clock, win};

pub const COUNTER_BITS: u32 = 12;
pub const COUNTER_BLOCK_PX: u32 = 16;
const WM_PROBE_FOCUS: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Counter,
    Text,
}

pub struct ProbeWindow {
    hwnd: isize,
    edit: isize,
    paints: Arc<AtomicU64>,
    painter_cycles: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl ProbeWindow {
    /// Opens the window at `rect` (screen pixels) and returns once it is shown.
    pub fn open(content: Content, rect: RECT) -> Result<Self> {
        let paints = Arc::new(AtomicU64::new(0));
        let painter_cycles = Arc::new(AtomicU64::new(0));
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let paints = paints.clone();
            let painter_cycles = painter_cycles.clone();
            std::thread::spawn(move || {
                run_window(content, rect, &paints, &painter_cycles, &ready_tx)
            })
        };
        let (hwnd, edit) = ready_rx
            .recv()
            .map_err(|_| "window thread ended early".to_owned())??;
        Ok(Self {
            hwnd,
            edit,
            paints,
            painter_cycles,
            thread: Some(thread),
        })
    }

    pub const fn raw_hwnd(&self) -> isize {
        self.hwnd
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }

    /// The window rectangle in screen pixels.
    pub fn rect(&self) -> RECT {
        let mut rect = RECT::default();
        // SAFETY: our own window; one out-parameter.
        let _ = unsafe { GetWindowRect(self.hwnd(), &raw mut rect) };
        rect
    }

    /// Paints done so far (counter content only).
    pub fn paints(&self) -> u64 {
        self.paints.load(Ordering::Relaxed)
    }

    /// Processor cycles the window thread has used so far.
    pub fn painter_cycles(&self) -> u64 {
        self.painter_cycles.load(Ordering::Relaxed)
    }

    /// Gives the edit control the focus of the window's own thread, so its caret shows.
    pub fn focus_edit(&self) {
        // SAFETY: our own window.
        let _ = unsafe { PostMessageW(Some(self.hwnd()), WM_PROBE_FOCUS, WPARAM(0), LPARAM(0)) };
    }

    /// Types one character into the probe's own edit control.
    pub fn type_char(&self, character: char) {
        let edit = HWND(self.edit as *mut c_void);
        // SAFETY: our own edit control; WM_CHAR carries the UTF-16 unit and a repeat count of 1.
        let _ = unsafe { PostMessageW(Some(edit), WM_CHAR, WPARAM(character as usize), LPARAM(1)) };
    }
}

impl Drop for ProbeWindow {
    fn drop(&mut self) {
        // SAFETY: our own window; WM_CLOSE destroys it and ends its thread.
        let _ = unsafe { PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type Ready = mpsc::Sender<Result<(isize, isize)>>;

fn run_window(
    content: Content,
    rect: RECT,
    paints: &AtomicU64,
    painter_cycles: &AtomicU64,
    ready: &Ready,
) {
    let (hwnd, edit) = match create_window(content, rect) {
        Ok(created) => created,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok((
        hwnd.0 as isize,
        edit.map_or(0, |edit| edit.0 as isize),
    )));
    let start = clock::thread_cycles();
    let mut message = MSG::default();
    'running: loop {
        match content {
            Content::Counter => {
                // SAFETY: standard message pump on the window's own thread.
                while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    if message.message == WM_QUIT {
                        break 'running;
                    }
                    // SAFETY: as above.
                    unsafe {
                        let _ = TranslateMessage(&raw const message);
                        DispatchMessageW(&raw const message);
                    }
                }
                // Waits for the next composition, so each composition shows one new paint.
                // SAFETY: no arguments.
                let _ = unsafe { DwmFlush() };
                let paint = paints.fetch_add(1, Ordering::Relaxed) + 1;
                paint_counter(hwnd, paint);
            }
            Content::Text => {
                // SAFETY: standard message pump on the window's own thread.
                if !unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
                    break 'running;
                }
                // SAFETY: as above.
                unsafe {
                    let _ = TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
        }
        painter_cycles.store(clock::thread_cycles() - start, Ordering::Relaxed);
    }
}

fn create_window(content: Content, rect: RECT) -> Result<(HWND, Option<HWND>)> {
    // SAFETY: the module handle of this executable.
    let instance = HINSTANCE(
        unsafe { GetModuleHandleW(None) }
            .map_err(win("GetModuleHandleW"))?
            .0,
    );
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: w!("ClipProbeWindow"),
        // SAFETY: stock objects and system cursors need no release.
        hbrBackground: HBRUSH(unsafe { GetStockObject(WHITE_BRUSH) }.0),
        // SAFETY: as above.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        ..Default::default()
    };
    // SAFETY: a valid class description; 0 means already registered by an earlier window.
    let _ = unsafe { RegisterClassW(&raw const class) };
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    // SAFETY: creates our own top-level window.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("ClipProbeWindow"),
            w!("clip probe"),
            WS_POPUP,
            rect.left,
            rect.top,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(win("CreateWindowExW"))?;
    let edit = match content {
        Content::Counter => None,
        // SAFETY: creates an edit control inside our own window.
        Content::Text => Some(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!("clip probe: caret and typing phases. "),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE as u32),
                    0,
                    0,
                    width,
                    height,
                    Some(hwnd),
                    None,
                    Some(instance),
                    None,
                )
            }
            .map_err(win("CreateWindowExW EDIT"))?,
        ),
    };
    // SAFETY: shows our own window without activating it.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    Ok((hwnd, edit))
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PROBE_FOCUS => {
            // SAFETY: our own window and its edit control, on the window's thread.
            if let Ok(edit) = unsafe { GetWindow(hwnd, GW_CHILD) } {
                // SAFETY: as above.
                let _ = unsafe { SetFocus(Some(edit)) };
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            // SAFETY: ends this window thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn paint_counter(hwnd: HWND, paint: u64) {
    // SAFETY: GDI drawing on our own window's DC, released below.
    unsafe {
        let dc = GetDC(Some(hwnd));
        let white = HBRUSH(GetStockObject(WHITE_BRUSH).0);
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        for bit in 0..COUNTER_BITS {
            let left = (bit * COUNTER_BLOCK_PX) as i32;
            let block = RECT {
                left,
                top: 0,
                right: left + COUNTER_BLOCK_PX as i32,
                bottom: COUNTER_BLOCK_PX as i32,
            };
            let brush = if (paint >> bit) & 1 == 1 {
                white
            } else {
                black
            };
            FillRect(dc, &raw const block, brush);
        }
        let label: Vec<u16> = format!("paint {paint:>8}").encode_utf16().collect();
        let _ = TextOutW(dc, 8, 40, &label);
        ReleaseDC(Some(hwnd), dc);
        let _ = GdiFlush();
    }
}
