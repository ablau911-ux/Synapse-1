//! #32 caller-level regression tests: real `UIElement` instances with
//! unreadable `IsPassword` (`None`) or `ES_PASSWORD` (`Some(true)`) must
//! withhold `name` and `value`, hash `""` in fallback runtime IDs, report
//! `is_password = true` with length-only readback in `element_value` /
//! `set_element_value`, and refuse `replace_element_text_selection` /
//! `append_element_text`.

use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use synapse_core::error_codes;
use uiautomation::{
    UIAutomation, UIElement,
    types::{ElementMode, Handle, TreeScope, UIProperty},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
            PostMessageW, SMTO_ABORTIFHUNG, SW_SHOWNOACTIVATE, SendMessageTimeoutW, ShowWindow,
            TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NULL, WM_SETTEXT, WS_CHILD,
            WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE, WaitMessage,
        },
    },
    core::{PCWSTR, w},
};

use super::{
    super::{
        common::{
            cached_maybe_password, cached_public_name, cached_value, fallback_runtime_id_hex,
            map_uia_error, with_automation,
        },
        resolve::{
            append_element_text, element_metadata, element_value, replace_element_text_selection,
            set_element_value,
        },
    },
    node_from_cached_element, snapshot_window_from_hwnd,
};
use crate::{A11yError, A11yResult};

const ES_PASSWORD_STYLE: WINDOW_STYLE = WINDOW_STYLE(0x0020);

struct EditTestWindow {
    parent_hwnd: i64,
    edit_hwnd: i64,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl EditTestWindow {
    fn spawn(password: bool, initial_text: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let text_wide: Vec<u16> = initial_text
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let style = WS_CHILD
                | WS_VISIBLE
                | if password {
                    ES_PASSWORD_STYLE
                } else {
                    WINDOW_STYLE(0)
                };
            // SAFETY: built-in window classes created and destroyed on this pumping thread.
            // The host must be shown: UIA's HWND provider omits the child HWNDs
            // of a hidden top-level window, so a never-shown host snapshots as
            // a bare root and the edit node is never found. Frameless popup,
            // shown without activation, so the human foreground is untouched.
            let parent = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("synapse-32-host"),
                    WS_POPUP,
                    80,
                    80,
                    320,
                    160,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            let edit = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("EDIT"),
                    PCWSTR(text_wide.as_ptr()),
                    style,
                    10,
                    10,
                    220,
                    32,
                    Some(parent),
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            // SAFETY: parent was created on this thread above.
            let _ = unsafe { ShowWindow(parent, SW_SHOWNOACTIVATE) };
            let _ = tx.send((parent.0 as i64, edit.0 as i64));
            pump_until_stopped(&stop_flag, parent);
        });
        let (parent_hwnd, edit_hwnd) = rx.recv().unwrap();
        Self {
            parent_hwnd,
            edit_hwnd,
            stop,
            thread: Some(handle),
        }
    }

    fn set_text(&self, text: &str) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut out = 0_usize;
        // SAFETY: valid edit HWND owned by this fixture's pumping thread.
        let res = unsafe {
            SendMessageTimeoutW(
                HWND(self.edit_hwnd as *mut c_void),
                WM_SETTEXT,
                WPARAM(0),
                LPARAM(wide.as_ptr().cast::<c_void>() as isize),
                SMTO_ABORTIFHUNG,
                500,
                Some(&raw mut out),
            )
        };
        assert_ne!(res.0, 0, "WM_SETTEXT must succeed on fixture edit");
    }
}

fn pump_until_stopped(stop: &AtomicBool, parent: HWND) {
    while !stop.load(Ordering::Acquire) {
        let mut msg = MSG::default();
        // SAFETY: standard thread message pump before destroying owned window.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { TranslateMessage(&raw const msg) };
            let _ = unsafe { DispatchMessageW(&raw const msg) };
        }
        if !stop.load(Ordering::Acquire) {
            let _ = unsafe { WaitMessage() };
        }
    }
    let _ = unsafe { DestroyWindow(parent) };
}

impl Drop for EditTestWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // SAFETY: wakes `WaitMessage` on the fixture thread so it destroys its window and exits.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.parent_hwnd as *mut c_void)),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn cache_without_is_password(automation: &UIAutomation, hwnd: i64) -> A11yResult<UIElement> {
    let cache = automation.create_cache_request().map_err(map_uia_error)?;
    for prop in [
        UIProperty::RuntimeId,
        UIProperty::BoundingRectangle,
        UIProperty::ProcessId,
        UIProperty::ControlType,
        UIProperty::LocalizedControlType,
        UIProperty::Name,
        UIProperty::HasKeyboardFocus,
        UIProperty::IsKeyboardFocusable,
        UIProperty::IsEnabled,
        UIProperty::IsOffscreen,
        UIProperty::AutomationId,
        UIProperty::ClassName,
        UIProperty::NativeWindowHandle,
        UIProperty::IsValuePatternAvailable,
        UIProperty::ValueValue,
    ] {
        cache.add_property(prop).map_err(map_uia_error)?;
    }
    cache
        .set_tree_scope(TreeScope::Element)
        .map_err(map_uia_error)?;
    cache
        .set_element_mode(ElementMode::Full)
        .map_err(map_uia_error)?;
    automation
        .element_from_handle_build_cache(Handle::from(hwnd as isize), &cache)
        .map_err(map_uia_error)
}

