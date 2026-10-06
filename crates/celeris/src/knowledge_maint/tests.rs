use super::*;
use task_core::org::{OrgKind, OrgNode};
use task_core::report::{Report, ReportId, ReportKind, ReportStore};
use task_core::{
    Budget, KnowledgeRunStore, ListFilter, ListOrder, Project, ProjectId, ProjectStatus,
    SqliteStore, TaskId as CoreTaskId, Tier, WorkerHint, WorkspaceSpec,
};

fn store_with_node() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "coding".into(),
            parent_id: None,
            name: "coding".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .expect("seed org");
    store
}

fn seed_project(store: &SqliteStore, archived: bool) -> ProjectId {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: if archived { Some(now) } else { None },
        paused_from: None,
        id: ProjectId::new(),
        title: "案件".into(),
        request: "依頼".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    project.id
}

#[allow(clippy::too_many_arguments)]
fn terminal_task(
    status: Status,
    assignee: Option<&str>,
    role: Option<&str>,
    project_id: Option<ProjectId>,
    conversation: bool,
) -> task_core::Task {
    let now = OffsetDateTime::now_utc();
    task_core::Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: CoreTaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "pegasus の初期セットアップ".into(),
        objective: "pegasus に pjsub の使い方を確認する".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 1,
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
        role: role.map(str::to_string),
        genre: None,
        aggregate: false,
        project_id,
        milestone_id: None,
        assignee: assignee.map(str::to_string),
        conversation: if conversation {
            Some(task_core::MessageId::new())
        } else {
            None
        },
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn add_report(store: &SqliteStore, task: &task_core::Task) {
    store
        .report_append(&Report {
            id: ReportId::new(),
            project_id: task.project_id,
            node_id: task.assignee.clone().unwrap_or_default(),
            task_id: Some(task.id),
            kind: ReportKind::Result,
            level: 0,
            headline: "pjsub の投げ方を確認した".into(),
            body: "pjsub -L node=1 で投げられる。".into(),
            sources: Vec::new(),
            read_at: None,
            created_at: task.updated_at,
        })
        .expect("append report");
}

fn kb_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    task_ops::knowledge::init(&root).expect("init");
    (dir, root)
}

/// 終端になったタスク（完了・失敗・中止・会話・アーカイブ案件の仕事）から知識整理 task を作らない。
/// 知識整理は日次 job に寄せた（ADR-0131 付記 D10）ので、この経路には作る関数が無い。
/// `retry_failed` と `apply_finished` も、知識整理 run の無い状態では何も作らないことを確かめる。
#[test]
fn terminal_tasks_do_not_get_a_knowledge_task_any_more() {
    let store = store_with_node();
    let (_dir, root) = kb_dir();
    let workspace_root = tempfile::tempdir().expect("workspace");
    let archived_project = seed_project(&store, true);
    let tasks = [
        terminal_task(Status::Done, Some("coding"), None, None, false),
        terminal_task(Status::Failed, Some("coding"), None, None, false),
        terminal_task(Status::Cancelled, Some("coding"), None, None, false),
        terminal_task(Status::Done, Some("coding"), None, None, true),
        terminal_task(
            Status::Done,
            Some("coding"),
            None,
            Some(archived_project),
            false,
        ),
    ];
    for task in &tasks {
        store.insert(task).expect("insert");
        add_report(&store, task);
    }
    let before = store
        .list_page(&ListFilter::default(), ListOrder::UpdatedDesc, None, 100)
        .expect("list")
        .items
        .len();

    let now = OffsetDateTime::now_utc();
    assert_eq!(
        retry_failed(&store, &root, true, 10, None, &[], &[], now)
            .expect("retry")
            .len(),
        0
    );
    assert_eq!(
        apply_finished(&store, &root, workspace_root.path(), now).expect("apply"),
        0
    );

    let after = store
        .list_page(&ListFilter::default(), ListOrder::UpdatedDesc, None, 100)
        .expect("list")
        .items;
    assert_eq!(after.len(), before, "知識整理 task は増えない");
    assert!(
        after
            .iter()
            .all(|t| t.role.as_deref() != Some(report::KNOWLEDGE_ROLE)),
        "知識整理の支援 task は 1 件も無い"
    );
    for task in &tasks {
        assert!(
            store.knowledge_run_get(task.id).expect("get").is_none(),
            "終端 task に knowledge_runs の行は付かない"
        );
    }
}

