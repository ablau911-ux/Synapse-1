//! Regression tests for #25: `act_spawn_agent` refuses an unknown model before
//! creating spawn artifacts, while preserving custom-provider and gateway ids.

use super::*;

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

#[test]
fn must_fail_codex_model_preflight_rejects_unknown_builtin_model() {
    let cache = r#"{"models":[{"slug":"gpt-6-astra"},{"slug":"gpt-5.6-sol"}]}"#;
    let error = validate_codex_model_with_sources(
        "definitely-not-a-model",
        "model = \"gpt-5.6-sol\"\n",
        cache,
    )
    .expect_err("unknown built-in model must fail before spawn");
    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(error.message.contains("before creating spawn artifacts"));
}

#[test]
fn must_fail_codex_model_preflight_malformed_cache_is_refused() {
    let error = validate_codex_model_with_sources("gpt-6-astra", "", "{not json")
        .expect_err("a malformed models_cache.json must refuse, never allow the model");
    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(
        error.message.contains("models_cache.json is malformed"),
        "{error:?}"
    );
}

#[test]
fn must_pass_codex_model_preflight_accepts_catalog_and_configured_models() {
    let cache = r#"{"models":[{"slug":"gpt-6-astra"}]}"#;
    validate_codex_model_with_sources("gpt-6-astra", "", cache).expect("catalog model");
    validate_codex_model_with_sources(
        "private-deployment-v2",
        "# configured explicitly\nmodel = 'private-deployment-v2'\n",
        cache,
    )
    .expect("configured default model");
}

#[test]
fn must_pass_codex_model_preflight_preserves_custom_provider_ids() {
    validate_codex_model_with_sources(
        "organization/custom/model",
        "model_provider = \"internal_gateway\"\n",
        r#"{"models":[]}"#,
    )
    .expect("custom providers own their model namespace");
    assert!(
        codex_model_config_allows(
            "private-model",
            r#"
                model_provider = "openai" # active provider
                [model_providers.openai]
                base_url = "https://gateway.example.test/v1" # custom OpenAI-compatible endpoint
            "#,
        )
        .expect("valid TOML with trailing comments")
    );
    assert!(
        codex_model_config_allows(
            "profile-model",
            r#"
                profile = "work"
                [profiles.work]
                model = "profile-model"
            "#,
        )
        .expect("active profile")
    );
    let home = tempfile::tempdir().expect("temp codex home");
    fs::write(
        home.path().join("config.toml"),
        "model_provider = 'internal_gateway'\n",
    )
    .expect("write custom provider config");
    validate_codex_model_at_home("private-model", home.path())
        .expect("custom provider must not require models_cache.json");
}

#[test]
fn must_fail_claude_model_preflight_rejects_obvious_native_typo() {
    let error = validate_claude_model_with_context("definitely-not-a-model", false, &[])
        .expect_err("unknown native model must fail before spawn");
    assert_eq!(
        error_code(&error),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{error:?}"
    );
    assert!(error.message.contains("before creating spawn artifacts"));
    let nonsense = validate_claude_model_with_context("claude-sonnet-doomed-9", false, &[])
        .expect_err("family-like nonsense without a numeric version must fail");
    assert_eq!(
        error_code(&nonsense),
        Some(error_codes::TOOL_PARAMS_INVALID),
        "{nonsense:?}"
    );
    assert!(
        nonsense.message.contains("claude-sonnet-doomed-9"),
        "the refusal must name the rejected model: {nonsense:?}"
    );
}

#[test]
fn must_pass_claude_model_preflight_preserves_native_and_custom_models() {
    validate_claude_model_with_context("sonnet", false, &[]).expect("native alias");
    validate_claude_model_with_context("sonnet[1m]", false, &[]).expect("1m alias");
    validate_claude_model_with_context("opusplan", false, &[]).expect("plan alias");
    validate_claude_model_with_context("claude-opus-4-8", false, &[]).expect("native full id");
    validate_claude_model_with_context("claude-3-5-sonnet-20241022", false, &[])
        .expect("legacy version-first id");
    validate_claude_model_with_context("glm-5.3", true, &[]).expect("custom provider id");
    validate_claude_model_with_context(
        "managed-deployment",
        false,
        &["managed-deployment".to_owned()],
    )
    .expect("configured provider id");
}

#[test]
fn must_pass_claude_model_preflight_reads_supported_settings_mappings() {
    let settings = json!({
        "model": "team-default",
        "fallbackModel": "team-fallback",
        "availableModels": ["managed-allowed-model"],
        "modelOverrides": {
            "claude-opus-4-8": "arn:aws:bedrock:example"
        },
        "modelPicker": {
            "options": [
                {
                    "model": "gateway-model-v3",
                    "behavesAs": "claude-sonnet-5"
                },
                {
                    "model": "unmapped-unknown-row"
                }
            ]
        }
    });
    let mut env = BTreeMap::new();
    let mut models = BTreeSet::new();
    merge_claude_model_settings(&settings, &mut env, &mut models);
    for expected in [
        "team-default",
        "team-fallback",
        "managed-allowed-model",
        "claude-opus-4-8",
        "arn:aws:bedrock:example",
        "gateway-model-v3",
    ] {
        assert!(
            models.contains(expected),
            "missing configured model {expected}"
        );
    }
    assert!(!models.contains("unmapped-unknown-row"));
}

#[test]
fn must_pass_openai_base_url_gateway_skips_the_builtin_model_allowlist() {
    assert!(is_custom_openai_base_url(Some(
        "https://gateway.example.test/v1"
    )));
    assert!(is_custom_openai_base_url(Some("  http://127.0.0.1:4000  ")));
    assert!(!is_custom_openai_base_url(None));
    assert!(!is_custom_openai_base_url(Some("")));
    assert!(!is_custom_openai_base_url(Some("   ")));
    assert!(!is_custom_openai_base_url(Some(
        "https://API.OPENAI.com/v1"
    )));
}
