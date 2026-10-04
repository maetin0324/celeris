use super::*;
use crate::execution_gate::{ExecutionGateDecision, GateSource};
use crate::execution_plan::PlanOrigin;
use crate::execution_plan::{
    EXECUTION_PLAN_SCHEMA, ExecutionPlanSpec, WorkUnitContext, WorkUnitSpec,
};
use crate::model::{TaskRouting, Usage};
use crate::model_policy::tests::task as sample_task;

fn wu(key: &str, kind: WorkUnitKind, title: &str, depends_on: &[&str]) -> WorkUnitSpec {
    WorkUnitSpec {
        key: key.to_string(),
        kind,
        title: title.to_string(),
        objective: format!("objective for {key}"),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    }
}

#[test]
fn empty_events_give_all_zero_defaults() {
    let task = sample_task("x", vec![]);
    let m = summarize(&task, &[]);
    assert_eq!(m.gate_mode, None);
    assert!(!m.has_plan);
    assert_eq!(m.work_units_total, 0);
    assert_eq!(m.continuations, 0);
    assert_eq!(m.repairs_total, 0);
    assert_eq!(m.replans, 0);
    assert_eq!(m.final_status, task.status);
}

#[test]
fn gate_decision_on_the_task_is_reflected() {
    let mut task = sample_task("x", vec![]);
    task.routing = Some(TaskRouting {
        execution: Some(ExecutionGateDecision {
            mode: ExecutionMode::Compound,
            source: GateSource::Policy,
            score: 6,
            threshold: 5,
            rule_id: "compound/score".to_string(),
            signals: vec![],
            policy_version: "exec-gate/1".to_string(),
            shadow: true,
            depth: None,
        }),
        ..TaskRouting::default()
    });
    let m = summarize(&task, &[]);
    assert_eq!(m.gate_mode, Some(ExecutionMode::Compound));
    assert!(m.gate_shadow);
}

#[test]
fn continuations_are_counted_from_the_continue_reason() {
    let task = sample_task("x", vec![]);
    let events = vec![
        Event::Transitioned {
            from: Status::Running,
            to: Status::Ready,
            reason: "continue".to_string(),
        },
        Event::Transitioned {
            from: Status::Running,
            to: Status::Ready,
            reason: "continue".to_string(),
        },
        Event::Transitioned {
            from: Status::Running,
            to: Status::Ready,
            reason: "worker_error".to_string(),
        },
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.continuations, 2);
    assert_eq!(m.retries, 1);
}

#[test]
fn budget_exhausted_is_grouped_by_kind_and_turns_is_mirrored() {
    let task = sample_task("x", vec![]);
    let fin = |kind: BudgetKind| Event::WorkerFinished {
        run_id: "r".to_string(),
        outcome: "continue: budget".to_string(),
        usage: None,
        role: None,
        metrics: None,
        end: Some(RunEnd::BudgetExhausted { kind }),
    };
    let events = vec![
        fin(BudgetKind::Turns),
        fin(BudgetKind::Turns),
        fin(BudgetKind::WallClock),
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.budget_exhausted_by_kind.get("turns"), Some(&2));
    assert_eq!(m.budget_exhausted_by_kind.get("wall_clock"), Some(&1));
    assert_eq!(m.max_turn_failures, 2);
}

