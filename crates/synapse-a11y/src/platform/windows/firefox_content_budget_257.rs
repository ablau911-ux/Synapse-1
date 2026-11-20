//! #257: exercise the real HWND snapshot entry point against a synthetic UIA
//! fragment. Chrome precedes two Documents; only the second Document is active.
//! The fixture creates a hidden window only when a test is executed. It never
//! shows, activates, focuses or sends input to any window.
//!
//! Coverage wire contract proposed from the frozen criteria: selected_content_root
//! is an ElementId, completeness_scope is "document", stop_reason describes the
//! first collection limit, and skipped_subtree_roots names omitted branches.
//! JSON assertions intentionally compile against the old AccessibleSubtree: missing
//! coverage fields fail assertions, rather than requiring nonexistent Rust fields.

#![allow(non_snake_case, non_upper_case_globals)] // COM method names; windows-rs UIA constants in patterns.

use std::{
    cell::RefCell,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde_json::Value;
use synapse_core::{AccessibleSubtree, ElementId};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_NOTIMPL, E_OUTOFMEMORY, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, SAFEARRAY},
            LibraryLoader::GetModuleHandleW,
            Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
            Variant::{VARIANT, VT_I4},
        },
        UI::{
            Accessibility::*,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
                PeekMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_GETOBJECT,
                WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
    },
    core::{Error, IUnknown, Interface, Result as WinResult, implement, w},
};

use super::{SNAPSHOT_NODE_BUDGET, invalidate_snapshot_cache, snapshot_window_from_hwnd};

const CHROME_COUNT: usize = 4_101;
const ACTIVE_NAME: &str = "257 active content";
const DECOY_NAME: &str = "257 inactive content";
pub(super) const CONTROLS: [(&str, UIA_CONTROLTYPE_ID); 3] = [
    ("257 consent", UIA_CheckBoxControlTypeId),
    ("257 details", UIA_HyperlinkControlTypeId),
    ("257 submit", UIA_ButtonControlTypeId),
];

/// Role text the provider reports explicitly, so assertions do not depend on
/// the OS UI language (snapshot roles are UIA localized control types).
fn localized_role(role: UIA_CONTROLTYPE_ID) -> Option<&'static str> {
    match role {
        role if role == UIA_CheckBoxControlTypeId => Some("check box"),
        role if role == UIA_HyperlinkControlTypeId => Some("link"),
        role if role == UIA_ButtonControlTypeId => Some("button"),
        _ => None,
    }
}

fn dead_element() -> Error {
    windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32).into()
}

#[derive(Clone, Copy)]
enum Content {
    Controls,
    Empty,
    OverBudget,
    Deep,
    Slow,
    ProviderError,
}

pub(super) struct Node {
    pub(super) name: String,
    role: UIA_CONTROLTYPE_ID,
    pub(super) parent: Option<usize>,
    pub(super) children: Vec<usize>,
    inactive: bool,
}

pub(super) struct Model {
    pub(super) hwnd: i64,
    pub(super) nodes: Vec<Node>,
    pub(super) active: usize,
    decoy: usize,
    focus: usize,
    pub(super) slow: Option<usize>,
    fault: Option<usize>,
    pub(super) delayed: AtomicBool,
    /// Root UIA class `MozillaWindowClass` (else a plain Win32 class).
    pub(super) gecko_class: bool,
    /// FrameworkId `Gecko` (else `Win32`); separate from the class so each
    /// Gecko signal can be tested alone.
    pub(super) gecko_framework: bool,
}

impl Model {
    /// Window root only; callers add the rest of the tree with `push`.
    pub(super) fn root(hwnd: i64, gecko: bool) -> Self {
        let mut model = Self {
            hwnd,
            nodes: Vec::new(),
            active: 0,
            decoy: 0,
            focus: 0,
            slow: None,
            fault: None,
            delayed: AtomicBool::new(false),
            gecko_class: gecko,
            gecko_framework: gecko,
        };
        model.push(None, "257 Firefox fixture", UIA_WindowControlTypeId);
        model
    }

