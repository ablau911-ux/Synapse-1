use std::{
    ffi::c_void,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, TryLockError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use synapse_core::{Rect, UiaPattern};
use uiautomation::{
    UIAutomation, UIElement,
    core::UICacheRequest,
    types::{ControlType, ElementMode, TreeScope, UIProperty},
    variants::{Value, Variant},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM},
    System::{
        Com::{
            APTTYPE, APTTYPE_MAINSTA, APTTYPE_MTA, APTTYPE_NA, APTTYPE_STA, APTTYPEQUALIFIER,
            COINIT_MULTITHREADED, CoGetApartmentType, CoInitializeEx, CoUninitialize,
        },
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::EnumThreadWindows,
};
use windows::core::BOOL;

use crate::{
    A11yError, A11yResult, ComApartmentKind, ElementSearchScope, UiaWorkerPhase, UiaWorkerReadback,
    ids::runtime_id_hex,
};

type UiaJob = Box<dyn FnOnce(&UIAutomation) + Send + 'static>;

#[cfg(test)]
#[path = "password_fail_closed_32.rs"]
mod password_fail_closed_32;

#[cfg(test)]
#[path = "worker_bound_tests.rs"]
mod worker_bound_tests;

static UIA_WORKER: OnceLock<Mutex<Option<Arc<ProcessUiaWorker>>>> = OnceLock::new();
/// UIA worker threads currently alive: the active one plus retired workers
/// whose thread is still blocked inside a stuck provider call (#61).
static LIVE_UIA_WORKER_THREADS: AtomicUsize = AtomicUsize::new(0);
/// Hard bound on [`LIVE_UIA_WORKER_THREADS`]; a new worker is refused (fast
/// `A11Y_UIA_WORKER_TIMEOUT`) rather than spawning an unbounded number of
/// threads and MTAs behind repeatedly hanging providers.
const DEFAULT_MAX_LIVE_UIA_WORKER_THREADS: usize = 4;
const MAX_LIVE_UIA_WORKER_THREADS_ENV: &str = "SYNAPSE_UIA_MAX_WORKER_THREADS";

fn max_live_uia_worker_threads() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var(MAX_LIVE_UIA_WORKER_THREADS_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_LIVE_UIA_WORKER_THREADS)
    })
}

/// Whether another worker thread may be started given the live count.
const fn may_start_worker(live: usize, max: usize) -> bool {
    live < max
}

