//! #18: every daemon child an integration test spawns with tokio's `Command`
//! must be killed when its handle is dropped, so an early assertion or `?`
//! error cannot leak a running daemon.
//!
//! Scope: every tests/ file that spawns `CARGO_BIN_EXE_synapse-mcp` through
//! `tokio::process::Command`. lifecycle_windows.rs uses `std::process::Command`
//! (no `kill_on_drop`), so it is out of this scanner's reach.

/// `(file, source)` of every scanned test file.
const SPAWNING_SOURCES: &[(&str, &str)] = &[
    ("cli_modes.rs", include_str!("cli_modes.rs")),
    ("cli_allow_shell.rs", include_str!("cli_allow_shell.rs")),
    (
        "canonical_daemon_lifecycle.rs",
        include_str!("canonical_daemon_lifecycle.rs"),
    ),
    (
        "chrome_bridge_origin_auth.rs",
        include_str!("chrome_bridge_origin_auth.rs"),
    ),
    (
        "http_liveness_reserve_isolation_19.rs",
        include_str!("http_liveness_reserve_isolation_19.rs"),
    ),
    (
        "http_slow_header_liveness.rs",
        include_str!("http_slow_header_liveness.rs"),
    ),
    (
        "local_agent_profile_trust.rs",
        include_str!("local_agent_profile_trust.rs"),
    ),
    (
        "session_registry_unknown_ids.rs",
        include_str!("session_registry_unknown_ids.rs"),
    ),
    (
        "task_approval_atomicity.rs",
        include_str!("task_approval_atomicity.rs"),
    ),
    ("support/mod.rs", include_str!("support/mod.rs")),
];

/// Lines of `.spawn()` calls whose own `Command::new(` builder lacks `.kill_on_drop(true)`.
fn spawns_without_kill_on_drop(source: &str) -> Vec<usize> {
    source
        .match_indices(".spawn()")
        .filter_map(|(offset, _)| {
            let before = &source[..offset];
            let builder_start = before.rfind("Command::new(").unwrap_or(0);
            (!before[builder_start..].contains(".kill_on_drop(true)"))
                .then(|| before.matches('\n').count() + 1)
        })
        .collect()
}

#[test]
fn must_pass_every_spawned_daemon_child_has_kill_on_drop() {
    let mut offenders = Vec::new();
    for (file, source) in SPAWNING_SOURCES {
        assert!(
            source.contains("CARGO_BIN_EXE_synapse-mcp"),
            "{file} no longer spawns the daemon; drop it from the scan list"
        );
        for line in spawns_without_kill_on_drop(source) {
            offenders.push(format!("{file}:{line}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "daemon child spawned without kill_on_drop(true) at {offenders:?}"
    );
}

#[test]
fn must_fail_spawn_without_kill_on_drop_is_flagged() {
    let leaking = "let a = Command::new(\"x\")\n    .arg(\"y\")\n    .spawn()?;\n";
    assert_eq!(spawns_without_kill_on_drop(leaking), vec![3]);

    let disabled = "let a = Command::new(\"x\").kill_on_drop(false).spawn()?;\n";
    assert_eq!(spawns_without_kill_on_drop(disabled), vec![1]);

    // A kill_on_drop on an earlier builder must not cover a later, unguarded one.
    let borrowed = "let a = Command::new(\"x\").kill_on_drop(true).spawn()?;\n\
                    let b = Command::new(\"y\")\n    .spawn()?;\n";
    assert_eq!(spawns_without_kill_on_drop(borrowed), vec![3]);
}
