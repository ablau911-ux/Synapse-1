//! Regression tests for #176 driving `run_worker_with_paths`,
//! `hidden_desktop_window_capture`, `hidden_desktop_window_context`,
//! `hidden_desktop_window_snapshot`, and `hidden_desktop_window_hwnds`
//! against a real Win32 hidden desktop (`CreateDesktopW`).

#![cfg(windows)]

use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, ensure};
use synapse_core::error_codes;
use uuid::Uuid;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW,
    DESKTOP_ENUMERATE, DESKTOP_HOOKCONTROL, DESKTOP_READ_CONTROL, DESKTOP_READOBJECTS,
    DESKTOP_WRITEOBJECTS, HDESK, SetThreadDesktop,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetDesktopWindow, WS_EX_NOACTIVATE, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::{
    DesktopWorkerOp, WorkerTempPaths, hidden_desktop_window_capture, hidden_desktop_window_context,
    hidden_desktop_window_hwnds, hidden_desktop_window_snapshot, routing, run_worker_with_paths,
    wide_null,
};

struct TestHiddenDesktop {
    name: String,
    handle: HDESK,
}

impl TestHiddenDesktop {
    fn create(tag: &str) -> anyhow::Result<Self> {
        let name = format!("SynapseTest176_{tag}_{}", Uuid::new_v4().simple());
        let name_wide = wide_null(&name);
        let access = DESKTOP_CREATEMENU.0
            | DESKTOP_CREATEWINDOW.0
            | DESKTOP_ENUMERATE.0
            | DESKTOP_HOOKCONTROL.0
            | DESKTOP_READOBJECTS.0
            | DESKTOP_READ_CONTROL.0
            | DESKTOP_WRITEOBJECTS.0;
        let handle = unsafe {
            CreateDesktopW(
                PCWSTR(name_wide.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS::default(),
                access,
                None,
            )
        }
        .context("CreateDesktopW for #176 test")?;
        Ok(Self { name, handle })
    }
}

impl Drop for TestHiddenDesktop {
    fn drop(&mut self) {
        let _ = unsafe { CloseDesktop(self.handle) };
    }
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_fail_worker_dispatch_refuses_foreign_desktop_hwnd_before_spawning_worker()
-> anyhow::Result<()> {
    let desktop = TestHiddenDesktop::create("foreign")?;
    let foreign_hwnd = unsafe { GetDesktopWindow() }.0 as isize as i64;
    let temp = WorkerTempPaths::new(true)
        .map_err(|error| anyhow::anyhow!("WorkerTempPaths: {}", error.message))?;

    let err = run_worker_with_paths(
        &desktop.name,
        DesktopWorkerOp::Capture,
        foreign_hwnd,
        None,
        false,
        None,
        &temp,
    )
    .err()
    .context("foreign hwnd must be refused by run_worker_with_paths")?;
    ensure!(
        error_code(&err) == Some(error_codes::TARGET_WINDOW_NOT_FOUND)
            && err.message.contains("is not a window of that desktop"),
        "expected TARGET_WINDOW_NOT_FOUND desktop membership refusal, got: {}",
        err.message
    );
    let bgra_path = temp.bgra_path.as_ref().context("bgra_path allocated")?;
    ensure!(
        !temp.json_path.exists() && !bgra_path.exists(),
        "#176: worker process must not be spawned when hwnd is not on the hidden desktop"
    );

    let ctx_err = hidden_desktop_window_context(&desktop.name, foreign_hwnd)
        .err()
        .context("context must refuse foreign hwnd")?;
    let snap_err = hidden_desktop_window_snapshot(&desktop.name, foreign_hwnd, 2)
        .err()
        .context("snapshot must refuse foreign hwnd")?;
    let cap_err = hidden_desktop_window_capture(&desktop.name, foreign_hwnd, None, false)
        .err()
        .context("capture must refuse foreign hwnd")?;
    for call_err in [&ctx_err, &snap_err, &cap_err] {
        ensure!(
            error_code(call_err) == Some(error_codes::TARGET_WINDOW_NOT_FOUND),
            "expected TARGET_WINDOW_NOT_FOUND, got: {}",
            call_err.message
        );
    }
    Ok(())
}

#[test]
fn must_fail_worker_dispatch_fails_closed_when_desktop_cannot_be_opened() -> anyhow::Result<()> {
    let foreign_hwnd = unsafe { GetDesktopWindow() }.0 as isize as i64;
    let temp = WorkerTempPaths::new(false)
        .map_err(|error| anyhow::anyhow!("WorkerTempPaths: {}", error.message))?;
    let missing_name = format!("SynapseMissing176_{}", Uuid::new_v4().simple());

    let err = run_worker_with_paths(
        &missing_name,
        DesktopWorkerOp::Context,
        foreign_hwnd,
        None,
        false,
        None,
        &temp,
    )
    .err()
    .context("missing desktop must fail closed")?;
    ensure!(
        error_code(&err) == Some(error_codes::TARGET_WINDOW_NOT_FOUND)
            && err.message.contains("window list unreadable")
            && !temp.json_path.exists(),
        "missing desktop must fail closed before worker spawn: {}",
        err.message
    );
    Ok(())
}

#[test]
fn must_pass_real_desktop_hwnd_enumeration_accepts_window_created_on_hidden_desktop()
-> anyhow::Result<()> {
    let desktop = TestHiddenDesktop::create("owned")?;
    let raw_hdesk = desktop.handle.0 as isize;
    let (hwnd_tx, hwnd_rx) = mpsc::channel::<anyhow::Result<i64>>();
    let (done_tx, done_rx) = mpsc::channel::<()>();

    let join = std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<windows::Win32::Foundation::HWND> {
            let hdesk = HDESK(raw_hdesk as *mut core::ffi::c_void);
            unsafe { SetThreadDesktop(hdesk) }.context("SetThreadDesktop")?;
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("Static"),
                    w!("synapse-176-hidden-window"),
                    WS_POPUP,
                    10,
                    10,
                    120,
                    80,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .context("CreateWindowExW on hidden desktop")?;
            Ok(hwnd)
        })();
        match result {
            Ok(hwnd) => {
                let _ = hwnd_tx.send(Ok(hwnd.0 as isize as i64));
                let _ = done_rx.recv_timeout(Duration::from_secs(5));
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            Err(err) => {
                let _ = hwnd_tx.send(Err(err));
            }
        }
    });

    let hidden_hwnd = hwnd_rx
        .recv_timeout(Duration::from_secs(5))
        .context("receive hidden hwnd")??;
    let listed = hidden_desktop_window_hwnds(&desktop.name)
        .map_err(|error| anyhow::anyhow!("hidden_desktop_window_hwnds: {}", error.message))?;
    let check =
        routing::ensure_hwnd_on_desktop(&desktop.name, hidden_hwnd, hidden_desktop_window_hwnds);
    let _ = done_tx.send(());
    let _ = join.join();

    ensure!(
        listed.contains(&hidden_hwnd),
        "hidden_desktop_window_hwnds must include window created on that desktop: {listed:?}"
    );
    check.map_err(|error| anyhow::anyhow!("ensure_hwnd_on_desktop failed: {}", error.message))?;
    Ok(())
}
