use super::*;
use crate::execution_plan::{WorkUnitContext, WorkUnitSpec};
use crate::model::{RunRole, Usage};
use crate::quota::QuotaWindow;

fn run(
    id: &str,
    role: RunIndexRole,
    start: &str,
    end: Option<&str>,
    tokens: (u64, u64),
    cost: Option<f64>,
) -> RunRow {
    RunRow {
        run_id: id.to_string(),
        task_id: "t".to_string(),
        work_unit_id: None,
        role,
        seq: 1,
        status: if end.is_some() {
            RunIndexStatus::Completed
        } else {
            RunIndexStatus::Running
        },
        adapter: None,
        model: None,
        account: None,
        session_id: None,
        checkpoint: None,
        usage: Some(Usage {
            input_tokens: Some(tokens.0),
            output_tokens: Some(tokens.1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: cost,
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        metrics: None,
        started_at: start.to_string(),
        finished_at: end.map(str::to_string),
    }
}

fn unit(key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
    WorkUnitRow::new(
        format!("id-{key}"),
        "t".to_string(),
        "p".to_string(),
        0,
        WorkUnitSpec {
            key: key.to_string(),
            kind,
            title: key.to_string(),
            objective: "o".to_string(),
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: None,
        },
        status,
        "2026-09-29T00:00:00Z".to_string(),
    )
}

fn quota(runs: u32, pct: f64, role: RunRole) -> QuotaUse {
    QuotaUse {
        source: "claude-oauth".to_string(),
        account: Some("acct".to_string()),
        window: QuotaWindow::FiveHour,
        used_pct: Some(pct),
        runs,
        method_counts: [("measured".to_string(), runs)].into_iter().collect(),
        runs_by_role: [(role.as_str().to_string(), runs)].into_iter().collect(),
    }
}

/// ADR-0079 §7 R4a (a) `rollup_matches_hand_computed_fixture`: root → child → grandchild の 3 段で、各節点の
/// subtree の run・トークン・定価（`cost_usd_complete` の伝播）・quota・壁時計・leaf の done / total・
/// 未回答の決定が手計算と一致する。
#[test]
fn rollup_matches_hand_computed_fixture() {
    let root = TaskId::new();
    let child = TaskId::new();
    let grandchild = TaskId::new();
    let nodes = vec![
        RollupNodeFacts {
            task_id: root,
            parent_id: None,
            depth: 1,
            // planner 1 本（10:00〜10:01）、reviewer 1 本（12:00〜12:05、$0.50）。
            runs: vec![
                run(
                    "r-plan",
                    RunIndexRole::Planner,
                    "2026-09-29T10:00:00Z",
                    Some("2026-09-29T10:01:00Z"),
                    (100, 10),
                    Some(0.10),
                ),
                run(
                    "r-rev",
                    RunIndexRole::Reviewer,
                    "2026-09-29T12:00:00Z",
                    Some("2026-09-29T12:05:00Z"),
                    (50, 5),
                    Some(0.50),
                ),
            ],
            work_units: vec![
                unit("a", WorkUnitKind::Implement, WorkUnitStatus::Done),
                unit("c", WorkUnitKind::Task, WorkUnitStatus::Running),
                unit(
                    "integrate-s1",
                    WorkUnitKind::Integrate,
                    WorkUnitStatus::Pending,
                ),
                unit("old", WorkUnitKind::Implement, WorkUnitStatus::Superseded),
            ],
            quota: vec![quota(1, 2.0, RunRole::Planner)],
            open_decisions: 1,
        },
        RollupNodeFacts {
            task_id: child,
            parent_id: Some(root),
            depth: 2,
            // worker 1 本（10:10〜10:20、$1.00）、単価不明の worker 1 本（10:30〜10:40）。
            runs: vec![
                run(
                    "c-w1",
                    RunIndexRole::Worker,
                    "2026-09-29T10:10:00Z",
                    Some("2026-09-29T10:20:00Z"),
                    (1000, 100),
                    Some(1.00),
                ),
                run(
                    "c-w2",
                    RunIndexRole::Worker,
                    "2026-09-29T10:30:00Z",
                    Some("2026-09-29T10:40:00Z"),
                    (200, 20),
                    None,
                ),
            ],
            work_units: vec![
                unit("x", WorkUnitKind::Implement, WorkUnitStatus::Done),
                unit("y", WorkUnitKind::Implement, WorkUnitStatus::Ready),
                unit("g", WorkUnitKind::Task, WorkUnitStatus::Done),
            ],
            quota: vec![quota(2, 3.5, RunRole::Worker)],
            open_decisions: 2,
        },
        RollupNodeFacts {
            task_id: grandchild,
            parent_id: Some(child),
            depth: 3,
            // worker 1 本（09:50〜11:00、$0.25）と reviewer 1 本（11:00〜、まだ走っている、$0.05）。
            runs: vec![
                run(
                    "g-w",
                    RunIndexRole::Worker,
                    "2026-09-29T09:50:00Z",
                    Some("2026-09-29T11:00:00Z"),
                    (300, 30),
                    Some(0.25),
                ),
                run(
                    "g-rev",
                    RunIndexRole::Reviewer,
                    "2026-09-29T11:00:00Z",
                    None,
                    (0, 0),
                    Some(0.05),
                ),
            ],
            work_units: vec![],
            quota: vec![quota(1, 1.0, RunRole::Worker)],
            open_decisions: 0,
        },
    ];
    let out = rollup(&nodes);
    assert_eq!(out.len(), 3);
    let (r, c, g) = (&out[0], &out[1], &out[2]);

    // 孫: 自分の分 = subtree。
    assert_eq!(g.own, g.subtree);
    assert_eq!(g.subtree.tasks, 1);
    assert_eq!(g.subtree.runs, 1);
    assert_eq!(g.subtree.reviewer_runs, 1);
    assert_eq!(g.subtree.runs_in_flight, 1);
    assert_eq!(g.subtree.tokens, 330);
    assert!((g.subtree.cost_usd - 0.30).abs() < 1e-9);
    assert!((g.subtree.reviewer_cost_usd - 0.05).abs() < 1e-9);
    assert!(g.subtree.cost_usd_complete);
    assert_eq!(g.subtree.wall_ms, Some(70 * 60 * 1000));
    assert_eq!(g.subtree.busy_ms, 70 * 60 * 1000);

    // 子: 自分（worker 2、1,320 トークン、$1.00 だが不完全、leaf 1/2、子 task 1/1、決定 2）+ 孫。
    assert_eq!(c.own.runs, 2);
    assert!(
        !c.own.cost_usd_complete,
        "an unpriced run makes the cost a lower bound"
    );
    assert_eq!((c.own.leaves_done, c.own.leaves_total), (1, 2));
    assert_eq!((c.own.child_tasks_done, c.own.child_tasks_total), (1, 1));
    assert_eq!(c.subtree.tasks, 2);
    assert_eq!(c.subtree.runs, 3);
    assert_eq!(c.subtree.reviewer_runs, 1);
    assert_eq!(c.subtree.runs_by_role.get("worker"), Some(&3));
    assert_eq!(c.subtree.runs_by_role.get("reviewer"), Some(&1));
    assert_eq!(c.subtree.tokens, 1320 + 330);
    assert!((c.subtree.cost_usd - 1.30).abs() < 1e-9);
    assert!(!c.subtree.cost_usd_complete, "incompleteness propagates up");
    assert_eq!(c.subtree.open_decisions, 2);
    assert_eq!((c.subtree.leaves_done, c.subtree.leaves_total), (1, 2));
    // 壁時計: 孫の 09:50 → 孫の 11:00（子の run は 10:10〜10:40 でその内側）。
    assert_eq!(
        c.subtree.first_run_started_at.as_deref(),
        Some("2026-09-29T09:50:00Z")
    );
    assert_eq!(
        c.subtree.last_run_finished_at.as_deref(),
        Some("2026-09-29T11:00:00Z")
    );
    assert_eq!(c.subtree.wall_ms, Some(70 * 60 * 1000));
    assert_eq!(c.subtree.busy_ms, (10 + 10 + 70) * 60 * 1000);
    assert_eq!(c.subtree.quota.len(), 1);
    assert_eq!(c.subtree.quota[0].runs, 3);
    assert_eq!(c.subtree.quota[0].used_pct, Some(4.5));

    // root: 3 節点の合計。leaf は root の a（done、superseded の old は数えない）と子の x / y。
    assert_eq!(r.subtree.tasks, 3);
    assert_eq!(
        r.subtree.runs, 4,
        "planner 1 + child workers 2 + grandchild worker 1"
    );
    assert_eq!(r.subtree.reviewer_runs, 2);
    assert_eq!(r.subtree.runs_by_role.get("planner"), Some(&1));
    assert_eq!(r.subtree.runs_by_role.get("worker"), Some(&3));
    assert_eq!(r.subtree.runs_by_role.get("reviewer"), Some(&2));
    assert_eq!(r.subtree.input_tokens, 100 + 50 + 1000 + 200 + 300);
    assert_eq!(r.subtree.output_tokens, 10 + 5 + 100 + 20 + 30);
    assert_eq!(r.subtree.tokens, 1815);
    assert!((r.subtree.cost_usd - 1.90).abs() < 1e-9);
    assert!((r.subtree.reviewer_cost_usd - 0.55).abs() < 1e-9);
    assert!(!r.subtree.cost_usd_complete);
    assert_eq!((r.subtree.leaves_done, r.subtree.leaves_total), (2, 3));
    assert_eq!(
        (r.subtree.child_tasks_done, r.subtree.child_tasks_total),
        (1, 2)
    );
    assert_eq!(r.subtree.open_decisions, 3);
    assert_eq!(r.subtree.runs_in_flight, 1);
    // 壁時計: 孫の 09:50 → root の reviewer の 12:05 = 2h15m。
    assert_eq!(r.subtree.wall_ms, Some(135 * 60 * 1000));
    assert_eq!(r.subtree.busy_ms, (1 + 5 + 10 + 10 + 70) * 60 * 1000);
    assert_eq!(r.subtree.quota[0].runs, 4);
    assert_eq!(r.subtree.quota[0].used_pct, Some(6.5));
    assert_eq!(
        r.subtree.quota[0].runs_by_role.get("worker"),
        Some(&3),
        "{:?}",
        r.subtree.quota
    );
    // root の合計 = subtree の和。
    assert_eq!(
        r.subtree,
        RollupMetrics::sum([&r.own, &c.own, &g.own]),
        "the root's totals equal the sum over the subtree"
    );

    // 深さ別（U-R7）: 深さ 1 の reviewer 1 本 $0.50、深さ 3 の reviewer 1 本 $0.05。
    let depths = by_depth(&nodes);
    assert_eq!(
        depths.iter().map(|d| d.depth).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(depths[0].metrics.reviewer_runs, 1);
    assert!((depths[0].metrics.reviewer_cost_usd - 0.50).abs() < 1e-9);
    assert_eq!(depths[1].metrics.reviewer_runs, 0);
    assert_eq!(depths[2].metrics.reviewer_runs, 1);
    assert_eq!(depths[1].metrics, c.own);
}

#[test]
fn a_node_without_runs_has_no_wall_clock_and_a_complete_cost() {
    let m = node_metrics(&RollupNodeFacts {
        task_id: TaskId::new(),
        parent_id: None,
        depth: 1,
        runs: vec![],
        work_units: vec![],
        quota: vec![],
        open_decisions: 0,
    });
    assert_eq!(m.tasks, 1);
    assert_eq!(m.wall_ms, None);
    assert!(m.cost_usd_complete);
    assert_eq!(RollupMetrics::sum([]), RollupMetrics::default());
}