/// Counts a worker thread for exactly its lifetime.
/// Counts one live worker thread in `counter` until dropped. Production uses
/// [`LIVE_UIA_WORKER_THREADS`]; tests pass a private counter so concurrent
/// tests cannot disturb each other's readback.
struct WorkerThreadGuard(&'static AtomicUsize);

impl WorkerThreadGuard {
    fn register() -> Self {
        Self::register_in(&LIVE_UIA_WORKER_THREADS)
    }

    fn register_in(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for WorkerThreadGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

const DEFAULT_UIA_WORKER_JOB_TIMEOUT: Duration = Duration::from_secs(10);
const UIA_WORKER_BUSY_POLL_INTERVAL: Duration = Duration::from_millis(10);
const FALLBACK_RUNTIME_ID_PREFIX: &str = "ffffffff";
const FNV64_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV64_PRIME: u64 = 0x0100_0000_01b3;

#[derive(Debug)]
pub(super) struct RuntimeIdHexReadback {
    pub hex: String,
    pub used_fallback: bool,
}

struct ProcessUiaWorker {
    tx: mpsc::Sender<UiaJob>,
    timed_out: Arc<AtomicBool>,
    job_lock: Mutex<()>,
}

fn worker_slot() -> &'static Mutex<Option<Arc<ProcessUiaWorker>>> {
    UIA_WORKER.get_or_init(|| Mutex::new(None))
}

fn lock_worker_slot() -> A11yResult<MutexGuard<'static, Option<Arc<ProcessUiaWorker>>>> {
    worker_slot()
        .lock()
        .map_err(|err| A11yError::internal(err.to_string()))
}

fn worker() -> A11yResult<Arc<ProcessUiaWorker>> {
    let mut guard = lock_worker_slot()?;
    if let Some(worker) = guard.as_ref() {
        if !worker.timed_out.load(Ordering::SeqCst) {
            return Ok(Arc::clone(worker));
        }
        tracing::warn!(
            code = "A11Y_UIA_WORKER_RETIRE_TIMEOUT",
            "retiring timed-out UIA worker before accepting another job"
        );
        *guard = None;
    }

    let live = LIVE_UIA_WORKER_THREADS.load(Ordering::SeqCst);
    if !may_start_worker(live, max_live_uia_worker_threads()) {
        tracing::error!(
            code = "A11Y_UIA_WORKER_LIMIT",
            live_worker_threads = live,
            max_worker_threads = max_live_uia_worker_threads(),
            "refusing to start another UIA worker: earlier workers are still blocked in provider calls"
        );
        return Err(A11yError::uia_worker_timeout(
            UiaWorkerPhase::WorkerLimit,
            format!(
                "phase=worker_limit live_worker_threads={live} max_worker_threads={} remediation=retry later; earlier UIA provider calls are still hung and starting more workers would leak threads",
                max_live_uia_worker_threads()
            ),
        ));
    }
    let worker = Arc::new(start_worker()?);
    *guard = Some(Arc::clone(&worker));
    Ok(worker)
}

fn start_worker() -> A11yResult<ProcessUiaWorker> {
    let (tx, rx) = mpsc::channel::<UiaJob>();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread_guard = WorkerThreadGuard::register();
    thread::Builder::new()
        .name("synapse-a11y-uia-mta".to_owned())
        .spawn(move || {
            let _alive = thread_guard;
            uia_worker_thread(rx, ready_tx);
        })
        .map_err(|err| A11yError::internal(format!("spawn UIA worker thread failed: {err}")))?;

    let readback = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|err| A11yError::internal(format!("UIA worker did not initialize: {err}")))??;
    tracing::info!(
        code = "A11Y_UIA_WORKER_READY",
        thread_id = readback.thread_id,
        apartment = ?readback.apartment,
        owned_window_count = readback.owned_window_count,
        "UI Automation worker initialized"
    );
    Ok(ProcessUiaWorker {
        tx,
        timed_out: Arc::new(AtomicBool::new(false)),
        job_lock: Mutex::new(()),
    })
}

#[allow(clippy::needless_pass_by_value)]
fn uia_worker_thread(
    rx: mpsc::Receiver<UiaJob>,
    ready: mpsc::SyncSender<A11yResult<UiaWorkerReadback>>,
) {
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init.is_err() {
        let _ = ready.send(Err(A11yError::internal(format!(
            "CoInitializeEx(COINIT_MULTITHREADED) failed: {init:?}"
        ))));
        return;
    }

    let automation = match UIAutomation::new_direct().map_err(map_uia_error) {
        Ok(automation) => automation,
        Err(err) => {
            let _ = ready.send(Err(err));
            unsafe {
                CoUninitialize();
            }
            return;
        }
    };

    let readback = read_current_worker_state();
    let _ = ready.send(Ok(readback));

    while let Ok(job) = rx.recv() {
        job(&automation);
    }

    unsafe {
        CoUninitialize();
    }
}

pub(super) fn with_automation<T: Send + 'static>(
    action: impl FnOnce(&UIAutomation) -> A11yResult<T> + Send + 'static,
) -> A11yResult<T> {
    with_automation_operation("uia_worker_job", DEFAULT_UIA_WORKER_JOB_TIMEOUT, action)
}