    pub(super) fn push(
        &mut self,
        parent: Option<usize>,
        name: &str,
        role: UIA_CONTROLTYPE_ID,
    ) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: name.to_owned(),
            role,
            parent,
            children: Vec::new(),
            inactive: false,
        });
        if let Some(parent) = parent {
            self.nodes[parent].children.push(id);
        }
        id
    }

    fn new(hwnd: i64, content: Content) -> Self {
        let mut model = Self::root(hwnd, true);
        let chrome = model.push(Some(0), "257 browser chrome", UIA_ToolBarControlTypeId);
        for index in 0..CHROME_COUNT {
            model.push(
                Some(chrome),
                &format!("257 chrome {index}"),
                UIA_ButtonControlTypeId,
            );
        }
        let decoy = model.push(Some(0), DECOY_NAME, UIA_DocumentControlTypeId);
        model.nodes[decoy].inactive = true;
        model.decoy = decoy;
        for (name, role) in CONTROLS {
            let child = model.push(Some(decoy), &format!("decoy {name}"), role);
            model.nodes[child].inactive = true;
        }
        let active = model.push(Some(0), ACTIVE_NAME, UIA_DocumentControlTypeId);
        model.active = active;
        model.focus = active;
        match content {
            Content::Empty => {}
            Content::Deep => {
                let mut parent = active;
                for index in 0..8 {
                    parent = model.push(
                        Some(parent),
                        &format!("257 depth {index}"),
                        UIA_GroupControlTypeId,
                    );
                }
                model.focus = model.push(Some(parent), "257 deep button", UIA_ButtonControlTypeId);
            }
            Content::OverBudget => {
                for index in 0..SNAPSHOT_NODE_BUDGET + 1 {
                    model.push(
                        Some(active),
                        &format!("257 content {index}"),
                        UIA_ButtonControlTypeId,
                    );
                }
            }
            Content::Controls | Content::Slow | Content::ProviderError => {
                for (name, role) in CONTROLS {
                    model.push(Some(active), name, role);
                }
                let first = model.nodes[active].children[0];
                model.focus = first;
                // Delay/fault a later child: discovery of the focused control
                // itself must not consume the injected collection deadline.
                let last = model.nodes[active].children[2];
                if matches!(content, Content::Slow) {
                    model.slow = Some(last);
                }
                if matches!(content, Content::ProviderError) {
                    model.fault = Some(last);
                }
            }
        }
        model
    }

    fn provider(self: &Arc<Self>, index: usize) -> IRawElementProviderSimple {
        Provider {
            model: Arc::clone(self),
            index,
        }
        .into()
    }

    fn fragment(self: &Arc<Self>, index: usize) -> WinResult<IRawElementProviderFragment> {
        self.provider(index).cast()
    }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot
)]
struct Provider {
    model: Arc<Model>,
    index: usize,
}

impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> WinResult<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> WinResult<IUnknown> {
        // S_OK plus NULL is the UIA contract for unsupported patterns.
        Err(Error::empty())
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> WinResult<VARIANT> {
        let node = &self.model.nodes[self.index];
        if property == UIA_NamePropertyId {
            if self.model.fault == Some(self.index) {
                return Err(dead_element());
            }
            if self.model.slow == Some(self.index)
                && !self.model.delayed.swap(true, Ordering::SeqCst)
            {
                thread::sleep(Duration::from_millis(450));
            }
        }
        Ok(match property {
            UIA_ControlTypePropertyId => VARIANT::from(node.role.0),
            UIA_LocalizedControlTypePropertyId => {
                localized_role(node.role).map_or_else(VARIANT::default, VARIANT::from)
            }
            UIA_NamePropertyId => VARIANT::from(node.name.as_str()),
            UIA_AutomationIdPropertyId => {
                VARIANT::from(format!("fixture257-{}", self.index).as_str())
            }
            UIA_ClassNamePropertyId => VARIANT::from(match (self.index, self.model.gecko_class) {
                (0, true) => "MozillaWindowClass",
                (0, false) => "Synapse257Win32Window",
                _ => "SyntheticFragment257",
            }),
            UIA_FrameworkIdPropertyId => VARIANT::from(if self.model.gecko_framework {
                "Gecko"
            } else {
                "Win32"
            }),
            UIA_ProcessIdPropertyId => {
                VARIANT::from(i32::try_from(std::process::id()).map_err(|_| Error::from(E_FAIL))?)
            }
            UIA_IsControlElementPropertyId
            | UIA_IsContentElementPropertyId
            | UIA_IsKeyboardFocusablePropertyId
            | UIA_IsEnabledPropertyId => VARIANT::from(true),
            UIA_IsOffscreenPropertyId => VARIANT::from(node.inactive),
            UIA_HasKeyboardFocusPropertyId => VARIANT::from(self.index == self.model.focus),
            UIA_IsPasswordPropertyId => VARIANT::from(false),
            UIA_NativeWindowHandlePropertyId if self.index == 0 => {
                VARIANT::from(self.model.hwnd as i32)
            }
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> WinResult<IRawElementProviderSimple> {
        if self.index == 0 {
            unsafe { UiaHostProviderFromHwnd(HWND(self.model.hwnd as *mut _)) }
        } else {
            Err(Error::empty())
        }
    }
}

impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> WinResult<IRawElementProviderFragment> {
        let node = &self.model.nodes[self.index];
        let next = match direction {
            NavigateDirection_Parent => node.parent,
            NavigateDirection_FirstChild => node.children.first().copied(),
            NavigateDirection_LastChild => node.children.last().copied(),
            NavigateDirection_NextSibling | NavigateDirection_PreviousSibling => {
                node.parent.and_then(|parent| {
                    let siblings = &self.model.nodes[parent].children;
                    let position = siblings.iter().position(|index| *index == self.index)?;
                    let next = if direction == NavigateDirection_NextSibling {
                        position.checked_add(1)?
                    } else {
                        position.checked_sub(1)?
                    };
                    siblings.get(next).copied()
                })
            }
            _ => None,
        };
        next.map_or_else(|| Err(Error::empty()), |index| self.model.fragment(index))
    }

    fn GetRuntimeId(&self) -> WinResult<*mut SAFEARRAY> {
        // A dead element: UIA surfaces this to a raw-walker client as a walker
        // error (0x80040201). Measured 2026-10-16: a Navigate or property E_FAIL
        // (even UIA_E_ELEMENTNOTAVAILABLE from Navigate) reaches the client as
        // S_OK + NULL / an empty value, indistinguishable from end-of-list.
        if self.model.fault == Some(self.index) {
            return Err(dead_element());
        }
        // UIA owns this array after a successful return.
        let values = [
            UiaAppendRuntimeId as i32,
            i32::try_from(self.index).map_err(|_| Error::from(E_FAIL))?,
        ];
        let array = unsafe { SafeArrayCreateVector(VT_I4, 0, 2) };
        if array.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        for (index, value) in values.iter().enumerate() {
            let index = i32::try_from(index).map_err(|_| Error::from(E_FAIL))?;
            if let Err(error) =
                unsafe { SafeArrayPutElement(array, &raw const index, ptr::from_ref(value).cast()) }
            {
                let _ = unsafe { SafeArrayDestroy(array) };
                return Err(error);
            }
        }
        Ok(array)
    }

    fn BoundingRectangle(&self) -> WinResult<UiaRect> {
        Ok(UiaRect {
            left: 100.0,
            top: 100.0,
            width: 300.0,
            height: 200.0,
        })
    }
    fn GetEmbeddedFragmentRoots(&self) -> WinResult<*mut SAFEARRAY> {
        Ok(ptr::null_mut())
    }
    fn SetFocus(&self) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn FragmentRoot(&self) -> WinResult<IRawElementProviderFragmentRoot> {
        self.model.provider(0).cast()
    }
}

impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(&self, _: f64, _: f64) -> WinResult<IRawElementProviderFragment> {
        self.model.fragment(self.model.active)
    }
    fn GetFocus(&self) -> WinResult<IRawElementProviderFragment> {
        self.model.fragment(self.model.focus)
    }
}

thread_local! { static ROOT: RefCell<Option<IRawElementProviderSimple>> = const { RefCell::new(None) }; }

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_GETOBJECT && lparam.0 as i32 == UiaRootObjectId {
        return ROOT.with(|root| {
            root.borrow()
                .as_ref()
                .map_or(LRESULT(0), |provider| unsafe {
                    UiaReturnRawElementProvider(hwnd, wparam, lparam, provider)
                })
        });
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

pub(super) struct Fixture {
    pub(super) model: Arc<Model>,
    stop: mpsc::Sender<()>,
    join: Option<JoinHandle<WinResult<()>>>,
}

impl Fixture {
    fn spawn(content: Content) -> Self {
        Self::spawn_with(true, move |hwnd| Model::new(hwnd, content))
    }

    /// Hidden window of class `MozillaWindowClass` (`gecko`) or a plain Win32
    /// class, serving the UIA tree `build` creates for the window handle.
    pub(super) fn spawn_with(
        gecko: bool,
        build: impl FnOnce(i64) -> Model + Send + 'static,
    ) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop, stop_rx) = mpsc::channel();
        let join = thread::spawn(move || -> WinResult<()> {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            }
            let instance = unsafe { GetModuleHandleW(None)? };
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: if gecko {
                    w!("MozillaWindowClass")
                } else {
                    w!("Synapse257Win32Window")
                },
                ..Default::default()
            };
            let _ = unsafe { RegisterClassW(&raw const class) };
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class.lpszClassName,
                    w!("Firefox257"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    400,
                    300,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )?
            };
            let model = Arc::new(build(hwnd.0 as i64));
            ROOT.with(|root| *root.borrow_mut() = Some(model.provider(0)));
            let _ = ready_tx.send(model);
            let mut message = MSG::default();
            while stop_rx.try_recv().is_err() {
                while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    let _ = unsafe { TranslateMessage(&raw const message) };
                    unsafe {
                        DispatchMessageW(&raw const message);
                    }
                }
                thread::sleep(Duration::from_millis(1));
            }
            unsafe {
                DestroyWindow(hwnd)?;
            }
            ROOT.with(|root| *root.borrow_mut() = None);
            unsafe {
                CoUninitialize();
            }
            Ok(())
        });
        let model = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("#257 fixture initialization failed: {error}"));
        Self {
            model,
            stop,
            join: Some(join),
        }
    }

    pub(super) fn snapshot(&self, depth: u32) -> AccessibleSubtree {
        invalidate_snapshot_cache();
        snapshot_window_from_hwnd(self.model.hwnd, depth)
            .unwrap_or_else(|error| panic!("#257 real snapshot entry failed: {error}"))
    }

    /// Identity of a direct child of the window root.
    pub(super) fn id(&self, index: usize) -> ElementId {
        self.find_id(index, false)
    }

    /// Identity of an element at any depth below the window root.
    pub(super) fn nested_id(&self, index: usize) -> ElementId {
        self.find_id(index, true)
    }

    fn find_id(&self, index: usize, nested: bool) -> ElementId {
        // UIA fragment runtime ids are assigned by UIA, not guessed from HWNDs.
        // Resolve only the known Document to establish the assertion's identity;
        // this discovery is NOT used to obtain the snapshot under test.
        use uiautomation::{
            types::{Handle, TreeScope, UIProperty},
            variants::Variant,
        };
        let hwnd = self.model.hwnd;
        let name = self.model.nodes[index].name.clone();
        let scope = if nested {
            TreeScope::Descendants
        } else {
            TreeScope::Children
        };
        super::super::common::with_automation(move |automation| {
            let root = automation
                .element_from_handle(Handle::from(hwnd as isize))
                .map_err(super::super::common::map_uia_error)?;
            let condition = automation
                .create_property_condition(UIProperty::Name, Variant::from(name.as_str()), None)
                .map_err(super::super::common::map_uia_error)?;
            let document = root
                .find_first(scope, &condition)
                .map_err(super::super::common::map_uia_error)?;
            let cache = super::super::common::create_cache_request(
                automation,
                0,
                uiautomation::types::ElementMode::Full,
                super::super::common::TreeView::Raw,
            )?;
            let document = document
                .build_updated_cache(&cache)
                .map_err(super::super::common::map_uia_error)?;
            super::element_id_from_cached_element(&document, hwnd)
        })
        .unwrap_or_else(|error| panic!("#257 independent Document identity read failed: {error}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(join) = self.join.take() {
            let result = join.join();
            assert!(
                matches!(result, Ok(Ok(()))),
                "#257 fixture teardown failed: {result:?}"
            );
        }
    }
}

