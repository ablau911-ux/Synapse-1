//! Test-only import shim: only the optional related-evidence OS probe is injected.
//! Every other UIA operation still calls the real implementation.
pub use ::synapse_a11y::*;
use std::cell::Cell;

thread_local! {
    static UNREADABLE: Cell<bool> = const { Cell::new(false) };
    static READS: Cell<usize> = const { Cell::new(0) };
    static WATCH_TARGET: Cell<Option<i64>> = const { Cell::new(None) };
    static CAPTURES: Cell<usize> = const { Cell::new(0) };
}

pub(super) struct UnreadableForeground(bool, Option<i64>);

impl Drop for UnreadableForeground {
    fn drop(&mut self) {
        UNREADABLE.set(self.0);
        WATCH_TARGET.set(self.1);
    }
}

pub(super) fn probe(unreadable: bool, target: i64) -> UnreadableForeground {
    READS.set(0);
    CAPTURES.set(0);
    UnreadableForeground(
        UNREADABLE.replace(unreadable),
        WATCH_TARGET.replace(Some(target)),
    )
}

pub(super) fn foreground_reads() -> usize {
    READS.get()
}

pub(super) fn target_captures() -> usize {
    CAPTURES.get()
}

pub fn foreground_context(hwnd: i64) -> A11yResult<synapse_core::ForegroundContext> {
    if WATCH_TARGET.get() == Some(hwnd) {
        CAPTURES.set(CAPTURES.get() + 1);
    }
    ::synapse_a11y::foreground_context(hwnd)
}

pub fn current_foreground_context() -> A11yResult<synapse_core::ForegroundContext> {
    if UNREADABLE.get() {
        READS.set(READS.get() + 1);
        Err(A11yError::NoForeground {
            detail: "TF18 injected OS foreground read failure".to_owned(),
        })
    } else {
        ::synapse_a11y::current_foreground_context()
    }
}
