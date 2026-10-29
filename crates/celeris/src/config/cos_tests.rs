use super::super::Config;
use super::*;

fn load(text: &str) -> Result<Config, ConfigError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.toml");
    let text = format!("{text}\n");
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
fn cos_chat_config_defaults_to_fixed_harness_and_route_candidates() {
    let cfg = load("").unwrap();
    assert!(cfg.cos.enabled);
    assert_eq!(cfg.cos.harness, CosHarness::ClaudeCode);
    assert!(cfg.cos.route_candidates.is_empty());
    assert_eq!(cfg.cos.max_turns, 70);
    assert_eq!(cfg.cos.max_wall_secs, 900);
    assert_eq!(cfg.cos.triage.policy_skill, "cos-inbox-triage");
}

/// ADR 2026-10-28-cos-routine-delegated-answers D3: the escalation vocabulary and policy version 5.
#[test]
fn cos_triage_human_required_uses_the_routine_answer_vocabulary() {
    let cfg = load("").unwrap();
    assert_eq!(cfg.cos.triage.policy_version, "6");
    assert_eq!(
        cfg.cos.triage.human_required,
        [
            "objective_change",
            "beyond_frame",
            "external_publish",
            "destructive",
            "secret_or_security",
            "standing_permission",
            "explicit_human",
        ]
    );
    for old in [
        "fundamental_change",
        "resource_overrun",
        "security",
        "unknown",
    ] {
        let text = format!("[cos.triage]\nhuman_required = [\"{old}\"]\n");
        let err = load(&text).expect_err(old).to_string();
        assert!(err.contains("unknown reason"), "{old}: {err}");
    }
    assert!(load("[cos.triage]\nhuman_required = [\"beyond_frame\"]\n").is_ok());
}

#[test]
fn cos_chat_config_rejects_legacy_route_fields() {
    for text in [
        "[cos]\nprovider = \"missing\"\n",
        "[cos]\ntier = \"frontier\"\n",
        "[cos]\nmodel = \"old-model\"\n",
        "[cos]\nllm_source = \"claude_oauth\"\n",
        "[[cos.fallbacks]]\nharness = \"codex\"\n",
    ] {
        assert!(matches!(load(text), Err(ConfigError::Parse(_))), "{text}");
    }
    assert!(load("[cos]\nmax_turns = 0\n").is_err());
}

/// ADR 2026-10-29-cos-authorization-fallback-after-triage D2: the safety net of an authorization
/// item defaults to `max_wall_secs + 300`, follows `max_wall_secs`, and an explicit value wins.
#[test]
fn cos_triage_authorization_safety_net_default_and_override() {
    let cfg = load("").expect("empty config loads");
    assert_eq!(cfg.cos.triage.authorization_unavailable_after_secs, None);
    assert_eq!(cfg.cos.authorization_unavailable_after_secs(), 1200);
    let cfg = load("[cos]\nmax_wall_secs = 1800\n").expect("loads");
    assert_eq!(cfg.cos.authorization_unavailable_after_secs(), 2100);
    let cfg = load("[cos.triage]\nunavailable_after_secs = 5000\n").expect("loads");
    assert_eq!(
        cfg.cos.authorization_unavailable_after_secs(),
        5000,
        "never shorter than unavailable_after_secs"
    );
    let cfg = load("[cos.triage]\nauthorization_unavailable_after_secs = 3600\n").expect("loads");
    assert_eq!(cfg.cos.authorization_unavailable_after_secs(), 3600);
    let err = load("[cos.triage]\nauthorization_unavailable_after_secs = 120\n")
        .expect_err("not longer than unavailable_after_secs");
    assert!(
        err.to_string()
            .contains("authorization_unavailable_after_secs must be greater"),
        "{err}"
    );
    let example = Config::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    )))
    .expect("example loads");
    assert_eq!(example.cos.authorization_unavailable_after_secs(), 1200);
}
