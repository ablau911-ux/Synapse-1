//! #123/#177 frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row: which Chrome command lines
//! make a debugger attach banner-free.

use super::debugger_banner_free;

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[test]
fn chrome_bridge_debugger_banner_table_123() {
    const EXE: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
    let rows: [(&str, Vec<String>, bool); 8] = [
        (
            "1j_agent_chrome_with_silent_switch_is_banner_free",
            args(&[
                EXE,
                r"--user-data-dir=C:\agent\chrome-ud",
                "--silent-debugger-extension-api",
            ]),
            true,
        ),
        (
            "1j_quoted_switch_is_banner_free",
            args(&[EXE, "\"--silent-debugger-extension-api\""]),
            true,
        ),
        (
            "1j_switch_with_value_is_banner_free",
            args(&[EXE, "--silent-debugger-extension-api=1"]),
            true,
        ),
        (
            "1j_upper_case_switch_is_banner_free",
            args(&[EXE, "--SILENT-DEBUGGER-EXTENSION-API"]),
            true,
        ),
        (
            "1j_someone_else_owner_chrome_without_switch_shows_banner",
            args(&[EXE, "--profile-directory=Default"]),
            false,
        ),
        ("1j_empty_command_line_fails_closed", Vec::new(), false),
        (
            "1j_hostile_lookalike_switch_is_not_banner_free",
            args(&[EXE, "--silent-debugger-extension-api-x"]),
            false,
        ),
        (
            "1j_hostile_switch_inside_another_value_is_not_banner_free",
            args(&[EXE, "--app=https://x.test/?--silent-debugger-extension-api"]),
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (row, command_line, expected) in rows {
        let got = debugger_banner_free(&command_line);
        if got != expected {
            failures.push(format!("row {row}: banner_free {got}, expected {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#123 debugger banner table rows failed:\n{}",
        failures.join("\n")
    );
}