#[test]
fn work_units_and_replans_are_derived_from_execution_planned_and_transitions() {
    let task = sample_task("x", vec![]);
    let plan1 = ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "r".to_string(),
        work_units: vec![
            wu("a", WorkUnitKind::Implement, "a", &[]),
            wu("b", WorkUnitKind::Implement, "b", &["a"]),
        ],
        phases: Vec::new(),
        children: Vec::new(),
    };
    let plan2 = ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "r".to_string(),
        work_units: vec![
            wu("a", WorkUnitKind::Implement, "a", &[]),
            wu("b", WorkUnitKind::Implement, "b", &["a"]),
            wu("c", WorkUnitKind::Implement, "c", &["b"]),
        ],
        phases: Vec::new(),
        children: Vec::new(),
    };
    let events = vec![
        Event::ExecutionPlanned {
            plan_id: "p1".to_string(),
            version: 1,
            origin: PlanOrigin::Planner,
            supersedes: None,
            reason: None,
            plan: Box::new(plan1),
        },
        Event::WorkUnitTransitioned {
            work_unit_id: "wu-a".to_string(),
            key: "a".to_string(),
            from: WorkUnitStatus::Ready,
            to: WorkUnitStatus::Done,
            reason: "completed".to_string(),
            run_id: Some("r1".to_string()),
        },
        Event::ExecutionPlanned {
            plan_id: "p2".to_string(),
            version: 2,
            origin: PlanOrigin::Human,
            supersedes: Some("p1".to_string()),
            reason: Some("replan".to_string()),
            plan: Box::new(plan2),
        },
    ];
    let m = summarize(&task, &events);
    assert!(m.has_plan);
    assert_eq!(m.work_units_total, 3, "{m:?}");
    assert_eq!(m.work_units_done, 1, "{m:?}");
    assert_eq!(m.replans, 1);
}

/// atomic 化のときに `ExecutionPlanned` に repair WU のタイトルごと載る経路（D16 の repair 分類が
/// events から復元できる）。
#[test]
fn repairs_are_classified_from_the_execution_planned_spec_when_available() {
    let task = sample_task("x", vec![]);
    let plan = ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "reviewer repair".to_string(),
        work_units: vec![
            wu("main", WorkUnitKind::Implement, "main", &[]),
            wu(
                "repair-1",
                WorkUnitKind::Repair,
                "repair (format): 修復",
                &[],
            ),
        ],
        phases: Vec::new(),
        children: Vec::new(),
    };
    let events = vec![Event::ExecutionPlanned {
        plan_id: "p1".to_string(),
        version: 1,
        origin: PlanOrigin::Repair,
        supersedes: None,
        reason: Some("review_repair".to_string()),
        plan: Box::new(plan),
    }];
    let m = summarize(&task, &events);
    assert_eq!(m.repairs_total, 1);
    assert_eq!(m.repairs_by_class.get("format"), Some(&1));
}

/// 既に計画のある Task に足された repair WU は `WorkUnitTransitioned` だけで作られ（D16）、
/// class は events から復元できないので `"unknown"` に落ちる（既知の限界。ADR の逸脱節参照）。
#[test]
fn repairs_without_a_recoverable_title_fall_back_to_unknown() {
    let task = sample_task("x", vec![]);
    let events = vec![Event::WorkUnitTransitioned {
        work_unit_id: "wu-repair-1".to_string(),
        key: "repair-1".to_string(),
        from: WorkUnitStatus::Pending,
        to: WorkUnitStatus::Ready,
        reason: "review_repair".to_string(),
        run_id: None,
    }];
    let m = summarize(&task, &events);
    assert_eq!(m.repairs_total, 1);
    assert_eq!(m.repairs_by_class.get("unknown"), Some(&1));
}

/// ADR-0074 D6.2/§6 F1 (i)（Phase F1）: `RepairScheduled` があれば、title の接頭辞を復元しなくても
/// class が分かる（`unknown` にならない）。
#[test]
fn repair_scheduled_event_names_the_class() {
    let task = sample_task("x", vec![]);
    let events = vec![
        Event::WorkUnitTransitioned {
            work_unit_id: "wu-repair-1".to_string(),
            key: "repair-1".to_string(),
            from: WorkUnitStatus::Pending,
            to: WorkUnitStatus::Ready,
            reason: "review_repair".to_string(),
            run_id: None,
        },
        Event::RepairScheduled {
            work_unit_id: "wu-repair-1".to_string(),
            key: "repair-1".to_string(),
            class: "review_timeout".to_string(),
            origin: crate::execution::RepairOrigin::Review,
        },
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.repairs_total, 1);
    assert_eq!(m.repairs_by_class.get("review_timeout"), Some(&1));
    assert!(!m.repairs_by_class.contains_key("unknown"), "{m:?}");
}

