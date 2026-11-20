//! Frozen #268 r3 G1-G5. Classifier grid never spawns; facade GUI branches
//! are unreachable even when classification fails. No production visibility changes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::support::*;
use crate::server::{
    SynapseService,
    g4_unit_support::error_code,
    user_active_258_support::{USER_ACTIVE, human_typed},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

const WRAPPERS: &[(&str, &[&str])] = &[
    ("bare", &[]),
    ("cmd.exe", &["/c"]),
    ("powershell.exe", &["-c"]),
    ("pwsh.exe", &["-NoProfile", "-c"]),
    ("pwsh.exe", &["-Login", "-c"]),
    ("pwsh.exe", &["-UnknownSwitch", "-c"]),
    ("powershell.exe", &["-ExecutionPolicy", "Bypass", "-co"]),
    ("nested", &[]),
];

// Cartesian product, not a list of preselected failing examples.
const POSITIONS: &[(&str, &str)] = &[
    ("direct", "{}"),
    ("semicolon", "echo safe; {}"),
    ("ampersand", "echo safe & {}"),
    ("and", "echo safe && {}"),
    ("or", "echo safe || {}"),
    ("pipe", "echo safe | {}"),
    ("ps-if", "if ($false) { {} }"),
    ("cmd-else", "if 1==2 (echo safe) else {}"),
    ("cmd-for", "for %i in (1) do {}"),
    ("ps-foreach", "foreach ($i in 1) { {} }"),
    ("iex", "iex '{}'"),
    ("literal-call", "& '{}'"),
];

pub(super) const GUI: &[&str] = &[
    "Start-Process firefox",
    "saps firefox",
    "start firefox",
    "start/max firefox",
    "@start firefox",
    "explorer https://example.com",
    "explorer.exe https://example.com",
    "Invoke-Item .",
    "ii .",
    "rundll32.exe url.dll,FileProtocolHandler https://example.com",
    "rundll32.exe shell32.dll,ShellExec_RunDLL https://example.com",
    "g268-gui-probe.exe",
];

pub(super) const CONSOLE: &[&str] = &[
    "git status",
    "git commit -m 'write tests'",
    "npm start",
    "docker start db",
    "net start",
    "Start-Sleep 0",
    "Start-Job",
    r"Get-ChildItem C:\proj\start",
    "Get-Content start/run.sh",
];

pub(super) fn fixture_env(work: &Path) -> BTreeMap<String, String> {
    let pe = gui_pe(work);
    // G2 must fail deterministically when an ordinary quoted argument is
    // re-parsed: "write tests" then wrongly resolves this GUI executable.
    std::fs::copy(pe, work.join("write.exe")).expect("quoted argument GUI trap");
    let mut paths = vec![work.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").expect("PATH"),
    ));
    BTreeMap::from([
        (
            "PATH".into(),
            std::env::join_paths(paths)
                .expect("fixture PATH")
                .to_str()
                .expect("UTF8 PATH")
                .into(),
        ),
        ("PATHEXT".into(), ".EXE".into()),
    ])
}

fn quote_script(script: &str) -> String {
    format!("\"{}\"", script.replace('"', "`\""))
}

fn entry(wrapper: (&str, &[&str]), script: &str, direct: bool) -> (String, Vec<String>) {
    let (command, switches) = wrapper;
    if command == "bare" && direct {
        let mut tokens = script.split_whitespace();
        return (
            tokens.next().expect("nonempty command").into(),
            tokens.map(str::to_owned).collect(),
        );
    }
    if command == "nested" {
        return (
            "cmd.exe".into(),
            vec![
                "/c".into(),
                format!("powershell -c {}", quote_script(script)),
            ],
        );
    }
    // A compound bare line needs an evaluator at the public guard boundary.
    // No *additional* wrapper surrounds its payload; direct bare executables
    // above exercise the non-shell path too.
    let (command, switches) = if command == "bare" {
        ("powershell.exe", &["-c"][..])
    } else {
        (command, switches)
    };
    let mut args: Vec<String> = switches.iter().map(|s| (*s).into()).collect();
    args.push(script.into());
    (command.into(), args)
}

