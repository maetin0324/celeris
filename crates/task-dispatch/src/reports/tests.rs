use super::*;
use std::sync::Arc;
use task_core::org::{OrgKind, OrgNode};
use task_core::{
    Budget, ProjectId, SqliteStore, Status, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};
use task_worker::Evidence;

fn org_node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn store_with_org() -> Arc<dyn TaskStore> {
    let store = SqliteStore::open_in_memory().expect("open");
    for n in [
        org_node("secretary", None, OrgKind::Secretary),
        org_node("coding", Some("secretary"), OrgKind::Department),
        org_node("coding-poc", Some("coding"), OrgKind::Section),
        org_node("infra", Some("secretary"), OrgKind::Department),
    ] {
        store.org_upsert(&n).expect("seed");
    }
    Arc::new(store)
}

fn task(assignee: Option<&str>, project: Option<ProjectId>) -> Task {
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
        title: "PoC を書く".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Cheap,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: ".".into(),
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
        project_id: project,
        milestone_id: None,
        assignee: assignee.map(str::to_string),
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn done(summary: &str) -> Result<RunOutcome, AdapterError> {
    Ok(RunOutcome {
        terminal: Terminal::Done {
            summary: summary.into(),
            evidence: vec![Evidence {
                criterion: 0,
                command: Some("cargo test".into()),
                exit: Some(0),
                stdout_tail: None,
            }],
            usage: None,
        },
        exit_code: Some(0),
    })
}

#[test]
fn a_done_run_becomes_one_result_report_for_the_assignee() {
    let store = store_with_org();
    let project = ProjectId::new();
    let task = task(Some("coding-poc"), Some(project));
    store.insert(&task).expect("insert");
    let terminal = terminal_report(&done("ベンチが 1.8 倍速くなった")).expect("terminal");
    let report = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("record")
    .expect("some");
    assert_eq!(report.kind, report::ReportKind::Result);
    assert_eq!(report.node_id, "coding-poc");
    assert_eq!(report.level, 2);
    assert_eq!(report.project_id, Some(project));
    assert_eq!(report.headline, "ベンチが 1.8 倍速くなった");
    assert!(report.body.contains("cargo test"), "{}", report.body);
    // 良い知らせは複製されない（圧縮を待つ）。
    let all = store
        .report_list(&task_core::ReportFilter::default())
        .expect("list");
    assert_eq!(all.len(), 1);
}

/// ADR-0034 D7（Phase 27）: ワーカーが `report.kind` を宣言したら、**固定表で写すだけ**。
/// `"proposal"` 以外・未知・欠落は従来どおり `result`。
#[test]
fn a_worker_can_declare_its_done_result_as_a_proposal() {
    let store = store_with_org();
    let task = task(Some("coding-poc"), Some(ProjectId::new()));
    store.insert(&task).expect("insert");
    let terminal = terminal_report(&done("この framing で論文が書けそう")).expect("terminal");
    let report = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        Some("proposal"),
        OffsetDateTime::now_utc(),
    )
    .expect("record")
    .expect("some");
    assert_eq!(report.kind, report::ReportKind::Proposal);
    // 固定表（未知・欠落は `result`。`bad_news` / `question` を `done` から名乗らせはしない）。
    assert_eq!(
        declared_kind(Some("proposal")),
        report::ReportKind::Proposal
    );
    assert_eq!(declared_kind(Some("bad_news")), report::ReportKind::Result);
    assert_eq!(declared_kind(Some("bogus")), report::ReportKind::Result);
    assert_eq!(declared_kind(None), report::ReportKind::Result);
}

/// 監査 M-6（Phase 27）: 対話用タスク（人への返事）は報告を作らない。
#[test]
fn a_conversation_task_produces_no_report() {
    let store = store_with_org();
    let mut task = task(Some("coding-poc"), Some(ProjectId::new()));
    task.conversation = Some(task_core::MessageId::new());
    store.insert(&task).expect("insert");
    for terminal in [
        terminal_report(&done("順調です")).expect("terminal"),
        TerminalReport::Error {
            message: "落ちた".into(),
            retryable: false,
        },
        TerminalReport::Question {
            text: "どちらにしますか".into(),
        },
    ] {
        assert_eq!(
            record_run_report(
                store.as_ref(),
                &task,
                "run-1",
                &terminal,
                None,
                OffsetDateTime::now_utc()
            )
            .expect("record"),
            None
        );
    }
    assert!(
        store
            .report_list(&task_core::ReportFilter::default())
            .expect("list")
            .is_empty()
    );
}

