//! Regression for #209: the dashboard reads the session list through the
//! blocking-pool entry point, and it must return the same read model as the
//! synchronous implementation.

use serde_json::Value;
use tempfile::TempDir;

use crate::server::SynapseService;
use crate::server::session_registry::unix_time_ms_now;
use crate::server::session_tools::SessionListResponse;

fn listed_ids(response: &SessionListResponse) -> Vec<&str> {
    response
        .sessions
        .iter()
        .map(|summary| summary.registry.session_id.as_str())
        .collect()
}

/// Seeds one open and one explicitly closed session at the current time.
fn seed_open_and_closed(service: &SynapseService) -> anyhow::Result<()> {
    let now = unix_time_ms_now();
    let mut registry = service
        .session_registry_ref()
        .lock()
        .map_err(|_error| anyhow::anyhow!("session registry poisoned"))?;
    registry.record_seen("open-209", None, now);
    registry.record_seen("closed-209", None, now);
    registry.record_closed("closed-209", now);
    Ok(())
}

/// Drops every field that legitimately changes between two reads of the same
/// registry (wall-clock stamps and "ms ago" ages), leaving the read model.
fn without_clock_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| {
                !(key == "now_unix_ms"
                    || key == "generated_at_unix_ms"
                    || key == "expires_in_ms"
                    || key == "silent_ms"
                    || key.ends_with("_ms_ago"))
            });
            fields.values_mut().for_each(without_clock_fields);
        }
        Value::Array(items) => items.iter_mut().for_each(without_clock_fields),
        _ => {}
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_blocking_session_list_hides_closed_sessions_unless_asked() -> anyhow::Result<()>
{
    let _lease = crate::test_support::lease_serial("session_list_blocking_209");
    let temp = TempDir::new()?;
    let service = SynapseService::try_new_isolated_for_tests(temp.path())?;
    seed_open_and_closed(&service)?;

    let open_only = service
        .session_list_impl_blocking(false)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let with_closed = service
        .session_list_impl_blocking(true)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;

    assert!(!open_only.include_closed);
    assert!(with_closed.include_closed);
    assert!(
        listed_ids(&open_only).contains(&"open-209"),
        "the open session must be listed: {:?}",
        listed_ids(&open_only)
    );
    assert!(
        !listed_ids(&open_only).contains(&"closed-209"),
        "a closed session must not be listed without include_closed: {:?}",
        listed_ids(&open_only)
    );
    assert!(listed_ids(&with_closed).contains(&"open-209"));
    assert!(
        listed_ids(&with_closed).contains(&"closed-209"),
        "include_closed must list the closed session: {:?}",
        listed_ids(&with_closed)
    );
    assert_eq!(with_closed.total_count, open_only.total_count + 1);
    assert_eq!(with_closed.returned_count, open_only.returned_count + 1);
    let closed = with_closed
        .sessions
        .iter()
        .find(|summary| summary.registry.session_id == "closed-209")
        .ok_or_else(|| anyhow::anyhow!("closed-209 missing"))?;
    assert_eq!(closed.registry.lifecycle, "closed");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_blocking_session_list_matches_the_synchronous_read_model() -> anyhow::Result<()>
{
    let _lease = crate::test_support::lease_serial("session_list_blocking_209");
    let temp = TempDir::new()?;
    let service = SynapseService::try_new_isolated_for_tests(temp.path())?;
    seed_open_and_closed(&service)?;

    for include_closed in [false, true] {
        let blocking = service
            .session_list_impl_blocking(include_closed)
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let direct = service
            .session_list_impl(include_closed)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;

        assert!(
            !blocking.sessions.is_empty(),
            "seeded sessions must be listed"
        );
        assert_eq!(blocking.include_closed, include_closed);
        assert_eq!(listed_ids(&blocking), listed_ids(&direct));
        let mut blocking_json = serde_json::to_value(&blocking)?;
        let mut direct_json = serde_json::to_value(&direct)?;
        // The host OS foreground and the ambient process/window scan are live
        // OS facts, not registry read model; they are compared structurally
        // (present) but not value-for-value.
        for json in [&mut blocking_json, &mut direct_json] {
            let fields = json
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("session list must serialize to an object"))?;
            for key in ["human_os_foreground", "attached_agent_registry"] {
                assert!(fields.remove(key).is_some(), "{key} missing from response");
            }
        }
        without_clock_fields(&mut blocking_json);
        without_clock_fields(&mut direct_json);
        assert_eq!(
            blocking_json, direct_json,
            "blocking and synchronous read models differ (include_closed={include_closed})"
        );
    }
    Ok(())
}
