use super::*;
use task_core::report::ReportStore;
use task_core::{
    Budget, ParentUnit, RunIndexRole, RunIndexStatus, RunRow, SqliteStore, TaskKind, Tier,
    TreeInfo, Usage, WorkUnitContext, WorkUnitSpec, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn task(title: &str, status: Status, parent: Option<(&Task, &str)>) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        paused_at: None,
        tree: parent.map(|(p, unit)| TreeInfo {
            root_id: task_core::tree::root_id_of(p),
            depth: task_core::tree::depth_of(p) + 1,
            parent_unit: Some(ParentUnit {
                task_id: p.id,
                plan_id: "p".into(),
                unit_key: unit.into(),
                stage: "s1".into(),
                attempt: 1,
            }),
            base_commit: None,
        }),
        routing: None,
        mode: Default::default(),
        skills: vec![],
        repos: vec![],
        id: TaskId::new(),
        parent_id: parent.map(|(p, _)| p.id),
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 2,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp/ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 30,
            max_wall_secs: 900,
            max_retries: 2,
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

fn task_row(key: &str, status: WorkUnitStatus, child: Option<TaskId>, seq: u32) -> WorkUnitRow {
    let mut row = WorkUnitRow::new(
        format!("id-{key}"),
        "t".into(),
        "p".into(),
        seq,
        WorkUnitSpec {
            key: key.into(),
            kind: WorkUnitKind::Task,
            title: format!("unit {key}"),
            objective: "o".into(),
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: Some("s1".into()),
        },
        status,
        "2026-09-29T00:00:00Z".into(),
    );
    row.phase = Some("s1".into());
    row.child_task_id = child.map(|c| c.to_string());
    row
}

fn run(store: &SqliteStore, t: &Task, id: &str, role: RunIndexRole, cost: Option<f64>) {
    store
        .run_index_start(RunRow {
            run_id: id.into(),
            task_id: t.id.to_string(),
            work_unit_id: None,
            role,
            seq: 1,
            status: RunIndexStatus::Completed,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: Some(Usage {
                input_tokens: Some(10),
                output_tokens: Some(1),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_usd: cost,
            }),
            metrics: None,
            started_at: "2026-09-29T10:00:00Z".into(),
            finished_at: Some("2026-09-29T10:05:00Z".into()),
        })
        .unwrap();
}

/// D11 / §7 R4a (c): 段階の子の要約は、子の subtree（孫を含む）の run・reviewer の run・定価（不完全の印）と、
/// 子の最新の報告の見出しを 1 行にする。まだ作られていない unit は「未作成」。別の段階・superseded の unit は出さない。
#[test]
fn current_stall_is_the_last_event_of_a_live_node() {
    let t = task("n", Status::Ready, None);
    let stall = Event::StallDetected {
        task_id: t.id,
        detail: "子 task が見つからない".into(),
        reason: "child_missing".into(),
        since: "2026-09-29T00:00:00Z".into(),
        path: Vec::new(),
    };
    let got = current_stall(&t, std::slice::from_ref(&stall));
    assert_eq!(
        got,
        Some(TreeNodeStall {
            reason: "child_missing".into(),
            since: Some("2026-09-29T00:00:00Z".into()),
            detail: "子 task が見つからない".into(),
        })
    );
    // 何か起きたら（最後の event が別物なら）止まりではない。
    let later = Event::Answered {
        question: "q".into(),
        answer: "a".into(),
    };
    assert_eq!(current_stall(&t, &[stall.clone(), later]), None);
    // 終端の節点は出さない。
    let done = task("n", Status::Done, None);
    assert_eq!(current_stall(&done, &[stall]), None);
    assert_eq!(current_stall(&t, &[]), None);
}

#[test]
fn stage_child_summaries_sum_the_child_subtree_and_quote_its_latest_report() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = task("root", Status::Ready, None);
    store.insert(&root).unwrap();
    let child = task("Phase 2", Status::Done, Some((&root, "p2")));
    store.insert(&child).unwrap();
    let grandchild = task("P2-A", Status::Done, Some((&child, "p2a")));
    store.insert(&grandchild).unwrap();
    run(&store, &child, "c1", RunIndexRole::Worker, Some(0.40));
    run(&store, &grandchild, "g1", RunIndexRole::Worker, None);
    run(
        &store,
        &grandchild,
        "g2",
        RunIndexRole::Reviewer,
        Some(0.10),
    );
    for (headline, at) in [
        ("古い見出し", "2026-09-29T10:00:00Z"),
        ("Phase 2 を終えました", "2026-09-29T11:00:00Z"),
    ] {
        store
            .report_append(&task_core::report::Report {
                id: task_core::report::ReportId::new(),
                project_id: None,
                node_id: "dev".into(),
                task_id: Some(child.id),
                kind: task_core::report::ReportKind::Result,
                level: 1,
                headline: headline.into(),
                body: String::new(),
                sources: vec![],
                read_at: None,
                created_at: OffsetDateTime::parse(
                    at,
                    &time::format_description::well_known::Rfc3339,
                )
                .unwrap(),
            })
            .unwrap();
    }
    let mut other_stage = task_row("x", WorkUnitStatus::Done, None, 2);
    other_stage.phase = Some("s2".into());
    let units = vec![
        task_row("p2", WorkUnitStatus::Done, Some(child.id), 0),
        task_row("p3", WorkUnitStatus::Pending, None, 1),
        other_stage,
        task_row("gone", WorkUnitStatus::Superseded, None, 3),
    ];
    let lines = stage_child_summaries(&store, &units, "s1").unwrap();
    assert_eq!(
        lines,
        vec![
            "p2 Phase 2: done / 2 run（reviewer 1）/ $0.50（不完全） — Phase 2 を終えました"
                .to_string(),
            "p3 unit p3: 未作成（pending）".to_string(),
        ]
    );
}
