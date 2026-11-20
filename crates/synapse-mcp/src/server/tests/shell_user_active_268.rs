//! criteria_b4.md L5-L12: real shell facade, #258 activity seam, console-only
//! effects. Preservation cases end in an active GUI probe, keeping every new
//! test RED until the missing gate exists without making preservation RED.
#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "shell_user_active_268_support.rs"]
mod support;
use crate::server::{
    g4_unit_support::error_code,
    g7a_support::audit_rows,
    user_active_258_support::{
        USER_ACTIVE, USER_ACTIVITY_UNKNOWN, call_process, human_typed, inject, notepad_launch,
    },
};
use serde_json::json;
use support::*;

#[path = "shell_user_active_268_r2.rs"]
mod r2;

#[path = "shell_user_active_268_r3.rs"]
mod r3;

#[path = "shell_user_active_268_r4.rs"]
mod r4;

#[path = "shell_user_active_268_r5.rs"]
mod r5;

#[path = "shell_user_active_268_r6.rs"]
mod r6;

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_gui_verbs_before_child_pid_or_job_row_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(true, work.path());
    let _human = human_typed(SESSION, 59_999); // one ms inside #258 quiet window
    // Every verb is in unreachable code. Today's gate omission can start only
    // the console marker shell, never Firefox, Explorer or any other GUI.
    for route in ROUTES {
        for (index, syntax, detected) in [
            (0, "Start-Process firefox", "start-process"),
            (1, "saps firefox", "saps"),
            (2, "cmd /c start firefox", "start"),
            (3, "explorer https://example.com", "explorer"),
            (4, "explorer.exe https://example.com", "explorer"),
            (5, "Invoke-Item .", "invoke-item"),
            (6, "ii .", "ii"),
            (
                7,
                "rundll32.exe url.dll,FileProtocolHandler https://example.com",
                "rundll32",
            ),
            (8, "sTaRt-PrOcEsS firefox", "start-process"),
        ] {
            let marker = work.path().join(format!("{route}-{index}.pid"));
            assert!(!marker.exists());
            assert_no_job_rows();
            let result = call(&fx.service, safe_gui_probe(route, syntax, &marker)).await;
            // Deliberately settle an unexpected success before checking SoT:
            // durable children must not escape the test's cleanup on RED.
            if let Ok(response) = &result {
                completed(&fx.service, response.clone()).await;
            }
            assert!(
                !marker.exists(),
                "#268 shell started before refusal; child PID: {:?}",
                std::fs::read_to_string(&marker)
            );
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, detected);
        }
        let marker = work.path().join(format!("cmd-{route}.started"));
        let params = request(
            route,
            "cmd.exe",
            vec![
                "/d".into(),
                "/c".into(),
                format!(
                    "echo started>\"{}\" & if 1==2 start firefox",
                    marker.display()
                ),
            ],
            work.path(),
        );
        let result = call(&fx.service, params).await;
        if let Ok(response) = &result {
            completed(&fx.service, response.clone()).await;
        }
        assert!(
            !marker.exists(),
            "cmd started before builtin start was refused"
        );
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVE, "start");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_owner_examples_use_258_refusal_shape_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path()); // no executable can ever be spawned, including on RED
    let _human = human_typed(SESSION, 100);
    // Make the owner's bare executable deterministic without depending on
    // Firefox being installed. Deny-all policy prevents this PE from running.
    let pe = gui_pe(work.path());
    std::fs::copy(pe, work.path().join("firefox.exe")).expect("Firefox classification fixture");
    for route in ROUTES {
        for (command, args, detected) in [
            (
                "powershell.exe",
                vec!["-NoProfile", "-Command", "Start-Process firefox"],
                "start-process",
            ),
            ("cmd.exe", vec!["/d", "/c", "start firefox"], "start"),
            (
                "firefox.exe",
                vec!["--new-tab", "https://example.com"],
                "firefox",
            ),
        ] {
            let mut params = request(
                route,
                command,
                args.into_iter().map(str::to_owned).collect(),
                work.path(),
            );
            let mut paths = vec![work.path().to_path_buf()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").expect("PATH"),
            ));
            params["env"] = json!({"PATH":std::env::join_paths(paths).expect("PATH join")
                .to_str().expect("UTF8 PATH")});
            let result = call(&fx.service, params).await;
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, detected);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_unknown_activity_before_any_child_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(true, work.path());
    let _unknown = inject(SESSION, false, None, None);
    for route in ROUTES {
        let marker = work.path().join(format!("unknown-{route}.pid"));
        assert!(!marker.exists());
        assert_no_job_rows();
        let result = call(
            &fx.service,
            safe_gui_probe(route, "Start-Process firefox", &marker),
        )
        .await;
        if let Ok(response) = &result {
            completed(&fx.service, response.clone()).await;
        }
        assert!(
            !marker.exists(),
            "unknown monitoring started a shell: {:?}",
            std::fs::read_to_string(&marker)
        );
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVITY_UNKNOWN, "start-process");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_resolved_gui_pe_first_token_call_and_cmd_wrappers_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path()); // classification fixtures must NEVER be executed
    let _human = human_typed(SESSION, 100);
    let gui = gui_pe(work.path());
    let gui_name = gui
        .file_name()
        .expect("filename")
        .to_string_lossy()
        .into_owned();
    let app = AppPath::new(&gui);
    for route in ROUTES {
        for (command, args, detected, path_override) in [
            (
                gui.display().to_string(),
                Vec::new(),
                gui_name.clone(),
                false,
            ),
            (gui_name.clone(), Vec::new(), gui_name.clone(), true),
            (app.name.clone(), Vec::new(), app.name.clone(), false),
            (
                "powershell.exe".into(),
                vec![
                    "-NoProfile".into(),
                    "-Command".into(),
                    format!("& '{}'", gui.display()),
                ],
                gui_name.clone(),
                false,
            ),
            (
                "cmd.exe".into(),
                vec![
                    "/d".into(),
                    "/c".into(),
                    format!("call \"{}\"", gui.display()),
                ],
                gui_name.clone(),
                false,
            ),
            (
                "cmd.exe".into(),
                vec!["/d".into(), "/c".into(), format!("\"{}\"", gui.display())],
                gui_name.clone(),
                false,
            ),
        ] {
            let mut params = request(route, &command, args, work.path());
            if path_override {
                let mut paths = vec![work.path().to_path_buf()];
                paths.extend(std::env::split_paths(
                    &std::env::var_os("PATH").expect("PATH"),
                ));
                params["env"] = json!({"PATH":std::env::join_paths(paths).expect("PATH join")
                    .to_str().expect("UTF8 PATH")});
            }
            let result = call(&fx.service, params).await;
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, &detected);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_shell_owner_approval_audited_and_scoped_to_one_call_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(true, work.path());
    for route in ROUTES {
        for known in [true, false] {
            let _activity = inject(SESSION, known, Some(100), None);
            let marker = work.path().join(format!("approved-{route}-{known}.pid"));
            assert!(!marker.exists());
            let mut params = safe_gui_probe(route, "Start-Process firefox", &marker);
            params["owner_approved"] = json!(true);
            params["reason"] = json!(REASON);
            let before = audit_rows(&fx.service).len();
            let response = call(&fx.service, params)
                .await
                .expect("approved call admitted");
            completed(&fx.service, response).await;
            assert!(
                std::fs::read_to_string(&marker)
                    .expect("console child PID")
                    .trim()
                    .parse::<u32>()
                    .expect("PID")
                    > 0
            );
            let rows = audit_rows(&fx.service);
            assert!(
                rows[before..].iter().any(|row| {
                    let text = row.to_string();
                    text.contains(REASON) && text.contains("\"owner_approved\":true")
                }),
                "#268 approval and reason absent from persistent audit: {:?}",
                &rows[before..]
            );
            let next = work.path().join(format!("unapproved-{route}-{known}.pid"));
            let result = call(
                &fx.service,
                safe_gui_probe(route, "Start-Process firefox", &next),
            )
            .await;
            if let Ok(response) = &result {
                completed(&fx.service, response.clone()).await;
            }
            assert!(!next.exists(), "approval leaked to next call");
            gate_refusal(
                result,
                if known {
                    USER_ACTIVE
                } else {
                    USER_ACTIVITY_UNKNOWN
                },
                "start-process",
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_approved_without_nonempty_reason_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(true, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for (index, reason) in [None, Some(json!("")), Some(json!(" \t\r\n "))]
            .into_iter()
            .enumerate()
        {
            let marker = work.path().join(format!("bad-reason-{route}-{index}.pid"));
            let mut params = safe_gui_probe(route, "Start-Process firefox", &marker);
            params["owner_approved"] = json!(true);
            if let Some(reason) = reason {
                params["reason"] = reason;
            }
            let result = call(&fx.service, params).await;
            if let Ok(response) = &result {
                completed(&fx.service, response.clone()).await;
            }
            assert!(!marker.exists(), "blank reason bypassed gate");
            assert_no_job_rows();
            let error = result.expect_err("nonempty reason required");
            assert_eq!(
                error_code(&error).as_deref(),
                Some(synapse_core::error_codes::TOOL_PARAMS_INVALID),
                "{error:?}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_shell_console_idle_and_hidden_routes_before_active_refusal_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(true, work.path());
    // L9: unchanged console commands really execute during physical activity.
    {
        let _human = human_typed(SESSION, 100);
        for route in ROUTES {
            for (command, args) in [
                ("rtk", vec!["git", "status"]),
                ("cargo", vec!["--version"]),
                (
                    "powershell.exe",
                    vec!["-NoProfile", "-Command", "Get-ChildItem"],
                ),
                ("python", vec!["-c", "print('g268-console-ok')"]),
            ] {
                let cwd = if command == "rtk" {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                } else {
                    work.path()
                };
                let params = request(
                    route,
                    command,
                    args.into_iter().map(str::to_owned).collect(),
                    cwd,
                );
                let response = call(&fx.service, params)
                    .await
                    .expect("console command while active");
                completed(&fx.service, response).await;
            }
        }
        // L13's documented limit: detection does not inspect script contents.
        // The script's GUI statement is unreachable on every implementation.
        let marker = work.path().join("script-file.pid");
        let script = work.path().join("console-script.ps1");
        let body = format!(
            "[IO.File]::WriteAllText('{}', [string]$PID); if ($false) {{ Start-Process firefox }}",
            marker.display().to_string().replace('\'', "''")
        );
        std::fs::write(&script, body).expect("script fixture");
        let params = request(
            "inline",
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                script.display().to_string(),
            ],
            work.path(),
        );
        completed(
            &fx.service,
            call(&fx.service, params)
                .await
                .expect("script contents are not scanned"),
        )
        .await;
        assert!(marker.exists(), "console script did not execute");
    }
    // L10: exact quiet-window boundary, older input, and no recorded input.
    for age in [Some(60_000), Some(60_001), None] {
        let _idle = inject(SESSION, true, age, None);
        for route in ROUTES {
            let marker = work.path().join(format!("idle-{route}-{age:?}.pid"));
            let response = call(
                &fx.service,
                safe_gui_probe(route, "Start-Process firefox", &marker),
            )
            .await
            .expect("idle GUI syntax admitted, console-only execution");
            completed(&fx.service, response).await;
            assert!(marker.exists(), "idle call did not execute");
        }
    }
    // L11: process hidden-desktop route remains ungated. Denied launch policy
    // guarantees it never starts Notepad or creates a real GUI window.
    {
        let _human = human_typed(SESSION, 100);
        let error = call_process(
            &fx.service,
            SESSION,
            notepad_launch(json!({"desktop":"agent:session"})),
        )
        .await
        .expect_err("isolated service has no allow-launch policy");
        let code = error_code(&error).expect("typed hidden-desktop refusal");
        assert_ne!(code, USER_ACTIVE, "hidden desktop newly gated: {error:?}");
        assert_ne!(
            code, USER_ACTIVITY_UNKNOWN,
            "hidden desktop newly gated: {error:?}"
        );
    }
    // Existing invariants above must pass; missing active gate alone makes
    // this preservation test RED today, rather than inventing a regression.
    let _human = human_typed(SESSION, 100);
    let marker = work.path().join("active-after-preservation.pid");
    let result = call(
        &fx.service,
        safe_gui_probe("inline", "Start-Process firefox", &marker),
    )
    .await;
    if let Ok(response) = &result {
        completed(&fx.service, response.clone()).await;
    }
    assert!(
        !marker.exists(),
        "active GUI syntax ran after preservation checks"
    );
    gate_refusal(result, USER_ACTIVE, "start-process");
}