/// ADR-0047 D4: `done` の知識整理 run は `artifacts/knowledge-candidates.json` を読んで適用し、
/// `knowledge_runs` を `done` にする。`failed` の run は候補を読まず `failed` にする。
#[test]
fn apply_finished_applies_a_done_run_and_fails_a_failed_run() {
    let store = store_with_node();
    let (_dir, root) = kb_dir();
    let workspace_root = tempfile::tempdir().expect("workspace");

    let source = terminal_task(Status::Done, Some("coding"), None, None, false);
    store.insert(&source).expect("insert");
    add_report(&store, &source);

    // done の run: `<workspace_root>/<run_task_id>/artifacts/knowledge-candidates.json` を用意する。
    let done_run_task = terminal_task(
        Status::Done,
        Some("coding"),
        Some(report::KNOWLEDGE_ROLE),
        None,
        false,
    );
    store.insert(&done_run_task).expect("insert");
    let artifacts_dir = workspace_root
        .path()
        .join(done_run_task.id.to_string())
        .join("artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("mkdir");
    std::fs::write(
        artifacts_dir.join("knowledge-candidates.json"),
        r#"{"candidates": [{"op": "create", "path": "environment/tools/newtool.md",
                "title": "newtool", "tags": [], "scope": "environment", "body": "使い方。",
                "sources": ["task:x"], "confidence": "high"}]}"#,
    )
    .expect("write candidates");
    store
        .knowledge_run_create(source.id, done_run_task.id, OffsetDateTime::now_utc())
        .expect("create run");

    // failed の run（別の元タスク）。
    let source2 = terminal_task(Status::Done, Some("coding"), None, None, false);
    store.insert(&source2).expect("insert");
    add_report(&store, &source2);
    let failed_run_task = terminal_task(
        Status::Failed,
        Some("coding"),
        Some(report::KNOWLEDGE_ROLE),
        None,
        false,
    );
    store.insert(&failed_run_task).expect("insert");
    store
        .knowledge_run_create(source2.id, failed_run_task.id, OffsetDateTime::now_utc())
        .expect("create run");

    let now = OffsetDateTime::now_utc();
    let applied = apply_finished(&store, &root, workspace_root.path(), now).expect("apply");
    assert_eq!(applied, 1, "done の run だけ数える");

    let done = store
        .knowledge_run_get(source.id)
        .expect("get")
        .expect("some");
    assert_eq!(done.state, KnowledgeRunState::Done);
    let summary = done.summary.expect("summary");
    assert_eq!(summary.ingested, 1);
    assert!(root.join("environment/tools/newtool.md").exists());

    let failed = store
        .knowledge_run_get(source2.id)
        .expect("get")
        .expect("some");
    assert_eq!(failed.state, KnowledgeRunState::Failed);
    assert!(failed.summary.is_none());

    // 未終端の run は触らない。
    let source3 = terminal_task(Status::Done, Some("coding"), None, None, false);
    store.insert(&source3).expect("insert");
    add_report(&store, &source3);
    let running_run_task = terminal_task(
        Status::Running,
        Some("coding"),
        Some(report::KNOWLEDGE_ROLE),
        None,
        false,
    );
    store.insert(&running_run_task).expect("insert");
    store
        .knowledge_run_create(source3.id, running_run_task.id, OffsetDateTime::now_utc())
        .expect("create run");
    assert_eq!(
        apply_finished(&store, &root, workspace_root.path(), now).expect("apply"),
        0
    );
    assert_eq!(
        store
            .knowledge_run_get(source3.id)
            .expect("get")
            .expect("some")
            .state,
        KnowledgeRunState::Scheduled
    );
}

/// run タスクに `WorkerStarted` を 1 つ書く（`via` の判定材料）。
fn started_with(store: &SqliteStore, task: &task_core::Task, adapter: &str) {
    store
        .append_event(
            task.id,
            &task_core::Event::WorkerStarted {
                run_id: ulid::Ulid::new().to_string(),
                adapter: adapter.to_string(),
                model: "m".into(),
                provider: Some("p".into()),
                account: None,
                role: None,
                task_role: task.role.clone(),
            },
        )
        .expect("append");
}

/// 元タスク 1 件ぶんの「失敗した知識整理 run」を作る（報告つき）。
fn failed_run(store: &SqliteStore) -> (task_core::Task, task_core::Task) {
    let source = terminal_task(Status::Done, Some("coding"), None, None, false);
    store.insert(&source).expect("insert");
    add_report(store, &source);
    let run_task = terminal_task(
        Status::Failed,
        Some("coding"),
        Some(report::KNOWLEDGE_ROLE),
        None,
        false,
    );
    store.insert(&run_task).expect("insert");
    store
        .knowledge_run_create(source.id, run_task.id, OffsetDateTime::now_utc())
        .expect("create run");
    (source, run_task)
}

