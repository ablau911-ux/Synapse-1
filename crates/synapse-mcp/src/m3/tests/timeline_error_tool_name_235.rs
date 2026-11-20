//! #235: timeline refusals must name the operation that was called. A bad
//! cursor on `timeline get` used to be reported as a `timeline_search` error.

use super::*;

fn params_with_cursor(cursor: &str) -> TimelineSearchParams {
    TimelineSearchParams {
        start_ts_ns: None,
        end_ts_ns: None,
        apps: None,
        text: None,
        kinds: None,
        actor: None,
        limit: None,
        cursor: Some(cursor.to_owned()),
    }
}

type TestRuntime = (tempfile::TempDir, Arc<Mutex<ReflexRuntime>>);

/// A real reflex runtime over an empty temp store, so refusals come from the
/// public operation entry points, not from `validate` with a caller-chosen name.
fn runtime() -> TestRuntime {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(
        synapse_storage::Db::open(&temp.path().join("db"), synapse_core::SCHEMA_VERSION)
            .expect("open db"),
    );
    let (action, _rx) = synapse_action::ActionHandle::channel();
    let runtime = ReflexRuntime::spawn(db, action, synapse_reflex::EventBus::default())
        .expect("reflex runtime");
    (temp, Arc::new(Mutex::new(runtime)))
}

fn code_of(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

#[test]
fn must_fail_get_with_a_bad_cursor_names_timeline_get() {
    let (_temp, runtime) = runtime();
    let error = get_timeline(
        &runtime,
        &TimelineGetParams {
            start_ts_ns: None,
            end_ts_ns: None,
            kinds: None,
            actor: None,
            limit: None,
            cursor: Some("not-a-real-cursor".to_owned()),
        },
    )
    .err()
    .unwrap_or_else(|| panic!("timeline_get must refuse a bad cursor"));
    assert_eq!(code_of(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error.message.starts_with("timeline_get cursor"),
        "wrong operation named: {}",
        error.message
    );
}

#[test]
fn must_fail_search_with_a_bad_kind_names_timeline_search() {
    let (_temp, runtime) = runtime();
    let mut params = params_with_cursor("00");
    params.cursor = None;
    params.kinds = Some(vec!["no_such_kind".to_owned()]);
    let error = search_timeline(&runtime, &params)
        .err()
        .unwrap_or_else(|| panic!("timeline_search must refuse an unknown kind"));
    assert_eq!(code_of(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error.message.starts_with("timeline_search kinds entry"),
        "wrong operation named: {}",
        error.message
    );
}

#[test]
fn must_fail_purge_with_a_bad_cursor_names_timeline_purge() {
    let (_temp, runtime) = runtime();
    let error = purge_timeline(
        &runtime,
        &TimelinePurgeParams {
            start_ts_ns: None,
            end_ts_ns: None,
            apps: None,
            text: None,
            kinds: None,
            actor: None,
            flag_ids: None,
            all: true,
            dry_run: true,
            cursor: Some("not-a-real-cursor".to_owned()),
        },
        "test-session-235",
    )
    .err()
    .unwrap_or_else(|| panic!("timeline_purge must refuse a bad cursor"));
    assert_eq!(code_of(&error), Some(error_codes::TOOL_PARAMS_INVALID));
    assert!(
        error.message.starts_with("timeline_purge cursor"),
        "wrong operation named: {}",
        error.message
    );
}

#[test]
fn must_pass_valid_cursor_is_accepted_for_every_operation() {
    for tool in ["timeline_get", "timeline_search", "timeline_purge"] {
        assert!(
            validate(&params_with_cursor("00ff"), tool).is_ok(),
            "{tool} must accept a hex cursor"
        );
    }
}
