//! Child of m4.rs; real selector, no desktop interaction.
use super::*;

fn window(pid: u32) -> ForegroundContext {
    serde_json::from_value(json!({"hwnd":249,"pid":pid,"process_name":"calculator.exe",
        "process_path":"C:\\fixture\\calculator.exe", "window_title":"Calculator fixture",
        "window_bounds":{"x":0,"y":0,"w":200,"h":100},"monitor_index":0,
        "dpi_scale":1.0,"is_fullscreen":false,"is_dwm_composed":true}))
    .expect("window fixture")
}

#[test]
fn must_pass_new_direct_pid_window_matches_exact_title() {
    let contexts = [window(100)];
    assert_eq!(contexts[0].pid, 100);
    let regex = regex::Regex::new("^Calculator fixture$").expect("regex");
    let selected = select_launch_window(&contexts, 100, &regex, &HashSet::new(), "calc.exe", &[]);
    assert_eq!(selected.map(|c| (c.hwnd, c.pid)), Some((249, 100)));
}

#[test]
fn must_fail_preexisting_broker_window_is_never_attributed_to_new_launch() {
    let contexts = [window(99)];
    let excluded = HashSet::from([249]);
    assert!(excluded.contains(&contexts[0].hwnd));
    let regex = regex::Regex::new("^Calculator fixture$").expect("regex");
    let selected = select_launch_window(&contexts, 100, &regex, &excluded, "calc.exe", &[]);
    assert_eq!(
        selected.map(|c| c.hwnd),
        None,
        "prelaunch HWND must remain excluded"
    );
    assert_eq!(contexts[0].pid, 99);
    assert_eq!(excluded, HashSet::from([249]));
}