pub(super) fn with_automation_operation<T: Send + 'static>(
    operation: impl Into<String>,
    timeout: Duration,
    action: impl FnOnce(&UIAutomation) -> A11yResult<T> + Send + 'static,
) -> A11yResult<T> {
    let operation = operation.into();
    #[cfg(test)]
    let _worker_test_gate = worker_test_gate::shared();
    let worker = worker()?;
    if worker.timed_out.load(Ordering::SeqCst) {
        retire_worker(&worker, "pre_send_timeout");
        return Err(worker_unavailable(&operation, timeout, None));
    }

    // #260: the queue wait has its own deadline. A request that is still
    // queued when it expires is refused (phase=worker_busy) and never marks
    // the worker hung; the execution deadline starts only at dispatch.
    let queued = Instant::now();
    let _job_guard = acquire_job_slot(&worker, &operation, timeout, queued)?;
    if worker.timed_out.load(Ordering::SeqCst) {
        retire_worker(&worker, "post_lock_timeout");
        return Err(worker_unavailable(&operation, timeout, Some(queued)));
    }
    run_admitted_job(&worker, &operation, timeout, queued.elapsed(), action)
}

/// Sends an admitted job and waits up to the full execution `timeout` for its
/// reply. Only an executing job that exceeds that deadline retires the worker.
fn run_admitted_job<T: Send + 'static>(
    worker: &Arc<ProcessUiaWorker>,
    operation: &str,
    timeout: Duration,
    queue_wait: Duration,
    action: impl FnOnce(&UIAutomation) -> A11yResult<T> + Send + 'static,
) -> A11yResult<T> {
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let dispatched = Instant::now();
    worker
        .tx
        .send(Box::new(move |automation| {
            let _ = reply_tx.send(action(automation));
        }))
        .map_err(|err| {
            retire_worker(worker, "send_failed");
            A11yError::internal(format!("send UIA worker job failed: {err}"))
        })?;
    match reply_rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            worker.timed_out.store(true, Ordering::SeqCst);
            retire_worker(worker, "reply_timeout");
            Err(A11yError::uia_worker_timeout(
                UiaWorkerPhase::ReplyRecv,
                format!(
                    "operation={operation} phase=worker_reply_recv timeout_ms={} elapsed_ms={} queue_wait_ms={} remediation=retry; UIA provider call did not return before the bounded worker deadline, daemon stayed alive, and the worker was retired so unrelated later requests can use a fresh worker",
                    timeout.as_millis(),
                    dispatched.elapsed().as_millis(),
                    queue_wait.as_millis()
                ),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            retire_worker(worker, "reply_disconnected");
            Err(A11yError::UiaWorkerReplyLost {
                detail: format!(
                    "receive UIA worker job failed for operation={operation}: channel disconnected"
                ),
            })
        }
    }
}

/// Refusal for a request whose worker was retired by an earlier job's
/// execution timeout; the request itself was never sent.
fn worker_unavailable(operation: &str, timeout: Duration, queued: Option<Instant>) -> A11yError {
    let elapsed = queued
        .map(|queued| format!(" elapsed_ms={}", queued.elapsed().as_millis()))
        .unwrap_or_default();
    A11yError::uia_worker_timeout(
        UiaWorkerPhase::WorkerUnavailable,
        format!(
            "operation={operation} phase=worker_unavailable previous_timeout=true timeout_ms={}{elapsed} remediation=retry; previous UIA worker job exceeded its bounded deadline and this worker was retired so the next request can use a fresh worker",
            timeout.as_millis()
        ),
    )
}

