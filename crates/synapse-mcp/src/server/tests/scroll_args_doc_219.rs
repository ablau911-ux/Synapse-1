//! #219 variant guard: the published `target_act` schema must document the
//! unit of `args` for `verb=scroll` ([dx, dy] in wheel notches, one notch = one
//! wheel step), not only its `run_shell` meaning. An agent reading the schema
//! otherwise has no way to know that "120" means 120 wheel steps.

use super::{TargetActParams, schema_for};

fn args_description() -> String {
    let schema = serde_json::to_value(schema_for!(TargetActParams)).expect("schema json");
    schema["properties"]["args"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn must_fail_args_schema_without_the_scroll_unit_is_rejected() {
    let description = args_description();
    for needle in ["scroll", "[dx, dy]", "notch"] {
        assert!(
            description.contains(needle),
            "target_act `args` description must document verb=scroll ({needle}): {description:?}"
        );
    }
}

#[test]
fn must_pass_args_schema_still_documents_run_shell() {
    let description = args_description();
    assert!(description.contains("run_shell"), "{description:?}");
}