/// planner が replan で自ら書いた repair WU は class `"planner"`（`unknown` にならない）。
#[test]
fn planner_authored_repair_work_units_are_classified_as_planner() {
    let task = sample_task("x", vec![]);
    let plan = ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "r".to_string(),
        work_units: vec![wu(
            "repair-1",
            WorkUnitKind::Repair,
            "fix the thing directly (no title convention)",
            &[],
        )],
        phases: Vec::new(),
        children: Vec::new(),
    };
    let events = vec![
        Event::ExecutionPlanned {
            plan_id: "p1".to_string(),
            version: 2,
            origin: PlanOrigin::Planner,
            supersedes: Some("p0".to_string()),
            reason: Some("replan (planner run)".to_string()),
            plan: Box::new(plan),
        },
        Event::RepairScheduled {
            work_unit_id: "wu-repair-1".to_string(),
            key: "repair-1".to_string(),
            class: "planner".to_string(),
            origin: crate::execution::RepairOrigin::Planner,
        },
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.repairs_by_class.get("planner"), Some(&1), "{m:?}");
    assert!(!m.repairs_by_class.contains_key("unknown"), "{m:?}");
}

#[test]
fn tokens_cost_and_peak_context_are_summed_and_maxed() {
    let task = sample_task("x", vec![]);
    let fin = |input: u64, output: u64, cost: f64, peak: u64| Event::WorkerFinished {
        run_id: "r".to_string(),
        outcome: "done: ok".to_string(),
        usage: Some(Usage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(input / 2),
            cache_creation_tokens: None,
            cost_usd: Some(cost),
            duplicate_reads: None,
            session_resumed: None,
        }),
        role: None,
        metrics: Some(crate::model::RunMetrics {
            wall_ms: 1000,
            retries: 0,
            peak_context_tokens: Some(peak),
            turns: Some(3),
        }),
        end: Some(RunEnd::Completed),
    };
    let events = vec![fin(100, 20, 0.5, 500), fin(200, 40, 0.25, 900)];
    let m = summarize(&task, &events);
    assert_eq!(m.total_input_tokens, Some(300));
    assert_eq!(m.total_cache_read_tokens, Some(150));
    assert_eq!(m.total_output_tokens, Some(60));
    assert_eq!(m.cost_usd, Some(0.75));
    assert_eq!(m.peak_context_tokens, Some(900));
}

#[test]
fn runs_by_role_counts_worker_reviewer_and_planner() {
    let task = sample_task("x", vec![]);
    let started = |role: Option<RunRole>| Event::WorkerStarted {
        run_id: "r".to_string(),
        adapter: "fake".to_string(),
        model: "m".to_string(),
        provider: None,
        account: None,
        role,
        task_role: None,
    };
    let events = vec![
        started(None),
        started(Some(RunRole::Reviewer)),
        started(Some(RunRole::Planner)),
        started(Some(RunRole::Planner)),
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.runs_by_role.get("worker"), Some(&1));
    assert_eq!(m.runs_by_role.get("reviewer"), Some(&1));
    assert_eq!(m.runs_by_role.get("planner"), Some(&2));
}

#[test]
fn wall_ms_is_only_present_for_terminal_tasks() {
    let mut task = sample_task("x", vec![]);
    let m = summarize(&task, &[]);
    assert_eq!(m.wall_ms, None, "Draft はまだ終端ではない");
    task.status = Status::Done;
    task.updated_at = task.created_at + time::Duration::seconds(30);
    let m = summarize(&task, &[]);
    assert_eq!(m.wall_ms, Some(30_000));
}

// ---- ADR-0074 D4.3（Phase F3 quota）----

