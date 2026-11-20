//! #77: discover Chromium children inside a native host; retain menu content.
use super::*;
use std::sync::OnceLock;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::{
    Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE,
            WINDOW_STYLE, WNDCLASSW, WS_CHILD,
        },
    },
    core::w,
};

struct Windows(Vec<HWND>);
impl Drop for Windows {
    fn drop(&mut self) {
        for hwnd in self.0.iter().rev() {
            // SAFETY: all handles are owned by this fixture on the current thread.
            let _ = unsafe { DestroyWindow(*hwnd) };
        }
    }
}

unsafe extern "system" fn renderer_77_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: forwards the OS-provided window message unchanged to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn host_with_renderer(renderer: bool) -> Windows {
    static CLASS: OnceLock<()> = OnceLock::new();
    CLASS.get_or_init(|| {
        let instance = unsafe { GetModuleHandleW(None) }.expect("module");
        let class = WNDCLASSW {
            lpfnWndProc: Some(renderer_77_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: w!("Chrome_WidgetWin_FU2_77"),
            ..Default::default()
        };
        assert_ne!(
            unsafe { RegisterClassW(&class) },
            0,
            "register isolated renderer class"
        );
    });
    let root = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("NativeHost77"),
            WINDOW_STYLE(0),
            0,
            0,
            400,
            300,
            None,
            None,
            None,
            None,
        )
    }
    .expect("hidden native root");
    let mut owned = Windows(vec![root]);
    if renderer {
        let child = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("Chrome_WidgetWin_FU2_77"),
                w!("Embedded77"),
                WS_CHILD,
                0,
                0,
                400,
                300,
                Some(root),
                None,
                Some(GetModuleHandleW(None).expect("module").into()),
                None,
            )
        }
        .expect("embedded child");
        owned.0.push(child);
    }
    owned
}

#[test]
fn must_pass_native_host_discovers_embedded_chromium_root_and_direct_root() {
    let owned = host_with_renderer(true);
    assert_eq!(window_class_name(owned.0[0]), "Static");
    assert_eq!(window_class_name(owned.0[1]), "Chrome_WidgetWin_FU2_77");
    assert_eq!(
        chromium_accessibility_root_hwnd(owned.0[0]),
        Some(owned.0[1])
    );
    assert_eq!(
        chromium_accessibility_root_hwnd(owned.0[1]),
        Some(owned.0[1])
    );
}

#[test]
fn must_fail_native_host_without_renderer_returns_none_and_preserves_host() {
    let owned = host_with_renderer(false);
    let before = window_class_name(owned.0[0]);
    assert!(!before.is_empty());
    assert_eq!(chromium_accessibility_root_hwnd(owned.0[0]), None);
    assert_eq!(window_class_name(owned.0[0]), before);
}

fn menu_node() -> AccessibleNode {
    serde_json::from_value(serde_json::json!({"element_id":"0x77:2a","name":"Open",
        "role":"menu item","bbox":{"x":10,"y":150,"w":100,"h":30},
        "enabled":true,"focused":false,"patterns":[],"children_count":0,"depth":2}))
    .expect("menu fixture")
}

#[test]
fn must_pass_named_menu_items_are_renderer_content() {
    let root = synapse_core::Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };
    for role in ["menu item", "Menu Item", "MENU ITEM"] {
        let mut node = menu_node();
        node.role = role.into();
        assert!(is_chromium_renderer_content_node(&node, root), "{role}");
    }
}

#[test]
fn must_fail_menu_item_chrome_empty_geometry_and_anonymous_structure_are_excluded() {
    let root = synapse_core::Rect {
        x: 0,
        y: 0,
        w: 400,
        h: 300,
    };
    let mut zero = menu_node();
    zero.bbox.w = 0;
    let mut chrome = menu_node();
    chrome.automation_id = Some("view_menu".into());
    let mut above = menu_node();
    above.bbox.y = 0;
    let mut anonymous = menu_node();
    anonymous.name.clear();
    for node in [zero, chrome, above, anonymous] {
        let before = node.clone();
        assert!(!is_chromium_renderer_content_node(&node, root));
        assert_eq!(node, before);
    }
}
