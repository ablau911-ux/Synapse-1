//! Frozen #268 r4 T1-T5. Extend r3's payload grid with per-shell quoting.
//! Classification never spawns; facade probes retain deny-all shell policy.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::{
    r3::{CONSOLE, GUI, fixture_env, mismatch},
    support::*,
};
use crate::server::user_active_258_support::{USER_ACTIVE, human_typed};

const CMD_WRAPPERS: &[(&str, &[&str])] = &[("cmd.exe", &["/c"]), ("cmd.exe", &["/d", "/k"])];
const PS_WRAPPERS: &[(&str, &[&str])] = &[
    ("powershell.exe", &["-c"]),
    ("pwsh.exe", &["-NoProfile", "-c"]),
    ("pwsh.exe", &["-Login", "-c"]),
    ("pwsh.exe", &["-UnknownSwitch", "-c"]),
    ("powershell.exe", &["-ExecutionPolicy", "Bypass", "-co"]),
    ("powershell.exe", &[]),
];
const CMD_POSITIONS: &[(&str, &str)] = &[
    ("direct", "{}"),
    ("ampersand", "echo safe & {}"),
    ("pipe", "echo safe | {}"),
    ("else", "if 1==2 (echo safe) else {}"),
    ("for", "for %j in (1) do {}"),
];
const PS_POSITIONS: &[(&str, &str)] = &[
    ("direct", "{}"),
    ("semicolon", "echo safe; {}"),
    ("pipe", "echo safe | {}"),
    ("if", "if ($false) { {} }"),
    ("foreach", "foreach ($i in 1) { {} }"),
];
const CMD_QUOTES: &[(&str, &str)] = &[
    ("plain", "{}"),
    ("apostrophe", "echo it's & {}"),
    ("backtick", "for /f \"usebackq\" %i in (`echo 1`) do {}"),
    ("empty-variable", "if \"%x%\"==\"\" {}"),
    ("empty-argument", "if \"%1\"==\"\" {}"),
];
const PS_QUOTES: &[(&str, &str)] = &[
    ("plain", "{}"),
    ("caret", "echo a^; {}"),
    ("assignment", "$p = {}"),
    ("return", "return {}"),
    ("double-subexpression", "Write-Output \"$({})\""),
    ("iex-literal", "iex '{}'"),
];

fn args_for(switches: &[&str], script: &str) -> Vec<String> {
    switches
        .iter()
        .map(|switch| (*switch).to_owned())
        .chain(std::iter::once(script.to_owned()))
        .collect()
}

#[test]
fn must_fail_shell_r4_generated_per_shell_quoting_grid_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    let mut rows = 0;
    for (dialect, wrappers, positions, quotes) in [
        ("cmd", CMD_WRAPPERS, CMD_POSITIONS, CMD_QUOTES),
        ("powershell", PS_WRAPPERS, PS_POSITIONS, PS_QUOTES),
    ] {
        for &(command, switches) in wrappers {
            for &(position, position_template) in positions {
                for &(quote, quote_template) in quotes {
                    for (gui, payloads) in [(true, GUI), (false, CONSOLE)] {
                        for payload in payloads {
                            let payload = if quote == "iex-literal" {
                                payload.replace('\'', "''")
                            } else {
                                (*payload).to_owned()
                            };
                            let quoted = quote_template.replacen("{}", &payload, 1);
                            let script = position_template.replacen("{}", &quoted, 1);
                            let args = args_for(switches, &script);
                            if let Some(failure) =
                                mismatch(&fx.service, work.path(), &env, command, &args, gui)
                            {
                                failures.push(format!(
                                    "{dialect}/{switches:?}/{position}/{quote}: {failure}"
                                ));
                            }
                            rows += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(rows, 4_830, "wrapper x position x quoting x r3 payloads");
    assert!(
        failures.is_empty(),
        "#268 r4 T1-T3 {rows} rows: {failures:#?}"
    );
}

#[test]
fn must_fail_shell_r4_t2_raw_cmd_if_condition_widths_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    for condition in [
        r#"if "%x%"=="""#,
        r#"if "%1"=="""#,
        r#"if /i "%x%"=="""#,
        r#"if not "%1"=="""#,
        r#"if /i not "%x%"=="""#,
        r#"if "%x%" == """#,
        r#"if "%x%"== """#,
        r#"if "%x%" =="""#,
        "if exist start",
        "if not exist start",
        "if defined start",
        "if not defined start",
        "if errorlevel 1",
        "if not errorlevel 1",
    ] {
        for (payload, gui) in [("start https://example.com", true), ("git status", false)] {
            let args = args_for(&["/c"], &format!("{condition} {payload}"));
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, "cmd.exe", &args, gui) {
                failures.push(failure);
            }
        }
    }
    for payload in ["start https://example.com", "git status"] {
        let script = format!("if \"a\"==\"\" (echo safe) else {payload}");
        let args = args_for(&["/c"], &script);
        if let Some(failure) = mismatch(
            &fx.service,
            work.path(),
            &env,
            "cmd.exe",
            &args,
            payload.starts_with("start"),
        ) {
            failures.push(failure);
        }
    }
    assert!(
        failures.is_empty(),
        "#268 r4 T2 raw conditions: {failures:#?}"
    );
}

#[test]
fn must_fail_shell_r4_t3_module_qualification_and_cmd_r_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    for &(command, switches) in PS_WRAPPERS {
        for script in [
            r"Microsoft.PowerShell.Management\Start-Process notepad -PassThru",
            r"mIcRoSoFt.PoWeRsHeLl.MaNaGeMeNt\sTaRt-PrOcEsS notepad",
            r"$p = Microsoft.PowerShell.Management\Start-Process notepad -PassThru",
        ] {
            let args = args_for(switches, script);
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, command, &args, true) {
                failures.push(failure);
            }
        }
    }
    for switch in ["/R", "/r"] {
        for (script, gui) in [("start https://example.com", true), ("git status", false)] {
            let args = args_for(&[switch], script);
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, "cmd.exe", &args, gui) {
                failures.push(failure);
            }
        }
    }
    assert!(failures.is_empty(), "#268 r4 T3: {failures:#?}");
}