fn quota_event(
    run_id: &str,
    work_unit_id: Option<&str>,
    source: &str,
    account: Option<&str>,
    windows: Vec<crate::quota::QuotaWindowUse>,
) -> Event {
    let weighted_tokens = 100.0;
    Event::QuotaEstimated {
        run_id: run_id.to_string(),
        work_unit_id: work_unit_id.map(str::to_string),
        source: source.to_string(),
        account: account.map(str::to_string),
        method: crate::quota::representative_method(&windows),
        windows,
        weighted_tokens,
        calibration: None,
        weights_version: crate::quota::WEIGHTS_VERSION.to_string(),
        list_price_usd: None,
    }
}

fn measured_window(
    window: crate::quota::QuotaWindow,
    used_pct: f64,
) -> crate::quota::QuotaWindowUse {
    crate::quota::QuotaWindowUse {
        window,
        before: Some(0.1),
        after: Some(0.1 + used_pct / 100.0),
        resets_at: Some(5_000),
        used_pct: Some(used_pct),
        method: QuotaMethod::Measured,
    }
}

#[test]
fn cost_usd_complete_is_false_with_an_unpriced_model() {
    let task = sample_task("x", vec![]);
    let priced = Event::WorkerFinished {
        run_id: "r1".to_string(),
        outcome: "done: ok".to_string(),
        usage: Some(Usage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: Some(1.0),
            duplicate_reads: None,
            session_resumed: None,
        }),
        role: None,
        metrics: None,
        end: Some(RunEnd::Completed),
    };
    let unpriced = Event::WorkerFinished {
        run_id: "r2".to_string(),
        outcome: "done: ok".to_string(),
        usage: Some(Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(200_000),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: None, // 単価表に無いモデル（例: gpt-6-sol）
            duplicate_reads: None,
            session_resumed: None,
        }),
        role: None,
        metrics: None,
        end: Some(RunEnd::Completed),
    };
    let m = summarize(&task, std::slice::from_ref(&priced));
    assert!(m.cost_usd_complete, "priced-only は complete");

    let m = summarize(&task, &[priced, unpriced]);
    assert!(!m.cost_usd_complete, "unpriced な run が混じれば false");
    assert_eq!(
        m.cost_usd,
        Some(1.0),
        "cost_usd 自体は priced 分だけ合計する"
    );
}

#[test]
fn cost_usd_complete_defaults_true_with_no_runs() {
    let task = sample_task("x", vec![]);
    let m = summarize(&task, &[]);
    assert!(m.cost_usd_complete);
}

#[test]
fn quota_is_aggregated_by_source_account_and_window() {
    let task = sample_task("x", vec![]);
    let events = vec![
        quota_event(
            "r1",
            None,
            "claude-oauth",
            Some("a"),
            vec![measured_window(crate::quota::QuotaWindow::FiveHour, 4.0)],
        ),
        quota_event(
            "r2",
            None,
            "claude-oauth",
            Some("a"),
            vec![measured_window(crate::quota::QuotaWindow::FiveHour, 6.0)],
        ),
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.quota.len(), 1, "{:?}", m.quota);
    let row = &m.quota[0];
    assert_eq!(row.source, "claude-oauth");
    assert_eq!(row.account.as_deref(), Some("a"));
    assert_eq!(row.runs, 2);
    assert_eq!(row.used_pct, Some(10.0));
    assert_eq!(m.quota_unknown_runs, 0);
}