pub(super) fn mismatch(
    service: &SynapseService,
    work: &Path,
    env: &BTreeMap<String, String>,
    command: &str,
    args: &[String],
    gui: bool,
) -> Option<String> {
    // detected_launch/script_launch are private. Same narrow pub(crate)
    // entry as r2, with real activity gating and PE resolution, no subprocess.
    let result = service.guard_shell_gui_launch(command, args, work.to_str(), env, SESSION);
    let correct = match &result {
        Ok(()) => !gui,
        Err(error) => gui && error_code(error).as_deref() == Some(USER_ACTIVE),
    };
    (!correct).then(|| format!("gui={gui}: {command} {args:?} => {result:?}"))
}

#[test]
fn must_fail_shell_r3_generated_classifier_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    let mut rows = 0;
    for &wrapper in WRAPPERS {
        for &(position, template) in POSITIONS {
            for (gui, payloads) in [(true, GUI), (false, CONSOLE)] {
                for payload in payloads {
                    // Preserve literals when embedding a script literal; only
                    // evaluator payloads are eligible for recursive parsing.
                    let payload = if matches!(position, "iex" | "literal-call") {
                        payload.replace('\'', "''")
                    } else {
                        (*payload).into()
                    };
                    let script = template.replacen("{}", &payload, 1);
                    let (command, args) = entry(wrapper, &script, position == "direct");
                    if let Some(failure) =
                        mismatch(&fx.service, work.path(), &env, &command, &args, gui)
                    {
                        failures.push(format!("{wrapper:?}/{position}: {failure}"));
                    }
                    rows += 1;
                }
            }
        }
    }
    assert_eq!(
        rows, 2_016,
        "8 wrappers x 12 positions x (12 GUI + 9 console)"
    );
    assert!(failures.is_empty(), "#268 r3 {rows} rows: {failures:#?}");
}

#[test]
fn must_fail_shell_r3_remaining_grammar_boundaries_and_paths_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    // G1: closing delimiters, newline, condition widths, all PS keywords,
    // dot-source and call. Generate both outcomes at every boundary.
    for template in [
        "(echo safe) {}",
        "{ echo safe } {}",
        "echo safe\n{}",
        "if not 1==2 {}",
        "if exist missing {}",
        "if defined MISSING {}",
        "if errorlevel 1 {}",
        "call {}",
        ". '{}'",
        "if ($false) { {} }",
        "elseif ($false) { {} }",
        "else { {} }",
        "foreach ($i in @()) { {} }",
        "for (;;) { {} }",
        "while ($false) { {} }",
        "do { {} } until ($true)",
        "until ($true) { {} }",
        "switch (1) { 1 { {} } }",
        "try { {} }",
        "catch { {} }",
        "finally { {} }",
        "trap { {} }",
    ] {
        for (payload, gui) in [("Start-Process firefox", true), ("npm start", false)] {
            let script = template.replacen("{}", payload, 1);
            let (command, args) = entry(("powershell.exe", &["-c"]), &script, false);
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, &command, &args, gui) {
                failures.push(failure);
            }
        }
    }
    // G4: a path's last segment never grants it launch-verb semantics.
    for script in [
        r"C:\proj\start",
        "start/run.sh",
        "tools/Start-Process",
        r"C:\proj\ii",
    ] {
        let args = vec!["-c".into(), script.into()];
        if let Some(failure) = mismatch(
            &fx.service,
            work.path(),
            &env,
            "powershell.exe",
            &args,
            false,
        ) {
            failures.push(failure);
        }
    }
    assert!(failures.is_empty(), "#268 r3 G1/G4: {failures:#?}");
}

