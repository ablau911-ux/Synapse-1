//! FROZEN behaviour table of Gitea #2360 (roadmap item #633), rows S1-S12.
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/readonly_commands_table_2360.rs
//! (registered by the `#[cfg(all(test, windows))] #[path]` line at the end of
//! shell_profile_policy.rs).
//!
//! The Windows system folder is a fixture (`C:\Fixture\System32`), so no row
//! depends on this PC. The list file is given as the outcome of reading it:
//! "no file" is a `NotFound` read (the default list), a file is its text.
//! "Allowed" = `program` returns the fixture folder's `<name>.exe`;
//! "refused" = `None`. S11 passes on a stub that allows everything, so it has
//! a control case (a normal agent making the same calls is refused).

use std::io;
use std::path::Path;

use super::ReadOnlyCommands;

const SYSTEM: &str = r"C:\Fixture\System32";

fn no_file() -> io::Result<String> {
    Err(io::Error::from(io::ErrorKind::NotFound))
}

fn file(text: &str) -> io::Result<String> {
    Ok(text.to_owned())
}

fn commands(read: io::Result<String>) -> ReadOnlyCommands {
    ReadOnlyCommands::from_file(read, Path::new(SYSTEM))
}

fn words(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// One `program` call and the program it must give (`None` = refused).
struct Call {
    command: &'static str,
    args: &'static [&'static str],
    env: bool,
    working_dir: bool,
    program: Option<&'static str>,
}

const fn call(
    command: &'static str,
    args: &'static [&'static str],
    program: Option<&'static str>,
) -> Call {
    Call {
        command,
        args,
        env: false,
        working_dir: false,
        program,
    }
}

fn check_calls(row: &str, list: &ReadOnlyCommands, calls: &[Call], failures: &mut Vec<String>) {
    for case in calls {
        let got = list.program(case.command, &words(case.args), case.env, case.working_dir);
        let want = case.program.map(|program| Path::new(SYSTEM).join(program));
        if got != want {
            failures.push(format!(
                "{row}: {} {:?} env={} working_dir={}: expected {want:?}, got {got:?}",
                case.command, case.args, case.env, case.working_dir
            ));
        }
    }
}

/// One profile-gate call: `shell` (`launch: false`) or `process launch`.
struct Gate {
    profile: &'static str,
    launch: bool,
    command: &'static str,
    args: &'static [&'static str],
    allowed: bool,
}

fn check_gates(row: &str, list: &ReadOnlyCommands, gates: &[Gate], failures: &mut Vec<String>) {
    for gate in gates {
        let args = words(gate.args);
        let got = if gate.launch {
            list.allows_process_launch(gate.profile, gate.command, &args, false, false)
        } else {
            list.allows_shell_command(gate.profile, gate.command, &args, false, false)
        };
        if got != gate.allowed {
            let tool = if gate.launch {
                "process launch"
            } else {
                "shell"
            };
            failures.push(format!(
                "{row}: {tool} profile={} {} {:?}: expected {}, got {got}",
                gate.profile, gate.command, gate.args, gate.allowed
            ));
        }
    }
}

fn check_problems(row: &str, list: &ReadOnlyCommands, named: &[&str], failures: &mut Vec<String>) {
    let problems = list.problems();
    let matches = problems.len() == named.len()
        && problems
            .iter()
            .zip(named)
            .all(|(problem, name)| problem.contains(name));
    if !matches {
        failures.push(format!(
            "{row}: expected problems naming {named:?}, got {problems:?}"
        ));
    }
}