fn acquire_job_slot<'a>(
    worker: &'a ProcessUiaWorker,
    operation: &str,
    timeout: Duration,
    queued: Instant,
) -> A11yResult<MutexGuard<'a, ()>> {
    loop {
        if worker.timed_out.load(Ordering::SeqCst) {
            return Err(worker_unavailable(operation, timeout, Some(queued)));
        }
        match worker.job_lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(err)) => {
                return Err(A11yError::internal(format!(
                    "UIA worker job lock poisoned for operation={operation}: {err}"
                )));
            }
            Err(TryLockError::WouldBlock) => {
                let elapsed = queued.elapsed();
                let Some(remaining) = timeout.checked_sub(elapsed) else {
                    // Queue timeout: never sent, the worker is not retired (#260).
                    return Err(A11yError::uia_worker_timeout(
                        UiaWorkerPhase::WorkerBusy,
                        format!(
                            "operation={operation} phase=worker_busy queue_timeout=true timeout_ms={} elapsed_ms={} remediation=retry; UIA worker was still running an earlier job when this request's queue-wait deadline expired, so it was never sent and the worker stays in service",
                            timeout.as_millis(),
                            elapsed.as_millis()
                        ),
                    ));
                };
                thread::sleep(remaining.min(UIA_WORKER_BUSY_POLL_INTERVAL));
            }
        }
    }
}

fn retire_worker(worker: &Arc<ProcessUiaWorker>, reason: &'static str) {
    match lock_worker_slot() {
        Ok(mut guard) => {
            if guard
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, worker))
            {
                *guard = None;
                tracing::warn!(
                    code = "A11Y_UIA_WORKER_RETIRED",
                    reason,
                    live_worker_threads = LIVE_UIA_WORKER_THREADS.load(Ordering::SeqCst),
                    "retired UIA worker; a later UIA request will start a fresh worker"
                );
            }
        }
        Err(error) => {
            tracing::error!(
                code = "A11Y_UIA_WORKER_RETIRE_FAILED",
                reason,
                detail = %error,
                "could not lock UIA worker slot for retirement"
            );
        }
    }
}

pub(super) fn worker_readback() -> A11yResult<UiaWorkerReadback> {
    with_automation(|_automation| Ok(read_current_worker_state()))
}

fn read_current_worker_state() -> UiaWorkerReadback {
    let thread_id = unsafe { GetCurrentThreadId() };
    UiaWorkerReadback {
        thread_id,
        apartment: read_current_apartment(),
        owned_window_count: owned_window_count(thread_id),
    }
}

fn read_current_apartment() -> ComApartmentKind {
    let mut apartment = APTTYPE::default();
    let mut qualifier = APTTYPEQUALIFIER::default();
    if unsafe { CoGetApartmentType(&raw mut apartment, &raw mut qualifier) }.is_err() {
        return ComApartmentKind::Unknown;
    }
    match apartment {
        value if value == APTTYPE_STA => ComApartmentKind::Sta,
        value if value == APTTYPE_MTA => ComApartmentKind::Mta,
        value if value == APTTYPE_NA => ComApartmentKind::Neutral,
        value if value == APTTYPE_MAINSTA => ComApartmentKind::MainSta,
        _ => ComApartmentKind::Unknown,
    }
}

fn owned_window_count(thread_id: u32) -> usize {
    unsafe extern "system" fn enum_window(_hwnd: HWND, lparam: LPARAM) -> BOOL {
        let count = unsafe { &mut *(lparam.0 as *mut usize) };
        *count = count.saturating_add(1);
        BOOL(1)
    }

    let mut count = 0_usize;
    let _ = unsafe {
        EnumThreadWindows(
            thread_id,
            Some(enum_window),
            LPARAM((&raw mut count).cast::<c_void>() as isize),
        )
    };
    count
}

