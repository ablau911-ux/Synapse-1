#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;
use synapse_livefeed::config::CameraConfig;

const FIELDS: &[&str] = &[
    "FPS",
    "TILE_PX",
    "HISTORY_MS",
    "HISTORY_MAX_BYTES",
    "LOOK_LONG_EDGE",
    "JPEG_QUALITY",
    "MAX_CHANGE_CROPS",
    "WAIT_MAX_MS",
    "START_TIMEOUT_MS",
];
const CHILD: &str = "SYNAPSE_CAMERA_CONFIG_TEST_CHILD";

fn assert_bad_config() {
    let error = CameraConfig::from_env().expect_err("bad config must not use defaults");
    // Require an actual error type and the public error code, not a panic or fallback.
    let typed: &dyn std::error::Error = &error;
    assert!(
        typed.to_string().contains("LIVEFEED_BAD_CONFIG"),
        "expected LIVEFEED_BAD_CONFIG, got {error:?}"
    );
}

fn isolated(test: &str, field: &str, value: &str) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    for field in FIELDS {
        command.env_remove(format!("SYNAPSE_LIVEFEED_{field}"));
    }
    let output = command
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, "1")
        .env(format!("SYNAPSE_LIVEFEED_{field}"), value)
        .output()
        .expect("run child with isolated config environment");
    assert!(
        output.status.success(),
        "{field}={value:?}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "child must execute exactly the requested test"
    );
}

#[test]
fn must_fail_bad_env_value_typed_error() {
    if std::env::var_os(CHILD).is_some() {
        assert_bad_config();
        return;
    }
    for field in FIELDS {
        for invalid in ["not-a-number", "-1", "184467440737095516160", ""] {
            isolated("must_fail_bad_env_value_typed_error", field, invalid);
        }
    }
}

#[test]
fn must_fail_zero_rejected() {
    if std::env::var_os(CHILD).is_some() {
        assert_bad_config();
        return;
    }
    for field in FIELDS {
        isolated("must_fail_zero_rejected", field, "0");
    }
}