fn coverage(tree: &AccessibleSubtree, selected: &ElementId) -> Value {
    let value =
        serde_json::to_value(tree).unwrap_or_else(|error| panic!("serialize snapshot: {error}"));
    assert_eq!(
        value["selected_content_root"],
        serde_json::to_value(selected)
            .unwrap_or_else(|error| panic!("serialize ElementId: {error}")),
        "#257: snapshot must identify the active Document, even when browser chrome exhausted coverage"
    );
    assert_eq!(
        value["completeness_scope"], "document",
        "#257: distinguish content coverage from whole-window coverage"
    );
    value
}

#[test]
fn must_pass_257_active_content_controls_survive_more_than_4000_preceding_chrome_nodes() {
    let fixture = Fixture::spawn(Content::Controls);
    let tree = fixture.snapshot(4);
    for (name, role) in [
        ("257 consent", "check box"),
        ("257 details", "link"),
        ("257 submit", "button"),
    ] {
        let matches: Vec<_> = tree.nodes.iter().filter(|node| node.name == name).collect();
        assert_eq!(
            matches.len(),
            1,
            "#257: active {name} starved by preceding chrome"
        );
        assert_eq!(matches[0].role, role, "#257: wrong content control type");
    }
    assert!(
        tree.nodes.len() <= SNAPSHOT_NODE_BUDGET,
        "#257: content prioritization must retain node ceiling"
    );
    assert!(
        tree.nodes.iter().any(|node| node.name == "257 chrome 0"),
        "#257: retain browser chrome coverage after prioritizing content"
    );
}

