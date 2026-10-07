use super::*;

fn args(root: &Path) -> RootArgs {
    RootArgs {
        root: Some(root.to_path_buf()),
        // 実ホームの設定を読ませない（テストは `~/.local/share/celeris/knowledge` に触らない）。
        config: Some(PathBuf::from("/nonexistent/celeris.toml")),
    }
}

/// `init` → `record` → `search` → `get` が tempdir の KB だけで完結する（DB もネットワークも使わない）。
#[test]
fn the_cli_works_on_a_temporary_knowledge_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    assert_eq!(run_init(&args(&root)).expect("init"), ExitCode::SUCCESS);
    assert!(root.join("user/profile.md").exists());
    // 2 回目も成功する（冪等）。
    assert_eq!(run_init(&args(&root)).expect("again"), ExitCode::SUCCESS);

    // `search` は KB が無ければエラー（黙って空にしない）。
    let missing = dir.path().join("nope");
    assert!(
        run_search(&SearchArgs {
            query: vec!["x".into()],
            scope: None,
            limit: 10,
            json: false,
            root: args(&missing),
        })
        .is_err()
    );

    assert_eq!(
        run_search(&SearchArgs {
            query: vec!["pegasus".into()],
            scope: Some("environment".into()),
            limit: 5,
            json: true,
            root: args(&root),
        })
        .expect("search"),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run_get(&GetArgs {
            path: "user/profile.md".into(),
            json: true,
            root: args(&root),
        })
        .expect("get"),
        ExitCode::SUCCESS
    );
    // 根の外は読めない。
    assert!(
        run_get(&GetArgs {
            path: "../../etc/passwd".into(),
            json: false,
            root: args(&root),
        })
        .is_err()
    );
}

/// ADR-0052 D3: `knowledge rerun <task_id>` は `knowledge_runs` の 1 行を
/// 「まだやり直していない失敗」に戻す（run を作るのはデーモンの次の tick）。
#[test]
fn rerun_resets_the_knowledge_run_row_and_rejects_unknown_ids() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task_id = TaskId::new();
    let run_task_id = TaskId::new();
    let now = time::OffsetDateTime::now_utc();
    store
        .knowledge_run_create(task_id, run_task_id, now)
        .expect("create");
    store
        .knowledge_run_finish(
            task_id,
            task_core::KnowledgeRunState::Done,
            now,
            Some(&task_core::KnowledgeRunSummary::default()),
            Some(task_core::VIA_LANGMEM),
        )
        .expect("finish");
    store
        .knowledge_run_retry(task_id, TaskId::new(), now)
        .expect("retry");

    assert_eq!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: task_id.to_string()
            }
        )
        .expect("rerun"),
        ExitCode::SUCCESS
    );
    let run = store
        .knowledge_run_get(task_id)
        .expect("get")
        .expect("some");
    assert_eq!(run.state, task_core::KnowledgeRunState::Failed);
    assert!(run.retried_at.is_none(), "もう 1 回だけ自動でやり直せる");
    assert!(run.via.is_none());

    // 知識整理 run が無いタスク・id の綴り間違いはエラー（黙って成功しない）。
    assert!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: TaskId::new().to_string()
            }
        )
        .is_err()
    );
    assert!(
        run_rerun(
            &store,
            &RerunArgs {
                task_id: "not-an-ulid".into()
            }
        )
        .is_err()
    );
    // `needs_db` は rerun だけ真。
    assert!(
        KnowledgeCommand::Rerun(RerunArgs {
            task_id: task_id.to_string()
        })
        .needs_db()
    );
    assert!(!KnowledgeCommand::Reindex(RootArgs::default()).needs_db());
}

/// `--root` が最優先。設定が読めなければ既定（`~/.local/share/celeris/knowledge`）に落ちる。
#[test]
fn the_root_flag_wins_over_an_unreadable_config() {
    let explicit = PathBuf::from("/tmp/celerisctl-kb-test");
    assert_eq!(
        root_of(&RootArgs {
            root: Some(explicit.clone()),
            config: Some(PathBuf::from("/nonexistent/celeris.toml")),
        }),
        explicit
    );
    let fallback = root_of(&RootArgs {
        root: None,
        config: Some(PathBuf::from("/nonexistent/celeris.toml")),
    });
    assert!(fallback.ends_with("knowledge"), "{}", fallback.display());
}

/// ADR 2026-10-07 cos-live-fixes D2: CoS の `record` は KB を書かず、`POST /api/v1/knowledge/inbox`
/// を run credential で `/cos/operations` に包んで呼ぶ（偽 server。外部ネットワークなし）。
#[test]
fn cos_live_fix_d2_cli_record_with_cos_credential_calls_the_api() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    let (api, server) = super::super::cos_ops::tests::fake_server();
    let args = RecordArgs {
        title: "fern03 の使い方".into(),
        scope: "project:agent-platform".into(),
        tags: vec!["fern03".into()],
        sources: vec!["message:01MSG".into()],
        confidence: Some("high".into()),
        path: None,
        attachment_ids: vec!["01ATTACHMENT".into()],
        json: false,
        root: args(&root),
    };
    let options = super::super::cos_ops::Options::default();
    let result = record_via_cos(
        &api,
        &args,
        "PDF の要点。".into(),
        &options,
        "celeris-cos-run.secret",
    )
    .expect("record via api");
    assert_eq!(result["ok"], true);
    let (headers, body) = server.join().expect("server");
    assert!(
        headers.starts_with("POST /api/v1/cos/operations HTTP/1.1"),
        "{headers}"
    );
    assert!(headers.contains("Authorization: Bearer celeris-cos-run.secret"));
    assert!(!headers.contains("admin-secret"));
    assert_eq!(body["request"]["method"], "POST");
    assert_eq!(body["request"]["path"], "/api/v1/knowledge/inbox");
    let request = &body["request"]["body"];
    assert_eq!(request["title"], "fern03 の使い方");
    assert_eq!(request["scope"], "project:agent-platform");
    assert_eq!(request["body"], "PDF の要点。");
    assert_eq!(request["sources"], serde_json::json!(["message:01MSG"]));
    assert_eq!(request["confidence"], "high");
    assert_eq!(
        request["attachment_ids"],
        serde_json::json!(["01ATTACHMENT"])
    );
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| !r.trim().is_empty())
    );
    assert!(
        body["idempotency_key"]
            .as_str()
            .is_some_and(|k| k.starts_with("knowledge-record-"))
    );
    // KB の根には何も作らない。
    assert!(!root.exists());
}

/// 同じ題名・本文の再送は同じ冪等キー（CoS の再試行で候補が 2 つにならない）。
#[test]
fn cos_live_fix_d2_cli_idempotency_key_is_stable() {
    let a = fnv1a(&[b"t", b"\n", b"body"]);
    assert_eq!(a, fnv1a(&[b"t", b"\n", b"body"]));
    assert_ne!(a, fnv1a(&[b"t", b"\n", b"other"]));
}

/// CoS でない `record` は `--attachment-id` を受けない（pin は API の transaction でしか書けない）。
#[test]
fn cos_live_fix_d2_cli_direct_record_refuses_attachment_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    let error = run_record(&RecordArgs {
        title: "x".into(),
        scope: "user".into(),
        tags: vec![],
        sources: vec!["task:01X".into()],
        confidence: None,
        path: None,
        attachment_ids: vec!["01A".into()],
        json: false,
        root: args(&root),
    })
    .expect_err("refused");
    assert!(error.to_string().contains("--attachment-id"), "{error}");
}
