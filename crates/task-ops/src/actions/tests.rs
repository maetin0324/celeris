use super::*;
use task_core::{OrgKind, SqliteStore};

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn cos_task() -> Task {
    use task_core::{Budget, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec};
    let t = now();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "対話".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
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
        created_at: t,
        updated_at: t,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: Some("cos".into()),
        conversation: Some(task_core::MessageId::new()),
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn seed_engineering(store: &SqliteStore) {
    let t = now();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "cos".into(),
            parent_id: None,
            name: "Chief of Staff".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: t,
            updated_at: t,
        })
        .unwrap();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "engineering".into(),
            parent_id: Some("cos".into()),
            name: "Engineering".into(),
            kind: OrgKind::Department,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: t,
            updated_at: t,
        })
        .unwrap();
}

/// テスト専用の最小パーサ（`task_worker::actions_from_result_json` と同じ規則を、
/// `task-worker` に依存せずに再現する。本物の解析のテストは `task-worker` 側にある）。
fn parse(json: &str) -> (Vec<ConsoleAction>, Vec<String>) {
    let value: serde_json::Value = serde_json::from_str(json).expect("json");
    let items = value
        .get("actions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut valid = Vec::new();
    let mut malformed = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        match serde_json::from_value::<ConsoleAction>(item) {
            Ok(action) => valid.push(action),
            Err(e) => malformed.push(format!("action #{}: {e}", i + 1)),
        }
    }
    (valid, malformed)
}

/// `create_task`: 有効な action は `ready` のタスクを作り、`assignee` 省略なら matching は
/// ここでは走らせない（ディスパッチャの `assign_if_needed` が次 tick で決める）。
#[test]
fn create_task_makes_a_ready_task_and_records_a_summary() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"直す","objective":"直して",
           "acceptance":["直った"],"harness":"coding","tier":"frontier"}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("not idempotent-skipped");
    assert_eq!(outcome.executed.len(), 1);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task exists");
    assert_eq!(stored.worker_hint.tier, task_core::Tier::Frontier);
    // ADR-0069 D1: CoS の tier はヒントとして記録するだけ（lane はディスパッチ時に policy が決める）。
    assert_eq!(
        stored.routing.as_ref().map(|r| r.tier_source),
        Some(task_core::TierSource::Hint)
    );
    assert_eq!(stored.title, "直す");
    assert_eq!(stored.status, Status::Ready);
    assert_eq!(stored.genre.as_deref(), Some("coding"));
    assert_eq!(stored.assignee, None, "matching は別経路");
    assert!(outcome.executed[0].summary.contains("直す"));
}

/// ADR-0074 D2.1（Phase F3 途中確認、区切り 1 (a)）: CoS が `create_task.pause_after` を書けば
/// `Task.routing.pause_after` に写り、出自は `PauseSource::Agent`（人の明示より安全側に倒す
/// ので、`tier`/`assignee` と違ってそのまま採用する）。
#[test]
fn create_task_carries_pause_after_from_cos_with_agent_source() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"直す","objective":"直して",
           "acceptance":["直った"],"harness":"coding",
           "pause_after":{"mode":"each_phase"}}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("not idempotent-skipped");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task exists");
    let routing = stored.routing.expect("routing recorded");
    assert_eq!(routing.pause_after, task_core::PausePolicy::EachPhase);
    assert_eq!(routing.pause_after_source, task_core::PauseSource::Agent);
}

/// Phase 98（ADR-0018、実機障害 2026-09-22）: `create_task.workspace` が既知のクラスタを指す
/// `{"kind":"remote", ...}` なら `WorkspaceSpec::Remote` のタスクが作られる。
#[test]
fn create_task_with_a_known_cluster_workspace_makes_a_remote_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"pegasusinfo を実行","objective":"実行して",
           "acceptance":["結果が分かる"],"harness":"coding",
           "workspace":{"kind":"remote","cluster":"pegasus","path":"~"}}]}"#,
    );
    let known_clusters = vec!["pegasus".to_string(), "sirius".to_string()];
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &known_clusters,
        &task,
        "run-cluster",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("not idempotent-skipped");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task exists");
    assert_eq!(
        stored.workspace,
        task_core::WorkspaceSpec::Remote {
            cluster: "pegasus".to_string(),
            path: "~".into(),
            mode: None,
        }
    );
}