/// ADR-0052 D3: 失敗した知識整理 run は次の tick で**ちょうど 1 回**だけ作り直される（3 回目は無い）。
/// `not_before`（backfill 禁止）は見ない ＝ 実機で 2026-09-20/21 に落ちた古い run も拾える。
#[test]
fn a_failed_run_is_retried_exactly_once() {
    let store = store_with_node();
    let (_dir, root) = kb_dir();
    let workspace_root = tempfile::tempdir().expect("workspace");
    let (source, first_run) = failed_run(&store);
    started_with(&store, &first_run, LANGMEM_ADAPTER);

    let now = OffsetDateTime::now_utc();
    apply_finished(&store, &root, workspace_root.path(), now).expect("apply");
    let run = store
        .knowledge_run_get(source.id)
        .expect("get")
        .expect("some");
    assert_eq!(run.state, KnowledgeRunState::Failed);
    assert_eq!(run.via.as_deref(), Some(task_core::VIA_LANGMEM));
    assert!(run.retried_at.is_none());

    // 1 回目のやり直し: 新しい run タスクができ、`retried_at` が入る。
    let retried = retry_failed(&store, &root, true, 10, None, &[], &[], now).expect("retry_failed");
    assert_eq!(retried.len(), 1, "1 tick に 1 件");
    let second_run_id = retried[0];
    assert_ne!(second_run_id, first_run.id);
    let run = store
        .knowledge_run_get(source.id)
        .expect("get")
        .expect("some");
    assert_eq!(run.run_task_id, second_run_id);
    assert_eq!(run.state, KnowledgeRunState::Scheduled);
    assert!(run.retried_at.is_some());
    assert!(run.via.is_none(), "やり直しで経路の記録は消える");
    let second = store.get(second_run_id).expect("get").expect("some");
    assert_eq!(second.worker_hint.adapter.as_deref(), Some(LANGMEM_ADAPTER));
    assert_eq!(
        task_core::report::support_kind(&second),
        Some("knowledge"),
        "やり直しも裏方の支援タスク"
    );

    // まだ終わっていないので 2 回目の呼び出しでは何も起きない。
    assert!(
        retry_failed(&store, &root, true, 10, None, &[], &[], now)
            .expect("retry again")
            .is_empty()
    );

    // 2 回目も失敗した（`apply_finished` が `failed` にする）→ もう作り直さない（`retried_at` あり）。
    store
        .knowledge_run_finish(source.id, KnowledgeRunState::Failed, now, None, None)
        .expect("finish");
    assert!(
        retry_failed(&store, &root, true, 10, None, &[], &[], now)
            .expect("no third")
            .is_empty(),
        "3 回目は無い"
    );

    // `celerisctl knowledge rerun` 相当（`retried_at` を消す）でもう 1 回だけ拾われる。
    assert!(store.knowledge_run_reset(source.id).expect("reset"));
    assert_eq!(
        retry_failed(&store, &root, true, 10, None, &[], &[], now)
            .expect("after rerun")
            .len(),
        1
    );
}

/// `enabled = false` / KB 未初期化なら、やり直しも起きない。
#[test]
fn retry_noops_when_disabled_or_the_kb_is_missing() {
    let store = store_with_node();
    let (_dir, root) = kb_dir();
    let (_source, run_task) = failed_run(&store);
    started_with(&store, &run_task, LANGMEM_ADAPTER);
    let now = OffsetDateTime::now_utc();
    apply_finished(&store, &root, root.as_path(), now).expect("apply");
    assert!(
        retry_failed(&store, &root, false, 10, None, &[], &[], now)
            .expect("disabled")
            .is_empty()
    );
    assert!(
        retry_failed(
            &store,
            &root.join("does-not-exist"),
            true,
            10,
            None,
            &[],
            &[],
            now
        )
        .expect("no kb")
        .is_empty()
    );
}

/// ADR-0052 D2: フォールバックで走った run も適用は同じで、`via = "fallback:<adapter>"` が
/// `knowledge_runs` と `summary_json` の両方に残る。
#[test]
fn a_fallback_run_is_applied_the_same_way_and_records_its_via() {
    let store = store_with_node();
    let (_dir, root) = kb_dir();
    let workspace_root = tempfile::tempdir().expect("workspace");

    let source = terminal_task(Status::Done, Some("coding"), None, None, false);
    store.insert(&source).expect("insert");
    add_report(&store, &source);
    let run_task = terminal_task(
        Status::Done,
        Some("coding"),
        Some(report::KNOWLEDGE_ROLE),
        None,
        false,
    );
    store.insert(&run_task).expect("insert");
    // フォールバック先は汎用のアダプタ（ここでは開発用の `fake`）。
    started_with(&store, &run_task, "fake");
    let artifacts_dir = workspace_root
        .path()
        .join(run_task.id.to_string())
        .join("artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("mkdir");
    std::fs::write(
        artifacts_dir.join("knowledge-candidates.json"),
        r#"{"candidates": [{"op": "create", "path": "environment/tools/fallback.md",
                "title": "fallback", "tags": [], "scope": "environment", "body": "使い方。",
                "sources": ["task:x"], "confidence": "high"}]}"#,
    )
    .expect("write candidates");
    store
        .knowledge_run_create(source.id, run_task.id, OffsetDateTime::now_utc())
        .expect("create run");

    let now = OffsetDateTime::now_utc();
    assert_eq!(
        apply_finished(&store, &root, workspace_root.path(), now).expect("apply"),
        1
    );
    let run = store
        .knowledge_run_get(source.id)
        .expect("get")
        .expect("some");
    assert_eq!(run.state, KnowledgeRunState::Done);
    assert_eq!(run.via.as_deref(), Some("fallback:fake"));
    let summary = run.summary.expect("summary");
    assert_eq!(summary.ingested, 1, "適用は経路に関係なく同じ");
    assert_eq!(summary.via.as_deref(), Some("fallback:fake"));
    assert!(root.join("environment/tools/fallback.md").exists());
}
