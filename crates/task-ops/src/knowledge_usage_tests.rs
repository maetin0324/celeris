//! ADR 2026-11-12-knowledge-usage-per-run D4: report は計測した run だけを分母にし、agent の種類・案件ごとに
//! 読み取り率を出す。curation は適用ごとの件数と統合の比率を出す。
//!
//! 守る不変条件: 計測していない run（`knowledge` が `None`）を 0 回として分母に入れない。統合の比率は
//! 統合 ÷（新規＋統合）。既存の試験は KB の使われ方を集計していない。

use super::*;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, KnowledgeUse, ProjectId, RunMetrics, RunRole, Status,
    Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn sample_task(project: Option<ProjectId>) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status: Status::Done,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::local("/tmp/ws"),
        repos: Vec::new(),
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 1,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: project,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn finished(role: Option<RunRole>, knowledge: Option<KnowledgeUse>) -> Event {
    Event::WorkerFinished {
        run_id: "run".into(),
        outcome: "done".into(),
        usage: None,
        role,
        metrics: Some(RunMetrics {
            knowledge,
            ..Default::default()
        }),
        end: None,
    }
}

fn curation(date: &str, new: u32, merged: u32, deleted: u32, fixed: u32) -> Event {
    Event::KnowledgeCurationApplied {
        date: date.into(),
        merged,
        new,
        deleted,
        fixed,
        commit_sha: None,
        commit_error: None,
        push: "skipped".into(),
        push_detail: None,
    }
}

#[test]
fn read_rate_counts_only_measured_runs_per_agent_and_project() {
    let project = ProjectId::new();
    let read = KnowledgeUse {
        search: 2,
        get: 1,
        ..Default::default()
    };
    let record_only = KnowledgeUse {
        record: 1,
        ..Default::default()
    };
    let tasks = vec![
        (
            sample_task(Some(project)),
            vec![
                (0, finished(None, Some(read))),
                (1, finished(None, Some(record_only))),
                (2, finished(None, Some(KnowledgeUse::default()))),
                // 計測していない run は分母に入れない。
                (3, finished(None, None)),
                (4, finished(Some(RunRole::Reviewer), Some(read))),
            ],
        ),
        (
            sample_task(None),
            vec![
                (0, finished(None, Some(KnowledgeUse::default()))),
                (1, curation("2026-11-02", 3, 1, 0, 2)),
            ],
        ),
        (
            sample_task(None),
            vec![(0, curation("2026-11-01", 0, 0, 4, 0))],
        ),
    ];
    let report = usage_report(&tasks, |_| "agent-platform".to_string());

    let worker = report
        .runs
        .iter()
        .find(|r| r.agent == "worker" && r.project.as_deref() == Some("agent-platform"))
        .expect("worker row");
    assert_eq!(worker.measured_runs, 3);
    assert_eq!(worker.read_runs, 1);
    assert_eq!((worker.search, worker.get, worker.record), (2, 1, 1));
    assert_eq!(worker.read_rate, Some(1.0 / 3.0));
    let reviewer = report
        .runs
        .iter()
        .find(|r| r.agent == "reviewer")
        .expect("reviewer row");
    assert_eq!((reviewer.measured_runs, reviewer.read_runs), (1, 1));
    let no_project = report
        .runs
        .iter()
        .find(|r| r.agent == "worker" && r.project.is_none())
        .expect("row without a project");
    assert_eq!(no_project.read_rate, Some(0.0));

    // curation は日付順。統合の比率は 統合 ÷（新規＋統合）、分母 0 なら None。
    let dates: Vec<_> = report
        .curation
        .iter()
        .map(|c| c.date.clone().unwrap_or_default())
        .collect();
    assert_eq!(dates, vec!["2026-11-01", "2026-11-02"]);
    assert_eq!(report.curation[0].merge_ratio, None);
    assert_eq!(report.curation[1].merge_ratio, Some(0.25));
    let total = &report.curation_total;
    assert_eq!(
        (total.new, total.merged, total.deleted, total.fixed),
        (3, 1, 4, 2)
    );
    assert_eq!(total.merge_ratio, Some(0.25));
}

#[test]
fn cos_rows_group_measured_cos_runs_per_project() {
    let project = ProjectId::new();
    let rows = cos_rows(
        &[
            (
                Some(project),
                KnowledgeUse {
                    get: 1,
                    ..Default::default()
                },
            ),
            (Some(project), KnowledgeUse::default()),
            (
                None,
                KnowledgeUse {
                    record: 1,
                    ..Default::default()
                },
            ),
        ],
        |_| "agent-platform".to_string(),
    );
    assert_eq!(rows.len(), 2);
    let general = &rows[0];
    assert_eq!(
        (general.agent.as_str(), general.project.as_deref()),
        ("cos", None)
    );
    assert_eq!((general.measured_runs, general.read_runs), (1, 0));
    let scoped = &rows[1];
    assert_eq!(scoped.project.as_deref(), Some("agent-platform"));
    assert_eq!(
        (scoped.measured_runs, scoped.read_runs, scoped.get),
        (2, 1, 1)
    );
    assert_eq!(scoped.read_rate, Some(0.5));
}
