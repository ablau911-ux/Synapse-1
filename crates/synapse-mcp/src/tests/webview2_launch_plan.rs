use super::*;

fn launch_params(
    target: &str,
    cdp_debug: Option<bool>,
) -> Result<ActLaunchParams, serde_json::Error> {
    serde_json::from_value(json!({
        "target": target,
        "cdp_debug": cdp_debug,
    }))
}

#[test]
fn must_pass_explicit_webview2_debug_uses_host_environment_instead_of_chrome_arguments()
-> anyhow::Result<()> {
    let params = launch_params("C:\\apps\\visual-test.exe", Some(true))?;
    assert!(chromium_cdp_launch(&params).is_none());
    assert!(webview2_cdp_launch_port(&params)?.is_some());
    let port = webview2_cdp_launch_port(&params)?.ok_or_else(|| anyhow::anyhow!("missing port"))?;
    let spawned = params_with_webview2_launch_env(params.clone(), Some(port), false);
    assert_eq!(
        spawned.env.get("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
        Some(&format!("--remote-debugging-port={port}"))
    );
    Ok(())
}

#[test]
fn must_fail_ordinary_and_chromium_launches_get_no_webview2_debug_port() -> anyhow::Result<()> {
    let ordinary = launch_params("C:\\apps\\visual-test.exe", None)?;
    assert!(chromium_cdp_launch(&ordinary).is_none());
    assert_eq!(webview2_cdp_launch_port(&ordinary)?, None);

    let browser = launch_params("C:\\apps\\chrome.exe", Some(true))?;
    assert!(chromium_cdp_launch(&browser).is_some());
    assert_eq!(webview2_cdp_launch_port(&browser)?, None);
    Ok(())
}

#[test]
fn must_pass_caller_supplied_webview2_port_is_kept() -> anyhow::Result<()> {
    let mut params = launch_params("C:\\apps\\visual-test.exe", Some(true))?;
    params.env.insert(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS".to_owned(),
        "--disable-gpu --remote-debugging-port=9227".to_owned(),
    );
    assert_eq!(webview2_cdp_launch_port(&params)?, Some(9227));
    let spawned = params_with_webview2_launch_env(params, Some(9227), false);
    assert_eq!(
        spawned
            .env
            .get("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            .map(String::as_str),
        Some("--disable-gpu --remote-debugging-port=9227")
    );
    Ok(())
}

#[test]
fn must_fail_invalid_caller_webview2_port_is_refused() -> anyhow::Result<()> {
    for invalid in [
        "--remote-debugging-port=0",
        "--remote-debugging-port=abc",
        "--disable-gpu --remote-debugging-port=70000",
    ] {
        let mut params = launch_params("C:\\apps\\visual-test.exe", Some(true))?;
        params.env.insert(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS".to_owned(),
            invalid.to_owned(),
        );
        let Err(error) = webview2_cdp_launch_port(&params) else {
            panic!("{invalid:?} must be refused, not replaced by a reserved port");
        };
        assert_eq!(
            error.data.as_ref().and_then(|data| data.get("code")),
            Some(&json!(error_codes::TOOL_PARAMS_INVALID)),
            "{invalid:?}: {error:?}"
        );
        assert!(
            error.message.contains("remote-debugging-port"),
            "{invalid:?}: {}",
            error.message
        );
    }
    Ok(())
}