/// ADR-0059 D1（Phase 99）: `create_task.workspace.mode = "shared"` は、実行後のタスクの
/// `WorkspaceSpec::Remote.mode` にそのまま届く（`NewTaskSpec.workspace_mode` を経由する）。
#[test]
fn create_task_with_workspace_mode_shared_makes_a_shared_remote_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"pegasusinfo を実行","objective":"実行して",
           "acceptance":["結果が分かる"],"harness":"coding",
           "workspace":{"kind":"remote","cluster":"pegasus","path":"~","mode":"shared"}}]}"#,
    );
    let known_clusters = vec!["pegasus".to_string()];
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &known_clusters,
        &task,
        "run-cluster-shared",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("not idempotent-skipped");
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task exists");
    assert_eq!(
        stored.workspace,
        task_core::WorkspaceSpec::Remote {
            cluster: "pegasus".to_string(),
            path: "~".into(),
            mode: Some(task_core::WorkspaceMode::Shared),
        }
    );
}

/// `[[clusters]]` に無いクラスタは action 全体を検証で落とす（タスクは作られず、理由が残る）。
#[test]
fn create_task_with_an_unknown_cluster_is_rejected() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"t","objective":"o",
           "acceptance":["ok"],"workspace":{"kind":"remote","cluster":"nowhere","path":"~"}}]}"#,
    );
    let known_clusters = vec!["pegasus".to_string()];
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &known_clusters,
        &task,
        "run-unknown-cluster",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0].reason.contains("unknown cluster"),
        "{:?}",
        outcome.failed
    );
    assert!(store.list(None).unwrap().is_empty(), "何も作らない");
}

/// ADR-0062 B1/B3（Phase 107）: 明示の `assignee` と明示の remote workspace が両方あり、
/// その担当が `cluster:<id>` を持たなければ action 全体を検証で落とす（タスクは作られない）。
#[test]
fn create_task_with_an_explicit_assignee_lacking_the_cluster_tool_is_rejected() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let now_t = now();
    let org = vec![
        OrgNode {
            profile: Default::default(),
            id: "cos".into(),
            parent_id: None,
            name: "Chief of Staff".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now_t,
            updated_at: now_t,
        },
        OrgNode {
            profile: task_core::Profile {
                tools: vec!["tavily".to_string(), "exa".to_string()],
                ..Default::default()
            },
            id: "web-research".into(),
            parent_id: Some("cos".into()),
            name: "Web Research".into(),
            kind: OrgKind::Department,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now_t,
            updated_at: now_t,
        },
        OrgNode {
            profile: task_core::Profile {
                tools: vec!["cluster:sirius".to_string()],
                ..Default::default()
            },
            id: "cluster-hpc".into(),
            parent_id: Some("cos".into()),
            name: "Cluster & HPC".into(),
            kind: OrgKind::Department,
            genre: None,
            brief: String::new(),
            position: 1,
            created_at: now_t,
            updated_at: now_t,
        },
    ];
    // ADR-0069 D1: 担当の指定が効くのは人が `@<node>` で名指ししたときだけ。
    let mut task = cos_task();
    task.objective = "sirius で計測して。@web-research に頼んで".into();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"sirius で計測","objective":"計測して",
           "acceptance":["結果が分かる"],"assignee":"web-research",
           "workspace":{"kind":"remote","cluster":"sirius","path":"~"}}]}"#,
    );
    let known_clusters = vec!["sirius".to_string()];
    let outcome = execute(
        &store,
        &org,
        &[],
        &[],
        &known_clusters,
        &task,
        "run-cluster-tool",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0].reason.contains("cluster:sirius"),
        "{:?}",
        outcome.failed
    );
    assert!(
        outcome.failed[0].reason.contains("cluster-hpc"),
        "候補ノードを挙げる: {:?}",
        outcome.failed
    );
    assert!(store.list(None).unwrap().is_empty(), "何も作らない");
}

