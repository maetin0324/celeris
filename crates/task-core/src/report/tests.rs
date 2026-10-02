use super::*;
use crate::org::OrgKind;

fn node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        name: id.to_string(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn org() -> Vec<OrgNode> {
    vec![
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("coding-poc", Some("coding"), OrgKind::Section),
        node("infra", Some("secretary"), OrgKind::Department),
    ]
}

#[test]
fn level_and_ancestors_walk_up_to_the_secretary() {
    let org = org();
    assert_eq!(level_of(&org, "secretary"), 0);
    assert_eq!(level_of(&org, "coding"), 1);
    assert_eq!(level_of(&org, "coding-poc"), 2);
    assert_eq!(
        ancestors_of(&org, "coding-poc"),
        vec!["coding".to_string(), "secretary".to_string()]
    );
    assert!(ancestors_of(&org, "ghost").is_empty());
    assert_eq!(level_of(&org, "ghost"), 0);
}

#[test]
fn infra_node_prefers_the_infra_department_then_the_secretary() {
    let org = org();
    assert_eq!(infra_node(&org).map(|n| n.id.as_str()), Some("infra"));
    let without_infra: Vec<OrgNode> = org.into_iter().filter(|n| n.id != "infra").collect();
    assert_eq!(
        infra_node(&without_infra).map(|n| n.id.as_str()),
        Some("secretary")
    );
}

#[test]
fn done_error_and_question_have_their_own_kinds_and_wording() {
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    let done = report_for_done(
        "coding-poc",
        2,
        None,
        task,
        "PoC を書く",
        "ベンチが 1.8 倍速くなった",
        &["cargo test: exit 0".to_string()],
        &["bench.json (artifacts/bench.json)".to_string()],
        Vec::new(),
        now,
    );
    assert_eq!(done.kind, ReportKind::Result);
    assert_eq!(done.headline, "ベンチが 1.8 倍速くなった");
    assert!(done.body.contains("cargo test: exit 0"), "{}", done.body);
    assert!(done.body.contains("bench.json"), "{}", done.body);

    let long = "x".repeat(200);
    let err = report_for_error("coding-poc", 2, None, task, "PoC を書く", &long, false, now);
    assert_eq!(err.kind, ReportKind::BadNews);
    assert_eq!(
        err.headline,
        format!("PoC を書く が失敗: {}", "x".repeat(80))
    );
    assert!(err.body.contains("retryable: false"), "{}", err.body);

    let q = report_for_question(
        "coding-poc",
        2,
        None,
        task,
        "PoC を書く",
        "どのクラスタを使いますか\n（pegasus か sirius）",
        now,
    );
    assert_eq!(q.kind, ReportKind::Question);
    assert_eq!(q.headline, "どのクラスタを使いますか");
    assert!(q.body.contains("pegasus"), "{}", q.body);
}

#[test]
fn bad_news_is_copied_to_every_ancestor_up_to_the_secretary() {
    let now = OffsetDateTime::now_utc();
    let original = report_for_error(
        "coding-poc",
        2,
        None,
        TaskId::new(),
        "PoC",
        "落ちた",
        true,
        now,
    );
    let copies = bad_news_chain(&original, &org(), now);
    assert_eq!(copies.len(), 2);
    assert_eq!(copies[0].node_id, "coding");
    assert_eq!(copies[0].level, 1);
    assert_eq!(copies[0].sources, vec![original.id]);
    assert_eq!(copies[1].node_id, "secretary");
    assert_eq!(copies[1].level, 0);
    assert_eq!(copies[1].sources, vec![copies[0].id]);
    assert!(
        copies
            .iter()
            .all(|c| c.kind == ReportKind::BadNews && c.headline == original.headline)
    );
}

#[test]
fn compaction_is_due_on_the_count_or_on_the_age() {
    let now = OffsetDateTime::now_utc();
    let make = |at: OffsetDateTime| {
        report_for_done(
            "coding-poc",
            2,
            None,
            TaskId::new(),
            "t",
            "s",
            &[],
            &[],
            Vec::new(),
            at,
        )
    };
    let three: Vec<Report> = (0..3).map(|_| make(now)).collect();
    assert!(!compaction_due(&three, now, 4, 7200));
    let four: Vec<Report> = (0..4).map(|_| make(now)).collect();
    assert!(compaction_due(&four, now, 4, 7200));
    let old = vec![make(now - time::Duration::seconds(7_201))];
    assert!(compaction_due(&old, now, 4, 7200));
    assert!(!compaction_due(&[], now, 4, 7200));
}

#[test]
fn the_compaction_objective_contains_every_child_report() {
    let now = OffsetDateTime::now_utc();
    let a = report_for_done(
        "coding-poc",
        2,
        None,
        TaskId::new(),
        "A",
        "A の結果",
        &[],
        &[],
        Vec::new(),
        now,
    );
    let b = report_for_question(
        "coding-poc",
        2,
        None,
        TaskId::new(),
        "B",
        "B を聞きたい",
        now,
    );
    let text = compaction_objective(
        &node("coding", Some("secretary"), OrgKind::Department),
        &[a.clone(), b.clone()],
    );
    assert!(text.contains(&a.headline), "{text}");
    assert!(text.contains(&b.headline), "{text}");
    assert!(text.contains("3〜8 行"), "{text}");
    // ADR-0006 Phase 115 D1: この静的な文字列は work_dir を知らないので、`artifacts/result.json`
    // を（絶対でも相対でも）断定しない。実行時の Instructions に置き場を任せる。
    assert!(!text.contains("artifacts/result.json"), "{text}");
}

#[test]
fn notification_waits_two_hours_unless_there_is_bad_news() {
    let now = OffsetDateTime::now_utc();
    // 2 時間未満で未読 → 通知しない。
    assert!(!notify_now(
        3,
        0,
        Some(now - time::Duration::minutes(30)),
        now
    ));
    // 2 時間以上で未読 → 通知する。
    assert!(notify_now(3, 0, Some(now - time::Duration::hours(3)), now));
    // 悪い知らせは即時。
    assert!(notify_now(
        1,
        1,
        Some(now - time::Duration::minutes(1)),
        now
    ));
    // 未読が無ければ通知しない。
    assert!(!notify_now(0, 0, None, now));
}

// ---- `reports` 表（SQLite）----

fn store_with_org() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("open");
    for n in org() {
        crate::store::TaskStore::org_upsert(&store, &n).expect("seed org");
    }
    store
}

