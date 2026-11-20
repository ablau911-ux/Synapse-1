use super::*;

#[cfg(windows)]
pub(crate) async fn post_element_window_message_click(
    params: &ActClickParams,
    element: &ActClickElementTarget,
    screen_point: Point,
    timing: DoubleClickTiming,
) -> Result<String, ErrorData> {
    let readback =
        windows_hwnd_message_click_readback(&element.element_id, screen_point, params.button)
            .map_err(|error| mark_click_not_dispatched(action_error_to_mcp(&error)))?;
    super::super::point_post::post_hwnd_clicks(params, &readback, timing).await?;
    Ok("software_window_message".to_owned())
}

#[cfg(not(windows))]
pub(crate) async fn post_element_window_message_click(
    _params: &ActClickParams,
    element: &ActClickElementTarget,
    _screen_point: Point,
    _timing: DoubleClickTiming,
) -> Result<String, ErrorData> {
    Err(mark_click_not_dispatched(action_error_to_mcp(
        &ActionError::BackendUnavailable {
            detail: format!(
                "act_click element target {} HWND message fallback requires Windows",
                element.element_id
            ),
        },
    )))
}

#[cfg(windows)]
pub(crate) fn element_center(
    element_id: &synapse_core::ElementId,
    recording: bool,
    foreground_coordinate: bool,
) -> Result<Point, ErrorData> {
    element_center_with(
        element_id,
        recording,
        |root| {
            synapse_a11y::foreground_context(root)
                .map(|context| context.window_bounds)
                .map_err(|error| action_error_to_mcp(&element_resolution_error(element_id, error)))
        },
        |element_id, root| {
            native_click_point_with(
                element_id,
                root,
                recording,
                foreground_coordinate,
                |element_id| {
                    synapse_a11y::element_bounding_rect(element_id).map_err(|error| {
                        action_error_to_mcp(&element_resolution_error(element_id, error))
                    })
                },
                |root| {
                    synapse_a11y::foreground_context(root)
                        .map(|context| context.window_bounds)
                        .map_err(|error| {
                            action_error_to_mcp(&element_resolution_error(element_id, error))
                        })
                },
                element_coordinate_fallback_metadata,
                crate::server::foreground_click_actionability::actionable_click_point,
            )
        },
    )
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn native_click_point_with(
    element_id: &synapse_core::ElementId,
    root: i64,
    recording: bool,
    foreground_coordinate: bool,
    mut read_bbox: impl FnMut(&synapse_core::ElementId) -> Result<synapse_core::Rect, ErrorData>,
    mut read_bounds: impl FnMut(i64) -> Result<synapse_core::Rect, ErrorData>,
    mut read_metadata: impl FnMut(
        &synapse_core::ElementId,
    ) -> Result<synapse_a11y::ElementMetadataReadback, ErrorData>,
    mut actionable: impl FnMut(
        &synapse_core::ElementId,
        i64,
        &[UiaPattern],
        &mut synapse_core::Rect,
    ) -> Result<
        Point,
        crate::server::foreground_click_actionability::ClickPointRefusal,
    >,
) -> Result<Point, ErrorData> {
    // PostMessage resolves the existing bbox without scrolling the UI.
    // Only the foreground coordinate tier may make the target actionable.
    if recording || !foreground_coordinate {
        let rect = read_bbox(element_id)?;
        let window_bounds = if recording { rect } else { read_bounds(root)? };
        return crate::server::foreground_click_actionability::visible_click_point(
            rect,
            window_bounds,
        )
        .map_err(|refusal| element_click_point_refusal(element_id, refusal));
    }
    let metadata = read_metadata(element_id)?;
    let mut bbox = metadata.bbox;
    actionable(element_id, root, &metadata.patterns, &mut bbox)
        .map_err(|refusal| element_click_point_refusal(element_id, refusal))
}

#[cfg(windows)]
pub(crate) fn element_center_with(
    element_id: &synapse_core::ElementId,
    recording: bool,
    mut bounds: impl FnMut(i64) -> Result<synapse_core::Rect, ErrorData>,
    mut uia: impl FnMut(&synapse_core::ElementId, i64) -> Result<Point, ErrorData>,
) -> Result<Point, ErrorData> {
    let root = element_id
        .parts()
        .map_err(|error| {
            action_error_to_mcp(&ActionError::TargetInvalid {
                detail: error.to_string(),
            })
        })?
        .hwnd;
    let gate = crate::server::foreground_click_actionability::visible_click_point;
    let ocr_rect = browser_ocr_rect_or_error(element_id)?;
    let point = if let Some(rect) = ocr_rect {
        // Recording emits no real cursor input and permits synthetic HWNDs.
        let window_bounds = if recording { rect } else { bounds(root)? };
        gate(rect, window_bounds)
    } else {
        return uia(element_id, root);
    };
    point.map_err(|refusal| element_click_point_refusal(element_id, refusal))
}

#[cfg(windows)]
pub(crate) fn element_click_point_refusal(
    element_id: &synapse_core::ElementId,
    refusal: crate::server::foreground_click_actionability::ClickPointRefusal,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        refusal.detail,
        Some(
            json!({"code": refusal.code, "reason": refusal.reason, "tool": "act_click",
            "element_id": element_id, "source_of_truth": "live target window bounds and element bbox"}),
        ),
    )
}

