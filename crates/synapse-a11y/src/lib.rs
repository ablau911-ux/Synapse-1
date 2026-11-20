#![allow(unsafe_code)]
#![allow(
    clippy::assigning_clones,
    clippy::branches_sharing_code,
    clippy::doc_markdown,
    clippy::format_push_string,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::needless_collect,
    clippy::option_if_let_else,
    clippy::significant_drop_tightening,
    clippy::struct_excessive_bools,
    clippy::struct_field_names,
    clippy::too_long_first_doc_paragraph,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    reason = "synapse-a11y keeps pedantic/nursery style lint debt explicit while using clippy -D warnings for correctness and behavior regressions"
)]
#![cfg_attr(test, allow(clippy::float_cmp))]

// #257 test-only COM provider: windows::core::implement emits absolute
// ::windows_core paths. Re-export its required names without adding a dependency
// or changing production code. Keep Result/Error out of root wildcard imports.
#[cfg(all(test, windows))]
extern crate self as windows_core;
#[cfg(all(test, windows))]
#[allow(unused_imports)]
pub(crate) use windows::core::{
    AsImpl, ComObject, ComObjectInner, ComObjectInterface, DYNAMIC_CAST_IID, GUID, HRESULT,
    IInspectable, IInspectable_Vtbl, IUnknown, IUnknownImpl, Interface, InterfaceRef,
    StaticComObject, imp,
};

mod cdp;
mod cdp_action;
mod cdp_actionability;
mod cdp_ax_timeout;
mod cdp_binding;
mod cdp_capture;
mod cdp_clock;
mod cdp_console;
mod cdp_dialog;
mod cdp_dom;
mod cdp_emulation;
mod cdp_files;
mod cdp_lifecycle;
mod cdp_network;
mod cdp_privacy;
mod cdp_value;
mod endpoint_owner;
mod error;
mod events;
mod field_kind;
mod hwnd_check;
mod ids;
mod lock_state;
mod native_runtime_identity;
mod platform;
#[cfg(windows)]
pub use platform::{has_chromium_child_window, launch_process_record};
mod re_resolve;
mod snapshot;
mod ui_element;
mod window;

pub use cdp::*;
#[cfg(windows)]
pub use cdp_action::*;
#[cfg(windows)]
pub use cdp_action::{CdpMouseStrokePoint, cdp_mouse_stroke_target};
#[cfg(windows)]
pub use cdp_actionability::*;
pub use cdp_ax_timeout::*;
pub use cdp_binding::*;
pub use cdp_clock::*;
pub use cdp_console::*;
pub use cdp_dialog::*;
pub use cdp_dom::*;
pub use cdp_emulation::*;
pub use cdp_files::*;
pub use cdp_lifecycle::*;
pub use cdp_network::*;
pub use cdp_privacy::{password_safe_expression, password_safe_function};
pub use endpoint_owner::{EndpointOwner, ProcessStep};
pub use error::*;
pub use events::*;
pub use field_kind::{
    ComboBoxReadback, Dropdown, DropdownKind, FieldKind, OptionChoice, SetMethod,
};
pub use hwnd_check::{as_foreground_error, hwnd_validity};
pub use ids::*;
pub use lock_state::*;
pub use re_resolve::*;
pub use snapshot::*;
pub use ui_element::*;
pub use window::millis_since_last_input;
pub use window::*;

#[cfg(test)]
mod tests;