pub(super) fn create_cache_request(
    automation: &UIAutomation,
    depth: u32,
    element_mode: ElementMode,
    tree_view: TreeView,
) -> A11yResult<UICacheRequest> {
    let cache = automation.create_cache_request().map_err(map_uia_error)?;
    for property in [
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
        UIProperty::IsControlElement,
        UIProperty::IsContentElement,
        UIProperty::IsPassword,
        UIProperty::NativeWindowHandle,
        UIProperty::IsInvokePatternAvailable,
        UIProperty::IsTogglePatternAvailable,
        UIProperty::IsValuePatternAvailable,
        UIProperty::IsSelectionPatternAvailable,
        UIProperty::IsSelectionItemPatternAvailable,
        UIProperty::IsExpandCollapsePatternAvailable,
        UIProperty::IsLegacyIAccessiblePatternAvailable,
        UIProperty::IsScrollPatternAvailable,
        UIProperty::IsScrollItemPatternAvailable,
        UIProperty::IsTextPatternAvailable,
        UIProperty::IsWindowPatternAvailable,
        UIProperty::IsTransformPatternAvailable,
        UIProperty::IsRangeValuePatternAvailable,
        UIProperty::ValueValue,
        UIProperty::RangeValueValue,
        UIProperty::ToggleToggleState,
        UIProperty::SelectionItemIsSelected,
        UIProperty::ExpandCollapseExpandCollapseState,
    ] {
        cache.add_property(property).map_err(map_uia_error)?;
    }
    let tree_filter = match tree_view {
        TreeView::Control => automation.get_control_view_condition(),
        TreeView::Raw => automation.create_true_condition(),
    }
    .map_err(map_uia_error)?;
    cache.set_tree_filter(tree_filter).map_err(map_uia_error)?;
    let scope = if depth == 0 {
        TreeScope::Element
    } else {
        TreeScope::Subtree
    };
    cache.set_tree_scope(scope).map_err(map_uia_error)?;
    cache
        .set_element_mode(element_mode)
        .map_err(map_uia_error)?;
    Ok(cache)
}
pub(super) fn cached_patterns(element: &UIElement) -> Vec<UiaPattern> {
    let mut patterns = Vec::new();
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsInvokePatternAvailable,
        UiaPattern::Invoke,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsTogglePatternAvailable,
        UiaPattern::Toggle,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsValuePatternAvailable,
        UiaPattern::Value,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsSelectionPatternAvailable,
        UiaPattern::Selection,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsSelectionItemPatternAvailable,
        UiaPattern::SelectionItem,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsExpandCollapsePatternAvailable,
        UiaPattern::ExpandCollapse,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsLegacyIAccessiblePatternAvailable,
        UiaPattern::LegacyIAccessible,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsScrollPatternAvailable,
        UiaPattern::Scroll,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsScrollItemPatternAvailable,
        UiaPattern::ScrollItem,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsTextPatternAvailable,
        UiaPattern::Text,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsWindowPatternAvailable,
        UiaPattern::Window,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsTransformPatternAvailable,
        UiaPattern::Transform,
    );
    push_pattern(
        element,
        &mut patterns,
        UIProperty::IsRangeValuePatternAvailable,
        UiaPattern::RangeValue,
    );
    patterns
}

fn push_pattern(
    element: &UIElement,
    patterns: &mut Vec<UiaPattern>,
    property: UIProperty,
    pattern: UiaPattern,
) {
    if cached_bool(element, property) {
        patterns.push(pattern);
    }
}

pub(super) fn cached_bool(element: &UIElement, property: UIProperty) -> bool {
    element
        .get_cached_property_value(property)
        .ok()
        .and_then(|variant| <&Variant as TryInto<bool>>::try_into(&variant).ok())
        .unwrap_or(false)
}

fn cached_password_status(element: &UIElement) -> Option<bool> {
    element
        .get_cached_property_value(UIProperty::IsPassword)
        .ok()
        .and_then(|variant| strict_password_variant(&variant))
}

fn strict_password_variant(variant: &Variant) -> Option<bool> {
    if variant.get_type() != Variant::from(false).get_type() {
        return None;
    }
    <&Variant as TryInto<bool>>::try_into(variant).ok()
}

fn password_text_allowed(is_password: Option<bool>) -> bool {
    is_password == Some(false)
}

/// Fail-closed password decision (#32): a failed `IsPassword` read (`None`)
/// counts as "maybe password" and must never expose plaintext.
fn maybe_password(is_password: Option<bool>) -> bool {
    !password_text_allowed(is_password)
}