/// ADR-0069 D1（Phase 114）: CoS（LLM）が書いた `assignee` は、人の発言に `@<node>` が無ければ
/// 捨てられ（`routing.dropped_assignee` と返事の要約に残る）、担当は matching に任される。
#[test]
fn cos_supplied_assignee_is_dropped_unless_the_human_named_it() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let json = r#"{"actions":[{"type":"create_task","title":"直す","objective":"直して",
           "acceptance":["直った"],"assignee":"engineering","tier":"frontier",
           "features":{"judgment":"low","verifiability":"high"}}]}"#;
    let parsed = parse(json);
    let task = cos_task(); // 人の発言は "o"（名指し無し）
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-drop",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert!(
        outcome.executed[0]
            .summary
            .contains("担当の指定 engineering は人の明示ではない"),
        "{}",
        outcome.executed[0].summary
    );
    let stored = store
        .get(outcome.executed[0].task_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(stored.assignee, None, "matching に任せる");
    let routing = stored.routing.expect("routing");
    assert_eq!(routing.dropped_assignee.as_deref(), Some("engineering"));
    assert!(!routing.assignee_explicit);
    assert_eq!(routing.tier_source, task_core::TierSource::Hint);
    let features = routing.features.expect("features hints");
    assert_eq!(features.judgment, Some(task_core::Level::Low));

    // 人が `@engineering` と `tier:frontier` を書いていれば、その指定に従う。
    let mut task = cos_task();
    task.objective = "@engineering に tier:frontier で頼んで".into();
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-keep",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    let stored = store
        .get(outcome.executed[0].task_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(stored.assignee.as_deref(), Some("engineering"));
    let routing = stored.routing.expect("routing");
    assert_eq!(routing.dropped_assignee, None);
    assert!(routing.assignee_explicit);
    assert_eq!(routing.tier_source, task_core::TierSource::Human);
    assert!(!outcome.executed[0].summary.contains("担当の指定"));
}

#[test]
fn human_mentions_are_matched_on_word_boundaries() {
    assert!(human_mentions_node("@engineering にお願い", "engineering"));
    assert!(human_mentions_node("担当は @engineering", "engineering"));
    assert!(!human_mentions_node(
        "@engineering-x にお願い",
        "engineering"
    ));
    assert!(!human_mentions_node("engineering にお願い", "engineering"));
    assert!(!human_mentions_node("@x", ""));
    assert!(human_mentions_tier(
        "tier:cheap でいい",
        task_core::Tier::Cheap
    ));
    assert!(human_mentions_tier(
        "Tier=Frontier",
        task_core::Tier::Frontier
    ));
    assert!(!human_mentions_tier(
        "frontier で",
        task_core::Tier::Frontier
    ));
}

/// 実機 2026-09-21: CoS が tier と mode の両方に `standard` を書き、正しい create_task
/// 全体が捨てられた。tier の common value は既定の production mode として受ける。
#[test]
fn create_task_tolerates_standard_in_mode_as_production() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"直す","objective":"直して",
           "acceptance":["直った"],"tier":"standard","mode":"standard"}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-standard-mode",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("not idempotent-skipped");

    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task exists");
    assert_eq!(stored.worker_hint.tier, task_core::Tier::Standard);
    assert_eq!(stored.mode, task_core::TaskMode::Production);
}

/// 検証に落ちた action（受け入れ条件無し）は実行されず、理由が残る。
#[test]
fn an_invalid_create_task_is_not_executed_and_gets_a_reason() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(r#"{"actions":[{"type":"create_task","title":"t","objective":"o"}]}"#);
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].kind, "create_task");
    assert!(outcome.failed[0].reason.contains("acceptance"));
    assert!(store.list(None).unwrap().is_empty(), "何も作らない");
}

/// 知らない harness（`genres` が設定されていれば検証される）や存在しない project も
/// 実行されない。
#[test]
fn unknown_project_is_rejected() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"t","objective":"o",
           "acceptance":["ok"],"project":"01ZZZZZZZZZZZZZZZZZZZZZZZZ"}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].reason.contains("does not exist"));
}

/// `propose_project`: `proposed` の案件が作られ、絶対パスの repos は primary + 名前で入る。
#[test]
fn propose_project_creates_a_proposed_project_with_repos() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"propose_project","title":"新案件","request":"やりたい",
           "repos":["/tmp/agent-platform"]}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(outcome.executed.len(), 1);
    let project_id = outcome.executed[0].project_id.expect("project id");
    let project = store.project_get(project_id).unwrap().expect("project");
    assert_eq!(project.title, "新案件");
    assert_eq!(project.status, ProjectStatus::Proposed);
    let repos = store.repo_list(project_id).unwrap();
    assert_eq!(repos.len(), 1);
    assert!(repos[0].is_primary);
    assert_eq!(repos[0].name, "agent-platform");
}