#[test]
fn must_pass_shell_r4_quoted_and_escaped_data_stays_data_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let mut failures = Vec::new();
    for (command, switches, script) in [
        (
            "cmd.exe",
            &["/c"][..],
            "echo safe ^& start https://example.com",
        ),
        (
            "cmd.exe",
            &["/c"][..],
            "echo \"safe & start https://example.com\"",
        ),
        (
            "powershell.exe",
            &["-c"][..],
            "echo a`; Start-Process notepad",
        ),
        (
            "powershell.exe",
            &["-c"][..],
            "Write-Output '$(Start-Process notepad)'",
        ),
        (
            "powershell.exe",
            &["-c"][..],
            "Write-Output \"`$(Start-Process notepad)\"",
        ),
        ("powershell.exe", &["-c"][..], r"C:\proj\Start-Process"),
        ("powershell.exe", &["-c"][..], "tools/Start-Process"),
    ] {
        let args = args_for(switches, script);
        if let Some(failure) = mismatch(&fx.service, work.path(), &env, command, &args, false) {
            failures.push(failure);
        }
    }
    assert!(
        failures.is_empty(),
        "#268 r4 literal controls: {failures:#?}"
    );
}

#[test]
fn must_fail_shell_r4_t4_powershell_builtins_shadow_gui_path_268() {
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    let env = fixture_env(work.path());
    let pe = work.path().join("g268-gui-probe.exe");
    let mut failures = Vec::new();
    for builtin in [
        "write",
        "echo",
        "sleep",
        "cat",
        "type",
        "Write-Output",
        "Write-Host",
        "Start-Sleep",
        "Get-Content",
        "ForEach-Object",
    ] {
        std::fs::copy(&pe, work.path().join(format!("{builtin}.exe")))
            .expect("deterministic GUI PATH shadow");
        for &(command, switches) in PS_WRAPPERS {
            for template in ["{} 0", "1 | % { {} $_ }", "if ($false) { {} 0 }"] {
                let args = args_for(switches, &template.replacen("{}", builtin, 1));
                if let Some(failure) =
                    mismatch(&fx.service, work.path(), &env, command, &args, false)
                {
                    failures.push(failure);
                }
            }
            // Explicit .exe invocation still means the GUI PE, not a builtin.
            let args = args_for(switches, &format!("{builtin}.exe 0"));
            if let Some(failure) = mismatch(&fx.service, work.path(), &env, command, &args, true) {
                failures.push(failure);
            }
        }
    }
    assert!(failures.is_empty(), "#268 r4 T4 precedence: {failures:#?}");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r4_d6_cmd_apostrophe_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    // No allowlist: the real GUI statement cannot execute even on RED.
    let fx = setup(false, work.path());
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        assert_no_job_rows();
        let result = call(
            &fx.service,
            request(
                route,
                "cmd.exe",
                args_for(&["/c"], "echo it's & start https://example.com"),
                work.path(),
            ),
        )
        .await;
        assert_no_job_rows();
        gate_refusal(result, USER_ACTIVE, "start");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_shell_r4_d7_cmd_empty_string_if_at_facade_268() {
    let _jobs = crate::m4::ShellJobRootGuard::new();
    let work = tempfile::tempdir().expect("work");
    let fx = setup(false, work.path()); // deny-all, including unexpected RED paths
    let _human = human_typed(SESSION, 100);
    for route in ROUTES {
        for script in [
            r#"if "%x%"=="" start https://example.com"#,
            r#"if "%1"=="" start https://example.com"#,
        ] {
            assert_no_job_rows();
            let result = call(
                &fx.service,
                request(route, "cmd.exe", args_for(&["/c"], script), work.path()),
            )
            .await;
            assert_no_job_rows();
            gate_refusal(result, USER_ACTIVE, "start");
        }
    }
}
