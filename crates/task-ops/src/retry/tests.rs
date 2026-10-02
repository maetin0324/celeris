use super::*;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, Event, MessageId, SqliteStore, TaskKind, Tier, Trigger,
    WorkerHint,
};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open")
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn base_spec(title: &str) -> crate::add::NewTaskSpec {
    crate::add::NewTaskSpec {
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        title: title.to_string(),
        objective: "do it".to_string(),
        // ADR-0067 D2: `human` チェックには成果物か知識ベースの参照が要る。
        acceptance: vec![
            crate::add::CriterionSpec::Human {
                text: "looks right".to_string(),
            },
            crate::add::CriterionSpec::ArtifactExists {
                name: "result.md".to_string(),
            },
        ],
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(crate::add::PriorityInput::Number(0)),
        parent: None,
        depends_on: vec![],
        max_turns: None,
        max_wall_secs: None,
        max_retries: 2,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        workspace: None,
        cluster: None,
        workspace_mode: None,
        adapter: None,
        labels: Vec::new(),
        category: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: crate::add::SpecProvenance::default(),
    }
}

fn make_failed(store: &SqliteStore, title: &str) -> Task {
    // ADR-0044 D3: ラベル・種類を付けてから失敗させる（やり直しが引き継ぐことを確かめるため）。
    let spec = crate::add::NewTaskSpec {
        labels: vec!["infra".into()],
        category: Some(task_core::TaskCategory::Bug),
        ..base_spec(title)
    };
    let task = crate::add::create_task(store, spec, now()).expect("create");
    store
        .apply_transition(task.id, Trigger::Accept, None)
        .expect("accept");
    store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .expect("dispatch");
    let outcome = store
        .apply_transition(task.id, Trigger::WorkerError { retryable: false }, None)
        .expect("worker_error");
    assert_eq!(outcome.next, Status::Failed);
    store.get(task.id).expect("get").expect("some")
}

/// `store.insert` で直接書く（`add.rs` の検証を経由しない。依存関係の状態を自由に組み立てるため）。
fn raw_task(status: Status, depends_on: Vec<TaskId>, conversation: Option<MessageId>) -> Task {
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
        title: "dependent".to_string(),
        objective: "depend on it".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![ArtifactRef {
            name: "spec".to_string(),
            path: "spec.md".to_string(),
            sha256: "abc".to_string(),
            kind: "doc".to_string(),
            declared: true,
        }],
        depends_on,
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp/workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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
        assignee: None,
        conversation,
        labels: Vec::new(),
        category: Default::default(),
    }
}

#[test]
fn retry_duplicates_fields_and_records_retried_event() {
    let store = store();
    let original = make_failed(&store, "investigate incident");

    let result = retry_task(&store, original.id, false, None, now()).expect("retry");
    assert!(result.rewired.is_empty());
    let new_task = store.get(result.task_id).expect("get").expect("some");
    assert_eq!(new_task.status, Status::Draft);
    assert_eq!(new_task.attempts, 0);
    assert_eq!(new_task.title, original.title);
    assert_eq!(new_task.objective, original.objective);
    assert_eq!(new_task.acceptance, original.acceptance);
    assert_eq!(new_task.inputs, original.inputs);
    assert_eq!(new_task.worker_hint, original.worker_hint);
    assert_eq!(new_task.budget, original.budget);
    assert_eq!(new_task.role, original.role);
    assert_eq!(new_task.genre, original.genre);
    assert_eq!(new_task.project_id, original.project_id);
    assert_eq!(new_task.milestone_id, original.milestone_id);
    assert_eq!(new_task.assignee, original.assignee);
    assert_eq!(new_task.parent_id, original.parent_id);
    assert_eq!(new_task.workspace, original.workspace);
    assert_eq!(new_task.depends_on, original.depends_on);
    assert_eq!(new_task.conversation, None);
    // ADR-0044 D3（Phase 53）: 人が付けた分類（ラベル・種類）は引き継ぐ。
    assert_eq!(new_task.labels, original.labels);
    assert_eq!(new_task.category, original.category);

    let events: Vec<Event> = store
        .events_for(new_task.id)
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(matches!(&events[0], Event::Created { task, .. } if task.id == new_task.id));
    assert!(matches!(&events[1], Event::Retried { from } if *from == original.id));
}

#[test]
fn retry_with_accept_starts_ready() {
    let store = store();
    let original = make_failed(&store, "retry me");
    let result = retry_task(&store, original.id, true, None, now()).expect("retry");
    let new_task = store.get(result.task_id).expect("get").expect("some");
    assert_eq!(new_task.status, Status::Ready);
}

// ---- ADR-0062 Phase 108: `retry` の `workspace` ----