#[test]
fn a_task_without_an_assignee_produces_no_report() {
    let store = store_with_org();
    let task = task(None, Some(ProjectId::new()));
    store.insert(&task).expect("insert");
    let terminal = terminal_report(&done("できました")).expect("terminal");
    let made = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("record");
    assert!(made.is_none());
    assert!(
        store
            .report_list(&task_core::ReportFilter::default())
            .expect("list")
            .is_empty()
    );
}

#[test]
fn an_unknown_assignee_produces_no_report_instead_of_becoming_the_secretarys() {
    // 監査 M-5: `level_of` は未知ノードで 0 を返すので、チェック無しだと「秘書の報告」に化ける。
    let store = store_with_org();
    let task = task(Some("no-such-node"), Some(ProjectId::new()));
    store.insert(&task).expect("insert");
    let terminal = terminal_report(&done("できました")).expect("terminal");
    let made = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("record");
    assert!(made.is_none());
    assert!(
        store
            .report_list(&task_core::ReportFilter::default())
            .expect("list")
            .is_empty()
    );
}

#[test]
fn an_error_run_becomes_bad_news_that_reaches_the_secretary() {
    let store = store_with_org();
    let task = task(Some("coding-poc"), Some(ProjectId::new()));
    store.insert(&task).expect("insert");
    let result: Result<RunOutcome, AdapterError> = Ok(RunOutcome {
        terminal: Terminal::Error {
            message: "ビルドが壊れている".into(),
            retryable: true,
        },
        exit_code: Some(1),
    });
    let terminal = terminal_report(&result).expect("terminal");
    let report = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("record")
    .expect("some");
    assert_eq!(report.kind, report::ReportKind::BadNews);
    assert_eq!(report.headline, "PoC を書く が失敗: ビルドが壊れている");
    assert!(report.body.contains("retryable: true"), "{}", report.body);
    let at_secretary = store
        .report_list(&task_core::ReportFilter {
            level: Some(0),
            ..Default::default()
        })
        .expect("list");
    assert_eq!(at_secretary.len(), 1, "秘書まで複製される");
    assert_eq!(at_secretary[0].headline, report.headline);
    assert_eq!(
        store
            .report_list(&task_core::ReportFilter::default())
            .expect("list")
            .len(),
        3
    );
}