#[test]
fn must_pass_257_truncated_window_reports_selected_root_scope_stop_reason_and_skipped_roots() {
    let fixture = Fixture::spawn(Content::Controls);
    let selected = fixture.id(fixture.model.active);
    let tree = fixture.snapshot(4);
    assert!(
        tree.truncated,
        "#257: >4000 chrome nodes cannot be complete within 4000 nodes"
    );
    let value = coverage(&tree, &selected);
    assert!(
        matches!(
            value["stop_reason"].as_str(),
            Some("node_budget" | "deadline")
        ),
        "#257: distinguish node exhaustion from provider deadline: {value}"
    );
    assert!(
        value["skipped_subtree_roots"]
            .as_array()
            .is_some_and(|roots| !roots.is_empty()),
        "#257: identify omitted subtree roots"
    );
}

#[test]
fn must_fail_257_inactive_decoy_document_is_never_selected_even_when_it_precedes_active_content() {
    let fixture = Fixture::spawn(Content::Controls);
    let selected = fixture.id(fixture.model.active);
    let decoy = fixture.id(fixture.model.decoy);
    let tree = fixture.snapshot(4);
    let value = coverage(&tree, &selected);
    assert_ne!(
        value["selected_content_root"],
        serde_json::to_value(decoy).unwrap_or_else(|error| panic!("serialize decoy: {error}")),
        "#257: inactive Document selected"
    );
}

