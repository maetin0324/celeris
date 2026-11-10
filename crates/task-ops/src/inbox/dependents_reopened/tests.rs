//! ADR 2026-11-08-health-enforcement-loop D7.3・決定 p9-scope の受信箱の項目 `dependents_reopened`。
//!
//! 守る不変条件: 依存先 A の終端からの復帰で戻した後続がまだ動いていない（今 draft・ready で、最後の遷移が
//! `dependency_reopened`）間だけ、A に 1 件の項目が出る。本文には戻した後続の task id と依存先、戻さなかった後続の
//! 理由があり、選択肢は戻した後続ごとの cancel。後続が動き出すか終端になると消え、戻した後続の無い（kept だけの）
//! event では出ない。
//!
//! 既存の試験で検出できない理由: task-core の `dependency_reopen_` の試験は A の events に `DependentsReopened` が
//! 追記されることまでしか見ず、受信箱の組み立ては見ない。受信箱の既存の試験（`human_inbox_every_inbox_kind_*`）は
//! 作った `AttentionItem` を写すだけで、event からの導出（まだ動いていない後続だけを数える・消える条件・kept だけの
//! event を出さない）を通らない。時刻は使わず、遷移の event の順だけで決まる。
use super::*;
use crate::human_inbox::{HumanInbox, InboxItem, InboxKind};
use crate::inbox::EvidenceView;
use crate::view::ViewContext;
use std::time::Duration as StdDuration;
use task_core::{Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec};
use time::OffsetDateTime;

fn insert(store: &SqliteStore, title: &str, depends_on: Vec<TaskId>) -> TaskId {
    let now = OffsetDateTime::now_utc();
    let t = Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.to_string(),
        objective: "do it".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on,
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            // やり直しの余地なし: 失敗は failed になり、後続へ `dependency_failed` が連鎖する。
            max_retries: 0,
            max_tokens: None,
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
    };
    store.insert(&t).expect("insert");
    t.id
}

fn start(store: &SqliteStore, id: TaskId) {
    assert!(
        store
            .acquire_lease(id, &format!("run-{id}"), StdDuration::from_secs(60))
            .expect("lease")
    );
    assert_eq!(
        store.get(id).expect("get").expect("task").status,
        Status::Running
    );
}

fn fail(store: &SqliteStore, id: TaskId) {
    start(store, id);
    let outcome = store
        .apply_transition(id, Trigger::WorkerError { retryable: false }, None)
        .expect("fail");
    assert_eq!(outcome.next, Status::Failed);
}

fn status(store: &SqliteStore, id: TaskId) -> Status {
    store.get(id).expect("get").expect("task").status
}

/// 人が `original` を retry して後継を作る（`plan_retry` と同じく `depends_on` を写し、後継は draft で始まる）。
fn retry(store: &SqliteStore, original: TaskId) -> TaskId {
    let mut next = store.get(original).expect("get").expect("task");
    next.id = TaskId::new();
    next.status = Status::Draft;
    next.title = "successor".to_string();
    store
        .retry_task(original, &next, "human:test")
        .expect("retry");
    next.id
}

fn ctx() -> ViewContext {
    ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(
            std::path::PathBuf::from("/tmp/workspaces"),
            None,
        ),
        retry_backoff_base: StdDuration::from_secs(10),
        retry_backoff_max: StdDuration::from_secs(300),
        max_requeues: 5,
        clusters: Default::default(),
    }
}

fn no_evidence(_task: &Task, _run_id: &str) -> Vec<EvidenceView> {
    Vec::new()
}

fn items(store: &SqliteStore) -> Vec<InboxItem> {
    let out: HumanInbox = crate::human_inbox::human_inbox(
        store,
        None,
        &ctx(),
        OffsetDateTime::now_utc(),
        &no_evidence,
    )
    .expect("inbox");
    out.items
        .into_iter()
        .filter(|i| i.kind == InboxKind::DependentsReopened)
        .collect()
}

fn item_of(store: &SqliteStore, dependency: TaskId) -> Option<InboxItem> {
    items(store)
        .into_iter()
        .find(|i| i.task.as_ref().map(|t| t.id) == Some(dependency))
}

fn latest_record(store: &SqliteStore, id: TaskId) -> Option<Event> {
    store
        .latest_event_rows(LatestEventKind::DependentsReopened)
        .expect("latest")
        .into_iter()
        .find(|r| r.task_id == id)
        .map(|r| r.event)
}