/// `true` when the cached `IsPassword` flag is true OR could not be read.
pub(super) fn cached_maybe_password(element: &UIElement) -> bool {
    maybe_password(cached_password_status(element))
}

/// Returns `read()` only when the password flag is explicitly false; the text
/// is not even fetched for a (maybe-)password element.
fn gated_text(is_password: Option<bool>, read: impl FnOnce() -> String) -> String {
    if password_text_allowed(is_password) {
        read()
    } else {
        String::new()
    }
}

pub(super) fn cached_public_name(element: &UIElement) -> String {
    gated_text(cached_password_status(element), || {
        element.get_cached_name().unwrap_or_default()
    })
}

/// On-screen value source-of-truth: prefer the `ValuePattern` string, then the
/// `RangeValuePattern` number. Returns `None` when neither pattern is exposed or
/// the value is empty, so `AccessibleNode.value` stays absent rather than blank.
pub(super) fn cached_value(element: &UIElement) -> Option<String> {
    if !password_text_allowed(cached_password_status(element)) {
        return None;
    }
    if cached_bool(element, UIProperty::IsValuePatternAvailable)
        && let Ok(variant) = element.get_cached_property_value(UIProperty::ValueValue)
        && let Ok(text) = variant.get_string()
        && !text.is_empty()
    {
        return Some(text);
    }
    if cached_bool(element, UIProperty::IsRangeValuePatternAvailable)
        && let Ok(variant) = element.get_cached_property_value(UIProperty::RangeValueValue)
        && let Ok(Value::R8(number)) = variant.get_value()
    {
        return Some(format_range_value(number));
    }
    None
}

/// Checked/selected/expanded state (#189) for the patterns the element exposes,
/// e.g. `toggle=1;selected=true;expand=0`. `None` when it exposes none of them.
pub(super) fn cached_control_state(element: &UIElement) -> Option<String> {
    let cached_i32 = |property| match element
        .get_cached_property_value(property)
        .ok()?
        .get_value()
    {
        Ok(Value::I4(state)) => Some(state),
        _ => None,
    };
    let mut parts = Vec::new();
    if cached_bool(element, UIProperty::IsTogglePatternAvailable)
        && let Some(state) = cached_i32(UIProperty::ToggleToggleState)
    {
        parts.push(format!("toggle={state}"));
    }
    if cached_bool(element, UIProperty::IsSelectionItemPatternAvailable) {
        let selected = cached_bool(element, UIProperty::SelectionItemIsSelected);
        parts.push(format!("selected={selected}"));
    }
    if cached_bool(element, UIProperty::IsExpandCollapsePatternAvailable)
        && let Some(state) = cached_i32(UIProperty::ExpandCollapseExpandCollapseState)
    {
        parts.push(format!("expand={state}"));
    }
    (!parts.is_empty()).then(|| parts.join(";"))
}

fn format_range_value(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.0}")
    } else {
        format!("{number}")
    }
}

pub(super) fn cached_rect(element: &UIElement) -> Rect {
    element.get_cached_bounding_rectangle().map_or(
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        |rect| Rect {
            x: rect.get_left(),
            y: rect.get_top(),
            w: rect.get_right().saturating_sub(rect.get_left()),
            h: rect.get_bottom().saturating_sub(rect.get_top()),
        },
    )
}

pub(super) fn cached_role(element: &UIElement) -> String {
    let localized = element
        .get_cached_localized_control_type()
        .unwrap_or_default();
    if !localized.is_empty() {
        return localized;
    }

    element
        .get_cached_control_type()
        .map_or_else(|_err| "unknown".to_owned(), control_type_name)
}

fn control_type_name(control_type: ControlType) -> String {
    format!("{control_type:?}")
}

pub(super) fn cached_hwnd(element: &UIElement) -> Option<i64> {
    let handle = element.get_cached_native_window_handle().ok()?;
    let raw: isize = handle.into();
    Some(raw as i64)
}

