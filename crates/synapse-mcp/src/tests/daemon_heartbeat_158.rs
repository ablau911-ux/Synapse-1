//! #158: diagnostic heartbeat. Pure classifier + heartbeat file write/read.
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::daemon_heartbeat::{
    ClassifyInput, HEARTBEAT_FILE, Heartbeat, Suspected, assess, classify, heartbeat_path,
    read_heartbeat, resolve_interval, run_loop, write_heartbeat,
};

const INTERVAL_MS: u64 = 30_000;

fn input(last_beat: Option<u64>, activity: u64, boot: Option<u64>) -> ClassifyInput {
    ClassifyInput {
        last_beat_unix_ms: last_beat,
        interval_ms: INTERVAL_MS,
        last_activity_unix_ms: activity,
        boot_unix_ms: boot,
    }
}

fn beat(run_id: &str, at: u64) -> Heartbeat {
    Heartbeat {
        schema_version: 1,
        run_id: run_id.to_owned(),
        pid: 4242,
        interval_ms: INTERVAL_MS,
        beat_unix_ms: at,
        seq: 7,
    }
}

#[test]
fn must_pass_interval_default_valid_and_disable_158() {
    let default = Some(Duration::from_secs(30));
    assert_eq!(resolve_interval(None), (default, None));
    assert_eq!(resolve_interval(Some("  ")), (default, None));
    assert_eq!(
        resolve_interval(Some(" 5 ")),
        (Some(Duration::from_secs(5)), None)
    );
    assert_eq!(resolve_interval(Some("0")), (None, None));
}

#[test]
fn must_fail_invalid_interval_falls_back_and_reports_158() {
    for bad in ["abc", "-1", "1.5"] {
        let (interval, rejected) = resolve_interval(Some(bad));
        assert_eq!(interval, Some(Duration::from_secs(30)), "{bad}");
        assert_eq!(rejected.as_deref(), Some(bad), "{bad}");
    }
}

#[test]
fn must_pass_classifier_hang_when_heartbeat_went_stale_before_last_activity_158() {
    let beat_at = 1_000_000;
    // Ledger activity 3 intervals + 1ms after the last beat: stale, hang.
    let activity = beat_at + 3 * INTERVAL_MS + 1;
    assert_eq!(
        classify(&input(Some(beat_at), activity, None)),
        Suspected::Hang
    );
}

#[test]
fn must_fail_classifier_boundary_exactly_three_intervals_is_not_hang_158() {
    let beat_at = 1_000_000;
    let activity = beat_at + 3 * INTERVAL_MS;
    assert_eq!(
        classify(&input(Some(beat_at), activity, None)),
        Suspected::Kill
    );
}

#[test]
fn must_pass_classifier_fresh_heartbeat_is_kill_or_power_by_boot_time_158() {
    let beat_at = 1_000_000;
    // Heartbeat newer than the last tool activity: fresh at crash time.
    assert_eq!(
        classify(&input(Some(beat_at), beat_at - 5_000, None)),
        Suspected::Kill
    );
    // Machine booted before the last beat: process was killed, not power loss.
    assert_eq!(
        classify(&input(Some(beat_at), beat_at, Some(beat_at - 60_000))),
        Suspected::Kill
    );
    // Machine booted after the last beat: power/OS reset.
    assert_eq!(
        classify(&input(Some(beat_at), beat_at, Some(beat_at + 60_000))),
        Suspected::Power
    );
}

#[test]
fn must_fail_classifier_unknown_without_heartbeat_or_interval_158() {
    assert_eq!(classify(&input(None, 5, None)), Suspected::Unknown);
    let mut zero = input(Some(1), 1, None);
    zero.interval_ms = 0;
    assert_eq!(classify(&zero), Suspected::Unknown);
}

#[test]
fn must_pass_suspected_labels_158() {
    assert_eq!(Suspected::Hang.as_str(), "hang");
    assert_eq!(Suspected::Kill.as_str(), "kill");
    assert_eq!(Suspected::Power.as_str(), "power");
    assert_eq!(Suspected::Unknown.as_str(), "unknown");
}

