//! ADR 2026-11-08-health-enforcement-loop D4.3 の受信箱の項目 `health_repeat`。既存の受信箱の試験は
//! `HealthFindingsRepeated` を知らないので、(root, set_sha256) ごとに 1 件・源が Blocked を出たら消える・
//! root が終端なら出ない、をここで固定する。時刻は使わず、出来事（遷移の event）の順だけで決まる。
use super::*;
use crate::human_inbox::{HumanInbox, InboxItem, InboxKind};
use crate::inbox::EvidenceView;
use crate::view::ViewContext;
use std::time::Duration as StdDuration;
use task_core::{
    Budget, Check, Criterion, SqliteStore, TaskKind, Tier, Trigger, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn task(parent: Option<TaskId>, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: parent,
        kind: TaskKind::Execute,
        title: "health task".to_string(),
        objective: "make it healthy".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
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
            max_retries: 2,
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
    }
}

fn insert(store: &SqliteStore, parent: Option<TaskId>, status: Status) -> Task {
    let t = task(parent, status);
    store.insert(&t).expect("insert");
    t
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

fn inbox(store: &SqliteStore) -> HumanInbox {
    crate::human_inbox::human_inbox(store, None, &ctx(), OffsetDateTime::now_utc(), &no_evidence)
        .expect("inbox")
}

fn repeats(out: &HumanInbox) -> Vec<&InboxItem> {
    out.items
        .iter()
        .filter(|i| i.kind == InboxKind::HealthRepeat)
        .collect()
}

fn source_ids(item: &InboxItem) -> Vec<TaskId> {
    let mut ids: Vec<TaskId> = item.blocking.tasks.iter().map(|t| t.id).collect();
    ids.sort();
    ids
}

fn fps(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// 2 つの源（別の leaf・別の経路、深さも違う）が同じ集合に当たっても root に 1 件。event は root に載る。
#[test]
fn health_limits_inbox_two_sources_same_set_one_item() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Running);
    let mid = insert(&store, Some(root.id), Status::Running);
    let a = insert(&store, Some(root.id), Status::Blocked);
    let b = insert(&store, Some(mid.id), Status::Blocked);
    let at_a = record_findings_repeated(
        &store,
        &a,
        HealthRepeatSite::WorkUnit { key: "wu-a".into() },
        &fps(&["fp-x", "fp-y"]),
    )
    .expect("record a");
    let at_b = record_findings_repeated(
        &store,
        &b,
        HealthRepeatSite::Phase { key: "core".into() },
        &fps(&["fp-y", "fp-x", "fp-x"]),
    )
    .expect("record b");
    assert_eq!((at_a, at_b), (root.id, root.id));
    let on_root = store
        .events_for(root.id)
        .expect("root events")
        .into_iter()
        .filter(|(_, e)| matches!(e, Event::HealthFindingsRepeated { .. }))
        .count();
    assert_eq!(on_root, 2);

    let out = inbox(&store);
    let items = repeats(&out);
    assert_eq!(items.len(), 1, "{items:?}");
    let item = items[0];
    assert_eq!(item.task.as_ref().map(|t| t.id), Some(root.id));
    let mut want = vec![a.id, b.id];
    want.sort();
    assert_eq!(source_ids(item), want);
    assert!(item.answer.native.is_none(), "受信箱では答えない");
    assert!(item.id.starts_with(&format!("health_repeat-{}-", root.id)));
    assert_eq!(out.counts.by_kind.get("health_repeat"), Some(&1));
}

/// 集合が違えば同じ root でも別の項目。root 自身が源（root なら自分）でも root に載る。
#[test]
fn health_limits_inbox_different_sets_two_items() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Blocked);
    let a = insert(&store, Some(root.id), Status::Blocked);
    record_findings_repeated(
        &store,
        &a,
        HealthRepeatSite::WorkUnit { key: "wu-a".into() },
        &fps(&["fp-x"]),
    )
    .expect("record a");
    let self_root = record_findings_repeated(
        &store,
        &root,
        HealthRepeatSite::Review,
        &fps(&["fp-x", "fp-z"]),
    )
    .expect("record root");
    assert_eq!(self_root, root.id);

    let out = inbox(&store);
    let items = repeats(&out);
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(
        items
            .iter()
            .all(|i| i.task.as_ref().map(|t| t.id) == Some(root.id))
    );
    let mut sources: Vec<Vec<TaskId>> = items.iter().map(|i| source_ids(i)).collect();
    sources.sort();
    let mut want = vec![vec![a.id], vec![root.id]];
    want.sort();
    assert_eq!(sources, want);
    assert_ne!(items[0].id, items[1].id);
}

