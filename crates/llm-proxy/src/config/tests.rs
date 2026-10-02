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
fn cheap_only_default_and_legacy_qwen_config() {
    assert_eq!(
        ModelsConfig::default().qwen,
        HashMap::from([(Tier::Cheap, "qwen3.8-27b".to_string())])
    );
    let legacy: LlmProxyConfig = toml::from_str(
        r#"
[models.qwen]
frontier = "qwen3.8-27b"
standard = "qwen3.8-27b"
cheap = "qwen3.8-27b"
"#,
    )
    .expect("legacy config parses");
    assert_eq!(legacy.models.qwen, ModelsConfig::default().qwen);
}
