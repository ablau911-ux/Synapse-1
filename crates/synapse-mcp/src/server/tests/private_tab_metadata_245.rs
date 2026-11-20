//! Child of m1_tools. Deserialize actual bridge wire input: catches discarded
//! incognito flag without referencing a field missing from main's Rust types.
use super::*;

fn tab(private: bool) -> crate::chrome_debugger_bridge::ChromeDebuggerTabTarget {
    serde_json::from_value(json!({
        "target_id":"chrome-tab:245", "tab_id":245, "chrome_window_id":7,
        "target_type":"page", "url":"https://private.example/SECRET_PATH_245",
        "title":"SECRET_TITLE_245", "incognito":private, "active":true
    }))
    .expect("bridge wire fixture")
}

#[test]
fn must_pass_ordinary_tab_retains_identity_and_ordinary_redaction() {
    let input = tab(false);
    assert_eq!(input.title, "SECRET_TITLE_245");
    let entry = browser_tab_entry(42, input);
    assert_eq!(entry.title, "SECRET_TITLE_245");
    assert_eq!(entry.url, "https://private.example/redacted");
    assert_eq!(entry.tab_id, 245);
    assert_eq!(entry.chrome_window_id, Some(7));
}

#[test]
fn must_fail_incognito_metadata_cannot_escape_list_or_mutation_readback() {
    let input = tab(true);
    assert_eq!(input.title, "SECRET_TITLE_245");
    let entry = browser_tab_entry(42, input);
    for entry in [entry.clone(), redact_browser_tab_entry_url(entry)] {
        let wire = serde_json::to_value(entry).expect("serialized entry");
        assert_eq!(wire["incognito"], json!(true));
        assert_eq!(wire["url"], json!("private:redacted"));
        assert_eq!(wire["title"], json!("Private tab (redacted)"));
        assert!(!wire.to_string().contains("SECRET_"));
        assert!(!wire.to_string().contains("private.example"));
    }
}
