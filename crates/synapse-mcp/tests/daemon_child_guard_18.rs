//! #18: every daemon child that a test spawns must be killed on EVERY exit
//! path (early `?`, `bail!`, failed assertion, panic), not only when the test
//! reaches its explicit cleanup block.
//!
//! The older `cli_modes_kill_on_drop_18.rs` scanner only reads a hand-written
//! file list and skips `std::process` children, so `lifecycle_windows.rs`
//! (five unguarded `std::process::Child` daemons) escaped it. This guard
//! discovers every test source at run time, so a new test file is covered
//! without editing a list.
//!
//! Accepted guards for a daemon spawn (`Command::new(<daemon binary>)`):
//! * tokio builder with `.kill_on_drop(true)`, or
//! * a std child moved straight into a `*Guard` type (`XGuard::new(Command::new(..)...spawn()..)`
//!   or `...spawn().map(XGuard::new)`), in a file that has an `impl Drop for` guard.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

/// Text that marks a `Command::new(..)` builder as spawning the synapse daemon.
const DAEMON_MARKERS: &[&str] = &["CARGO_BIN_EXE_synapse-mcp", "bin()", "mcp_binary_path()"];

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every integration-test source of this crate plus the shared stdio client.
fn spawning_test_sources() -> Vec<(String, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_rs(&manifest.join("tests"), &mut files).expect("read tests dir");
    collect_rs(&manifest.join("../synapse-test-utils/src"), &mut files)
        .expect("read synapse-test-utils src");
    files
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path).expect("read test source");
            (path.display().to_string().replace('\\', "/"), source)
        })
        .collect()
}

/// Lines of daemon `.spawn()` calls with no kill-on-drop guard.
fn unguarded_daemon_spawns(source: &str) -> Vec<usize> {
    let has_drop_guard = source.contains("impl Drop for");
    source
        .match_indices(".spawn()")
        .filter_map(|(offset, _)| {
            let before = &source[..offset];
            let builder_start = before.rfind("Command::new(")?;
            let builder = &before[builder_start..];
            if !DAEMON_MARKERS.iter().any(|marker| builder.contains(marker)) {
                return None;
            }
            let line_start = before[..builder_start].rfind('\n').map_or(0, |i| i + 1);
            let prefix = &before[line_start..builder_start];
            let after = &source[offset..];
            let suffix = &after[..after.find(';').unwrap_or(after.len())];
            let kill_on_drop = builder.contains(".kill_on_drop(true)");
            let drop_guard =
                has_drop_guard && (prefix.contains("Guard") || suffix.contains("Guard"));
            (!kill_on_drop && !drop_guard).then(|| before.matches('\n').count() + 1)
        })
        .collect()
}

#[test]
fn must_pass_every_daemon_spawn_in_every_test_source_is_guarded() {
    let mut offenders = Vec::new();
    for (file, source) in spawning_test_sources() {
        if file.ends_with("daemon_child_guard_18.rs") {
            continue; // this file only holds synthetic samples
        }
        for line in unguarded_daemon_spawns(&source) {
            offenders.push(format!("{file}:{line}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "daemon child spawned without a kill-on-drop guard (leaks on early exit) at {offenders:?}"
    );
}

#[test]
fn must_pass_discovery_covers_std_process_and_shared_harness_files() {
    let files: Vec<String> = spawning_test_sources()
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    for expected in [
        "tests/lifecycle_windows.rs",
        "tests/daemon_session_window_144.rs",
        "tests/cli_modes.rs",
        "tests/support/mod.rs",
        "synapse-test-utils/src/stdio_mcp_client.rs",
    ] {
        assert!(
            files.iter().any(|file| file.ends_with(expected)),
            "{expected} must be scanned; discovered {files:?}"
        );
    }
}

#[test]
fn must_pass_guarded_daemon_spawns_are_accepted() {
    let tokio_guarded = "let a = Command::new(bin())\n    .kill_on_drop(true)\n    .spawn()?;\n";
    assert!(unguarded_daemon_spawns(tokio_guarded).is_empty());

    let wrapped = "impl Drop for DaemonGuard { fn drop(&mut self) {} }\n\
                   let a = DaemonGuard::new(Command::new(bin())\n    .arg(\"x\")\n    .spawn()?);\n";
    assert!(unguarded_daemon_spawns(wrapped).is_empty());

    let mapped = "impl Drop for ChildGuard { fn drop(&mut self) {} }\n\
                  let a = Command::new(bin())\n    .spawn()\n    .map(ChildGuard::new)?;\n";
    assert!(unguarded_daemon_spawns(mapped).is_empty());

    // Not the daemon: a helper process is out of this guard's scope.
    let helper = "let a = Command::new(\"powershell.exe\")\n    .spawn()?;\n";
    assert!(unguarded_daemon_spawns(helper).is_empty());
}

#[test]
fn must_fail_unguarded_std_and_tokio_daemon_spawns_are_flagged() {
    let std_child = "let mut first = Command::new(bin())\n    .args([\"--mode\"])\n    .spawn()\n    .context(\"spawn\")?;\n";
    assert_eq!(unguarded_daemon_spawns(std_child), vec![3]);

    let disabled =
        "let a = Command::new(env!(\"CARGO_BIN_EXE_synapse-mcp\")).kill_on_drop(false).spawn()?;\n";
    assert_eq!(unguarded_daemon_spawns(disabled), vec![1]);

    // A guard name without any Drop impl in the file is not a guard.
    let fake_guard = "let a = FakeGuard::new(Command::new(bin())\n    .spawn()?);\n";
    assert_eq!(unguarded_daemon_spawns(fake_guard), vec![2]);

    // A guarded earlier builder must not cover a later, unguarded one.
    let borrowed = "let a = Command::new(bin()).kill_on_drop(true).spawn()?;\n\
                    let b = Command::new(bin())\n    .spawn()?;\n";
    assert_eq!(unguarded_daemon_spawns(borrowed), vec![3]);
}