pub(super) fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub(super) const fn pattern_property(pattern: UiaPattern) -> UIProperty {
    match pattern {
        UiaPattern::Invoke => UIProperty::IsInvokePatternAvailable,
        UiaPattern::Toggle => UIProperty::IsTogglePatternAvailable,
        UiaPattern::Value => UIProperty::IsValuePatternAvailable,
        UiaPattern::Selection => UIProperty::IsSelectionPatternAvailable,
        UiaPattern::SelectionItem => UIProperty::IsSelectionItemPatternAvailable,
        UiaPattern::ExpandCollapse => UIProperty::IsExpandCollapsePatternAvailable,
        UiaPattern::LegacyIAccessible => UIProperty::IsLegacyIAccessiblePatternAvailable,
        UiaPattern::Scroll => UIProperty::IsScrollPatternAvailable,
        UiaPattern::ScrollItem => UIProperty::IsScrollItemPatternAvailable,
        UiaPattern::Text => UIProperty::IsTextPatternAvailable,
        UiaPattern::Window => UIProperty::IsWindowPatternAvailable,
        UiaPattern::Transform => UIProperty::IsTransformPatternAvailable,
        UiaPattern::RangeValue => UIProperty::IsRangeValuePatternAvailable,
    }
}

impl From<ElementSearchScope> for TreeScope {
    fn from(scope: ElementSearchScope) -> Self {
        match scope {
            ElementSearchScope::Children => Self::Children,
            ElementSearchScope::Descendants => Self::Descendants,
            ElementSearchScope::Subtree => Self::Subtree,
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum TreeView {
    Control,
    Raw,
}

pub(super) fn cached_runtime_id(element: &UIElement) -> A11yResult<Vec<i32>> {
    let value = element
        .get_cached_property_value(UIProperty::RuntimeId)
        .map_err(map_uia_error)?;
    match value.get_value().map_err(map_uia_error)? {
        Value::ArrayI4(items) => Ok(items),
        Value::SAFEARRAY(array) => array.try_into().map_err(map_uia_error),
        other => Err(A11yError::internal(format!(
            "cached RuntimeId had unexpected type {other}"
        ))),
    }
}

pub(super) fn cached_runtime_id_hex_or_fallback(
    element: &UIElement,
    hwnd: i64,
) -> A11yResult<RuntimeIdHexReadback> {
    let mut readback = match cached_runtime_id(element) {
        Ok(runtime_id) => Ok(RuntimeIdHexReadback {
            hex: runtime_id_hex(&runtime_id),
            used_fallback: false,
        }),
        Err(error) if runtime_id_is_unavailable(&error) => Ok(RuntimeIdHexReadback {
            hex: fallback_runtime_id_hex(element, hwnd),
            used_fallback: true,
        }),
        Err(error) => Err(error),
    }?;
    readback.hex = crate::native_runtime_identity::NativeRuntimeIdentity::from_cached_element(
        element,
        hwnd,
        &readback.hex,
    )?
    .runtime_id_hex();
    Ok(readback)
}

impl crate::native_runtime_identity::NativeRuntimeIdentity {
    fn from_cached_element(element: &UIElement, hwnd: i64, runtime_hex: &str) -> A11yResult<Self> {
        let name = if cached_maybe_password(element) {
            String::new()
        } else {
            element.get_cached_name().map_err(map_uia_error)?
        };
        let role = format!(
            "{:?}",
            element.get_cached_control_type().map_err(map_uia_error)?
        );
        let class_name = element.get_cached_classname().map_err(map_uia_error)?;
        let automation_id = element.get_cached_automation_id().map_err(map_uia_error)?;
        let process_id = element.get_cached_process_id().map_err(map_uia_error)?;
        Self::new(
            hwnd,
            runtime_hex,
            process_id,
            &role,
            &class_name,
            &automation_id,
            &name,
        )
        .map_err(|detail| A11yError::ElementStale {
            detail: format!("native element identity unavailable: {detail}; find again"),
        })
    }
}

pub(super) fn runtime_id_is_unavailable(error: &A11yError) -> bool {
    matches!(
        error,
        A11yError::Internal { detail } if detail.contains("cached RuntimeId had unexpected type EMPTY")
    )
}

pub(super) fn fallback_runtime_id_hex(element: &UIElement, hwnd: i64) -> String {
    let rect = cached_rect(element);
    let mut hash = FNV64_OFFSET_BASIS;
    hash_bytes(&mut hash, &hwnd.to_le_bytes());
    hash_bytes(
        &mut hash,
        &element.get_cached_process_id().unwrap_or(-1).to_le_bytes(),
    );
    hash_bytes(
        &mut hash,
        format!("{:?}", element.get_cached_control_type().ok()).as_bytes(),
    );
    hash_bytes(
        &mut hash,
        element
            .get_cached_classname()
            .unwrap_or_default()
            .as_bytes(),
    );
    hash_bytes(
        &mut hash,
        element
            .get_cached_automation_id()
            .unwrap_or_default()
            .as_bytes(),
    );
    hash_bytes(&mut hash, cached_public_name(element).as_bytes());
    hash_bytes(&mut hash, &rect.x.to_le_bytes());
    hash_bytes(&mut hash, &rect.y.to_le_bytes());
    hash_bytes(&mut hash, &rect.w.to_le_bytes());
    hash_bytes(&mut hash, &rect.h.to_le_bytes());
    format!("{FALLBACK_RUNTIME_ID_PREFIX}{hash:016x}")
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV64_PRIME);
    }
}

#[allow(clippy::missing_const_for_fn)]
#[allow(clippy::needless_pass_by_value)]
pub(super) fn map_uia_error(err: uiautomation::Error) -> A11yError {
    if uia_error_is_stale(&err) {
        return A11yError::ElementStale {
            detail: format!("UIA provider reported the element is no longer available: {err}"),
        };
    }
    A11yError::internal(err.to_string())
}

/// `UIA_E_ELEMENTNOTAVAILABLE` (also surfaced as "An event was unable to invoke
/// any of the subscribers"): the element left the tree after it was resolved.
const UIA_E_ELEMENTNOTAVAILABLE: i32 = i32::from_ne_bytes(0x8004_0201_u32.to_ne_bytes());
/// `RPC_E_DISCONNECTED`: the provider behind the element went away.
const RPC_E_DISCONNECTED: i32 = i32::from_ne_bytes(0x8001_0108_u32.to_ne_bytes());

/// The one place that decides a UIA failure means "this element is gone"
/// (stale), shared by tree reads, resolution and pattern dispatch (#124).
pub(super) fn uia_error_is_stale(err: &uiautomation::Error) -> bool {
    if matches!(err.code(), UIA_E_ELEMENTNOTAVAILABLE | RPC_E_DISCONNECTED) {
        return true;
    }
    let detail = err.to_string().to_ascii_lowercase();
    detail.contains("event was unable to invoke any of the subscribers")
        || detail.contains("element not available")
        || detail.contains("element is no longer available")
        || detail.contains("uia_e_elementnotavailable")
}

#[allow(clippy::missing_const_for_fn)]
fn _hwnd_from_i64(hwnd: i64) -> HWND {
    HWND(hwnd as *mut c_void)
}

#[cfg(test)]
#[path = "worker_admission_61.rs"]
mod worker_admission_61;

#[cfg(test)]
#[path = "worker_test_gate.rs"]
pub(super) mod worker_test_gate;

#[cfg(test)]
#[path = "worker_queue_deadline_260.rs"]
mod worker_queue_deadline_260;

#[cfg(all(test, windows))]
#[path = "password_variant_244.rs"]
mod password_variant_244;
