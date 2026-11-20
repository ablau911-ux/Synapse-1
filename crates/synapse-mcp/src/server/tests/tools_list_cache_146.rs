//! #146: `tools/list` and `initialize` stalled for 10-20 s under CPU load
//! because every call re-sanitized every tool schema (twice) on the async
//! worker. The tool router is fixed at construction, so the sanitized list is
//! computed once per process and shared.

use crate::server::SynapseService;

fn service() -> anyhow::Result<(SynapseService, tempfile::TempDir)> {
    let root = tempfile::tempdir()?;
    let service = SynapseService::try_new_isolated_for_tests(root.path())?;
    Ok((service, root))
}

#[test]
fn must_pass_sanitized_tools_are_computed_once_and_shared() -> anyhow::Result<()> {
    let (service, _root) = service()?;
    let first = service.full_sanitized_tools_shared();
    let second = service.full_sanitized_tools_shared();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    Ok(())
}

#[test]
fn must_pass_cached_tools_match_a_fresh_sanitize() -> anyhow::Result<()> {
    let (service, _root) = service()?;
    let cached = serde_json::to_value(&*service.full_sanitized_tools_shared())?;
    let fresh = serde_json::to_value(crate::server::schema_sanitize::sanitize_tools(
        SynapseService::tool_router().list_all(),
    ))?;
    assert_eq!(cached, fresh);
    Ok(())
}
