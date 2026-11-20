use super::{BrowserSeatConfig, SeatAction, SeatConfig, Viewport};

#[test]
fn must_fail_browser_config_rejects_non_web_schemes_and_invalid_viewports() {
    let file_url = SeatConfig::Browser(BrowserSeatConfig {
        initial_url: Some("file:///C:/private.txt".to_owned()),
        viewport: Viewport::default(),
    });
    assert!(file_url.validate().is_err());

    let zero_width = SeatConfig::Browser(BrowserSeatConfig {
        initial_url: Some("about:blank".to_owned()),
        viewport: Viewport {
            width: 0,
            height: 720,
        },
    });
    assert!(zero_width.validate().is_err());
}

#[test]
fn must_pass_browser_config_accepts_web_and_blank_urls_with_a_real_viewport() {
    for url in [
        "https://example.com/",
        "http://127.0.0.1:8080/",
        "about:blank",
    ] {
        let config = SeatConfig::Browser(BrowserSeatConfig {
            initial_url: Some(url.to_owned()),
            viewport: Viewport {
                width: 800,
                height: 600,
            },
        });
        assert!(config.validate().is_ok(), "{url} must be accepted");
    }
}

#[test]
fn must_pass_browser_text_action_accepts_text_up_to_the_limit() {
    for text in ["x".to_owned(), "x".repeat(super::MAX_TEXT_BYTES)] {
        assert!(SeatAction::TypeText { text }.validate().is_ok());
    }
}

#[test]
fn must_fail_browser_text_action_is_bounded() {
    assert!(
        SeatAction::TypeText {
            text: String::new()
        }
        .validate()
        .is_err()
    );
    assert!(
        SeatAction::TypeText {
            text: "x".repeat(super::MAX_TEXT_BYTES + 1)
        }
        .validate()
        .is_err()
    );
}