/// ADR-0076: `QuotaUse.runs_by_role` は `WorkerStarted.role` と run_id で join して数える。
/// role の無い（旧）`WorkerStarted`・`WorkerStarted` の無い run は worker。同じ run_id の
/// 再送は 1 件。WU ごとの集計（`group_quota_by_work_unit`）も同じ規則。
#[test]
fn quota_runs_by_role_defaults_legacy_events_to_worker() {
    let task = sample_task("x", vec![]);
    let started = |run_id: &str, role: Option<RunRole>| Event::WorkerStarted {
        run_id: run_id.to_string(),
        adapter: "claude-code".to_string(),
        model: "m".to_string(),
        provider: None,
        account: Some("a".to_string()),
        role,
        task_role: None,
    };
    let q = |run_id: &str, pct: f64| {
        quota_event(
            run_id,
            None,
            "claude-oauth",
            Some("a"),
            vec![measured_window(crate::quota::QuotaWindow::FiveHour, pct)],
        )
    };
    let events = vec![
        started("w1", None),
        started("p1", Some(RunRole::Planner)),
        started("rv1", Some(RunRole::Reviewer)),
        started("w2", Some(RunRole::Worker)),
        q("w1", 1.0),
        q("p1", 2.0),
        q("rv1", 3.0),
        q("rv1", 3.5), // 同じ run_id の再送（最後が有効、1 件）
        q("w2", 4.0),
        q("orphan", 0.5), // WorkerStarted の無い run（別タスクへの按分の再送など）
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.quota.len(), 1, "{:?}", m.quota);
    let row = &m.quota[0];
    assert_eq!(row.runs, 5);
    assert_eq!(row.runs_by_role.get("worker"), Some(&3), "{row:?}");
    assert_eq!(row.runs_by_role.get("planner"), Some(&1));
    assert_eq!(row.runs_by_role.get("reviewer"), Some(&1));
    assert_eq!(row.runs_by_role.values().sum::<u32>(), row.runs);

    let by_wu = group_quota_by_work_unit(&events);
    let atomic = &by_wu[&None][0];
    assert_eq!(atomic.runs_by_role, row.runs_by_role);

    // 旧 JSON（`runs_by_role` の無い `ExecutionMetrics.quota` 行）も読める。
    let mut legacy = serde_json::to_value(&m).expect("serialize");
    legacy["quota"][0]
        .as_object_mut()
        .expect("quota row")
        .remove("runs_by_role");
    let back: ExecutionMetrics = serde_json::from_value(legacy).expect("legacy metrics JSON");
    assert!(back.quota[0].runs_by_role.is_empty());
}

/// D4.3: `apportioned` は同じ `run_id` にもう 1 件出ることがある（グループが閉じたとき）。
/// 「最後の Event が有効」なので、`unknown`（先の暫定値）は上書きされて消える。
#[test]
fn quota_estimated_reemission_for_the_same_run_id_keeps_only_the_last_event() {
    let task = sample_task("x", vec![]);
    let pending = crate::quota::QuotaWindowUse {
        window: crate::quota::QuotaWindow::FiveHour,
        before: None,
        after: None,
        resets_at: None,
        used_pct: None,
        method: QuotaMethod::Unknown,
    };
    let resolved = measured_window(crate::quota::QuotaWindow::FiveHour, 9.0);
    let events = vec![
        quota_event("r1", None, "claude-oauth", Some("a"), vec![pending]),
        quota_event("r1", None, "claude-oauth", Some("a"), vec![resolved]),
    ];
    let m = summarize(&task, &events);
    assert_eq!(m.quota_unknown_runs, 0, "{:?}", m.quota);
    assert_eq!(m.quota[0].used_pct, Some(9.0));
    assert_eq!(
        m.quota[0].runs, 1,
        "1 回だけ数える（同じ run_id は畳み込む）"
    );
}

#[test]
fn quota_unknown_runs_counts_runs_that_never_resolved() {
    let task = sample_task("x", vec![]);
    let unknown_window = crate::quota::QuotaWindowUse {
        window: crate::quota::QuotaWindow::FiveHour,
        before: None,
        after: None,
        resets_at: None,
        used_pct: None,
        method: QuotaMethod::Unknown,
    };
    let events = vec![quota_event(
        "r1",
        Some("wu-a"),
        "codex-oauth",
        Some("b"),
        vec![unknown_window],
    )];
    let m = summarize(&task, &events);
    assert_eq!(m.quota_unknown_runs, 1);
    assert_eq!(m.quota[0].used_pct, None, "unknown は 0 ではない");
}