/// A の reopen で項目が出て、本文に戻した後続 B の id と依存先、kept の理由（retry の後継・未解決の依存先）がある。
/// B が running になると消える。別の依存先 X の項目は、戻した後続 Y を人が cancel する（終端）と消える。kept だけの
/// event（K の reopen で戻せる後続が無い）では項目が出ない。
#[test]
fn dependents_reopened_inbox_item_lifecycle() {
    let store = SqliteStore::open_in_memory().expect("store");
    let a = insert(&store, "dependency a", Vec::new());
    let e = insert(&store, "other dependency e", Vec::new());
    let b = insert(&store, "dependent b", vec![a]);
    let c = insert(&store, "dependent c", vec![a]);
    let d = insert(&store, "dependent d", vec![a, e]);
    let x = insert(&store, "dependency x", Vec::new());
    let y = insert(&store, "dependent y", vec![x]);
    let k = insert(&store, "dependency k", Vec::new());
    let l = insert(&store, "dependent l", vec![k, e]);
    fail(&store, e);
    fail(&store, a);
    fail(&store, x);
    fail(&store, k);
    for id in [b, c, d, y, l] {
        assert_eq!(status(&store, id), Status::Cancelled);
    }
    let c2 = retry(&store, c);
    assert!(items(&store).is_empty(), "復帰の前は項目が無い");

    for id in [a, x, k] {
        store
            .apply_transition(id, Trigger::Reopen, None)
            .expect("reopen");
    }
    assert_eq!(status(&store, b), Status::Ready);
    assert_eq!(status(&store, y), Status::Ready);
    assert_eq!(status(&store, l), Status::Cancelled);

    // A: 戻した後続 B と、戻さなかった C（retry の後継 C2）・D（未解決の依存先 E）。
    let item = item_of(&store, a).expect("A の項目");
    assert!(
        item.id.starts_with(&format!("dependents_reopened-{a}-")),
        "{}",
        item.id
    );
    assert_eq!(
        item.blocking.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![b]
    );
    let detail = item.detail_full.clone().expect("detail");
    for want in [
        b.to_string(),
        format!("依存先 {a}"),
        format!("{c}（retry の後継 {c2}"),
        format!("{d}（ほかの依存先 {e}"),
    ] {
        assert!(detail.contains(&want), "{want} が本文に無い: {detail}");
    }
    assert_eq!(
        item.options
            .iter()
            .map(|o| o.key.clone())
            .collect::<Vec<_>>(),
        vec![format!("cancel:{b}")]
    );
    assert_eq!(
        item.answer.native.as_ref().map(|n| n.path.clone()),
        Some(format!("/api/v1/tasks/{b}/cancel"))
    );
    assert_eq!(
        crate::human_inbox::dependents_reopened::cancel_target(&item, &format!("cancel:{b}")),
        Some(b)
    );
    assert_eq!(
        crate::human_inbox::dependents_reopened::cancel_target(&item, &format!("cancel:{d}")),
        None,
        "戻していない後続は cancel の対象にしない"
    );
    assert_eq!(
        crate::human_inbox::item_task_id(&store, &item.id),
        Some(a),
        "項目の id から依存先 A を引ける"
    );

    // K: kept だけの event（L はほかの依存先 E が failed のまま）。event はあるが項目は出ない。
    let Some(Event::DependentsReopened { restored, kept }) = latest_record(&store, k) else {
        panic!("K に DependentsReopened が無い");
    };
    assert!(restored.is_empty());
    assert_eq!(kept.len(), 1);
    assert!(
        item_of(&store, k).is_none(),
        "kept だけの event は項目を出さない"
    );

    // X: 戻した後続 Y がある間は出て、人が Y を cancel する（終端）と消える。
    assert!(item_of(&store, x).is_some());
    store
        .apply_transition(y, Trigger::Cancel, None)
        .expect("cancel y");
    assert!(
        item_of(&store, x).is_none(),
        "戻した後続が終端になると消える"
    );

    // A: B が動き出す（running）と消える。今 ready に戻っても、最後の遷移が dependency_reopened でなければ出ない。
    start(&store, b);
    assert!(item_of(&store, a).is_none(), "戻した後続が動き出すと消える");
    store
        .apply_transition(b, Trigger::InfraRequeue, None)
        .expect("requeue");
    assert_eq!(status(&store, b), Status::Ready);
    assert!(
        item_of(&store, a).is_none(),
        "動いた後の ready は戻した後続ではない"
    );
    assert!(items(&store).is_empty());
}