#[cfg(windows)]
pub(crate) fn element_resolution_error(
    element_id: &synapse_core::ElementId,
    error: synapse_a11y::A11yError,
) -> ActionError {
    let detail = error.to_string();
    match error {
        synapse_a11y::A11yError::ElementStale { .. } => ActionError::TransientElementExpired {
            element_id: element_id.clone(),
            detail,
        },
        _ => ActionError::ElementNotResolved {
            detail: format!("act_click element {element_id} could not be resolved: {detail}"),
        },
    }
}

#[cfg(windows)]
pub(crate) fn browser_ocr_rect_or_error(
    element_id: &synapse_core::ElementId,
) -> Result<Option<synapse_core::Rect>, ErrorData> {
    match crate::m1::browser_ocr_rect_from_element_id(element_id) {
        Some(rect) => Ok(Some(rect)),
        None if crate::m1::is_browser_ocr_element_id(element_id) => {
            Err(action_error_to_mcp(&ActionError::TargetInvalid {
                detail: format!(
                    "act_click browser OCR element {element_id} does not contain a valid non-empty bbox"
                ),
            }))
        }
        None => Ok(None),
    }
}

#[cfg(not(windows))]
pub(crate) fn element_center(
    element_id: &synapse_core::ElementId,
    _recording: bool,
    _foreground_coordinate: bool,
) -> Result<Point, ErrorData> {
    Err(action_error_to_mcp(&ActionError::BackendUnavailable {
        detail: format!(
            "act_click element target {element_id} requires Windows UI Automation bbox resolution"
        ),
    }))
}

pub(crate) fn element_is_coordinate_only(element_id: &synapse_core::ElementId) -> bool {
    crate::m1::is_browser_ocr_element_id(element_id)
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub(crate) struct HwndMessageClickReadback {
    pub(crate) root_hwnd: i64,
    pub(crate) hwnd: i64,
    pub(crate) class_name: String,
    pub(crate) client_point: Point,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub(crate) struct WindowCandidate {
    hwnd: HWND,
    class_name: String,
}

#[cfg(windows)]
pub(crate) fn windows_hwnd_message_click_readback(
    element_id: &synapse_core::ElementId,
    screen_point: Point,
    button: MouseButton,
) -> Result<HwndMessageClickReadback, ActionError> {
    let root_hwnd = element_id
        .parts()
        .map_err(|error| ActionError::TargetInvalid {
            detail: format!("act_click element id {element_id} could not be parsed: {error}"),
        })?
        .hwnd;
    hwnd_message_readback_for_root(root_hwnd, screen_point, button)
}

/// #107: resolve the child HWND under a screen point of a claimed root window.
/// Shared by element-id clicks and background point clicks (point_post).
#[cfg(windows)]
pub(crate) fn hwnd_message_readback_for_root(
    root_hwnd: i64,
    screen_point: Point,
    button: MouseButton,
) -> Result<HwndMessageClickReadback, ActionError> {
    let _ = mouse_button_messages(button)?;
    let root = hwnd_from_i64(root_hwnd)?;
    synapse_action::delivery_target::ensure_live_window(root_hwnd, "act_click root window")?;

    let target = best_hwnd_for_screen_point(root, screen_point)?;
    let mut client = WinPoint {
        x: screen_point.x,
        y: screen_point.y,
    };
    // SAFETY: the out-pointer refers to the live local `client`; a stale HWND makes the call
    // return FALSE, which is handled as an error.
    if !unsafe { ScreenToClient(target.hwnd, &raw mut client) }.as_bool() {
        return Err(ActionError::BackendUnavailable {
            detail: format!(
                "ScreenToClient failed for act_click target hwnd 0x{:x} at screen point {screen_point:?}",
                hwnd_to_i64(target.hwnd)
            ),
        });
    }

    let client_point = Point {
        x: client.x,
        y: client.y,
    };
    let _ = mouse_lparam(client_point)?;
    Ok(HwndMessageClickReadback {
        root_hwnd,
        hwnd: hwnd_to_i64(target.hwnd),
        class_name: target.class_name,
        client_point,
    })
}

#[cfg(windows)]
pub(crate) fn best_hwnd_for_screen_point(
    root: HWND,
    point: Point,
) -> Result<WindowCandidate, ActionError> {
    let root_rect = window_rect(root)?;
    if !rect_contains_point(&root_rect, point) {
        return Err(ActionError::TargetInvalid {
            detail: format!(
                "act_click element center {point:?} is outside root hwnd 0x{:x} rect {:?}",
                hwnd_to_i64(root),
                rect_tuple(&root_rect)
            ),
        });
    }

    // #197: z-order aware descent (topmost hittable sibling at each level), not smallest-area.
    let hwnd = super::super::hit_test::deepest_hit_hwnd(root, point);
    Ok(WindowCandidate {
        hwnd,
        class_name: window_class_name(hwnd),
    })
}

#[cfg(windows)]
pub(crate) fn post_mouse_message(
    hwnd: i64,
    message: u32,
    wparam: usize,
    client_point: Point,
) -> Result<(), ActionError> {
    let hwnd = hwnd_from_i64(hwnd)?;
    let lparam = mouse_lparam(client_point)?;
    // SAFETY: wparam/lparam are plain integers (button flags and packed client coordinates),
    // never pointers, so nothing can dangle in the target's queue; a stale HWND makes
    // PostMessageW return an error, which is mapped below.
    unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), lparam) }.map_err(|error| {
        ActionError::BackendUnavailable {
            detail: format!(
                "PostMessageW act_click mouse message 0x{message:x} failed for hwnd 0x{:x} client_point={client_point:?}: {error}",
                hwnd_to_i64(hwnd)
            ),
        }
    })
}

