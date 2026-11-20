//! #203: the live-server tool deny-list must also apply when there is no
//! foreground window (locked screen, desktop focus).
use std::collections::BTreeMap;

use super::{KEY_LIVE_SERVER_ALLOWED, live_server_no_foreground_denial};

fn live_server_metadata() -> BTreeMap<String, String> {
    BTreeMap::from([(KEY_LIVE_SERVER_ALLOWED.to_owned(), "true".to_owned())])
}

#[test]
fn must_fail_live_server_profile_denies_shell_launch_and_reflex_register_without_foreground() {
    let profile_id = "everquest.live".to_owned();
    let metadata = live_server_metadata();
    for tool in ["act_run_shell", "act_launch", "reflex_register"] {
        let denial = live_server_no_foreground_denial(&profile_id, &metadata, tool)
            .unwrap_or_else(|| panic!("{tool} must be denied for a live-server profile"));
        assert_eq!(denial.reason, "live_server_tool_not_foreground_input");
        assert_eq!(denial.profile_id, profile_id);
        assert!(denial.detail.contains(tool));
    }
}

#[test]
fn must_pass_live_server_profile_allows_tools_outside_the_deny_list_without_foreground() {
    let profile_id = "everquest.live".to_owned();
    let metadata = live_server_metadata();
    assert!(
        live_server_no_foreground_denial(&profile_id, &metadata, "act_run_shell_status").is_none()
    );
}

#[test]
fn must_pass_non_live_server_profile_is_not_denied_without_foreground() {
    let profile_id = "luanti.local".to_owned();
    let metadata = BTreeMap::from([(KEY_LIVE_SERVER_ALLOWED.to_owned(), "false".to_owned())]);
    assert!(live_server_no_foreground_denial(&profile_id, &metadata, "act_run_shell").is_none());
    assert!(
        live_server_no_foreground_denial(&profile_id, &BTreeMap::new(), "act_launch").is_none()
    );
}
