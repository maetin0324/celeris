use super::*;

#[test]
fn excerpt_cuts_by_characters_and_flattens_newlines() {
    assert_eq!(excerpt("あいうえお", 3), "あいう…");
    assert_eq!(excerpt("あいう", 3), "あいう");
    assert_eq!(excerpt("a\n b", 10), "a b");
}

#[test]
fn base_url_drops_the_trailing_slash_and_treats_empty_as_absent() {
    let config = NotifyConfig {
        web_base_url: Some("http://h:7720/".into()),
        ..NotifyConfig::default()
    };
    assert_eq!(config.base_url(), Some("http://h:7720"));
    let empty = NotifyConfig {
        web_base_url: Some(String::new()),
        ..NotifyConfig::default()
    };
    assert_eq!(empty.base_url(), None);
    assert_eq!(NotifyConfig::default().base_url(), None);
}

#[test]
fn defaults_match_the_adr() {
    let config = NotifyConfig::default();
    assert_eq!(config.discord_webhook_secret, "discord-webhook");
    assert_eq!(config.interval_secs, 30);
    assert_eq!(config.inbox_batch_secs, 60);
    assert_eq!(config.inbox_reminder_secs, 86_400);
    assert_eq!(config.digest_interval_secs, 3_600);
    assert_eq!(config.digest_max_lines, 10);
    assert!(config.web_base_url.is_none());
}

#[test]
fn content_is_clamped_to_one_discord_message() {
    let long = "あ".repeat(5_000);
    assert_eq!(clamp_content(&long).chars().count(), CONTENT_MAX_CHARS);
}

#[test]
fn webhook_url_is_none_without_a_secrets_dir_or_file() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    assert_eq!(webhook_url(None, "discord-webhook"), None);
    assert_eq!(webhook_url(Some(dir.path()), "discord-webhook"), None);
    std::fs::write(
        dir.path().join("discord-webhook"),
        "https://example.invalid/hook\n",
    )
    .unwrap_or_else(|e| panic!("write: {e}"));
    assert_eq!(
        webhook_url(Some(dir.path()), "discord-webhook").as_deref(),
        Some("https://example.invalid/hook")
    );
    // 空の秘密は「無い」と同じ扱い。
    std::fs::write(dir.path().join("empty"), "\n").unwrap_or_else(|e| panic!("write: {e}"));
    assert_eq!(webhook_url(Some(dir.path()), "empty"), None);
}

#[test]
fn notify_rejects_retired_link_config() {
    let error = toml::from_str::<NotifyConfig>("gui_base_url = 'http://localhost:7700'")
        .expect_err("retired setting must not be silently ignored");
    assert!(error.to_string().contains("unknown field `gui_base_url`"));
    assert!(error.to_string().contains("web_base_url"));
}

/// ADR 2026-10-15-root-done-cos-summary D5: the root-done summary is sent as the text the dispatcher fixed
/// (CoS summary + links); no source wait is re-checked, and only that kind plus the two CoS kinds are selected.
#[test]
fn root_done_summary_is_selected_and_rendered_as_is() {
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    let at = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
    let row = store
        .notification_upsert_pending(
            NotificationKind::RootDoneSummary,
            "t:1",
            "完了: x\nまとめ\nスレッド: http://web/?thread=a",
            None,
            at,
        )
        .unwrap()
        .unwrap();
    let batch = select_routes_batch(std::slice::from_ref(&row)).expect("selected");
    assert_eq!(batch.ids, vec![row.id]);
    let text = triage::render(&row, &NotifyConfig::default()).unwrap();
    assert_eq!(text, row.body);
    // The same (kind, key) is never created twice.
    assert!(
        store
            .notification_upsert_pending(
                NotificationKind::RootDoneSummary,
                "t:1",
                "again",
                None,
                at
            )
            .unwrap()
            .is_none()
    );
}
