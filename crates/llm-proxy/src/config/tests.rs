use super::*;

#[test]
fn effective_enabled_is_false_without_any_source() {
    let cfg = LlmProxyConfig::default();
    assert!(!cfg.effective_enabled());
}

#[test]
fn effective_enabled_is_true_with_a_relay_source() {
    let mut cfg = LlmProxyConfig::default();
    cfg.sources.openai_compatible.push(OpenAiCompatibleConfig {
        id: "qwen".into(),
        base_url: "http://127.0.0.1:18000/v1".into(),
        api_key: None,
        enabled: true,
    });
    assert!(cfg.effective_enabled());
}

#[test]
fn explicit_enabled_overrides_auto_detection() {
    let mut cfg = LlmProxyConfig {
        enabled: Some(false),
        ..LlmProxyConfig::default()
    };
    cfg.sources.openai_compatible.push(OpenAiCompatibleConfig {
        id: "qwen".into(),
        base_url: "http://127.0.0.1:18000/v1".into(),
        api_key: None,
        enabled: true,
    });
    assert!(!cfg.effective_enabled());
}

#[test]
fn proxy_tier_alias_rejected_models_table_is_a_load_error_with_the_new_form() {
    for models in [
        serde_json::json!({ "qwen": { "cheap": "qwen3.8-27b" } }),
        serde_json::json!({ "claude": { "standard": "claude-opus-5-5" } }),
        serde_json::json!({}),
    ] {
        let err = serde_json::from_value::<LlmProxyConfig>(serde_json::json!({ "models": models }))
            .expect_err("[llm_proxy.models] must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("[llm_proxy.models]"), "{msg}");
        assert!(msg.contains("<source_id>/<model_id>"), "{msg}");
        assert!(msg.contains("qwen/qwen3.8-27b"), "{msg}");
    }
}
