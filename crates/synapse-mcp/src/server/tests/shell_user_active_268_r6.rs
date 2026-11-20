//! Frozen #268 r6 G8/G9. Classifier rows never spawn; refusal facade uses
//! deny-all policy. The only allowlisted executable invocation is pwsh --version.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{r3::mismatch, support::*};
use crate::server::{
    g4_unit_support::error_code,
    user_active_258_support::{USER_ACTIVE, human_typed},
};
use std::collections::BTreeMap;

const EVALUATORS: [&str; 4] = [
    "iex",
    "IEX",
    "Invoke-Expression",
    r"Microsoft.PowerShell.Utility\Invoke-Expression",
];
const WRAPPERS: [&str; 2] = ["powershell.exe", "pwsh.exe"];
const POSITIONS: [&str; 2] = ["{}", "Write-Output safe; {}"];
const COMMON_PARAMETERS: [&str; 23] = [
    "",
    "-Verbose",
    "-Debug",
    "-ErrorAction Stop",
    "-ea Stop",
    "-WarningAction Continue",
    "-wa Continue",
    "-InformationAction Continue",
    "-ia Continue",
    "-ErrorVariable errors",
    "-ev errors",
    "-WarningVariable warnings",
    "-wv warnings",
    "-InformationVariable information",
    "-iv information",
    "-OutVariable output",
    "-ov output",
    "-OutBuffer 1",
    "-ob 1",
    "-PipelineVariable pipeline",
    "-pv pipeline",
    "-Verbose -ea Stop",
    "-Debug -WarningAction Continue -ov output",
];
const BINDINGS: [&str; 4] = ["'{}'", "\"{}\"", "-Command:'{}'", "-Command: '{}' "];
const UNDETERMINABLE: [&str; 12] = [
    "",
    "-Verbose",
    "-ErrorAction",
    "-Command",
    "-Command:",
    "-UnknownSwitch 'Get-Date'",
    "$action",
    "\"$action\"",
    "$(saps firefox)",
    "@(Start-Process firefox)",
    "('saps ' + 'firefox')",
    "'saps ' + 'firefox'",
];

#[test]
fn must_fail_shell_r6_g8_generated_evaluator_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = BTreeMap::new();
    let mut failures = Vec::new();
    let mut rows = 0;
    for evaluator in EVALUATORS {
        for wrapper in WRAPPERS {
            for position in POSITIONS {
                for (payload, refused) in [("saps firefox", true), ("Get-Date", false)] {
                    let mut expressions: Vec<String> = COMMON_PARAMETERS
                        .iter()
                        .map(|parameter| format!("{evaluator} {parameter} '{payload}'"))
                        .collect();
                    expressions.extend(BINDINGS.map(|binding| {
                        format!("{evaluator} {}", binding.replacen("{}", payload, 1))
                    }));
                    expressions.extend([
                        format!("{evaluator} -Command '{payload}'"),
                        format!("{evaluator} '{payload}' -Verbose"),
                        format!("{evaluator} '{payload}' -ErrorAction Stop"),
                    ]);
                    for expression in expressions {
                        let script = position.replacen("{}", &expression, 1);
                        let args = vec!["-c".into(), script];
                        if let Some(failure) =
                            mismatch(&fx.service, work.path(), &env, wrapper, &args, refused)
                        {
                            failures.push(failure);
                        }
                        rows += 1;
                    }
                }
            }
        }
    }
    assert_eq!(
        rows, 960,
        "4 evaluators x 2 wrappers x 2 positions x 30 forms x 2 outcomes"
    );
    assert!(failures.is_empty(), "#268 r6 G8 {rows} rows: {failures:#?}");
}