#[cfg(windows)]
pub(crate) fn mouse_button_messages(button: MouseButton) -> Result<(u32, u32, usize), ActionError> {
    match button {
        MouseButton::Left => Ok((WM_LBUTTONDOWN, WM_LBUTTONUP, 0x0001)),
        MouseButton::Right => Ok((WM_RBUTTONDOWN, WM_RBUTTONUP, 0x0002)),
        MouseButton::Middle => Ok((WM_MBUTTONDOWN, WM_MBUTTONUP, 0x0010)),
        MouseButton::X1 | MouseButton::X2 => Err(ActionError::BackendUnavailable {
            detail: format!(
                "act_click HWND message fallback supports left/right/middle buttons only, got {button:?}"
            ),
        }),
    }
}

#[cfg(windows)]
pub(crate) fn mouse_lparam(client_point: Point) -> Result<LPARAM, ActionError> {
    let x = i16::try_from(client_point.x).map_err(|error| ActionError::TargetInvalid {
        detail: format!(
            "act_click client x {} cannot fit a WM_* mouse lParam i16: {error}",
            client_point.x
        ),
    })?;
    let y = i16::try_from(client_point.y).map_err(|error| ActionError::TargetInvalid {
        detail: format!(
            "act_click client y {} cannot fit a WM_* mouse lParam i16: {error}",
            client_point.y
        ),
    })?;
    let packed = (u32::from(u16::from_ne_bytes(y.to_ne_bytes())) << 16)
        | u32::from(u16::from_ne_bytes(x.to_ne_bytes()));
    Ok(LPARAM(isize::try_from(packed).unwrap_or(isize::MAX)))
}

#[cfg(windows)]
pub(crate) fn window_rect(hwnd: HWND) -> Result<RECT, ActionError> {
    let mut rect = RECT::default();
    // SAFETY: the out-pointer refers to the live local `rect`; a stale HWND makes the call
    // return an error, which is mapped below.
    unsafe { GetWindowRect(hwnd, &raw mut rect) }.map_err(|error| {
        ActionError::ElementNotResolved {
            detail: format!(
                "GetWindowRect failed for act_click hwnd 0x{:x}: {error}",
                hwnd_to_i64(hwnd)
            ),
        }
    })?;
    Ok(rect)
}

#[cfg(windows)]
pub(crate) fn rect_contains_point(rect: &RECT, point: Point) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

#[cfg(windows)]
pub(crate) fn rect_tuple(rect: &RECT) -> (i32, i32, i32, i32) {
    (rect.left, rect.top, rect.right, rect.bottom)
}

#[cfg(windows)]
pub(crate) fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = vec![0_u16; 256];
    // SAFETY: `buffer` is a live, writable UTF-16 slice and GetClassNameW writes at most its
    // length; a stale HWND returns 0, which yields an empty class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
}

#[cfg(windows)]
pub(crate) fn hwnd_from_i64(hwnd: i64) -> Result<HWND, ActionError> {
    if hwnd == 0 {
        return Err(ActionError::TargetInvalid {
            detail: "act_click element root hwnd is null".to_owned(),
        });
    }
    Ok(HWND(hwnd as isize as *mut c_void))
}

#[cfg(windows)]
pub(crate) fn hwnd_to_i64(hwnd: HWND) -> i64 {
    hwnd.0 as isize as i64
}
