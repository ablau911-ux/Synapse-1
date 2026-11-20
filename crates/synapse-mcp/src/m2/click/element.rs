use std::time::Instant;

use rmcp::{ErrorData, model::ErrorCode};
use serde_json::{Value, json};
use synapse_action::{
    ActionError, ActionHandle, DoubleClickTiming, ElementClickOutcome, EmitState, RecordingBackend,
    click_element_or_fallback,
};
use synapse_core::{
    Action, ButtonAction, MouseButton, MouseTarget, Point, UiaPattern, error_codes,
};
use tokio::time::{Duration, sleep};

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT as WinPoint, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowRect, PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP,
    },
};

use crate::m2::postcondition::{mark_input_delivered, mark_input_possibly_delivered};

use super::{
    CLICK_REASON_DELIVERY_UNKNOWN, CLICK_REASON_SELECTION_ONLY, CLICK_TIER_FOREGROUND,
    CLICK_TIER_POSTMESSAGE, CLICK_TIER_UIA, ForegroundClickPolicy, acquire_click_foreground_lease,
    action_error_to_mcp, attach_click_tier_attempts, backend_used_name, click_backend_tier_used,
    click_error_code, click_reason_for_error_code, click_required_foreground, click_tier_delivered,
    click_tier_failed, record,
    schema::{
        ActClickElementTarget, ActClickParams, ActClickResponse, ActClickTierAttempt,
        postcondition_not_requested,
    },
};

#[path = "element_delivery.rs"]
mod delivery;
pub(crate) use delivery::*;
#[path = "element_metadata.rs"]
mod metadata;
use metadata::*;
#[path = "element_native.rs"]
mod native;
#[cfg(windows)]
pub(crate) use native::hwnd_message_readback_for_root;
pub(super) use native::*;
#[path = "element_dispatch.rs"]
mod dispatch;
pub(super) use dispatch::*;
#[path = "element_coordinate.rs"]
mod coordinate;
pub(super) use coordinate::*;

#[cfg(all(test, windows))]
#[path = "element_gate_variants_194.rs"]
mod element_gate_variants_194;

#[cfg(all(test, windows))]
#[path = "native_click_scroll_counts_194.rs"]
mod native_click_scroll_counts_194;

#[cfg(test)]
#[path = "coordinate_tier_table_107a.rs"]
mod coordinate_tier_table_107a;

#[cfg(test)]
#[path = "tests/dispatch_delivery_189.rs"]
pub(crate) mod dispatch_189_test_support;

#[cfg(test)]
#[path = "tests/element_at_most_once_189.rs"]
mod element_at_most_once_189;

#[path = "element_trace.rs"]
mod trace;
use trace::*;
