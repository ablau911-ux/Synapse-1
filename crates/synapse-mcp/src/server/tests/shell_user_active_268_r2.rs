//! Frozen #268 r2: quoted nesting, wrapper switches, argument positions and
//! denial persistence. GUI statements are never reachable, even on RED.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::support::*;
use crate::server::{
    SynapseService,
    g4_unit_support::error_code,
    g7a_support::audit_rows,
    user_active_258_support::{USER_ACTIVE, USER_ACTIVITY_UNKNOWN, human_typed, inject},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

const D1: &[(&str, &str)] = &[
    (
        r#"powershell -c "iex 'Start-Process firefox'""#,
        "start-process",
    ),
    (
        r#"cmd /c powershell "Start-Process firefox""#,
        "start-process",
    ),
];
const D2: &[(&str, &str)] = &[
    ("powershell -NoProfile firefox.exe", "firefox"),
    (
        "powershell -ExecutionPolicy Bypass -co firefox.exe",
        "firefox",
    ),
    ("powershell -NoProfile -com firefox.exe", "firefox"),
    ("cmd /c @start firefox", "start"),
    ("cmd /c start/max firefox", "start"),
];
const D3: &[&str] = &[
    "cmd /c npm start",
    r#"powershell -c "docker start db""#,
    "cmd /c net start",
    r#"powershell -c "Get-ChildItem C:\proj\start""#,
    "git log --format=start",
];

type ClassifierCase = (&'static str, Vec<&'static str>, bool);

fn classifier_cases() -> Vec<ClassifierCase> {
    vec![
        (
            "powershell.exe",
            vec!["-c", "iex 'Start-Process firefox'"],
            true,
        ),
        (
            "cmd.exe",
            vec!["/c", r#"powershell "Start-Process firefox""#],
            true,
        ),
        ("powershell.exe", vec!["-NoProfile", "firefox.exe"], true),
        (
            "powershell.exe",
            vec!["-ExecutionPolicy", "Bypass", "-co", "firefox.exe"],
            true,
        ),
        (
            "powershell.exe",
            vec!["-NoProfile", "-com", "firefox.exe"],
            true,
        ),
        ("cmd.exe", vec!["/c", "@start firefox"], true),
        ("cmd.exe", vec!["/c", "start/max firefox"], true),
        ("cmd.exe", vec!["/c", "npm start"], false),
        ("powershell.exe", vec!["-c", "docker start db"], false),
        ("cmd.exe", vec!["/c", "net start"], false),
        (
            "powershell.exe",
            vec!["-c", r"Get-ChildItem C:\proj\start"],
            false,
        ),
        ("git", vec!["log", "--format=start"], false),
        // Cmd IF is a command wrapper; retain the original refusal contract.
        ("cmd.exe", vec!["/c", "if 1==2 start firefox"], true),
    ]
}

fn firefox_fixture(work: &Path) {
    let gui = gui_pe(work);
    std::fs::copy(gui, work.join("firefox.exe")).expect("GUI classification fixture");
}

#[test]
fn must_fail_shell_r2_classifier_command_positions_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    firefox_fixture(work.path());
    let mut mismatches = Vec::new();
    // detected_launch is private. Exercise its existing guard boundary without
    // a shell call or subprocess; keep production visibility unchanged.
    for (command, args, gui) in classifier_cases() {
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        let result = fx.service.guard_shell_gui_launch(
            command,
            &args,
            work.path().to_str(),
            &BTreeMap::new(),
            SESSION,
        );
        let matches = match &result {
            Ok(()) => !gui,
            Err(error) => gui && error_code(error).as_deref() == Some(USER_ACTIVE),
        };
        if !matches {
            mismatches.push(format!("{command} {args:?}: gui={gui}, actual={result:?}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "#268 r2 classification: {mismatches:#?}"
    );
}

/// Exact anchored policies admit only these console probes. Every nested
/// command is inside if ($false), so a broken gate cannot launch any GUI.
fn allow_probes(service: &mut SynapseService, probes: &[Value]) {
    let patterns = probes
        .iter()
        .map(|probe| {
            let command = probe["command"].as_str().expect("command");
            let args: Vec<String> = serde_json::from_value(probe["args"].clone()).expect("args");
            let line = crate::m4::shell_command_line_from_parts(command, &args);
            format!("^{}$", regex::escape(&line))
        })
        .collect();
    service.m4_config = crate::m4::M4ServiceConfig::from_cli_parts(patterns, Vec::new(), 60_000)
        .expect("exact console probe policies");
}

async fn refused_probes(cases: &[(&str, &str)]) {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    firefox_fixture(work.path());
    let mut probes = Vec::new();
    for route in ROUTES {
        for (index, (syntax, detected)) in cases.iter().enumerate() {
            let marker = work.path().join(format!("r2-{route}-{index}.pid"));
            probes.push((safe_gui_probe(route, syntax, &marker), marker, *detected));
        }
    }
    allow_probes(
        &mut fx.service,
        &probes.iter().map(|(p, _, _)| p.clone()).collect::<Vec<_>>(),
    );
    for (probe, marker, detected) in probes {
        assert!(!marker.exists());
        assert_no_job_rows();
        let result = call(&fx.service, probe).await;
        if let Ok(response) = &result {
            completed(&fx.service, response.clone()).await;
        }
        assert!(
            !marker.exists(),
            "#268 r2 child PID before refusal: {:?}",
            std::fs::read_to_string(&marker)
        );
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVE, detected);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r2_nested_quoted_commands_before_child_268() {
    refused_probes(D1).await;
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r2_switches_and_cmd_builtins_before_child_268() {
    refused_probes(D2).await;
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_shell_r2_argument_verbs_execute_console_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let mut probes = Vec::new();
    for route in ROUTES {
        for (index, syntax) in D3.iter().enumerate() {
            let marker = work.path().join(format!("argument-{route}-{index}.pid"));
            probes.push((safe_gui_probe(route, syntax, &marker), marker));
        }
    }
    allow_probes(
        &mut fx.service,
        &probes.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(),
    );
    for (probe, marker) in probes {
        assert!(!marker.exists());
        let result = call(&fx.service, probe.clone()).await;
        let response = result.unwrap_or_else(|error| {
            panic!("#268 r2 argument/path token refused console probe {probe}: {error:?}")
        });
        completed(&fx.service, response).await;
        let pid: u32 = std::fs::read_to_string(&marker)
            .expect("console PID readback")
            .trim()
            .parse()
            .expect("console PID");
        assert!(pid > 0, "console probe did not run: {probe}");
    }
}

fn assert_denial_delta(before: &[Value], after: &[Value], code: &str, tool: &str) {
    let delta: Vec<&Value> = after.iter().filter(|row| !before.contains(row)).collect();
    let denied: Vec<_> = delta
        .iter()
        .filter(|row| row["tool"] == tool && row["status"] == "denied" && row["error_code"] == code)
        .collect();
    assert_eq!(
        denied.len(),
        1,
        "one persisted #258-shaped denial per call: {delta:?}"
    );
    assert_eq!(denied[0]["session_id"], SESSION, "{denied:?}");
    assert!(
        !delta.iter().any(|row| row["status"] == "started"),
        "refusal must precede any started row: {delta:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r2_refusal_writes_denial_audit_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    for code in [USER_ACTIVE, USER_ACTIVITY_UNKNOWN] {
        let _activity = inject(SESSION, code == USER_ACTIVE, Some(100), None);
        for route in ROUTES {
            let marker = work.path().join(format!("denial-{code}-{route}.pid"));
            let probe = safe_gui_probe(route, "Start-Process firefox", &marker);
            allow_probes(&mut fx.service, std::slice::from_ref(&probe));
            assert!(!marker.exists());
            assert_no_job_rows();
            let before = audit_rows(&fx.service);
            let result = call(&fx.service, probe).await;
            if let Ok(response) = &result {
                completed(&fx.service, response.clone()).await;
            }
            assert!(!marker.exists(), "denied launch started a child");
            assert_no_job_rows();
            gate_refusal(result, code, "start-process");
            let after = audit_rows(&fx.service); // independent CF_ACTION_LOG read
            let tool = if route == "start" {
                "act_run_shell_start"
            } else {
                "act_run_shell"
            };
            assert_denial_delta(&before, &after, code, tool);
        }
    }
}
