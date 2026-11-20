//! #75: the cleanup fault hook is DEBUG-BUILD-ONLY. It is compiled out of
//! release builds (module and call site are both `cfg(debug_assertions)`), and
//! in debug builds it fails a matching pid a bounded number of times.

const LIFECYCLE_SOURCE: &str = include_str!("../session_lifecycle.rs");
/// Every non-comment line that references the hook must be directly preceded
/// (ignoring blank lines) by `#[cfg(debug_assertions)]` or by another attribute
/// line that follows it - i.e. a `cfg(debug_assertions)` appears in the
/// contiguous attribute block above it.
fn assert_all_hook_references_are_debug_gated(source: &str) {
    let lines: Vec<&str> = source.lines().collect();
    let mut references = 0;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || !line.contains("cleanup_fault_hook") {
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[path") {
            continue;
        }
        if trimmed.starts_with("mod cleanup_fault_hook_75_tests") {
            continue;
        }
        references += 1;
        let gated = lines[..index]
            .iter()
            .rev()
            .take_while(|above| {
                let above = above.trim();
                above.starts_with("#[") || above.starts_with("//") || above.is_empty()
            })
            .any(|above| above.contains("cfg(debug_assertions)"));
        assert!(
            gated,
            "hook reference on line {} is not cfg(debug_assertions)-gated: {line}",
            index + 1
        );
    }
    assert!(
        references >= 2,
        "expected the mod declaration and the call site, found {references}"
    );
}

#[test]
fn must_fail_hook_module_and_call_site_are_compiled_out_of_release_builds() {
    assert_all_hook_references_are_debug_gated(LIFECYCLE_SOURCE);
}

#[cfg(debug_assertions)]
mod debug_behavior {
    use super::super::cleanup_fault_hook::FaultPlan;

    #[test]
    fn must_pass_parse_accepts_pid_and_optional_count() {
        // Bare pid: that pid, budget of exactly one failure.
        let bare = FaultPlan::parse("4242").expect("bare pid plan");
        assert_eq!(bare.consume_for(&[1, 2]), None, "other pids never match");
        assert_eq!(bare.consume_for(&[4242]), Some(4242));
        assert_eq!(bare.consume_for(&[4242]), None, "default budget is one");

        // `pid:count` with surrounding whitespace: that pid, exactly 3 firings.
        let counted = FaultPlan::parse(" 4242 : 3 ").expect("counted plan");
        assert_eq!(counted.consume_for(&[1, 2]), None, "other pids never match");
        for firing in 1..=3 {
            assert_eq!(
                counted.consume_for(&[4242]),
                Some(4242),
                "firing {firing} of 3 must report the planned pid"
            );
        }
        assert_eq!(counted.consume_for(&[4242]), None, "budget is exactly 3");
    }

    #[test]
    fn must_fail_parse_rejects_invalid_specs() {
        for spec in ["", "abc", "0", "4242:0", "4242:x", ":3", "-5", "4242:"] {
            assert!(
                FaultPlan::parse(spec).is_none(),
                "spec {spec:?} must be rejected"
            );
        }
    }

    #[test]
    fn must_pass_fails_only_the_matching_tree_and_only_the_planned_number_of_times() {
        let plan = FaultPlan::parse("4242:2").expect("plan");
        assert_eq!(
            plan.consume_for(&[1, 2, 3]),
            None,
            "unrelated tree untouched"
        );
        assert_eq!(plan.consume_for(&[4242, 7]), Some(4242));
        assert_eq!(plan.consume_for(&[7, 4242]), Some(4242));
        assert_eq!(
            plan.consume_for(&[4242]),
            None,
            "budget exhausted: retry verifies"
        );
    }

    #[test]
    fn must_pass_default_budget_is_a_single_failure() {
        let plan = FaultPlan::parse("99").expect("plan");
        assert_eq!(plan.consume_for(&[99]), Some(99));
        assert_eq!(plan.consume_for(&[99]), None);
    }

    #[test]
    fn must_fail_unarmed_global_hook_never_fires() {
        // The test process does not set the env var.
        assert_eq!(
            super::super::cleanup_fault_hook::forced_failure_pid(&[1, 2, 3]),
            None
        );
    }
}