#[test]
fn must_fail_shell_r3_switch_widths_and_depth_fail_closed_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    for switches in [
        vec!["-i"],
        vec!["-i", "-c"],
        vec!["-UnknownSwitch"],
        vec!["-UnknownSwitch", "-c"],
        vec!["-ExecutionPolicy:Bypass", "-c"],
        vec!["-Unknown:value", "-c"],
        vec!["-NoProfile", "-co"],
    ] {
        for (payload, gui) in [("Start-Process firefox", true), ("git status", false)] {
            let mut args: Vec<String> = switches.iter().map(|s| (*s).into()).collect();
            args.push(payload.into());
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, "pwsh.exe", &args, gui)
            {
                failures.push(failure);
            }
        }
    }
    // G3 stop forms remain the documented limitation, even if following text
    // looks like GUI syntax. No files/scripts are executed by this test.
    for flag in ["-File", "-f", "-EncodedCommand", "-e", "-ec"] {
        let args = vec![flag.into(), "Start-Process firefox".into()];
        if let Some(failure) = mismatch(&fx.service, work.path(), &env, "pwsh.exe", &args, false) {
            failures.push(failure);
        }
    }
    // Console leaf is deliberate: merely discovering a GUI verb at arbitrary
    // depth does not prove bounded, fail-closed recursion. Deep uncertainty
    // itself must refuse while active, with a clear depth/limit explanation.
    for depth in [16, 32] {
        let script = format!("{}git status", "powershell -c ".repeat(depth));
        let result = fx.service.guard_shell_gui_launch(
            "powershell.exe",
            &["-c".into(), script],
            work.path().to_str(),
            &env,
            SESSION,
        );
        match result {
            Err(error) => {
                let text = format!("{} {:?}", error.message, error.data).to_lowercase();
                if error_code(&error).as_deref() != Some(USER_ACTIVE)
                    || !(text.contains("depth") || text.contains("recursion"))
                {
                    failures.push(format!("depth={depth}: unclear refusal {error:?}"));
                }
            }
            Ok(()) => failures.push(format!("depth={depth}: unbounded evaluator accepted")),
        }
    }
    assert!(failures.is_empty(), "#268 r3 G3: {failures:#?}");
}

fn allow_exact(service: &mut SynapseService, probes: &[Value]) {
    let patterns = probes
        .iter()
        .map(|probe| {
            let command = probe["command"].as_str().expect("command");
            let args: Vec<String> = serde_json::from_value(probe["args"].clone()).expect("args");
            format!(
                "^{}$",
                regex::escape(&crate::m4::shell_command_line_from_parts(command, &args))
            )
        })
        .collect();
    service.m4_config = crate::m4::M4ServiceConfig::from_cli_parts(patterns, Vec::new(), 60_000)
        .expect("exact anchored console policy");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r3_d4_cmd_for_do_and_else_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for (index, syntax) in [
            "if 1==2 for %i in (1) do start firefox",
            "if 1==1 (echo safe>nul) else start firefox",
        ]
        .into_iter()
        .enumerate()
        {
            let marker = work.path().join(format!("r3-d4-{route}-{index}.pid"));
            let script = format!("echo started>\"{}\" & {syntax}", marker.display());
            let probe = request(
                route,
                "cmd.exe",
                vec!["/d".into(), "/c".into(), script],
                work.path(),
            );
            allow_exact(&mut fx.service, std::slice::from_ref(&probe));
            assert!(!marker.exists());
            assert_no_job_rows();
            let result = call(&fx.service, probe).await;
            if let Ok(response) = &result {
                completed(&fx.service, response.clone()).await;
            }
            assert!(
                !marker.exists(),
                "D4 shell started before refusal: {syntax}"
            );
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, "start");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r3_d5_unknown_pwsh_switch_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path()); // deny-all: no GUI spawn even on RED
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        assert_no_job_rows();
        let result = call(
            &fx.service,
            request(
                route,
                "pwsh.exe",
                vec![
                    "-UnknownSwitch".into(),
                    "-c".into(),
                    "Start-Process firefox".into(),
                ],
                work.path(),
            ),
        )
        .await;
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVE, "start-process");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r3_g2_quoted_commit_message_allowed_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    assert_no_job_rows();
    for route in ROUTES {
        let marker = work.path().join(format!("r3-g2-{route}.pid"));
        // Only the PID marker executes; no git command or GUI fixture can run.
        let mut probe = safe_gui_probe(route, "git commit -m 'write tests'", &marker);
        probe["env"] = serde_json::to_value(&env).expect("fixture env");
        allow_exact(&mut fx.service, std::slice::from_ref(&probe));
        assert!(!marker.exists());
        let response = call(&fx.service, probe)
            .await
            .expect("G2 quoted argument must be allowed");
        completed(&fx.service, response).await;
        let pid: u32 = std::fs::read_to_string(&marker)
            .expect("console PID readback")
            .trim()
            .parse()
            .expect("console PID");
        assert!(pid > 0, "allowed console probe did not execute");
    }
}