fn sample(
    node_id: &str,
    level: u32,
    kind: ReportKind,
    project: Option<ProjectId>,
    at: OffsetDateTime,
) -> Report {
    Report {
        id: ReportId::new(),
        project_id: project,
        node_id: node_id.to_string(),
        task_id: None,
        kind,
        level,
        headline: format!("{node_id} の報告"),
        body: "body".into(),
        sources: Vec::new(),
        read_at: None,
        created_at: at,
    }
}

#[test]
fn append_round_trips_including_a_report_without_a_project() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    let with_project = sample("coding-poc", 2, ReportKind::Result, Some(project), now);
    let without = sample("infra", 1, ReportKind::BadNews, None, now);
    store
        .report_append_all(&[with_project.clone(), without.clone()])
        .expect("append");
    assert_eq!(
        store.report_get(with_project.id).expect("get"),
        Some(with_project.clone())
    );
    let read_back = store.report_get(without.id).expect("get").expect("some");
    assert_eq!(read_back.project_id, None);
    assert_eq!(read_back, without);
    assert_eq!(store.report_get(ReportId::new()).expect("get"), None);
}

#[test]
fn list_filters_by_project_node_level_and_unread_and_is_newest_first() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = ProjectId::new();
    let other = ProjectId::new();
    let old = sample(
        "coding-poc",
        2,
        ReportKind::Result,
        Some(project),
        now - time::Duration::hours(1),
    );
    let new = sample("coding-poc", 2, ReportKind::Result, Some(project), now);
    let elsewhere = sample(
        "coding",
        1,
        ReportKind::Result,
        Some(other),
        now - time::Duration::minutes(30),
    );
    store
        .report_append_all(&[old.clone(), new.clone(), elsewhere.clone()])
        .expect("append");

    let all = store.report_list(&ReportFilter::default()).expect("list");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].id, new.id, "newest first");

    let by_project = store
        .report_list(&ReportFilter {
            project_id: Some(project),
            ..ReportFilter::default()
        })
        .expect("list");
    assert_eq!(by_project.len(), 2);
    let by_node = store
        .report_list(&ReportFilter {
            node_id: Some("coding".into()),
            ..ReportFilter::default()
        })
        .expect("list");
    assert_eq!(
        by_node.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![elsewhere.id]
    );
    let by_level = store
        .report_list(&ReportFilter {
            level: Some(1),
            ..ReportFilter::default()
        })
        .expect("list");
    assert_eq!(by_level.len(), 1);
    let limited = store
        .report_list(&ReportFilter {
            limit: 1,
            ..ReportFilter::default()
        })
        .expect("list");
    assert_eq!(limited.len(), 1);

    assert_eq!(store.report_mark_read(&[new.id], now).expect("read"), 1);
    // 既読のものをもう一度既読にしても何も変わらない。
    assert_eq!(store.report_mark_read(&[new.id], now).expect("read"), 0);
    let unread = store
        .report_list(&ReportFilter {
            unread_only: true,
            ..ReportFilter::default()
        })
        .expect("list");
    assert_eq!(unread.len(), 2);
    assert!(unread.iter().all(|r| r.id != new.id));
    assert_eq!(store.report_unread_counts(2).expect("counts"), (1, 0));
}

