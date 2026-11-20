//! Whether a `chrome.debugger` attach in a browser is banner-free (#123,
//! #177). Chrome shows its "started debugging this browser" banner on every
//! debugger attach unless the browser runs with
//! `--silent-debugger-extension-api`. The owner's Chrome does not, so the
//! background capture lane is used only in a browser that does (an
//! agent-launched Chrome); the owner's Chrome never gets a new banner.

use super::is_process_switch_arg;

/// The Chrome switch that suppresses the debugger banner.
pub(super) const SILENT_DEBUGGER_SWITCH: &str = "--silent-debugger-extension-api";

/// Whether the browser process command line `args` carries
/// [`SILENT_DEBUGGER_SWITCH`] as a switch of its own.
pub(super) fn debugger_banner_free(args: &[String]) -> bool {
    args.iter()
        .any(|arg| is_process_switch_arg(arg, SILENT_DEBUGGER_SWITCH))
}

#[cfg(test)]
#[path = "chrome_bridge_debugger_banner_table_123.rs"]
mod table_123;
