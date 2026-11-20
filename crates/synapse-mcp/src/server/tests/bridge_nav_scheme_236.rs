//! #236: Chrome blocks extension-initiated top-frame navigation to data: and
//! javascript: URLs, so a normal-Chrome bridge navigate to one must be refused
//! immediately instead of waiting out the readback timeout.

use super::*;

#[test]
fn must_fail_bridge_navigation_to_data_or_javascript_urls_is_refused() {
    for url in [
        "data:text/html,<h1>x</h1>",
        "DATA:text/plain,x",
        "javascript:alert(1)",
        // URL parsers (and Chrome) strip embedded tab/newline characters, so
        // these still resolve to the javascript: scheme.
        "java\tscript:alert(1)",
        "java\nscript:alert(1)",
        "JaVa\r\nScRiPt:alert(1)",
    ] {
        let refusal = bridge_navigation_url_refusal(Some(url))
            .unwrap_or_else(|| panic!("{url} must be refused"));
        assert!(refusal.contains("http(s)"), "{refusal}");
    }
}

/// The real bridge navigate path refuses a data:/javascript: URL with
/// TOOL_PARAMS_INVALID before `navigate_tab` is reached. No bridge is connected
/// in the test, so had `navigate_tab` run the error would be a bridge error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn must_fail_bridge_navigate_operation_refuses_data_and_javascript_before_navigate_tab()
-> anyhow::Result<()> {
    let dir = tempfile::TempDir::new()?;
    let service = super::tests::service_with_temp_db(dir.path())?;
    // An HWND with no raw-CDP endpoint, so the bridge branch is taken.
    let hwnd: i64 = 0x7_236;
    for (url, scheme) in [
        ("data:text/html,<h1>x</h1>", "data"),
        ("java\tscript:alert(1)", "javascript"),
    ] {
        let error = service
            .cdp_navigate_tab_impl(
                "cdp_navigate_tab",
                "i236-session",
                hwnd,
                "ABC236",
                CdpNavigateAction::Navigate,
                Some(url),
                1_000,
                false,
            )
            .await
            .expect_err("a data:/javascript: bridge navigation must be refused");
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID), "{error:?}");
        assert!(
            error
                .message
                .starts_with(&format!("cdp_navigate_tab cannot open a {scheme}: URL")),
            "{error:?}"
        );
    }
    Ok(())
}

#[test]
fn must_pass_web_urls_and_reloads_are_not_refused() {
    for url in [
        Some("https://example.com/"),
        Some("http://127.0.0.1:8080/x"),
        Some("about:blank"),
        None,
    ] {
        assert!(bridge_navigation_url_refusal(url).is_none(), "{url:?}");
    }
}
