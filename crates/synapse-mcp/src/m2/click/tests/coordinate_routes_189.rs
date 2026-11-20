//! Actual coordinate/foreground/PostMessage composition with counted native operations.
use super::*;
use std::sync::{Arc, Mutex};

pub(crate) struct NativePort {
    pub(crate) params: ActClickParams,
    pub(crate) counts: Arc<Mutex<[usize; 4]>>,
    pub(crate) fail_at_message: Option<usize>,
    pub(crate) permission: bool,
    pub(crate) caret: bool,
}

pub(crate) fn timing() -> DoubleClickTiming {
    DoubleClickTiming {
        window_ms: 500,
        inter_click_delay_ms: 0,
        source: "test",
    }
}

impl NativePort {
    pub(crate) fn new(permission: bool, fail_at_message: Option<usize>) -> Self {
        Self {
            params: serde_json::from_value(json!({
                "target": { "element_id": "0xbd:01020304" }, "hold_ms": 1
            }))
            .unwrap(),
            counts: Arc::new(Mutex::new([0; 4])),
            fail_at_message,
            permission,
            caret: false,
        }
    }

    async fn background_first(&mut self) -> Result<ActClickResponse, ErrorData> {
        let params = self.params.clone();
        coordinate_click_with(self, &params, timing(), Instant::now(), Vec::new()).await
    }
}

impl CoordinateClickPort for NativePort {
    fn point(&mut self, foreground: bool) -> Result<Point, ErrorData> {
        let id = synapse_core::element_id(189, "01020304");
        let rect = synapse_core::Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        native_click_point_with(
            &id,
            189,
            false,
            foreground,
            |_| Ok(rect),
            |_| Ok(rect),
            |_| {
                Ok(synapse_a11y::ElementMetadataReadback {
                    name: "control-189".to_owned(),
                    role: "button".to_owned(),
                    automation_id: None,
                    bbox: rect,
                    enabled: true,
                    keyboard_focusable: true,
                    patterns: vec![],
                    value: None,
                })
            },
            |_, _, _, _| {
                self.counts.lock().unwrap()[3] += 1;
                Ok(Point { x: 20, y: 20 })
            },
        )
    }
    fn requires_foreground(&mut self) -> Result<bool, ErrorData> {
        Ok(self.caret)
    }
    fn authorize(&mut self) -> Result<(), ErrorData> {
        let gate = super::super::super::authorize_foreground_click(
            self.permission,
            &super::super::super::ClaimFacts {
                target_root: Some(189),
                claim_held: false,
                session_target_matches: false,
            },
        );
        ForegroundClickPolicy::authorized(Some("test-189".to_owned()), gate)
            .authorize_delivery(None)
    }
    async fn post(&mut self, _: Point) -> Result<String, ErrorData> {
        use super::super::super::hwnd_post::PostedMouseStep;
        self.counts.lock().unwrap()[0] += 1;
        let mut message = 0;
        let fail_at = self.fail_at_message;
        let counts = self.counts.clone();
        super::super::super::point_post::post_hwnd_clicks_with(
            &self.params,
            timing(),
            false,
            || Ok(()),
            |step| {
                let index = message;
                message += 1;
                if fail_at == Some(index) {
                    return Err(ActionError::BackendUnavailable {
                        detail: "PostMessageW failed".to_owned(),
                    });
                }
                if matches!(step, PostedMouseStep::Down | PostedMouseStep::DoubleClick) {
                    counts.lock().unwrap()[1] += 1;
                }
                Ok(())
            },
        )
        .await?;
        Ok("software_window_message".to_owned())
    }
    async fn foreground(
        &mut self,
        _: Point,
        _: &mut Vec<ActClickTierAttempt>,
    ) -> Result<String, ErrorData> {
        self.counts.lock().unwrap()[2] += 1;
        Ok("software".to_owned())
    }
}

#[tokio::test]
async fn must_fail_partial_postmessage_blocks_internal_foreground_for_same_control() {
    let mut port = NativePort::new(true, Some(2));
    let error = port.background_first().await.expect_err("Up failure");
    assert_eq!(*port.counts.lock().unwrap(), [1, 1, 0, 0]);
    assert_eq!(error.data.as_ref().unwrap()["delivered"], true);
    assert_eq!(
        error.data.as_ref().unwrap()["tier_attempts"][0]["status"],
        "delivered"
    );
}

#[tokio::test]
async fn must_pass_presend_postmessage_uses_one_authorized_foreground_delivery() {
    let mut port = NativePort::new(true, Some(1));
    let result = port.background_first().await.expect("foreground fallback");
    assert_eq!(*port.counts.lock().unwrap(), [1, 0, 1, 1]);
    assert_eq!(result.tier_attempts.len(), 2);
}

#[tokio::test]
async fn must_fail_presend_postmessage_without_permission_never_scrolls_or_sends() {
    let mut port = NativePort::new(false, Some(1));
    let error = port
        .background_first()
        .await
        .expect_err("foreground denied");
    assert_eq!(*port.counts.lock().unwrap(), [1, 0, 0, 0]);
    assert_eq!(
        error.data.as_ref().unwrap()["code"],
        error_codes::TOOL_PROFILE_POLICY_DENIED
    );
}

#[tokio::test]
async fn must_pass_background_success_and_caret_route_keep_their_delivery_tiers() {
    let mut background = NativePort::new(false, None);
    background
        .background_first()
        .await
        .expect("PostMessage succeeds without foreground permission");
    assert_eq!(*background.counts.lock().unwrap(), [1, 1, 0, 0]);
    let mut caret = NativePort::new(true, None);
    caret.caret = true;
    caret.background_first().await.expect("caret route");
    assert_eq!(*caret.counts.lock().unwrap(), [0, 0, 1, 1]);
}

/// #189 R4: a failed caret-route read is a terminal refusal with no delivery.
#[tokio::test]
async fn must_fail_caret_route_read_error_is_terminal_without_any_delivery() {
    struct FailingCaret(NativePort);
    impl CoordinateClickPort for FailingCaret {
        fn point(&mut self, foreground: bool) -> Result<Point, ErrorData> {
            self.0.point(foreground)
        }
        fn requires_foreground(&mut self) -> Result<bool, ErrorData> {
            Err(crate::m1::mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "caret metadata read failed",
            ))
        }
        fn authorize(&mut self) -> Result<(), ErrorData> {
            self.0.authorize()
        }
        async fn post(&mut self, point: Point) -> Result<String, ErrorData> {
            self.0.post(point).await
        }
        async fn foreground(
            &mut self,
            point: Point,
            attempts: &mut Vec<ActClickTierAttempt>,
        ) -> Result<String, ErrorData> {
            self.0.foreground(point, attempts).await
        }
    }
    let mut port = FailingCaret(NativePort::new(true, None));
    let params = port.0.params.clone();
    let error = coordinate_click_with(&mut port, &params, timing(), Instant::now(), Vec::new())
        .await
        .expect_err("gate read failure");
    assert_eq!(*port.0.counts.lock().unwrap(), [0, 0, 0, 0]);
    let data = error.data.unwrap();
    assert_eq!(data["router_stop_reason"], "gate_read_failed");
    assert_eq!(data["dispatch_not_started"], true);
}