#[test]
fn a_question_run_becomes_a_question_report() {
    let store = store_with_org();
    let task = task(Some("coding-poc"), Some(ProjectId::new()));
    store.insert(&task).expect("insert");
    let result: Result<RunOutcome, AdapterError> = Ok(RunOutcome {
        terminal: Terminal::Question {
            text: "どのクラスタを使いますか".into(),
        },
        exit_code: Some(0),
    });
    let terminal = terminal_report(&result).expect("terminal");
    let report = record_run_report(
        store.as_ref(),
        &task,
        "run-1",
        &terminal,
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("record")
    .expect("some");
    assert_eq!(report.kind, report::ReportKind::Question);
    assert_eq!(report.headline, "どのクラスタを使いますか");
    // 質問は複製しない（悪い知らせではない）。
    assert_eq!(
        store
            .report_list(&task_core::ReportFilter::default())
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn a_provider_failure_is_not_a_report() {
    let err: Result<RunOutcome, AdapterError> = Err(AdapterError::Throttled {
        retry_after: std::time::Duration::from_secs(60),
    });
    assert_eq!(terminal_report(&err), None);
}

#[test]
fn a_cluster_that_cannot_be_reached_is_bad_news_for_infra() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let report = record_cluster_unavailable_report(
        store.as_ref(),
        "pegasus",
        "pegasus.ccs",
        "no master",
        None,
        now,
    )
    .expect("record")
    .expect("some");
    assert_eq!(report.node_id, "infra");
    assert_eq!(report.kind, report::ReportKind::BadNews);
    assert_eq!(report.project_id, None, "案件なし");
    assert_eq!(report.headline, "pegasus.ccs に接続できない");
    // infra → 秘書まで上がる。
    let at_secretary = store
        .report_list(&task_core::ReportFilter {
            level: Some(0),
            ..Default::default()
        })
        .expect("list");
    assert_eq!(at_secretary.len(), 1);
    // 同じホストの障害は間引かれる。
    assert!(
        cluster_report_recently_recorded(store.as_ref(), "pegasus.ccs", now, 3600).expect("check")
    );
    assert!(!cluster_report_recently_recorded(store.as_ref(), "sirius", now, 3600).expect("check"));
}

#[test]
fn a_compaction_run_becomes_the_parents_report_with_the_children_as_sources() {
    let store = store_with_org();
    let project = ProjectId::new();
    let now = OffsetDateTime::now_utc();
    // 子の報告 4 件（まとめのタスクより前に作られたもの）。
    let children: Vec<task_core::Report> = (0..4)
        .map(|n| task_core::Report {
            id: task_core::ReportId::new(),
            project_id: Some(project),
            node_id: "coding-poc".into(),
            task_id: None,
            kind: report::ReportKind::Result,
            level: 2,
            headline: format!("結果 {n}"),
            body: "本文".into(),
            sources: Vec::new(),
            read_at: None,
            created_at: now - time::Duration::minutes(10),
        })
        .collect();
    store.report_append_all(&children).expect("append");

    let mut compaction = task(Some("coding"), Some(project));
    compaction.role = Some(report::COMPACTION_ROLE.into());
    compaction.title = "報告のまとめ: coding".into();
    store.insert(&compaction).expect("insert");

    let terminal = terminal_report(&done("PoC は 1.8 倍速く、論文の framing は X で書けそう"))
        .expect("terminal");
    let summary = record_run_report(store.as_ref(), &compaction, "run-1", &terminal, None, now)
        .expect("record")
        .expect("some");
    assert_eq!(summary.node_id, "coding");
    assert_eq!(summary.level, 1);
    assert_eq!(summary.kind, report::ReportKind::Result);
    let mut got = summary.sources.clone();
    let mut want: Vec<_> = children.iter().map(|c| c.id).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want, "子の報告が sources に入る");
    // 子は次回の対象から外れ、まとめは 1 段上（秘書）の対象になる。
    assert!(
        store
            .report_unreviewed_children("coding")
            .expect("pending")
            .is_empty()
    );
    let up = store
        .report_unreviewed_children("secretary")
        .expect("pending");
    assert_eq!(
        up.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![summary.id]
    );

    // まとめる対象が 1 件も無ければ報告を作らない（やり直しで二重に作らないため）。
    let again = record_run_report(store.as_ref(), &compaction, "run-2", &terminal, None, now)
        .expect("record");
    assert!(again.is_none());
}

#[test]
fn a_compaction_runs_sources_do_not_cross_projects() {
    // 監査 H-1: 案件 A と B のまとめ run が並行しても、A のまとめの sources に B の子報告が混ざらない。
    let store = store_with_org();
    let project_a = ProjectId::new();
    let project_b = ProjectId::new();
    let now = OffsetDateTime::now_utc();
    let child_a = task_core::Report {
        id: task_core::ReportId::new(),
        project_id: Some(project_a),
        node_id: "coding-poc".into(),
        task_id: None,
        kind: report::ReportKind::Result,
        level: 2,
        headline: "A の結果".into(),
        body: "本文 A".into(),
        sources: Vec::new(),
        read_at: None,
        created_at: now - time::Duration::minutes(10),
    };
    let child_b = task_core::Report {
        id: task_core::ReportId::new(),
        project_id: Some(project_b),
        node_id: "coding-poc".into(),
        task_id: None,
        kind: report::ReportKind::Result,
        level: 2,
        headline: "B の結果".into(),
        body: "本文 B".into(),
        sources: Vec::new(),
        read_at: None,
        created_at: now - time::Duration::minutes(10),
    };
    store
        .report_append_all(&[child_a.clone(), child_b.clone()])
        .expect("append");

    let mut compaction_a = task(Some("coding"), Some(project_a));
    compaction_a.role = Some(report::COMPACTION_ROLE.into());
    store.insert(&compaction_a).expect("insert");
    let mut compaction_b = task(Some("coding"), Some(project_b));
    compaction_b.role = Some(report::COMPACTION_ROLE.into());
    store.insert(&compaction_b).expect("insert");

    let terminal = terminal_report(&done("A の案件のまとめ")).expect("terminal");
    let summary_a = record_run_report(store.as_ref(), &compaction_a, "run-a", &terminal, None, now)
        .expect("record")
        .expect("some");
    assert_eq!(
        summary_a.sources,
        vec![child_a.id],
        "A のまとめには A の子だけが入る"
    );

    // B は A のまとめが done になった後でも、自分の子だけをまとめて done にできる。
    let terminal_b = terminal_report(&done("B の案件のまとめ")).expect("terminal");
    let summary_b = record_run_report(
        store.as_ref(),
        &compaction_b,
        "run-b",
        &terminal_b,
        None,
        now,
    )
    .expect("record")
    .expect("some");
    assert_eq!(
        summary_b.sources,
        vec![child_b.id],
        "B のまとめには B の子だけが入る"
    );
}
