//! Frozen #268 r5 G7/D8/D9/D10. Classifier never spawns. Denial facade
//! probes use deny-all policy; allow probes execute only console PID markers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{
    r3::{CONSOLE, GUI, fixture_env, mismatch},
    support::*,
};
use crate::server::{
    SynapseService,
    user_active_258_support::{USER_ACTIVE, human_typed},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

type Invocation = (&'static str, Vec<String>);
const PREFIXES: [&str; 5] = ["-", "/", "\u{2013}", "\u{2014}", "\u{2015}"];
const PS_POSITIONS: [&str; 5] = [
    "{}",
    "echo safe; {}",
    "echo safe | {}",
    "if ($false) { {} }",
    "foreach ($i in 1) { {} }",
];
const CMD_POSITIONS: [&str; 5] = [
    "{}",
    "echo safe & {}",
    "echo safe | {}",
    "if 1==2 (echo safe) else {}",
    "for %i in (1) do {}",
];
const VALUES: [&str; 7] = [
    "$p = 'notepad'; Get-Process $p",
    "$action = 'start'",
    "$env:BROWSER = \"firefox\"",
    "@{ n = 'explorer' }",
    "Write-Output ('notepad')",
    "'g268-gui-probe.exe'",
    "$p = 'g268-gui-probe.exe'",
];

fn ps_wrappers(script: &str) -> Vec<Invocation> {
    let mut rows = Vec::new();
    for command in ["powershell.exe", "pwsh.exe"] {
        for prefix in PREFIXES {
            for switches in [
                vec![format!("{prefix}c")],
                vec!["-NoProfile".into(), format!("{prefix}Command")],
                vec![
                    format!("{prefix}ExecutionPolicy"),
                    "Bypass".into(),
                    format!("{prefix}co"),
                ],
            ] {
                let mut args = switches;
                args.push(script.into());
                rows.push((command, args));
            }
        }
    }
    rows
}

fn cmd_wrappers(script: &str) -> Vec<Invocation> {
    let mut rows = Vec::new();
    for switch in ["/c", "/k", "/r"] {
        rows.push(("cmd.exe", vec![format!("{switch}{script}")]));
        rows.push(("cmd.exe", vec![format!("{switch}\"{script}\"")]));
        rows.push((
            "powershell.exe",
            vec!["-c".into(), format!("cmd {switch}{script}")],
        ));
    }
    rows
}

fn collect_rows(
    service: &SynapseService,
    work: &Path,
    env: &BTreeMap<String, String>,
    invocations: Vec<Invocation>,
    gui: bool,
    failures: &mut Vec<String>,
) -> usize {
    let rows = invocations.len();
    for (command, args) in invocations {
        if let Some(failure) = mismatch(service, work, env, command, &args, gui) {
            failures.push(failure);
        }
    }
    rows
}

fn trap_env(work: &Path) -> BTreeMap<String, String> {
    let env = fixture_env(work);
    // D10 must distinguish quoted data from real GUI PEs, independently of
    // which applications happen to be installed on the host.
    for name in ["notepad", "firefox", "explorer", "start"] {
        std::fs::copy(
            work.join("g268-gui-probe.exe"),
            work.join(format!("{name}.exe")),
        )
        .expect("literal GUI trap (never executed)");
    }
    env
}

#[test]
fn must_fail_shell_r5_generated_switch_and_literal_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = trap_env(work.path());
    let mut failures = Vec::new();
    let mut rows = 0;
    for (positions, wrappers) in [
        (PS_POSITIONS, ps_wrappers as fn(&str) -> Vec<Invocation>),
        (CMD_POSITIONS, cmd_wrappers as fn(&str) -> Vec<Invocation>),
    ] {
        for position in positions {
            for (gui, payloads) in [(true, GUI), (false, CONSOLE)] {
                for payload in payloads {
                    let script = position.replacen("{}", payload, 1);
                    rows += collect_rows(
                        &fx.service,
                        work.path(),
                        &env,
                        wrappers(&script),
                        gui,
                        &mut failures,
                    );
                }
            }
        }
    }
    for position in PS_POSITIONS {
        for value in VALUES {
            let script = position.replacen("{}", value, 1);
            rows += collect_rows(
                &fx.service,
                work.path(),
                &env,
                ps_wrappers(&script),
                false,
                &mut failures,
            );
        }
    }
    assert_eq!(
        rows, 5_145,
        "39 wrappers x 5 positions x 21 payloads + 30 PS wrappers x 5 positions x 7 values"
    );
    assert!(
        failures.is_empty(),
        "#268 r5 D8/D9/D10 {rows} rows: {failures:#?}"
    );
}

fn indeterminate_wrappers() -> Vec<Invocation> {
    vec![
        ("cmd.exe", vec!["/d".into()]),
        ("cmd.exe", vec!["/c".into()]),
        (
            "powershell.exe",
            vec!["-NoProfile".into(), "-Command".into()],
        ),
        ("pwsh.exe", vec!["-ExecutionPolicy".into()]),
        ("pwsh.exe", vec!["-c".into(), "'git status".into()]),
        (
            "powershell.exe",
            vec!["-c".into(), "pwsh -NoProfile -c".into()],
        ),
    ]
}

fn assert_indeterminate(result: Result<Value, rmcp::ErrorData>) {
    let error = result
        .as_ref()
        .expect_err("G7 indeterminate wrapper must refuse");
    let text = format!("{} {:?}", error.message, error.data).to_lowercase();
    assert!(
        text.contains("could not determine whether this starts a gui"),
        "G7 diagnostic must explain uncertainty: {error:?}"
    );
    gate_refusal(result, USER_ACTIVE, "gui");
}

#[test]
fn must_fail_shell_r5_g7_unparseable_wrappers_fail_closed_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for (command, args) in indeterminate_wrappers() {
        assert_indeterminate(
            fx.service
                .guard_shell_gui_launch(
                    command,
                    &args,
                    work.path().to_str(),
                    &BTreeMap::new(),
                    SESSION,
                )
                .map(|()| Value::Null),
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r5_g7_indeterminate_facade_refuses_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for (command, args) in indeterminate_wrappers() {
            assert_no_job_rows();
            let result = call(&fx.service, request(route, command, args, work.path())).await;
            assert_no_job_rows();
            assert_indeterminate(result);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r5_d8_switch_prefix_facade_refuses_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for (command, args) in ps_wrappers("Start-Process firefox") {
            assert_no_job_rows();
            let result = call(&fx.service, request(route, command, args, work.path())).await;
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, "start-process");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r5_d9_glued_cmd_facade_refuses_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for (command, args) in cmd_wrappers("start https://example.com") {
            assert_no_job_rows();
            let result = call(&fx.service, request(route, command, args, work.path())).await;
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, "start");
        }
    }
}

fn allow_exact(service: &mut SynapseService, probe: &Value) {
    let command = probe["command"].as_str().expect("command");
    let args: Vec<String> = serde_json::from_value(probe["args"].clone()).expect("args");
    let line = crate::m4::shell_command_line_from_parts(command, &args);
    service.m4_config = crate::m4::M4ServiceConfig::from_cli_parts(
        vec![format!("^{}$", regex::escape(&line))],
        Vec::new(),
        60_000,
    )
    .expect("exact anchored console-only policy");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r5_d10_literal_values_allowed_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = trap_env(work.path());
    for route in ROUTES {
        for (index, value) in VALUES.into_iter().enumerate() {
            let marker = work.path().join(format!("r5-d10-{route}-{index}.pid"));
            // Literal is inside a false branch. Even on RED, only the marker
            // console can execute; no GUI fixture or Get-Process runs.
            let mut probe = safe_gui_probe(route, value, &marker);
            probe["env"] = serde_json::to_value(&env).expect("fixture env");
            allow_exact(&mut fx.service, &probe);
            assert!(!marker.exists());
            let response = call(&fx.service, probe)
                .await
                .expect("D10 data must be allowed");
            completed(&fx.service, response).await;
            let pid: u32 = std::fs::read_to_string(&marker)
                .expect("console marker readback")
                .trim()
                .parse()
                .expect("console child PID");
            assert!(pid > 0, "allowed value probe never executed");
        }
    }
}