/// 源が Blocked を出たら（人の comment の再開。回答 `Trigger::Answer`）その源は外れ、全部出たら項目は消える。出た後に別の理由で
/// また Blocked になっても、古い往復の源には戻らない。
#[test]
fn health_limits_inbox_source_leaving_blocked_clears() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Running);
    let a = insert(&store, Some(root.id), Status::Blocked);
    let b = insert(&store, Some(root.id), Status::Blocked);
    for (t, key) in [(&a, "wu-a"), (&b, "wu-b")] {
        record_findings_repeated(
            &store,
            t,
            HealthRepeatSite::WorkUnit { key: key.into() },
            &fps(&["fp-x"]),
        )
        .expect("record");
    }
    assert_eq!(source_ids(repeats(&inbox(&store))[0]), {
        let mut v = vec![a.id, b.id];
        v.sort();
        v
    });

    store
        .apply_transition(a.id, Trigger::Answer, None)
        .expect("a resumes");
    let out = inbox(&store);
    let items = repeats(&out);
    assert_eq!(items.len(), 1);
    assert_eq!(source_ids(items[0]), vec![b.id]);

    store
        .apply_transition(b.id, Trigger::Answer, None)
        .expect("b resumes");
    assert!(repeats(&inbox(&store)).is_empty());

    // 再開の後に別の理由（質問）でまた Blocked になっても、往復の項目は戻らない。
    store
        .apply_transition(b.id, Trigger::Dispatch, None)
        .expect("dispatch");
    store
        .apply_transition(b.id, Trigger::WorkerQuestion, None)
        .expect("question blocks");
    assert_eq!(
        store.get(b.id).expect("get").map(|t| t.status),
        Some(Status::Blocked)
    );
    assert!(repeats(&inbox(&store)).is_empty());

    // 同じ往復でまた止めれば（新しい event）、また 1 件出る。
    record_findings_repeated(
        &store,
        &b,
        HealthRepeatSite::WorkUnit { key: "wu-b".into() },
        &fps(&["fp-x"]),
    )
    .expect("record again");
    let out = inbox(&store);
    let items = repeats(&out);
    assert_eq!(items.len(), 1);
    assert_eq!(source_ids(items[0]), vec![b.id]);
}

/// root が終端なら、源が Blocked のままでも出さない。
#[test]
fn health_limits_inbox_terminal_root_hides_item() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Cancelled);
    let a = insert(&store, Some(root.id), Status::Blocked);
    record_findings_repeated(&store, &a, HealthRepeatSite::Review, &fps(&["fp-x"]))
        .expect("record");
    assert!(repeats(&inbox(&store)).is_empty());
}

/// 範囲つきの組み立て（CoS の一次対応が使う `human_inbox_touching`）は、源か root に触れれば全件と同じ項目を返す。
#[test]
fn health_limits_inbox_touching_matches_full_inbox() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Running);
    let a = insert(&store, Some(root.id), Status::Blocked);
    let b = insert(&store, Some(root.id), Status::Blocked);
    for t in [&a, &b] {
        record_findings_repeated(&store, t, HealthRepeatSite::Review, &fps(&["fp-x"]))
            .expect("record");
    }
    let now = OffsetDateTime::now_utc();
    let full =
        crate::human_inbox::human_inbox(&store, None, &ctx(), now, &no_evidence).expect("full");
    let full_item = repeats(&full)[0].clone();
    for touching in [root.id, a.id, b.id] {
        let scoped = crate::human_inbox::human_inbox_touching(
            &store,
            None,
            &ctx(),
            now,
            &no_evidence,
            &BTreeSet::from([touching]),
        )
        .expect("touching");
        let items = repeats(&scoped);
        assert_eq!(items.len(), 1, "touching {touching}");
        assert_eq!(items[0], &full_item);
    }
}

/// 不変条件（D4.3）: `health_repeat` の項目の選択肢は Blocked の源に効く口（comment）だけ。`POST /tasks/{id}/retry`
/// は Failed・Cancelled しか受けず（`crate::retry::plan_retry`）、Blocked の源には 409 になるので出さない。
/// 既存の `health_limits_inbox_` の試験は項目の数・源・消え方と native の無いことだけを見て、選択肢の key も、
/// 選択肢の口が源の今の状態で受け付けられるかも見ないので、Blocked の源に効かない選択肢が混ざっても検出できない。
/// ここでは選択肢の key を固定し、出した口（comment）が回答 `Trigger::Answer` で源を Blocked から出して項目を消すこと、
/// 出さない口（retry）が Blocked の源を拒むことを確かめる。
#[test]
fn health_limits_inbox_options_resume_blocked_source() {
    let store = SqliteStore::open_in_memory().expect("store");
    let root = insert(&store, None, Status::Running);
    let a = insert(&store, Some(root.id), Status::Blocked);
    record_findings_repeated(
        &store,
        &a,
        HealthRepeatSite::WorkUnit { key: "wu-a".into() },
        &fps(&["fp-x"]),
    )
    .expect("record");

    let out = inbox(&store);
    let items = repeats(&out);
    assert_eq!(items.len(), 1, "{items:?}");
    let item = items[0];
    let keys: Vec<&str> = item.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, vec!["comment"], "{:?}", item.options);
    assert_eq!(item.recommended.as_deref(), Some("comment"));
    assert!(
        item.options[0]
            .effect
            .contains(&format!("/tasks/{}/comments", a.id)),
        "{:?}",
        item.options
    );
    assert!(
        item.options.iter().all(|o| !o.effect.contains("/retry")),
        "{:?}",
        item.options
    );
    assert!(
        !item.detail.as_deref().unwrap_or_default().contains("retry"),
        "{:?}",
        item.detail
    );

    // 出さない口: retry は Blocked の源を拒む（案内すると人の操作は 409 になる）。
    let err = crate::retry::plan_retry(&store, a.id, false, None, false, OffsetDateTime::now_utc())
        .expect_err("a blocked source is not retryable");
    assert!(
        matches!(err, crate::error::OpsError::InvalidState { .. }),
        "{err:?}"
    );

    // 出した口: comment は回答（`Trigger::Answer`）で源を Blocked から出し、項目は消える。
    let result = crate::comment::post_human_comment_as(
        &store,
        a.id,
        Some("human:web".into()),
        "検査器の入力を直した".into(),
        OffsetDateTime::now_utc(),
    )
    .expect("comment");
    let resumed = result
        .transition
        .expect("the comment resumes the blocked source");
    assert_eq!((resumed.from, resumed.to), (Status::Blocked, Status::Ready));
    assert_eq!(resumed.reason, Trigger::Answer.name());
    assert!(repeats(&inbox(&store)).is_empty());
}