#[test]
fn must_pass_heartbeat_file_roundtrips_and_is_overwritten_not_appended_158() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    assert!(
        read_heartbeat(dir.path())?.is_none(),
        "missing file reads as None"
    );
    write_heartbeat(dir.path(), &beat("run-a", 100))?;
    assert_eq!(read_heartbeat(dir.path())?, Some(beat("run-a", 100)));
    let first_len = std::fs::metadata(heartbeat_path(dir.path()))?.len();
    for at in 101..150 {
        write_heartbeat(dir.path(), &beat("run-a", at))?;
    }
    assert_eq!(
        read_heartbeat(dir.path())?.map(|b| b.beat_unix_ms),
        Some(149)
    );
    let last_len = std::fs::metadata(heartbeat_path(dir.path()))?.len();
    assert!(
        last_len <= first_len + 2,
        "file must not grow: {first_len} -> {last_len}"
    );
    assert_eq!(
        heartbeat_path(dir.path())
            .file_name()
            .and_then(|n| n.to_str()),
        Some(HEARTBEAT_FILE)
    );
    Ok(())
}

#[test]
fn must_fail_corrupt_heartbeat_file_is_an_error_not_a_panic_158() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(heartbeat_path(dir.path()), b"{not json")?;
    let Err(error) = read_heartbeat(dir.path()) else {
        panic!("a corrupt heartbeat file must be a read error");
    };
    let parse = error
        .downcast_ref::<serde_json::Error>()
        .unwrap_or_else(|| panic!("error must be the JSON parse failure: {error:#}"));
    assert!(parse.is_syntax(), "{error:#}");
    assert!(
        format!("{error:#}").contains(HEARTBEAT_FILE),
        "error must name the heartbeat file: {error:#}"
    );
    // assess() must degrade to unknown rather than fail startup.
    let result = assess(dir.path(), "run-a", 10, 20, None);
    assert_eq!(result.suspected, Suspected::Unknown);
    assert_eq!(result.last_heartbeat_age_ms, None);
    Ok(())
}

#[test]
fn must_fail_assess_ignores_heartbeat_from_other_run_158() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    write_heartbeat(dir.path(), &beat("run-other", 1_000))?;
    let result = assess(dir.path(), "run-a", 1_000, 2_000, None);
    assert_eq!(result.suspected, Suspected::Unknown);
    assert_eq!(result.last_heartbeat_age_ms, None);
    Ok(())
}

#[test]
fn must_pass_assess_reports_age_and_suspicion_for_previous_run_158() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    write_heartbeat(dir.path(), &beat("run-a", 1_000_000))?;
    let hang = assess(
        dir.path(),
        "run-a",
        1_000_000 + 4 * INTERVAL_MS,
        2_000_000,
        None,
    );
    assert_eq!(hang.suspected, Suspected::Hang);
    assert_eq!(hang.last_heartbeat_age_ms, Some(1_000_000));
    let kill = assess(dir.path(), "run-a", 1_000_000, 1_000_500, None);
    assert_eq!(kill.suspected, Suspected::Kill);
    assert_eq!(kill.last_heartbeat_age_ms, Some(500));
    // A clock that went backwards must not underflow the age.
    let skew = assess(dir.path(), "run-a", 1_000_000, 10, None);
    assert_eq!(skew.last_heartbeat_age_ms, Some(0));
    Ok(())
}

#[test]
fn must_pass_run_loop_beats_until_stopped_158() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| run_loop(dir.path(), "run-loop", Duration::from_millis(10), &stop));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if read_heartbeat(dir.path())
                .ok()
                .flatten()
                .is_some_and(|b| b.seq >= 2)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Ordering::Release);
    });
    let last =
        read_heartbeat(dir.path())?.ok_or_else(|| anyhow::anyhow!("no heartbeat written"))?;
    assert_eq!(last.run_id, "run-loop");
    assert_eq!(last.pid, std::process::id());
    assert_eq!(last.interval_ms, 10);
    assert!(last.seq >= 2, "seq advanced: {}", last.seq);
    Ok(())
}