#[test]
fn group_quota_by_work_unit_splits_atomic_and_work_unit_runs() {
    let events = vec![
        quota_event(
            "r1",
            None,
            "claude-oauth",
            Some("a"),
            vec![measured_window(crate::quota::QuotaWindow::FiveHour, 2.0)],
        ),
        quota_event(
            "r2",
            Some("wu-a"),
            "claude-oauth",
            Some("a"),
            vec![measured_window(crate::quota::QuotaWindow::FiveHour, 3.0)],
        ),
    ];
    let by_wu = group_quota_by_work_unit(&events);
    assert_eq!(by_wu.get(&None).map(|v| v[0].used_pct), Some(Some(2.0)));
    assert_eq!(
        by_wu.get(&Some("wu-a".to_string())).map(|v| v[0].used_pct),
        Some(Some(3.0))
    );
}

fn continuation_metrics_run(id: &str, wu: Option<&str>, resumed: Option<bool>) -> RunRow {
    RunRow {
        run_id: id.into(),
        task_id: "task".into(),
        work_unit_id: wu.map(str::to_string),
        role: RunIndexRole::Worker,
        seq: 1,
        status: crate::RunIndexStatus::Completed,
        adapter: Some("claude-code".into()),
        model: None,
        account: None,
        session_id: None,
        checkpoint: None,
        usage: Some(Usage {
            input_tokens: Some(10),
            cache_read_tokens: Some(2),
            cache_creation_tokens: Some(3),
            duplicate_reads: Some(4),
            session_resumed: resumed,
            ..Usage::default()
        }),
        metrics: Some(RunMetrics {
            wall_ms: 100,
            ..RunMetrics::default()
        }),
        started_at: String::new(),
        finished_at: None,
    }
}

#[test]
fn continuation_metrics_compares_fresh_resumed_and_legacy_runs() {
    let runs = [
        continuation_metrics_run("fresh", None, Some(false)),
        continuation_metrics_run("resumed", None, Some(true)),
        continuation_metrics_run("old", None, None),
    ];
    let (summary, _) = summarize_continuation_runs(&[], &runs);
    assert_eq!(summary.fresh.runs, 1);
    assert_eq!(summary.resumed.wall_ms, 100);
    assert_eq!(summary.resumed.input_tokens, 15);
    assert_eq!(summary.resumed.duplicate_reads, 4);
    assert_eq!(summary.unknown.runs, 1);
}

#[test]
fn continuation_metrics_groups_by_work_unit_and_fallback_reason() {
    let events = vec![Event::worker_progress_with(
        "r2",
        "continuation session: fresh (reason=account_changed)",
        crate::ProgressFields::of(crate::ProgressKind::Status),
    )];
    let runs = [
        continuation_metrics_run("r1", Some("wu-a"), Some(true)),
        continuation_metrics_run("r2", Some("wu-b"), Some(false)),
    ];
    let (summary, by_wu) = summarize_continuation_runs(&events, &runs);
    assert_eq!(summary.fresh_fallback_by_reason["account_changed"], 1);
    assert_eq!(by_wu[&Some("wu-a".into())].resumed.runs, 1);
    assert_eq!(by_wu[&Some("wu-b".into())].fresh.input_tokens, 15);
}

#[test]
fn continuation_metrics_event_summary_and_serde_default() {
    let task = sample_task("x", vec![]);
    let events = vec![Event::WorkerFinished {
        run_id: "run".into(),
        outcome: "done".into(),
        role: None,
        usage: continuation_metrics_run("run", None, Some(true)).usage,
        metrics: Some(RunMetrics {
            wall_ms: 50,
            ..RunMetrics::default()
        }),
        end: None,
    }];
    let summary = summarize(&task, &events);
    assert_eq!(summary.continuation.resumed.runs, 1);
    assert_eq!(summary.continuation.resumed.wall_ms, 50);
    let mut json = serde_json::to_value(summary).unwrap();
    json.as_object_mut().unwrap().remove("continuation");
    let old: ExecutionMetrics = serde_json::from_value(json).unwrap();
    assert_eq!(old.continuation, ContinuationMetrics::default());
}
