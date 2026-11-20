//! #189 C4/C5/C6: CDP click event failures are typed by stage.
use super::*;

#[test]
fn must_pass_move_failure_is_not_dispatched_and_release_after_press_is_delivered() {
    assert_eq!(
        ClickEventStage::of(&DispatchMouseEventType::MouseMoved),
        ClickEventStage::BeforeButton
    );
    assert_eq!(
        ClickEventStage::of(&DispatchMouseEventType::MousePressed),
        ClickEventStage::Press
    );
    assert_eq!(
        ClickEventStage::of(&DispatchMouseEventType::MouseReleased),
        ClickEventStage::AfterPress
    );
    assert!(matches!(
        ClickEventStage::BeforeButton.error(&"mouseMoved lost"),
        A11yError::ClickNotDispatched { .. }
    ));
    assert!(matches!(
        ClickEventStage::Press.error(&"mousePressed lost"),
        A11yError::ClickDispatch {
            delivered: false,
            ..
        }
    ));
    assert!(matches!(
        ClickEventStage::AfterPress.error(&"mouseReleased lost"),
        A11yError::ClickDispatch {
            delivered: true,
            ..
        }
    ));
}
