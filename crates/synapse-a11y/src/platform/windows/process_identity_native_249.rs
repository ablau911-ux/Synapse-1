//! #249: native reader, not the injected launch selector.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::launch_process_record;
use std::{
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child() -> ChildGuard {
    ChildGuard(
        Command::new("cmd.exe")
            .args(["/d", "/q", "/c", "set /p synapse249="])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("own child"),
    )
}

fn filetime_now() -> u64 {
    // FILETIME epoch: 1601-01-01, units: 100 ns. Independent of production reader.
    116_444_736_000_000_000
        + u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
                / 100,
        )
        .expect("FILETIME fits")
}

#[test]
fn must_pass_native_child_parent_and_creation_time_match_spawn_interval() {
    let own_pid = std::process::id();
    let (_, parent_created) = launch_process_record(own_pid).expect("current process readable");
    let before = filetime_now();
    let mut child = child();
    assert_eq!(child.0.try_wait().expect("child status"), None);
    let identity = launch_process_record(child.0.id()).expect("native child identity");
    let after = filetime_now();
    assert_eq!(
        identity.0, own_pid,
        "ToolHelp parent must be actual spawning test process"
    );
    assert!(parent_created <= identity.1);
    assert!(
        (before..=after).contains(&identity.1),
        "creation FILETIME must match this spawn"
    );
    assert_eq!(
        launch_process_record(child.0.id()),
        Some(identity),
        "stable generation readback"
    );
    assert_eq!(
        child.0.try_wait().expect("independent status readback"),
        None
    );
}

#[test]
fn must_fail_unknown_and_reaped_process_have_no_native_identity() {
    assert_eq!(launch_process_record(0), None);
    assert_eq!(launch_process_record(u32::MAX), None);
    let mut child = child();
    let pid = child.0.id();
    assert_eq!(
        launch_process_record(pid).expect("live identity").0,
        std::process::id()
    );
    child.0.kill().expect("kill own child");
    child.0.wait().expect("reap own child");
    assert!(child.0.try_wait().expect("exit readback").is_some());
    assert_eq!(
        launch_process_record(pid),
        None,
        "reaped child must not remain attributable"
    );
}
