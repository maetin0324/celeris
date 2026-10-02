use super::*;

/// ADR-0074 §6 F3 (c)（途中確認）: 工程の後の途中確認（`blocked(awaiting_human)`）は
/// `PhaseCheckpoint` として 1 回だけ鳴り、`QuestionBlocked` は鳴らない。質問で止まった Task は
/// 従来どおり `QuestionBlocked` だけ。
#[test]
fn phase_checkpoint_is_not_a_question() {
    use task_core::{
        Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, Tier, Trigger, WorkerHint,
        WorkspaceSpec,
    };
    fn task(title: &str) -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            expected_write_paths: None,
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: title.into(),
            objective: "o".into(),
            acceptance: vec![Criterion {
                text: "c".into(),
                check: Check::Human,
            }],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "ws".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 1,
                max_wall_secs: 1,
                max_retries: 0,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        }
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let store =
        SqliteStore::open(&dir.path().join("celeris.db")).unwrap_or_else(|e| panic!("open: {e}"));
    let paused = task("二工程の仕事");
    store.insert(&paused).unwrap_or_else(|e| panic!("{e}"));
    store
        .apply_transition(paused.id, Trigger::Dispatch, None)
        .unwrap_or_else(|e| panic!("{e}"));
    store
        .apply_transition_with_events(
            paused.id,
            Trigger::PhaseGate {
                phase: "design".into(),
            },
            vec![task_core::Event::PhaseReported {
                phase: "design".into(),
                report: Box::new(task_core::PhaseReport {
                    phase: "design".into(),
                    phase_title: "設計".into(),
                    next_phase: Some("build".into()),
                    ..Default::default()
                }),
            }],
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let asked = task("質問の仕事");
    store.insert(&asked).unwrap_or_else(|e| panic!("{e}"));
    store
        .apply_transition(asked.id, Trigger::Dispatch, None)
        .unwrap_or_else(|e| panic!("{e}"));
    store
        .apply_transition(asked.id, Trigger::WorkerQuestion, None)
        .unwrap_or_else(|e| panic!("{e}"));

    let started = OffsetDateTime::now_utc() - time::Duration::hours(1);
    let schedule = || {
        schedule(&store, &NotifyConfig::default(), started, started)
            .unwrap_or_else(|e| panic!("schedule: {e}"))
    };
    let first = schedule();
    let checkpoints: Vec<&Notification> = first
        .iter()
        .filter(|n| n.kind == NotificationKind::PhaseCheckpoint)
        .collect();
    assert_eq!(checkpoints.len(), 1, "{first:?}");
    assert!(
        checkpoints[0].key.starts_with(&paused.id.to_string()),
        "{:?}",
        checkpoints[0]
    );
    assert!(
        checkpoints[0]
            .body
            .contains("『二工程の仕事』が工程『設計』まで進みました。続ける / replan / 取り下げ"),
        "{}",
        checkpoints[0].body
    );
    let questions: Vec<&Notification> = first
        .iter()
        .filter(|n| n.kind == NotificationKind::QuestionBlocked)
        .collect();
    assert_eq!(questions.len(), 1, "{first:?}");
    assert!(
        questions[0].key.starts_with(&asked.id.to_string()),
        "途中確認の Task は QuestionBlocked を鳴らさない: {questions:?}"
    );
    let second = schedule();
    assert!(
        !second
            .iter()
            .any(|n| n.kind == NotificationKind::PhaseCheckpoint),
        "2 回目の tick では鳴らない: {second:?}"
    );
}

#[test]
fn excerpt_cuts_by_characters_and_flattens_newlines() {
    assert_eq!(excerpt("あいうえお", 3), "あいう…");
    assert_eq!(excerpt("あいう", 3), "あいう");
    assert_eq!(excerpt("a\n b", 10), "a b");
}

#[test]
fn links_are_omitted_without_a_base_url() {
    assert_eq!(link(None, "/approvals"), "");
    assert_eq!(
        link(Some("http://h:7700"), "/approvals"),
        "\nhttp://h:7700/approvals"
    );
}

#[test]
fn base_url_drops_the_trailing_slash_and_treats_empty_as_absent() {
    let config = NotifyConfig {
        gui_base_url: Some("http://h:7700/".into()),
        ..NotifyConfig::default()
    };
    assert_eq!(config.base_url(), Some("http://h:7700"));
    let empty = NotifyConfig {
        gui_base_url: Some(String::new()),
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
    assert!(config.gui_base_url.is_none());
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
