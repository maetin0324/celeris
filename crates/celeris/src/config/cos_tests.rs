use super::super::Config;
use super::*;

fn load(text: &str) -> Result<Config, ConfigError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.toml");
    let text = format!("{text}\n[[providers]]\nid = \"fake\"\nadapter = \"fake\"\n");
    std::fs::write(&path, text).expect("write config");
    Config::load(&path)
}

#[test]
fn chat_config_defaults_follow_adr_d4() {
    let cfg = load("").expect("empty config loads");
    assert_eq!(cfg.cos.stream_retention_days, 30);
    let limits = cfg.chat_attachment_limits();
    assert_eq!(limits.max_file_bytes, 25 * 1024 * 1024);
    assert_eq!(limits.max_message_bytes, 100 * 1024 * 1024);
    assert_eq!(limits.max_files_per_message, 10);
    assert_eq!(limits.max_storage_bytes, 10 * 1024 * 1024 * 1024);
    assert_eq!(limits.orphan_ttl_hours, 24);
    assert_eq!(limits.unreferenced_retention_days, 30);
    // The config defaults and the store defaults are the same contract.
    let store = ChatAttachmentLimits::default();
    assert_eq!(limits.max_file_bytes, store.max_file_bytes);
    assert_eq!(limits.max_storage_bytes, store.max_storage_bytes);
    assert_eq!(cfg.cos.stream_retention(), time::Duration::days(30));
}

#[test]
fn chat_config_overrides_are_read() {
    let cfg = load(
        "[cos]\nstream_retention_days = 7\n\n[cos.attachments]\nmax_file_bytes = 1024\n\
         max_message_bytes = 4096\nmax_files_per_message = 3\nmax_storage_bytes = 65536\n\
         orphan_ttl_hours = 2\nunreferenced_retention_days = 5\n",
    )
    .expect("overrides load");
    assert_eq!(cfg.cos.stream_retention_days, 7);
    let limits = cfg.chat_attachment_limits();
    assert_eq!(limits.max_file_bytes, 1024);
    assert_eq!(limits.max_message_bytes, 4096);
    assert_eq!(limits.max_files_per_message, 3);
    assert_eq!(limits.max_storage_bytes, 65536);
    assert_eq!(limits.orphan_ttl_hours, 2);
    assert_eq!(limits.unreferenced_retention_days, 5);
}

#[test]
fn chat_config_partial_override_keeps_other_defaults() {
    let cfg = load("[cos.attachments]\nmax_files_per_message = 4\n").expect("partial");
    assert_eq!(cfg.cos.attachments.max_files_per_message, 4);
    assert_eq!(cfg.cos.attachments.max_file_bytes, 25 * 1024 * 1024);
    assert_eq!(cfg.cos.stream_retention_days, 30);
}

#[test]
fn chat_config_rejects_invalid_values() {
    for (text, needle) in [
        (
            "[cos]\nstream_retention_days = 0\n",
            "stream_retention_days",
        ),
        ("[cos.attachments]\nmax_file_bytes = 0\n", "max_file_bytes"),
        (
            "[cos.attachments]\nmax_file_bytes = 2048\nmax_message_bytes = 1024\n",
            "max_message_bytes",
        ),
        (
            "[cos.attachments]\nmax_storage_bytes = 1024\n",
            "max_storage_bytes",
        ),
        (
            "[cos.attachments]\nmax_files_per_message = 0\n",
            "max_files_per_message",
        ),
        (
            "[cos.attachments]\norphan_ttl_hours = 0\n",
            "orphan_ttl_hours",
        ),
        (
            "[cos.attachments]\nunreferenced_retention_days = 0\n",
            "unreferenced_retention_days",
        ),
    ] {
        match load(text) {
            Err(ConfigError::Invalid(msg)) => assert!(msg.contains(needle), "{text}: {msg}"),
            other => panic!("{text}: expected Invalid, got {other:?}"),
        }
    }
}

#[test]
fn chat_config_rejects_unknown_and_negative_keys() {
    assert!(matches!(
        load("[cos.attachments]\nbogus = 1\n"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        load("[cos.attachments]\nmax_file_bytes = -1\n"),
        Err(ConfigError::Parse(_))
    ));
    assert!(matches!(
        load("[cos]\nstream_retention_days = \"30\"\n"),
        Err(ConfigError::Parse(_))
    ));
}

#[test]
fn chat_config_attachment_data_dir_is_db_dir() {
    assert_eq!(
        CosConfig::attachment_data_dir(Path::new("/var/lib/celeris/celeris.db")),
        PathBuf::from("/var/lib/celeris")
    );
    assert_eq!(
        CosConfig::attachment_data_dir(Path::new("celeris.db")),
        PathBuf::from(".")
    );
}

#[test]
fn chat_config_example_toml_carries_the_defaults() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    ));
    let text = std::fs::read_to_string(path).expect("example");
    assert!(text.contains("[cos.attachments]"));
    let cfg = Config::load(path).expect("example loads");
    let example = cfg.chat_attachment_limits();
    let defaults = CosAttachmentsConfig::default().limits();
    assert_eq!(example.max_file_bytes, defaults.max_file_bytes);
    assert_eq!(example.max_message_bytes, defaults.max_message_bytes);
    assert_eq!(
        example.max_files_per_message,
        defaults.max_files_per_message
    );
    assert_eq!(example.max_storage_bytes, defaults.max_storage_bytes);
    assert_eq!(example.orphan_ttl_hours, defaults.orphan_ttl_hours);
    assert_eq!(
        example.unreferenced_retention_days,
        defaults.unreferenced_retention_days
    );
    assert_eq!(cfg.cos.stream_retention_days, 30);
}