/// `cos` の下に `web-research`（道具なし）と `cluster-hpc`（`cluster:sirius` を持つ）。
fn cluster_org() -> Vec<task_core::OrgNode> {
    use task_core::{OrgKind, OrgNode, Profile};
    let now = now();
    let dept = |id: &str, tools: &[&str]| OrgNode {
        profile: Profile {
            tools: tools.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        id: id.to_string(),
        parent_id: Some("cos".to_string()),
        name: id.to_string(),
        kind: OrgKind::Department,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    vec![
        OrgNode {
            profile: Default::default(),
            id: "cos".into(),
            parent_id: None,
            name: "cos".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: now,
            updated_at: now,
        },
        dept("web-research", &[]),
        dept("cluster-hpc", &["cluster:sirius"]),
    ]
}

/// 失敗した remote タスクを `workspace` を渡して retry すると、複製が新しい `workspace` になる
/// （本番の事故: web-research / literature-research の remote タスクを Local に直してやり直す）。
#[test]
fn retry_can_override_the_workspace_of_the_new_task() {
    let store = store();
    let mut original = raw_task(Status::Failed, vec![], None);
    original.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: "~".into(),
        mode: None,
    };
    store.insert(&original).expect("insert");

    let result = retry_task(
        &store,
        original.id,
        false,
        Some(WorkspaceSpec::local("/tmp/retry-local")),
        now(),
    )
    .expect("retry");
    let new_task = store.get(result.task_id).expect("get").expect("some");
    assert_eq!(new_task.workspace, WorkspaceSpec::local("/tmp/retry-local"));
}

/// `workspace.path` が空の `Local` は 422。
#[test]
fn retry_workspace_override_rejects_an_empty_local_path() {
    let store = store();
    let original = raw_task(Status::Failed, vec![], None);
    store.insert(&original).expect("insert");

    let err = retry_task(
        &store,
        original.id,
        false,
        Some(WorkspaceSpec::local("")),
        now(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err}");
}

/// 明示の `Remote` で元の担当が `cluster:<id>` を持たなければ 422（複製は作られない）。
#[test]
fn retry_workspace_override_rejects_remote_when_the_assignee_lacks_the_cluster_tool() {
    let store = store();
    for node in cluster_org() {
        store.org_upsert(&node).expect("seed org");
    }
    let mut original = raw_task(Status::Failed, vec![], None);
    original.assignee = Some("web-research".into());
    store.insert(&original).expect("insert");

    let err = retry_task(
        &store,
        original.id,
        false,
        Some(WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: "~".into(),
            mode: None,
        }),
        now(),
    )
    .unwrap_err();
    let OpsError::Validation(msg) = err else {
        panic!("{err}");
    };
    assert!(msg.contains("cluster:sirius"), "{msg}");
}

#[test]
fn retry_rewires_non_terminal_and_cascaded_cancelled_dependents_but_not_manual_cancels() {
    let store = store();
    // `original` は running のまま挿入し、apply_transition で failed に落として本物の cascade
    // （`DependencyFailed`）を起こす。
    let original = raw_task(Status::Running, vec![], None);
    store.insert(&original).expect("insert original");

    // 対話タスクは DependencyFailed の対象外（P-78）なので draft のまま残る。
    let conv_dependent = raw_task(Status::Draft, vec![original.id], Some(MessageId::new()));
    store
        .insert(&conv_dependent)
        .expect("insert conv dependent");

    // 普通の後続（ready）。original が failed になると cascade で cancelled(dependency_failed) になる。
    let normal_dependent = raw_task(Status::Ready, vec![original.id], None);
    store
        .insert(&normal_dependent)
        .expect("insert normal dependent");

    // 先に人が手動で cancel した後続。cascade は既に終端のタスクを飛ばす。
    let manually_cancelled = raw_task(Status::Ready, vec![original.id], None);
    store
        .insert(&manually_cancelled)
        .expect("insert manually cancelled");
    store
        .apply_transition(manually_cancelled.id, Trigger::Cancel, None)
        .expect("cancel");

    let outcome = store
        .apply_transition(original.id, Trigger::WorkerError { retryable: false }, None)
        .expect("worker_error");
    assert_eq!(outcome.next, Status::Failed);

    // cascade の前提を確認しておく。
    assert_eq!(
        store.get(conv_dependent.id).unwrap().unwrap().status,
        Status::Draft
    );
    assert_eq!(
        store.get(normal_dependent.id).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert_eq!(
        store.get(manually_cancelled.id).unwrap().unwrap().status,
        Status::Cancelled
    );

    let result = retry_task(&store, original.id, false, None, now()).expect("retry");
    let mut rewired = result.rewired.clone();
    rewired.sort();
    let mut expected = vec![conv_dependent.id, normal_dependent.id];
    expected.sort();
    assert_eq!(rewired, expected);

    let conv_after = store.get(conv_dependent.id).expect("get").expect("some");
    assert_eq!(
        conv_after.status,
        Status::Draft,
        "対話タスクの状態は変えない"
    );
    assert_eq!(conv_after.depends_on, vec![result.task_id]);

    let normal_after = store.get(normal_dependent.id).expect("get").expect("some");
    assert_eq!(
        normal_after.status,
        Status::Draft,
        "dependency_failed による cancelled は draft に戻す"
    );
    assert_eq!(normal_after.depends_on, vec![result.task_id]);
    let normal_events: Vec<Event> = store
        .events_for(normal_dependent.id)
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(normal_events.iter().any(|e| matches!(
        e,
        Event::Transitioned { from: Status::Cancelled, to: Status::Draft, reason } if reason == "retried"
    )));

    let manually_after = store
        .get(manually_cancelled.id)
        .expect("get")
        .expect("some");
    assert_eq!(
        manually_after.status,
        Status::Cancelled,
        "手動 cancel は対象外"
    );
    assert_eq!(manually_after.depends_on, vec![original.id], "張り替えない");
}

#[test]
fn retry_rejects_non_terminal_or_done_tasks_with_409() {
    let store = store();
    let draft = crate::add::create_task(&store, base_spec("still draft"), now()).expect("create");
    let err = retry_task(&store, draft.id, false, None, now()).unwrap_err();
    assert!(matches!(err, OpsError::InvalidState { .. }));

    let done = crate::add::create_task(&store, base_spec("will be done"), now()).expect("create");
    store
        .apply_transition(done.id, Trigger::Accept, None)
        .expect("accept");
    store
        .apply_transition(done.id, Trigger::Dispatch, None)
        .expect("dispatch");
    store
        .apply_transition(done.id, Trigger::WorkerDone, None)
        .expect("worker_done");
    store
        .apply_transition(done.id, Trigger::ReviewPass, None)
        .expect("review_pass");
    let done = store.get(done.id).expect("get").expect("some");
    assert_eq!(done.status, Status::Done);
    let err = retry_task(&store, done.id, false, None, now()).unwrap_err();
    assert!(matches!(err, OpsError::InvalidState { .. }));
}

#[test]
fn retry_missing_task_is_not_found() {
    let store = store();
    let err = retry_task(&store, TaskId::new(), false, None, now()).unwrap_err();
    assert!(matches!(err, OpsError::NotFound(_)));
}

/// ADR-0072「Phase F6 実装時の決定」: gate=shadow の下で判定された（`shadow: true`）タスクを
/// やり直すと、複製先は元の判定を持たず（最初の dispatch で今の設定で判定し直す）、人の明示の
/// `execution_hint` はそのまま残る。
fn fail_with_routing(store: &SqliteStore, hint: task_core::ExecutionHintSpec) -> Task {
    let original = make_failed(store, "gated under shadow");
    let mut t = original.clone();
    let mut routing = t.routing.clone().unwrap_or_default();
    routing.execution_hint = Some(hint);
    routing.execution = Some(task_core::ExecutionGateDecision {
        mode: task_core::ExecutionMode::Compound,
        source: task_core::GateSource::Hint,
        score: 13,
        threshold: 5,
        rule_id: "compound/long-and-broad".to_string(),
        signals: Vec::new(),
        policy_version: "exec-gate/1".to_string(),
        shadow: true,
        depth: None,
    });
    t.routing = Some(routing);
    store
        .update_task(
            &t,
            Event::Edited {
                fields: vec!["routing".into()],
                by: "test".into(),
            },
        )
        .expect("update")
}

#[test]
fn retry_drops_the_copied_gate_decision_and_keeps_an_explicit_hint() {
    let store = store();
    let hint = task_core::ExecutionHintSpec {
        mode: task_core::ExecutionMode::Compound,
        explicit: true,
    };
    let original = fail_with_routing(&store, hint);
    let result = retry_task(&store, original.id, true, None, now()).expect("retry");
    let copy = store.get(result.task_id).expect("get").expect("some");
    let routing = copy.routing.expect("routing");
    assert_eq!(routing.execution_hint, Some(hint));
    assert!(
        routing.execution.is_none(),
        "the copy is re-gated under the current config"
    );
    // 複製先に ExecutionHintSet は積まない（`execution` を書いていない）。
    let events = store.events_for(result.task_id).expect("events");
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::ExecutionHintSet { .. }))
    );
}

#[test]
fn retry_with_execution_sets_an_explicit_hint_on_the_copy_with_its_source() {
    let store = store();
    let original = fail_with_routing(
        &store,
        task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: false,
        },
    );
    let result = retry_task_with_execution(
        &store,
        original.id,
        true,
        None,
        Some(task_core::ExecutionMode::Compound),
        "mcp:chatgpt",
        now(),
    )
    .expect("retry");
    let copy = store.get(result.task_id).expect("get").expect("some");
    assert_eq!(copy.status, Status::Ready);
    let routing = copy.routing.expect("routing");
    assert_eq!(
        routing.execution_hint,
        Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true
        })
    );
    assert!(routing.execution.is_none());
    let events = store.events_for(result.task_id).expect("events");
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::ExecutionHintSet { source, .. } if source == "mcp:chatgpt"
    )));
}
