use crate::server::tool_profiles::{PUBLIC_TOOL_LIMIT, PUBLIC_TOOL_NAMES};

#[test]
fn must_pass_default_build_ships_livefeed_as_tool_44() {
    assert!(
        PUBLIC_TOOL_NAMES.contains(&"livefeed"),
        "default build must ship livefeed as tool 44"
    );
    assert_eq!(PUBLIC_TOOL_NAMES.len(), 44);
    assert_eq!(PUBLIC_TOOL_LIMIT, 44);
}