#[test]
fn cos_chat_config_defaults_and_unavailable_reason() {
    let cfg = load("").unwrap();
    assert!(cfg.cos.enabled);
    assert_eq!(cfg.cos.harness, CosHarness::ClaudeCode);
    assert_eq!(cfg.cos.tier, Tier::Frontier);
    assert_eq!(cfg.cos.max_turns, 70);
    assert_eq!(cfg.cos.max_wall_secs, 900);
    assert_eq!(cfg.cos.triage.policy_skill, "cos-inbox-triage");
    assert_eq!(cfg.cos.triage.policy_version, "1");
    assert_eq!(cfg.cos.triage.min_confidence, 0.85);
    assert_eq!(cfg.cos.triage.human_required, default_human_required());
    assert_eq!(cfg.cos.triage.unavailable_after_secs, 120);
    let reason = cfg.resolve_cos_provider().unwrap_err();
    assert!(reason.contains("no provider"), "{reason}");
    assert!(reason.contains("claude-code"), "{reason}");
}

#[test]
fn cos_chat_config_rejects_unknown_and_invalid_values() {
    for text in [
        "[cos]\nunknown = 1\n",
        "[cos.triage]\nunknown = 1\n",
        "[cos]\nharness = \"fake\"\n",
        "[cos]\nllm_source = \"invalid\"\n",
    ] {
        assert!(matches!(load(text), Err(ConfigError::Parse(_))), "{text}");
    }
    for (text, needle) in [
        ("[cos]\nllm_source = \"none\"\n", "llm_source"),
        ("[cos]\nllm_source = \"unknown\"\n", "llm_source"),
        ("[cos]\nmax_turns = 0\n", "max_turns"),
        ("[cos.triage]\nmin_confidence = 1.1\n", "min_confidence"),
        (
            "[cos.triage]\nunavailable_after_secs = 0\n",
            "unavailable_after_secs",
        ),
        ("[cos]\nprovider = \"missing\"\n", "provider"),
        (
            "[cos]\nharness = \"codex\"\nllm_source = \"claude_oauth\"\n",
            "llm_source",
        ),
        ("[cos]\nmodel = \"celeris/frontier\"\n", "model"),
    ] {
        let err = load(text).unwrap_err().to_string();
        assert!(err.contains(needle), "{text}: {err}");
    }
}

#[test]
fn cos_chat_config_explicit_provider_conflicts_and_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let provider = "[[providers]]\nid = \"claude\"\nadapter = \"claude-code\"\n[providers.tier_models.frontier]\nname = \"opus\"\nmodel_id = \"claude-opus-5-5\"\n";
    std::fs::write(
        &path,
        format!("[cos]\nprovider = \"claude\"\nllm_source = \"codex_oauth\"\n{provider}"),
    )
    .unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("llm_source")
    );
    std::fs::write(
        &path,
        format!("[cos]\nprovider = \"claude\"\nllm_source = \"claude_oauth\"\n{provider}"),
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let resolved = cfg.resolve_cos_provider().unwrap();
    assert_eq!(resolved.provider, "claude");
    assert_eq!(resolved.llm_source, LlmSourceRef::ClaudeOauth);
    assert_eq!(resolved.model.as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn cos_chat_config_reload_rejects_change_and_preserves_old_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[cos]\nmax_turns = 12\n[[providers]]\nid = \"claude\"\nadapter = \"claude-code\"\n",
    )
    .unwrap();
    let old = Config::load(&path).unwrap();
    std::fs::write(&path, "[cos]\nmax_turns = 99\nprovider = \"claude\"\nharness = \"codex\"\n[[providers]]\nid = \"claude\"\nadapter = \"claude-code\"\n").unwrap();
    assert!(Config::load(&path).is_err());
    assert_eq!(old.cos.max_turns, 12);
    assert_eq!(old.resolve_cos_provider().unwrap().provider, "claude");
}

#[test]
fn cos_chat_config_unavailable_tier_keeps_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[cos]\nprovider = \"claude\"\n[[providers]]\nid = \"claude\"\nadapter = \"claude-code\"\n[providers.tier_models.frontier]\nname = \"frontier\"\nunavailable_reason = \"account cannot access this model\"\n").unwrap();
    let cfg = Config::load(&path).unwrap();
    assert!(
        cfg.resolve_cos_provider()
            .unwrap_err()
            .contains("account cannot access this model")
    );
}