#[test]
fn must_fail_element_with_unreadable_is_password_redacts_name_value_and_fallback_id() {
    let win = EditTestWindow::spawn(false, "hunter2-secret");
    let (parent_hwnd, edit_hwnd) = (win.parent_hwnd, win.edit_hwnd);
    let (maybe_pw, pub_name, pub_val, node, fb_hash_1) = with_automation(move |automation| {
        let el = cache_without_is_password(automation, edit_hwnd)?;
        assert!(
            el.get_cached_property_value(UIProperty::IsPassword)
                .is_err(),
            "precondition: IsPassword must be unreadable when omitted from UICacheRequest"
        );
        let raw_val = el
            .get_cached_property_value(UIProperty::ValueValue)
            .map_err(map_uia_error)?
            .get_string()
            .map_err(map_uia_error)?;
        assert_eq!(raw_val, "hunter2-secret", "precondition: ValueValue cached");
        let node = node_from_cached_element(&el, None, 1, parent_hwnd, 0)?;
        Ok((
            cached_maybe_password(&el),
            cached_public_name(&el),
            cached_value(&el),
            node,
            fallback_runtime_id_hex(&el, parent_hwnd),
        ))
    })
    .unwrap();

    assert!(maybe_pw, "unreadable IsPassword must fail closed");
    assert_eq!(pub_name, "");
    assert_eq!(pub_val, None);
    assert_eq!(node.name, "");
    assert_eq!(node.value, None);

    win.set_text("changed-secret-999");
    let fb_hash_2 = with_automation(move |automation| {
        let el = cache_without_is_password(automation, edit_hwnd)?;
        Ok(fallback_runtime_id_hex(&el, parent_hwnd))
    })
    .unwrap();
    assert_eq!(
        fb_hash_1, fb_hash_2,
        "fallback runtime id must hash redacted name, never secret text"
    );
}

#[test]
fn must_fail_password_edit_control_withholds_plaintext_across_snapshot_value_and_insert_call_sites()
{
    let win = EditTestWindow::spawn(true, "s3cr3t!");
    let tree = snapshot_window_from_hwnd(win.parent_hwnd, 2).unwrap();
    let edit_node = tree
        .nodes
        .iter()
        .find(|node| node.depth > 0 && node.role.to_ascii_lowercase().contains("edit"))
        .expect("password edit node must be present in snapshot");
    assert_eq!(edit_node.name, "");
    assert_eq!(edit_node.value, None);

    let meta = element_metadata(&edit_node.element_id).unwrap();
    assert_eq!(meta.name, "");
    assert_eq!(meta.value, None);

    let readback = element_value(&edit_node.element_id).unwrap();
    assert!(readback.is_password);
    assert_eq!(readback.value, "");
    assert_eq!(readback.password_len, Some(7));

    let set_rb = set_element_value(&edit_node.element_id, "newSecret42").unwrap();
    assert!(set_rb.is_password);
    assert_eq!(set_rb.before_value, "");
    assert_eq!(set_rb.after_value, "");
    assert_eq!(set_rb.expected_after_value, None);
    assert_eq!(set_rb.before_password_len, Some(7));
    assert_eq!(set_rb.after_password_len, Some(11));

    let replace_err = replace_element_text_selection(&edit_node.element_id, "x").unwrap_err();
    assert!(matches!(
        replace_err,
        A11yError::ElementValueUnsupported { .. }
    ));
    assert_eq!(
        replace_err.code(),
        error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED
    );
    let append_err = append_element_text(&edit_node.element_id, "x").unwrap_err();
    assert_eq!(
        append_err.code(),
        error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED
    );
    assert_eq!(
        element_value(&edit_node.element_id).unwrap().password_len,
        Some(11)
    );
}

#[test]
fn must_pass_non_password_edit_control_exposes_value_and_allows_set_and_append() {
    let win = EditTestWindow::spawn(false, "hello");
    let tree = snapshot_window_from_hwnd(win.parent_hwnd, 2).unwrap();
    let edit_node = tree
        .nodes
        .iter()
        .find(|node| node.depth > 0 && node.role.to_ascii_lowercase().contains("edit"))
        .expect("normal edit node must be present in snapshot");
    assert_eq!(edit_node.value.as_deref(), Some("hello"));

    let readback = element_value(&edit_node.element_id).unwrap();
    assert!(!readback.is_password);
    assert_eq!(readback.value, "hello");
    assert_eq!(readback.password_len, None);

    let set_rb = set_element_value(&edit_node.element_id, "world").unwrap();
    assert!(!set_rb.is_password);
    assert_eq!(set_rb.before_value, "hello");
    assert_eq!(set_rb.after_value, "world");

    let ins_rb = append_element_text(&edit_node.element_id, "!").unwrap();
    assert_eq!(ins_rb.after_text_utf16_len, 6);
    assert_eq!(
        element_value(&edit_node.element_id).unwrap().value,
        "world!"
    );
}