#[test]
fn must_pass_257_empty_active_document_remains_selected_instead_of_using_populated_decoy() {
    let fixture = Fixture::spawn(Content::Empty);
    let selected = fixture.id(fixture.model.active);
    let tree = fixture.snapshot(4);
    coverage(&tree, &selected);
    let document = tree
        .nodes
        .iter()
        .find(|node| node.element_id == selected)
        .unwrap_or_else(|| panic!("#257: empty active Document omitted"));
    assert_eq!(
        document.children_count, 0,
        "#257: empty Document acquired decoy children"
    );
}

#[test]
fn must_fail_257_oversized_content_stays_bounded_and_reports_collection_limit() {
    let fixture = Fixture::spawn(Content::OverBudget);
    let selected = fixture.id(fixture.model.active);
    let tree = fixture.snapshot(4);
    assert!(tree.truncated);
    assert!(
        tree.nodes.len() <= SNAPSHOT_NODE_BUDGET,
        "#257: content traversal exceeded hard node boundary"
    );
    let value = coverage(&tree, &selected);
    assert!(
        matches!(
            value["stop_reason"].as_str(),
            Some("node_budget" | "deadline")
        ),
        "#257: oversized content must identify the observed limit"
    );
}

#[test]
fn must_pass_257_content_leaves_at_requested_depth_boundary_are_reachable() {
    let fixture = Fixture::spawn(Content::Controls);
    let tree = fixture.snapshot(2);
    for (name, _) in CONTROLS {
        assert!(
            tree.nodes.iter().any(|node| node.name == name),
            "#257: content leaf at requested depth boundary omitted: {name}"
        );
    }
    coverage(&tree, &fixture.id(fixture.model.active));
}

#[test]
fn must_fail_257_depth_boundary_reports_hidden_content_descendants() {
    let fixture = Fixture::spawn(Content::Deep);
    let selected = fixture.id(fixture.model.active);
    let tree = fixture.snapshot(2);
    assert!(
        tree.truncated,
        "#257: descendants beyond requested depth must remain incomplete"
    );
    let value = coverage(&tree, &selected);
    assert_eq!(
        value["stop_reason"], "depth_limit",
        "#257: depth-cut content must not claim complete coverage"
    );
    assert!(!tree.nodes.iter().any(|node| node.name == "257 deep button"));
}

#[test]
fn must_fail_257_slow_content_provider_reports_deadline_without_discarding_collected_nodes() {
    let fixture = Fixture::spawn(Content::Slow);
    let selected = fixture.id(fixture.model.active);
    // The process-wide UIA worker is shared by every test in this binary. Hold
    // it exclusively so no other test's job is queued ahead of this snapshot:
    // the elapsed time then measures this walk only (#2399 G4).
    let worker = super::super::common::worker_test_gate::exclusive();
    let started = Instant::now();
    let tree = fixture.snapshot(4);
    let elapsed = started.elapsed();
    drop(worker);
    assert!(
        elapsed < super::SNAPSHOT_WORKER_REPLY_TIMEOUT,
        "#257: finite slow provider must not exhaust worker reply timeout"
    );
    let value = coverage(&tree, &selected);
    assert!(
        fixture.model.delayed.load(Ordering::SeqCst),
        "#257: deadline fixture never reached active content"
    );
    assert!(
        tree.truncated,
        "#257: provider crossing deadline must be incomplete"
    );
    assert_eq!(value["stop_reason"], "deadline");
    assert!(
        tree.nodes.iter().any(|node| node.name == "257 consent"),
        "#257: preserve content collected before deadline"
    );
}

#[test]
fn must_fail_257_content_provider_error_reports_incomplete_coverage() {
    let fixture = Fixture::spawn(Content::ProviderError);
    let selected = fixture.id(fixture.model.active);
    let tree = fixture.snapshot(4);
    let value = coverage(&tree, &selected);
    assert!(
        tree.truncated,
        "#257: failed provider must never mean complete"
    );
    assert_eq!(value["stop_reason"], "provider_error");
}