#[test]
fn unreviewed_children_skips_reports_that_are_already_a_source() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let a = sample(
        "coding-poc",
        2,
        ReportKind::Result,
        None,
        now - time::Duration::minutes(2),
    );
    let b = sample(
        "coding-poc",
        2,
        ReportKind::Result,
        None,
        now - time::Duration::minutes(1),
    );
    store
        .report_append_all(&[a.clone(), b.clone()])
        .expect("append");
    let pending = store.report_unreviewed_children("coding").expect("pending");
    assert_eq!(
        pending.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a.id, b.id],
        "oldest first"
    );
    // 別の親（secretary）から見ると、coding の報告だけが対象（孫は見ない）。
    assert!(
        store
            .report_unreviewed_children("secretary")
            .expect("pending")
            .is_empty()
    );

    // まとめの報告が a を取り込むと、a は次回の対象から外れる。
    let mut summary = sample("coding", 1, ReportKind::Result, None, now);
    summary.sources = vec![a.id];
    store.report_append(&summary).expect("append");
    let pending = store.report_unreviewed_children("coding").expect("pending");
    assert_eq!(pending.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b.id]);
    // まとめ自身は secretary から見たレビュー対象になる。
    let up = store
        .report_unreviewed_children("secretary")
        .expect("pending");
    assert_eq!(
        up.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![summary.id]
    );
}

#[test]
fn a_bad_news_chain_leaves_only_the_top_copy_unreviewed() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let original = report_for_error(
        "coding-poc",
        2,
        None,
        TaskId::new(),
        "PoC",
        "落ちた",
        true,
        now,
    );
    let mut all = vec![original.clone()];
    all.extend(bad_news_chain(&original, &org(), now));
    store.report_append_all(&all).expect("append");
    // 各段のコピーが 1 段下を sources に持つので、圧縮の対象にはならない。
    assert!(
        store
            .report_unreviewed_children("coding")
            .expect("pending")
            .is_empty()
    );
    assert!(
        store
            .report_unreviewed_children("secretary")
            .expect("pending")
            .is_empty()
    );
    assert_eq!(store.report_unread_counts(0).expect("counts"), (1, 1));
}

fn plain_task(kind: TaskKind) -> Task {
    use crate::model::{Budget, Check, Criterion, Status, Tier, WorkerHint, WorkspaceSpec};
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
        kind,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
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

/// GUI 監査 H4（Phase 29）: 裏方タスクの印の優先順（対話 > 圧縮 > 承認 > 合成レビュー）。
#[test]
fn support_kind_classifies_background_tasks_by_a_fixed_priority() {
    let mut plain = plain_task(TaskKind::Execute);
    assert_eq!(
        support_kind(&plain),
        None,
        "人が見る本体の仕事には印を付けない"
    );

    plain.conversation = Some(crate::message::MessageId::new());
    assert_eq!(support_kind(&plain), Some("conversation"));

    let mut compaction = plain_task(TaskKind::Execute);
    compaction.role = Some(COMPACTION_ROLE.to_string());
    assert_eq!(support_kind(&compaction), Some("compaction"));

    // ADR-0047 D4（Phase 62）: 知識整理 run も裏方。
    let mut knowledge = plain_task(TaskKind::Execute);
    knowledge.role = Some(KNOWLEDGE_ROLE.to_string());
    assert_eq!(support_kind(&knowledge), Some("knowledge"));

    let approval = plain_task(TaskKind::Approval);
    assert_eq!(support_kind(&approval), Some("approval"));

    let review = plain_task(TaskKind::Review);
    assert_eq!(support_kind(&review), Some("review"));

    // Phase 42（実機 2026-09-18）: 計画 run は裏方。途中目標の「仕事」に数えない。
    let plan = plain_task(TaskKind::Plan);
    assert_eq!(support_kind(&plan), Some("plan"));

    // 対話が最優先（他の条件と重なっても対話が勝つ）。
    let mut both = plain_task(TaskKind::Approval);
    both.conversation = Some(crate::message::MessageId::new());
    assert_eq!(support_kind(&both), Some("conversation"));
}
