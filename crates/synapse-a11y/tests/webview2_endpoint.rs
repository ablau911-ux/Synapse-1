#[cfg(windows)]
fn host_browser_args() -> Vec<String> {
    vec![
        "msedgewebview2.exe".to_owned(),
        "--embedded-browser-webview=1".to_owned(),
        "--remote-debugging-port=9227".to_owned(),
    ]
}

#[cfg(windows)]
#[test]
fn must_pass_webview2_endpoint_reads_the_host_browser_process_port() {
    use synapse_a11y::webview2_browser_debug_port;

    assert_eq!(
        webview2_browser_debug_port("msedgewebview2.exe", &host_browser_args()),
        Some(9227)
    );
}

#[cfg(windows)]
#[test]
fn must_fail_webview2_endpoint_uses_only_the_host_owned_browser_process_port() {
    use synapse_a11y::webview2_browser_debug_port;

    let browser = host_browser_args();
    assert_eq!(
        webview2_browser_debug_port(
            "msedgewebview2.exe",
            &[
                browser[0].clone(),
                "--type=renderer".to_owned(),
                browser[2].clone()
            ]
        ),
        None
    );
    assert_eq!(webview2_browser_debug_port("chrome.exe", &browser), None);
    assert_eq!(
        webview2_browser_debug_port(
            "msedgewebview2.exe",
            &["--remote-debugging-port=0".to_owned()]
        ),
        None
    );
}
