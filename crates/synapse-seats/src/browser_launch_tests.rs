//! #73: a seat's private profile directory must never outlive a failed or
//! cancelled launch, nor a backend dropped without an explicit close.
use super::*;
use std::error::Error;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn scratch_root() -> PathBuf {
    std::env::temp_dir().join(format!("synapse-seat-launch-{}", uuid::Uuid::new_v4()))
}

async fn dir_entries(root: &std::path::Path) -> TestResult<Vec<String>> {
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

async fn wait_until_empty(root: &std::path::Path) -> TestResult<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let names = dir_entries(root).await?;
        if names.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(names);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn must_fail_spawn_failure_leaves_no_private_profile() -> TestResult {
    let root = scratch_root();
    tokio::fs::create_dir_all(&root).await?;
    // Exists (passes the configured-executable check) but cannot be spawned.
    let not_executable = root.join("not-an-executable.txt");
    tokio::fs::write(&not_executable, b"not a program").await?;
    let runtime_root = root.join("runtime");
    tokio::fs::create_dir_all(&runtime_root).await?;
    let config = BrowserRuntimeConfig {
        chromium_executable: not_executable,
        runtime_root: runtime_root.clone(),
        startup_timeout_ms: 1_000,
    };
    let result =
        BrowserBackend::launch(SeatId::default(), Viewport::default(), None, &config).await;
    assert!(result.is_err(), "spawning a text file must fail");
    let leftovers = dir_entries(&runtime_root).await?;
    tokio::fs::remove_dir_all(&root).await?;
    assert!(leftovers.is_empty(), "leaked profile dirs: {leftovers:?}");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn must_fail_cancelled_startup_removes_private_profile() -> TestResult {
    let root = scratch_root();
    let runtime_root = root.join("runtime");
    tokio::fs::create_dir_all(&runtime_root).await?;
    // Spawns fine but never publishes DevToolsActivePort, so launch is parked
    // in wait_for_cdp when the deadline below cancels it.
    let config = BrowserRuntimeConfig {
        chromium_executable: PathBuf::from(std::env::var("SystemRoot")?)
            .join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
        runtime_root: runtime_root.clone(),
        startup_timeout_ms: 60_000,
    };
    let launch = BrowserBackend::launch(SeatId::default(), Viewport::default(), None, &config);
    let outcome = tokio::time::timeout(Duration::from_millis(750), launch).await;
    assert!(
        outcome.is_err(),
        "launch must still be starting when cancelled"
    );
    let leftovers = wait_until_empty(&runtime_root).await?;
    tokio::fs::remove_dir_all(&root).await?;
    assert!(leftovers.is_empty(), "leaked profile dirs: {leftovers:?}");
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn must_fail_backend_dropped_without_close_removes_profile() -> TestResult {
    let (backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    drop(backend);
    let leftovers = wait_until_empty(&root).await?;
    tokio::fs::remove_dir_all(&root).await?;
    assert!(leftovers.is_empty(), "leaked profile dirs: {leftovers:?}");
    Ok(())
}

#[tokio::test]
async fn must_pass_guard_cleanup_removes_profile_and_keeps_runtime_root() -> TestResult {
    let runtime_root = scratch_root();
    let profile_root = runtime_root.join("browser-seat-cleanup");
    tokio::fs::create_dir_all(profile_root.join("profile")).await?;
    let mut guard = ProfileGuard::new(runtime_root.clone(), profile_root.clone());
    guard.cleanup().await?;
    drop(guard);
    let profile_exists = tokio::fs::try_exists(&profile_root).await?;
    let runtime_exists = tokio::fs::try_exists(&runtime_root).await?;
    tokio::fs::remove_dir_all(&runtime_root).await?;
    assert!(!profile_exists, "cleanup must remove the seat profile");
    assert!(runtime_exists, "cleanup must never remove the runtime root");
    Ok(())
}

#[tokio::test]
async fn must_pass_disarmed_guard_hands_the_live_profile_to_its_owner() -> TestResult {
    let runtime_root = scratch_root();
    let profile_root = runtime_root.join("browser-seat-live");
    tokio::fs::create_dir_all(profile_root.join("profile")).await?;
    let mut guard = ProfileGuard::new(runtime_root.clone(), profile_root.clone());
    guard.disarm();
    drop(guard);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let leftovers = dir_entries(&runtime_root).await?;
    tokio::fs::remove_dir_all(&runtime_root).await?;
    assert_eq!(
        leftovers,
        vec!["browser-seat-live".to_owned()],
        "a disarmed guard must leave the live backend's profile in place"
    );
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn must_pass_explicit_close_removes_profile_and_marks_torn_down() -> TestResult {
    let (mut backend, _peer, root) = cleanup_tests::unresponsive_backend().await?;
    backend.close().await?;
    let torn_down = backend.torn_down;
    drop(backend);
    let leftovers = dir_entries(&root).await?;
    tokio::fs::remove_dir_all(&root).await?;
    assert!(
        torn_down,
        "a completed close must mark the backend torn down"
    );
    assert!(leftovers.is_empty(), "leaked profile dirs: {leftovers:?}");
    Ok(())
}
