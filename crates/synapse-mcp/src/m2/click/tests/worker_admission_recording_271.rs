//! #271 (from #189 U9): a refused UIA worker admission takes the in-tier
//! coordinate route once, but only when input is really sent. Under a recording
//! session the coordinate tier would record a click that the UIA tier never
//! recorded, so the refusal must stay terminal there.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

struct AdmissionPort {
    records: bool,
    coordinate_calls: usize,
}

fn params() -> ActClickParams {
    serde_json::from_value(json!({"target":{"element_id":"0xbd:01020304"}})).unwrap()
}

fn timing() -> DoubleClickTiming {
    DoubleClickTiming {
        window_ms: 500,
        inter_click_delay_ms: 0,
        source: "test",
    }
}

impl ElementClickPort for AdmissionPort {
    fn metadata(&mut self) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData> {
        Ok(synapse_a11y::ElementMetadataReadback {
            name: "control-271".to_owned(),
            value: None,
            role: "button".to_owned(),
            automation_id: None,
            enabled: true,
            keyboard_focusable: true,
            patterns: vec![UiaPattern::Invoke],
            bbox: synapse_core::Rect {
                x: 10,
                y: 10,
                w: 20,
                h: 20,
            },
        })
    }

    /// The UIA worker refused admission: typed not dispatched, tier unavailable.
    fn click(&mut self) -> Result<ElementClickOutcome, ActionError> {
        Err(ActionError::ClickNotDispatched {
            source: Box::new(ActionError::BackendUnavailable {
                detail: "phase=worker_busy".to_owned(),
            }),
            tier_unavailable: true,
        })
    }

    async fn coordinate(
        &mut self,
        mut prior: Vec<ActClickTierAttempt>,
        _: &'static str,
    ) -> Result<ActClickResponse, ErrorData> {
        self.coordinate_calls += 1;
        prior.push(click_tier_delivered(
            CLICK_TIER_POSTMESSAGE,
            false,
            "coordinate delivery",
        ));
        Ok(click_response(
            &params(),
            timing(),
            Instant::now(),
            "postmessage".to_owned(),
            false,
            prior,
        ))
    }

    fn records_input(&self) -> bool {
        self.records
    }
}

async fn run(port: &mut AdmissionPort) -> Result<ActClickResponse, ErrorData> {
    execute_element_click_with(port, &params(), timing(), Instant::now()).await
}

#[tokio::test]
async fn must_fail_worker_admission_refusal_under_recording_never_takes_the_coordinate_route() {
    let mut port = AdmissionPort {
        records: true,
        coordinate_calls: 0,
    };
    let error = run(&mut port)
        .await
        .expect_err("a recording session keeps the refusal terminal");
    assert_eq!(
        port.coordinate_calls, 0,
        "#189 U9: the coordinate tier must not run while input is only recorded"
    );
    crate::server::dispatch_status_189_tests::assert_not_delivered(
        &error,
        "failed_before_delivery",
    );
}

/// Control: the identical refusal without a recording session takes the route
/// once, so the test above fails only through the recording condition.
#[tokio::test]
async fn must_pass_worker_admission_refusal_without_recording_takes_the_coordinate_route_once() {
    let mut port = AdmissionPort {
        records: false,
        coordinate_calls: 0,
    };
    let response = run(&mut port).await.expect("one in-tier fallback route");
    assert_eq!(port.coordinate_calls, 1);
    assert_eq!(response.backend_tier_used, CLICK_TIER_POSTMESSAGE);
}

/// The live port derives `records_input` from its recording backend.
#[test]
fn must_pass_live_element_port_reports_a_recording_backend_as_recorded_input() {
    let params = params();
    let crate::m2::ActClickTarget::Element(element) = &params.target else {
        panic!("element target");
    };
    let recording = RecordingBackend::new();
    let live = |recording: Option<&RecordingBackend>| {
        let (handle, _rx) = ActionHandle::channel();
        LiveElementClick {
            handle,
            params: &params,
            element,
            recording,
            timing: timing(),
            started: Instant::now(),
            policy: ForegroundClickPolicy::allowed(None),
            state: EmitState::new(),
        }
        .records_input()
    };
    assert!(live(Some(&recording)), "recording session records input");
    assert!(!live(None), "a real session sends input");
}
