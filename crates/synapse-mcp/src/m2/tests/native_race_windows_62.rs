//! Isolated no-activation windows on a message-pumping test UI thread; no owner windows.
//! The pump records the posted key/wheel messages the #62 tests inspect instead
//! of dispatching them, so `drain` still reads exactly what production posted,
//! while any other message (a concurrent UIA query, `WindowFromPoint`'s
//! `WM_NCHITTEST`) is answered instead of hanging the shared UIA worker.
use windows::{
    Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
    core::w,
};

use crate::test_support::{PumpedWindows, from_hwnd, to_hwnd};

/// Posted messages the #62 tests read back through [`Windows::drain`].
const RECORDED: &[u32] = &[WM_KEYDOWN, WM_KEYUP, WM_MOUSEWHEEL, WM_MOUSEHWHEEL];

pub(crate) struct Windows {
    pub root: HWND,
    pub other: HWND,
    pub child: HWND,
    ui: PumpedWindows,
}

fn create_root() -> anyhow::Result<HWND> {
    // SAFETY: builtin class, no borrowed creation data; owned by the UI thread.
    Ok(unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("Synapse #62 test"),
            WS_POPUP | WS_VISIBLE,
            -30000,
            -30000,
            100,
            100,
            None,
            None,
            None,
            None,
        )
    }?)
}

fn create_windows() -> anyhow::Result<Vec<i64>> {
    let root = create_root()?;
    let other = create_root()?;
    // SAFETY: live parent, builtin class; never shown or activated.
    let child = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            80,
            80,
            Some(root),
            None,
            None,
            None,
        )
    }?;
    // The child first: the UI thread destroys in this order on drop.
    Ok(vec![from_hwnd(child), from_hwnd(root), from_hwnd(other)])
}

impl Windows {
    pub(crate) fn new() -> Self {
        let ui = PumpedWindows::spawn_recording(create_windows, RECORDED).expect("test windows");
        let result = Self {
            child: to_hwnd(ui.hwnd(0)),
            root: to_hwnd(ui.hwnd(1)),
            other: to_hwnd(ui.hwnd(2)),
            ui,
        };
        assert_eq!(result.root_of_child(), result.root_id());
        result
    }
    pub(crate) fn root_id(&self) -> i64 {
        from_hwnd(self.root)
    }
    pub(crate) fn child_id(&self) -> i64 {
        from_hwnd(self.child)
    }
    pub(crate) fn root_of_child(&self) -> i64 {
        synapse_a11y::top_level_root_hwnd(self.child_id()).expect("native root readback")
    }
    pub(crate) fn reparent(&self) {
        let (child, other) = (self.child_id(), from_hwnd(self.other));
        // SAFETY: both handles live, owned by the UI thread this job runs on.
        let reparented = self.ui.run(move || {
            unsafe { SetParent(to_hwnd(child), Some(to_hwnd(other))) }
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        reparented
            .expect("UI thread job")
            .expect("reparent test child");
        assert_eq!(self.root_of_child(), from_hwnd(self.other));
    }
    /// Removes and returns the recorded `message` rows posted to the child.
    pub(crate) fn drain(&self, message: u32) -> Vec<(usize, isize)> {
        assert!(
            RECORDED.contains(&message),
            "message {message:#x} is not recorded"
        );
        self.ui
            .take_recorded(self.child_id(), message)
            .expect("recorded test messages")
    }
}
