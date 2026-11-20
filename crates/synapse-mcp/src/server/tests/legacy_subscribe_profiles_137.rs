//! FU1 #137: old ownerless cancellation must never be reachable via a profile.
//! Unit child of server::tool_profiles: exercise real discovery + dispatch policy.
use super::*;
use rmcp::ServerHandler;

const PROFILES: [ToolProfileKind; 5] = [
    ToolProfileKind::NormalAgent,
    ToolProfileKind::BrowserControl,
    ToolProfileKind::BrowserDebugger,
    ToolProfileKind::BreakGlass,
    ToolProfileKind::FullCapability,
];

#[test]
fn must_pass_owned_subscribe_facade_visible_on_every_profile_137() {
    let tools = SynapseService::tool_router().list_all();
    for profile in PROFILES {
        let names = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        assert!(
            visible_tool_names_for_profile(profile, &names).contains(&"subscribe".to_owned()),
            "{} must retain the owned facade",
            profile.as_str()
        );
    }
}

#[tokio::test]
async fn must_fail_legacy_cancel_on_every_profile_preserves_foreign_subscription_137() {
    let root = tempfile::tempdir().unwrap();
    let service = crate::test_support::service_in(root.path()).unwrap();
    let sse = service.sse_state().unwrap();
    let params = serde_json::from_value(serde_json::json!({"kinds":[]})).unwrap();
    let created =
        crate::m3::subscribe::subscribe_to_events(&sse, &params, Some("fu1-owner137".to_owned()))
            .unwrap();
    assert_eq!(
        sse.subscription_ids_for_session("fu1-owner137").unwrap(),
        vec![created.subscription_id.clone()]
    );
    for profile in PROFILES {
        let id = format!("fu1-intruder-{}", profile.as_str());
        service
            .write_tool_profile_assignment(&id, profile, "test", None, None)
            .unwrap();
        assert_eq!(
            service.tool_profile_snapshot(Some(&id)).unwrap().profile,
            profile
        );
        let listed = service.tools_for_session_profile(Some(&id)).unwrap();
        assert!(!listed.iter().any(|tool| tool.name == "subscribe_cancel"));
        let context = crate::test_support::request_context_for_session(&id).await;
        let request = serde_json::from_value(serde_json::json!({"name":"subscribe_cancel",
            "arguments":{"subscription_id":created.subscription_id}}))
        .unwrap();
        let error = ServerHandler::call_tool(&service, request, context)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|v| v.get("code"))
                .and_then(Value::as_str),
            Some(error_codes::TOOL_PROFILE_POLICY_DENIED),
            "{profile:?}: {error:?}"
        );
        assert_eq!(
            sse.subscription_ids_for_session("fu1-owner137").unwrap(),
            vec![created.subscription_id.clone()]
        );
    }
    let params = serde_json::from_value(serde_json::json!({
        "subscription_id":created.subscription_id}))
    .unwrap();
    let cancelled =
        crate::m3::subscribe::cancel_owned_subscription(&sse, &params, "fu1-owner137").unwrap();
    assert!(cancelled.cancelled);
    assert_eq!(
        sse.subscription_ids_for_session("fu1-owner137")
            .unwrap()
            .len(),
        0
    );
}
