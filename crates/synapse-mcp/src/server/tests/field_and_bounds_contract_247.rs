//! NEEDS-SEAM. Extract real set_field dispatch as:
//! async fn target_act_set_field_with<T,F,Fut>(&TargetActParams, dispatch:F)
//!   -> Result<T,ErrorData> where F:FnOnce(TargetActSetFieldTarget,String)->Fut,
//!   Fut:std::future::Future<Output=Result<T,ErrorData>>.
//! Extract real bounds result mapping as:
//! target_act_window_bounds_response(&TargetActParams, &synapse_a11y::WindowBoundsOutcome)
//!   -> (bool, &'static str, Value).
//! Both production branches must use these seams; no test-only implementation.
use super::*;

fn params(value: Value) -> TargetActParams {
    serde_json::from_value(value).expect("action fixture")
}

#[tokio::test]
async fn must_pass_explicit_replacement_including_empty_reaches_each_real_route() {
    for browser in [true, false] {
        for text in ["", "  replacement\nשלום  "] {
            let mut value = json!({"verb":"set_field", "text":text});
            value[if browser { "selector" } else { "element_id" }] = json!(if browser {
                "#field"
            } else {
                "0x2a:0000000000000001"
            });
            let mut field = "before".to_owned();
            assert_eq!(field, "before");
            let result = target_act_set_field_with(&params(value), |route, supplied| {
                assert_eq!(
                    matches!(route, TargetActSetFieldTarget::Browser { .. }),
                    browser
                );
                field = supplied;
                std::future::ready(Ok(json!({"value":field})))
            })
            .await
            .expect("explicit text accepted");
            assert_eq!(field, text);
            assert_eq!(result["value"], json!(text));
        }
    }
}

#[tokio::test]
async fn must_fail_omitted_or_null_text_is_parameter_error_without_delegate_or_field_write() {
    for selector in [true, false] {
        for explicit_null in [true, false] {
            let mut value = json!({"verb":"set_field"});
            value[if selector { "selector" } else { "element_id" }] = json!(if selector {
                "#field"
            } else {
                "0x2a:0000000000000001"
            });
            if explicit_null {
                value["text"] = Value::Null;
            }
            let mut field = "KEEP_247".to_owned();
            let mut calls = 0;
            assert_eq!(field, "KEEP_247");
            let error = target_act_set_field_with(&params(value), |_, text| {
                calls += 1;
                field = text;
                std::future::ready(Ok(json!({})))
            })
            .await
            .expect_err("missing replacement must be refused");
            assert_eq!(
                target_act_error_code(&error),
                Some(error_codes::TOOL_PARAMS_INVALID)
            );
            assert_eq!(calls, 0);
            assert_eq!(field, "KEEP_247");
        }
    }
}

#[test]
fn must_fail_real_field_handler_cannot_bypass_dispatch_validation_seam() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/background_router.rs"),
    )
    .expect("actual router");
    let field = source
        .split("\"set_field\" => {")
        .nth(1)
        .expect("field branch")
        .split("\"insert_text\" =>")
        .next()
        .expect("branch body");
    assert!(field.contains("target_act_set_field_with("));
    assert!(!field.contains("params.text.unwrap_or_default()"));
}

#[cfg(windows)]
#[test]
fn must_pass_bounds_check_compares_only_supplied_axes() {
    let request = params(json!({"verb":"set_window_bounds", "x":-10, "height":80}));
    let outcome = synapse_a11y::WindowBoundsOutcome {
        actual: Rect {
            x: -10,
            y: 999,
            w: 123,
            h: 80,
        },
        minimized: false,
    };
    let (ok, status, _) = target_act_window_bounds_response(&request, &outcome);
    assert_eq!((ok, status), (true, "ok"));
}

#[cfg(windows)]
#[test]
fn must_fail_any_requested_bounds_mismatch_is_verify_needed() {
    let request =
        params(json!({"verb":"set_window_bounds", "x":10,"y":20,"width":100,"height":80}));
    let expected = Rect {
        x: 10,
        y: 20,
        w: 100,
        h: 80,
    };
    for actual in [
        Rect { x: 11, ..expected },
        Rect { y: 21, ..expected },
        Rect { w: 101, ..expected },
        Rect { h: 81, ..expected },
    ] {
        let outcome = synapse_a11y::WindowBoundsOutcome {
            actual,
            minimized: false,
        };
        let (ok, status, value) = target_act_window_bounds_response(&request, &outcome);
        assert_eq!((ok, status), (false, "verify_needed"));
        assert_eq!(
            value["actual"],
            json!({"x":actual.x,"y":actual.y,"width":actual.w,"height":actual.h})
        );
        assert_eq!(
            outcome.actual, actual,
            "classification must not rewrite native readback"
        );
    }
}