#[test]
fn readonly_commands_table_2360() -> Result<(), String> {
    let mut failures = Vec::new();
    let default_list = commands(no_file());
    let full_path = r"C:\Fixture\System32\hostname.exe";
    let hostname = Some("hostname.exe");

    check_calls(
        "S1_plain_name_no_arguments",
        &default_list,
        &[call("hostname", &[], hostname)],
        &mut failures,
    );
    check_gates(
        "S1_plain_name_no_arguments",
        &default_list,
        &[Gate {
            profile: "normal_agent",
            launch: false,
            command: "hostname",
            args: &[],
            allowed: true,
        }],
        &mut failures,
    );
    check_calls(
        "S2_exe_suffix_upper_case_full_system_path",
        &default_list,
        &[
            call("hostname.exe", &[], hostname),
            call("HOSTNAME", &[], hostname),
            call(full_path, &[], hostname),
        ],
        &mut failures,
    );
    check_calls(
        "S3_other_folders",
        &default_list,
        &[
            call(r"C:\Temp\hostname.exe", &[], None),
            call(r".\hostname.exe", &[], None),
            call(r"..\System32\hostname.exe", &[], None),
        ],
        &mut failures,
    );
    check_calls(
        "S4_any_argument",
        &default_list,
        &[
            call("hostname", &["-s"], None),
            call("hostname", &["&", "whoami"], None),
        ],
        &mut failures,
    );
    let whoami_all = commands(file(r#"[["whoami", "/all"]]"#));
    check_calls(
        "S5_entry_with_an_argument",
        &whoami_all,
        &[
            call("whoami", &["/all"], Some("whoami.exe")),
            call("whoami", &["/ALL"], Some("whoami.exe")),
            call("whoami", &["/priv"], None),
            call("whoami", &["/all", "/fo", "csv"], None),
        ],
        &mut failures,
    );
    check_calls(
        "S6_environment_or_working_folder",
        &default_list,
        &[
            Call {
                env: true,
                ..call("hostname", &[], None)
            },
            Call {
                working_dir: true,
                ..call("hostname", &[], None)
            },
        ],
        &mut failures,
    );
    check_calls(
        "S7_interpreters",
        &default_list,
        &[
            call("powershell", &["-Command", "hostname"], None),
            call("cmd", &["/c", "hostname"], None),
        ],
        &mut failures,
    );
    let mixed = commands(file(
        r#"[["cmd", "/c", "dir"], ["C:\\Tools\\x.exe"], [], ["ipconfig"]]"#,
    ));
    check_problems(
        "S8_bad_entries_rejected_by_name",
        &mixed,
        &["\"cmd\"", "x.exe", "[]"],
        &mut failures,
    );
    check_calls(
        "S8_good_entry_kept",
        &mixed,
        &[call("ipconfig", &[], Some("ipconfig.exe"))],
        &mut failures,
    );
    let empty = commands(file("[]"));
    check_calls(
        "S9_empty_list",
        &empty,
        &[call("hostname", &[], None), call("whoami", &[], None)],
        &mut failures,
    );
    let not_a_list = commands(file(r#"{"hostname": true}"#));
    check_calls(
        "S10_file_not_a_list",
        &not_a_list,
        &[call("hostname", &[], None)],
        &mut failures,
    );
    check_problems(
        "S10_file_not_a_list",
        &not_a_list,
        &["not a list"],
        &mut failures,
    );
    let powershell: &[&str] = &["-Command", "Get-Process"];
    let cmd: &[&str] = &["/c", "dir"];
    check_gates(
        "S11_trusted_profiles",
        &default_list,
        &[
            Gate {
                profile: "break_glass",
                launch: false,
                command: "powershell",
                args: powershell,
                allowed: true,
            },
            Gate {
                profile: "full_capability",
                launch: false,
                command: "powershell",
                args: powershell,
                allowed: true,
            },
            Gate {
                profile: "break_glass",
                launch: true,
                command: "cmd.exe",
                args: cmd,
                allowed: true,
            },
            Gate {
                profile: "full_capability",
                launch: true,
                command: "cmd.exe",
                args: cmd,
                allowed: true,
            },
        ],
        &mut failures,
    );
    check_gates(
        "S11_control_normal_agent",
        &default_list,
        &[
            Gate {
                profile: "normal_agent",
                launch: false,
                command: "powershell",
                args: powershell,
                allowed: false,
            },
            Gate {
                profile: "normal_agent",
                launch: true,
                command: "cmd.exe",
                args: cmd,
                allowed: false,
            },
        ],
        &mut failures,
    );
    check_gates(
        "S12_browser_profiles",
        &default_list,
        &[
            Gate {
                profile: "browser_control",
                launch: false,
                command: "hostname",
                args: &[],
                allowed: false,
            },
            Gate {
                profile: "browser_debugger",
                launch: false,
                command: "hostname",
                args: &[],
                allowed: false,
            },
        ],
        &mut failures,
    );

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