/// 相対パスの repos は action 全体を実行しない（案件そのものも作らない）。
#[test]
fn propose_project_rejects_a_relative_repo_path() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"propose_project","title":"t","request":"r",
           "repos":["relative/path"]}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(store.project_list().unwrap().is_empty());
}

/// `ask_human`: 実行済みとして記録するだけ（何も作らない）。
#[test]
fn ask_human_is_recorded_without_creating_anything() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = cos_task();
    let parsed = parse(r#"{"actions":[{"type":"ask_human","text":"どちらがよいですか"}]}"#);
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(outcome.executed.len(), 1);
    assert_eq!(outcome.executed[0].kind, "ask_human");
    assert!(outcome.executed[0].summary.contains("どちらがよいですか"));
}

/// 冪等性: 同じ `run_id` の 2 回目は何もしない（`Ok(None)`）。
#[test]
fn the_same_run_id_executes_actions_only_once() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let task = cos_task();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"直す","objective":"直して","acceptance":["直った"]}]}"#,
    );
    let first = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-dup",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .expect("first run executes");
    assert_eq!(first.executed.len(), 1);
    assert_eq!(store.list(None).unwrap().len(), 1);

    let second = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-dup",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap();
    assert!(second.is_none(), "2 回目は何もしない");
    assert_eq!(
        store.list(None).unwrap().len(),
        1,
        "重複してタスクが増えない"
    );
}

/// malformed（`ConsoleAction` の形に合わなかった要素）も `failed` に写る。
#[test]
fn malformed_actions_from_parsing_are_reported_as_failures() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = cos_task();
    let parsed = parse(r#"{"actions":[{"type":"unknown_action"}]}"#);
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "run-1",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    assert!(outcome.executed.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].kind, "unknown");
}

/// `failure_note` / `to_metadata`。
#[test]
fn outcome_formats_a_failure_note_and_metadata() {
    let outcome = ActionsOutcome {
        executed: vec![ExecutedAction {
            kind: "create_task",
            summary: "→ タスクを作りました: t".into(),
            task_id: Some(TaskId::new()),
            project_id: None,
            milestone_id: None,
        }],
        failed: vec![FailedAction {
            kind: "add_milestone".into(),
            reason: "project x does not exist".into(),
        }],
    };
    let note = outcome.failure_note().expect("note");
    assert!(note.contains("実行できなかった action"));
    assert!(note.contains("add_milestone: project x does not exist"));
    let metadata = outcome.to_metadata().expect("metadata");
    assert_eq!(metadata.actions_executed.len(), 1);
    assert_eq!(metadata.actions_failed.len(), 1);
    assert!(ActionsOutcome::default().failure_note().is_none());
    assert!(ActionsOutcome::default().to_metadata().is_none());
}
#[test]
fn delegated_work_keeps_the_original_request_and_a_scope_review_condition() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_engineering(&store);
    let mut task = cos_task();
    task.objective = "スマホGUIを修正し、検証してください".into();
    let source_id = task_core::MessageId::new();
    task.conversation = Some(source_id);
    let source = task_core::Message {
        id: source_id,
        node_id: "cos".into(),
        project_id: None,
        role: task_core::MessageRole::User,
        text: task.objective.clone(),
        run_id: None,
        task_id: Some(task.id),
        metadata: None,
        created_at: now(),
    };
    store.message_append(&source).unwrap();
    store
        .message_append(&task_core::Message {
            id: task_core::MessageId::new(),
            text: "後から届いた別の依頼".into(),
            created_at: now() + time::Duration::seconds(1),
            ..source
        })
        .unwrap();
    let parsed = parse(
        r#"{"actions":[{"type":"create_task","title":"GUIを調査","objective":"改善案を書く","acceptance":["報告書がある"]}]}"#,
    );
    let outcome = execute(
        &store,
        &[],
        &[],
        &[],
        &[],
        &task,
        "source-run",
        &parsed.0,
        &parsed.1,
        now(),
    )
    .unwrap()
    .unwrap();
    let created = store
        .get(outcome.executed[0].task_id.unwrap())
        .unwrap()
        .unwrap();
    assert!(
        created
            .objective
            .contains("スマホGUIを修正し、検証してください")
    );
    assert!(!created.objective.contains("後から届いた別の依頼"));
    assert!(
        created
            .acceptance
            .iter()
            .any(|c| c.text.contains("調査報告や提案だけでは合格にせず"))
    );
}