#[test]
fn must_fail_shell_r6_g8_undeterminable_and_remaining_arguments_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = BTreeMap::new();
    let mut failures = Vec::new();
    let mut rows = 0;
    for evaluator in EVALUATORS {
        for wrapper in WRAPPERS {
            for position in POSITIONS {
                for argument in UNDETERMINABLE {
                    let script = position.replacen("{}", &format!("{evaluator} {argument}"), 1);
                    let result = fx.service.guard_shell_gui_launch(
                        wrapper,
                        &["-c".into(), script.clone()],
                        work.path().to_str(),
                        &env,
                        SESSION,
                    );
                    let correct = result.as_ref().err().is_some_and(|error| {
                        let text = format!("{} {:?}", error.message, error.data).to_lowercase();
                        error_code(error).as_deref() == Some(USER_ACTIVE)
                            && text.contains("could not determine whether this starts a gui")
                    });
                    if !correct {
                        failures.push(format!(
                            "undeterminable: {wrapper} {script:?} => {result:?}"
                        ));
                    }
                    rows += 1;
                }
                // G8: a parsed console literal must not hide evaluated arguments
                // later in the same segment, or later command segments.
                for suffix in [
                    "$(saps firefox)",
                    "@(Start-Process firefox)",
                    "; saps firefox",
                    "| saps firefox",
                ] {
                    let script =
                        position.replacen("{}", &format!("{evaluator} 'Get-Date' {suffix}"), 1);
                    if let Some(failure) = mismatch(
                        &fx.service,
                        work.path(),
                        &env,
                        wrapper,
                        &["-c".into(), script],
                        true,
                    ) {
                        failures.push(failure);
                    }
                    rows += 1;
                }
            }
        }
    }
    assert_eq!(
        rows, 256,
        "4 evaluators x 2 wrappers x 2 positions x 16 forms"
    );
    assert!(failures.is_empty(), "#268 r6 G8 {rows} rows: {failures:#?}");
}

#[test]
fn must_fail_shell_r6_g9_generated_info_only_wrapper_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let mut failures = Vec::new();
    let mut rows = 0;
    for (wrapper, flags) in [
        ("pwsh.exe", &["-v", "-Version", "--version"][..]),
        ("powershell.exe", &["-?", "-Help"][..]),
        ("cmd.exe", &["/?"][..]),
    ] {
        for flag in flags {
            for args in [vec![(*flag).into()], vec![flag.to_uppercase()]] {
                if let Some(failure) = mismatch(
                    &fx.service,
                    work.path(),
                    &BTreeMap::new(),
                    wrapper,
                    &args,
                    false,
                ) {
                    failures.push(failure);
                }
                rows += 1;
            }
        }
    }
    assert_eq!(rows, 12, "6 info switches x 2 casing variants");
    assert!(failures.is_empty(), "#268 r6 G9 {rows} rows: {failures:#?}");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r6_g8_iex_verbose_facade_refuses_before_spawn_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path()); // deny-all prevents real GUI even on RED
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        assert_no_job_rows();
        let result = call(
            &fx.service,
            ps_request(route, "iex -Verbose 'saps firefox'", work.path()),
        )
        .await;
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVE, "saps");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r6_g9_pwsh_version_allowed_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let mut fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let args = vec!["--version".into()];
    let line = crate::m4::shell_command_line_from_parts("pwsh.exe", &args);
    fx.service.m4_config = crate::m4::M4ServiceConfig::from_cli_parts(
        vec![format!("^{}$", regex::escape(&line))],
        Vec::new(),
        60_000,
    )
    .expect("exact anchored version-only console policy");
    for route in ROUTES {
        let response = call(
            &fx.service,
            request(route, "pwsh.exe", args.clone(), work.path()),
        )
        .await
        .expect("G9 info-only wrapper must be allowed while active");
        let finished = completed(&fx.service, response).await;
        let stdout = if route == "inline" {
            finished["run"]["stdout"]
                .as_str()
                .expect("inline stdout")
                .to_owned()
        } else {
            let path = finished["status"]["job"]["stdout_path"]
                .as_str()
                .expect("persisted stdout path");
            std::fs::read_to_string(path).expect("separate version output readback")
        };
        assert!(
            stdout.trim().starts_with("PowerShell "),
            "missing version output: {stdout:?}"
        );
    }
}
