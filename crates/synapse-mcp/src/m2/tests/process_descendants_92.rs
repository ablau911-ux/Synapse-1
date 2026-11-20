//! #92: process-tree membership is bounded, PID-reuse safe and cycle safe.

use super::{ProcessNode, descendant_pids, is_shared_host_process};

fn node(pid: u32, parent: Option<u32>, start_time: u64) -> ProcessNode {
    ProcessNode {
        pid,
        parent,
        start_time,
    }
}

fn sorted(mut pids: Vec<u32>) -> Vec<u32> {
    pids.sort_unstable();
    pids
}

#[test]
fn must_pass_children_and_grandchildren_are_descendants_unrelated_are_not() {
    let nodes = [
        node(10, Some(1), 100),
        node(20, Some(10), 110),
        node(21, Some(20), 120),
        node(30, Some(1), 105),
    ];
    assert_eq!(sorted(descendant_pids(&nodes, 10, 4)), vec![20, 21]);
}

#[test]
fn must_fail_unknown_root_has_no_descendants() {
    assert!(descendant_pids(&[node(20, Some(10), 110)], 10, 4).is_empty());
}

#[test]
fn must_fail_child_older_than_parent_is_a_reused_pid_not_a_descendant() {
    // pid 10 was reused: the child (start 50) predates the current pid 10 (100).
    let nodes = [node(10, Some(1), 100), node(20, Some(10), 50)];
    assert!(descendant_pids(&nodes, 10, 4).is_empty());
}

#[test]
fn must_fail_depth_is_bounded() {
    let nodes = [
        node(10, None, 1),
        node(11, Some(10), 2),
        node(12, Some(11), 3),
        node(13, Some(12), 4),
    ];
    assert_eq!(sorted(descendant_pids(&nodes, 10, 2)), vec![11, 12]);
}

#[test]
fn must_fail_parent_cycles_terminate() {
    let nodes = [node(10, Some(20), 1), node(20, Some(10), 2)];
    assert_eq!(descendant_pids(&nodes, 10, 8), vec![20]);
}

#[test]
fn must_pass_shared_shell_hosts_are_recognised_by_image_name() {
    assert!(is_shared_host_process("Explorer.EXE"));
    assert!(is_shared_host_process(r"C:\Windows\explorer.exe"));
    assert!(!is_shared_host_process("finance.exe"));
    assert!(!is_shared_host_process("explorer.exe.bak"));
}
