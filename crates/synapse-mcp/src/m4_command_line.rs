//! Windows command-line planning shared by the shell spawn paths and the audit /
//! authorization renderer (#24, #38).
//!
//! Rust's `Command` encodes every regular argument with the CRT convention (`"` becomes
//! `\"`). cmd.exe does not understand that convention, so an explicit `cmd.exe /c <payload>`
//! needs its own command-tail serializer. Spawning and rendering both consume the plan from
//! [`plan_spawn_args`], so the authorized/audited line is the line the OS receives.

/// One argument as it is appended to the child's command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SpawnArg {
    /// Encoded by `Command::arg` (CRT rules, see `quote_command_part`).
    Regular(String),
    /// Appended verbatim by `Command::raw_arg`.
    Raw(String),
}

/// Plan the arguments for `command`. Only an explicit `cmd`/`cmd.exe` whose command tail
/// (everything after the first `/c`, `/k` or `/r` switch) contains a quote gets a raw tail;
/// every other program keeps the CRT encoding.
pub(super) fn plan_spawn_args(command: &str, args: &[String]) -> Vec<SpawnArg> {
    if let Some(planned) = plan_cmd_tail(command, args) {
        return planned;
    }
    args.iter().cloned().map(SpawnArg::Regular).collect()
}

fn plan_cmd_tail(command: &str, args: &[String]) -> Option<Vec<SpawnArg>> {
    if !is_cmd_program(command) {
        return None;
    }
    let switch = args.iter().position(|arg| is_cmd_run_switch(arg))?;
    let tail = &args[switch + 1..];
    if !tail.iter().any(|arg| arg.contains('"')) {
        return None;
    }
    // cmd strips the first and last quote of the line after /c, so the verbatim tail is
    // wrapped in one extra pair: `/c "echo "a b" | find "a""` runs `echo "a b" | find "a"`.
    let body = tail
        .iter()
        .map(|arg| {
            if arg.contains('"') {
                arg.clone()
            } else {
                super::quote_command_part(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut planned: Vec<SpawnArg> = args[..=switch]
        .iter()
        .cloned()
        .map(SpawnArg::Regular)
        .collect();
    planned.push(SpawnArg::Raw(format!("\"{body}\"")));
    Some(planned)
}

fn is_cmd_program(command: &str) -> bool {
    let leaf = super::executable_leaf(command);
    leaf.eq_ignore_ascii_case("cmd") || leaf.eq_ignore_ascii_case("cmd.exe")
}

fn is_cmd_run_switch(arg: &str) -> bool {
    ["/c", "/k", "/r"]
        .iter()
        .any(|switch| arg.eq_ignore_ascii_case(switch))
}

/// Render the arguments (without the program) the way the OS command line carries them,
/// each preceded by a space.
pub(super) fn render_args(command: &str, args: &[String]) -> Vec<String> {
    plan_spawn_args(command, args)
        .into_iter()
        .map(|arg| match arg {
            SpawnArg::Regular(arg) => super::quote_command_part(&arg),
            SpawnArg::Raw(arg) => arg,
        })
        .collect()
}

/// Append the planned arguments to `command`.
pub(super) fn apply_args(command: &mut tokio::process::Command, program: &str, args: &[String]) {
    for arg in plan_spawn_args(program, args) {
        match arg {
            SpawnArg::Regular(arg) => {
                command.arg(arg);
            }
            #[cfg(windows)]
            SpawnArg::Raw(arg) => {
                command.raw_arg(arg);
            }
            #[cfg(not(windows))]
            SpawnArg::Raw(arg) => {
                command.arg(arg);
            }
        }
    }
}
