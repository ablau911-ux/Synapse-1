//! #139: closing a browser seat whose CDP socket is stalled must still tear down the
//! exact owned child and its private profile, inside a bound, and even when the caller's
//! cleanup deadline cancels the close future.
//!
//! Uses `cleanup_tests::unresponsive_backend`: a real hidden `powershell Start-Sleep`
//! child standing in for Chromium and a real WebSocket peer that never answers. Every
//! test reads the OS process table and the profile directory back separately.

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const CLOSE_BOUND: Duration = Duration::from_secs(5);
const POLL_DEADLINE: Duration = Duration::from_secs(10);

fn process_listed(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    output
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

async fn wait_gone(pid: u32) -> bool {
    let started = Instant::now();
    while started.elapsed() < POLL_DEADLINE {
        if !process_listed(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn wait_removed(path: &Path) -> bool {
    let started = Instant::now();
    while started.elapsed() < POLL_DEADLINE {
        if !tokio::fs::try_exists(path).await.unwrap_or(true) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn owned_pid(backend: &BrowserBackend) -> u32 {
    backend
        .identity
        .process_id
        .unwrap_or_else(|| panic!("fixture child must have a pid"))
}

#[tokio::test]
async fn must_pass_close_with_a_silent_cdp_peer_tears_down_within_a_bound_139() -> TestResult {
    let (mut backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    let pid = owned_pid(&backend);
    let profile = backend.profile_root.clone();
    assert!(process_listed(pid), "precondition: owned child alive");
    assert!(
        tokio::fs::try_exists(&profile).await?,
        "precondition: profile exists"
    );

    let outcome = tokio::time::timeout(CLOSE_BOUND, backend.close()).await;

    assert!(
        matches!(outcome, Ok(Ok(()))),
        "close must finish without CDP: {outcome:?}"
    );
    assert!(backend.torn_down);
    assert!(wait_gone(pid).await, "owned child {pid} survived close");
    assert!(
        !tokio::fs::try_exists(&profile).await?,
        "profile must be removed"
    );
    assert!(
        tokio::fs::try_exists(&root).await?,
        "runtime root must stay"
    );
    drop(backend);
    tokio::fs::remove_dir_all(&root).await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_close_cancelled_by_an_expired_deadline_still_kills_and_cleans_139() -> TestResult
{
    let (mut backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    let pid = owned_pid(&backend);
    let profile = backend.profile_root.clone();

    // An already-expired outer deadline: the close future is polled once, then dropped.
    let _ = tokio::time::timeout(Duration::ZERO, backend.close()).await;

    assert!(
        wait_gone(pid).await,
        "kill must be issued before the first await"
    );
    drop(backend);
    assert!(
        wait_removed(&profile).await,
        "drop after a cancelled close must remove the profile"
    );
    tokio::fs::remove_dir_all(&root).await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_close_after_the_child_already_exited_still_removes_profile_139() -> TestResult {
    let (mut backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    let pid = owned_pid(&backend);
    let profile = backend.profile_root.clone();
    backend.process.start_kill()?;
    let _ = backend.process.wait().await?;
    assert!(wait_gone(pid).await, "precondition: child already exited");

    let outcome = tokio::time::timeout(CLOSE_BOUND, backend.close()).await;

    assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
    assert!(!tokio::fs::try_exists(&profile).await?);
    drop(backend);
    tokio::fs::remove_dir_all(&root).await?;
    Ok(())
}

#[tokio::test]
async fn must_fail_locked_profile_keeps_the_failure_visible_then_drop_retries_139() -> TestResult {
    use std::os::windows::fs::OpenOptionsExt as _;
    let (mut backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    let pid = owned_pid(&backend);
    let profile = backend.profile_root.clone();
    // share_mode(0): no other handle may open or delete the file (os error 32).
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(profile.join("owned-marker"))?;

    let outcome = tokio::time::timeout(CLOSE_BOUND, backend.close()).await;

    match outcome {
        Ok(Err(SeatError::Backend(message))) => {
            assert!(message.contains("remove browser-seat profile"), "{message}");
        }
        other => panic!("a locked profile must surface a Backend error: {other:?}"),
    }
    assert!(!backend.torn_down, "a failed close must not claim teardown");
    assert!(
        wait_gone(pid).await,
        "the child must die even when profile removal fails"
    );
    drop(lock);
    drop(backend);
    assert!(
        wait_removed(&profile).await,
        "drop must retry the profile removal"
    );
    tokio::fs::remove_dir_all(&root).await?;
    Ok(())
}
