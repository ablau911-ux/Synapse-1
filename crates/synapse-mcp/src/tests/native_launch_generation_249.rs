//! Native reader fed into the actual launch selector: generation mismatch cannot use a live PID.
use super::*;

fn own_context() -> ForegroundContext {
    serde_json::from_value(json!({"hwnd":249,"pid":std::process::id(),
        "process_name":"fixture.exe","process_path":"fixture.exe","window_title":"native-249",
        "window_bounds":{"x":0,"y":0,"w":100,"h":100},"monitor_index":0,"dpi_scale":1.0,
        "is_fullscreen":false,"is_dwm_composed":true}))
    .expect("context")
}

#[test]
fn must_pass_exact_native_process_generation_can_attribute_new_window() {
    let contexts = [own_context()];
    let pid = std::process::id();
    let record = synapse_a11y::launch_process_record(pid).expect("native process identity");
    let selected = LaunchedTree::new(pid, record.1, synapse_a11y::launch_process_record, |_| {
        false
    })
    .select_window(
        &contexts,
        &regex::Regex::new("^native-249$").expect("regex"),
        &HashSet::new(),
        "fixture.exe",
        &[],
    );
    assert_eq!(selected.map(|v| (v.hwnd, v.pid)), Some((249, pid)));
    assert_eq!(synapse_a11y::launch_process_record(pid), Some(record));
}

#[test]
fn must_fail_live_pid_with_different_native_generation_is_not_the_launched_process() {
    let contexts = [own_context()];
    let pid = std::process::id();
    let record = synapse_a11y::launch_process_record(pid).expect("native process identity");
    // Deterministic reused-PID condition: live PID exists, retained launch generation differs.
    // Do not attempt to exhaust/recycle Windows' global PID namespace.
    let regex = regex::Regex::new("^native-249$").expect("regex");
    for expected_creation in [record.1 - 1, record.1 + 1] {
        assert_ne!(expected_creation, record.1);
        let selected = LaunchedTree::new(
            pid,
            expected_creation,
            synapse_a11y::launch_process_record,
            |_| false,
        )
        .select_window(&contexts, &regex, &HashSet::new(), "fixture.exe", &[]);
        assert_eq!(
            selected.map(|v| v.hwnd),
            None,
            "same PID is not proof of same generation"
        );
        assert_eq!(contexts[0].pid, pid);
        assert_eq!(synapse_a11y::launch_process_record(pid), Some(record));
    }
}
