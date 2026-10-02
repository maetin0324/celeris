use super::*;

/// ADR-0090 D2: wait で終わった unit の run は `blocked(cluster_jobs)`（continuation・retry に数えない）で、
/// task は `advance`。
#[test]
fn a_waiting_run_blocks_the_unit_on_cluster_jobs() {
    let wu = row("a", WorkUnitStatus::Running, &[]);
    let d = decide(
        RunEnd::Waiting,
        "run-1",
        &wu,
        std::slice::from_ref(&wu),
        ContinuationInputs::default(),
        WuLimits {
            max_continuations: 1,
            no_progress_limit: 1,
            max_retries: 0,
        },
    );
    assert_eq!(d.reason, "cluster_jobs");
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(
        d.updated.blocked_reason,
        Some(WorkUnitBlockedReason::ClusterJobs)
    );
    assert_eq!(d.updated.continuations, 0);
    assert_eq!(d.updated.retries, 0);
    assert_eq!(
        d.trigger,
        Trigger::Continue {
            why: ContinueWhy::Advance
        }
    );
    // 同じ段階の兄弟は止めない（settle は advance）。
    let sibling = row("b", WorkUnitStatus::Ready, &[]);
    assert_eq!(
        settle_phase(&[d.updated.clone(), sibling]),
        PhaseSettle::Advance
    );
}
use task_core::{WorkUnitContext, WorkUnitKind, WorkUnitSpec};

fn row(key: &str, status: WorkUnitStatus, depends_on: &[&str]) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        expected_write_paths: None,
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: key.to_string(),
        objective: format!("objective {key}"),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    WorkUnitRow::new(
        format!("id-{key}"),
        "task".into(),
        "plan".into(),
        0,
        spec,
        status,
        "2026-09-24T00:00:00Z".into(),
    )
}

fn prow(
    key: &str,
    seq: u32,
    phase: &str,
    status: WorkUnitStatus,
    depends_on: &[&str],
) -> WorkUnitRow {
    let mut r = row(key, status, depends_on);
    r.seq = seq;
    r.phase = Some(phase.to_string());
    r.spec.phase = Some(phase.to_string());
    r
}

fn integ(phase: &str, seq: u32, status: WorkUnitStatus, deps: &[&str]) -> WorkUnitRow {
    let mut r = prow(&format!("integrate-{phase}"), seq, phase, status, deps);
    r.kind = task_core::WorkUnitKind::Integrate;
    r.spec.kind = task_core::WorkUnitKind::Integrate;
    r
}

/// ADR-0079 付記「R7-9」D3（本番 01M3PAX6RVE7AX8Z6118KADME3 の形）: 段階 `phase-4` の統合 WU は done（依存は
/// p4a だけ）で、統合の後の replan が同じ段階に `land2` を足し、`land2` も done。生きた行はすべて done だが、
/// `land2` は統合されていないので計画の完了（`AllDone`）にしない（gate に任せる `Advance`）。
#[test]
fn settle_is_not_all_done_while_an_integrated_stage_has_an_unmerged_unit() {
    use WorkUnitStatus::*;
    let units = vec![
        prow("p4a", 0, "phase-4", Done, &[]),
        prow("land2", 0, "phase-4", Done, &[]),
        integ("phase-4", 1, Done, &["p4a"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::Advance);
    // 統合 WU の依存に入っていれば（統合済み）完了。
    let mut units = vec![
        prow("p4a", 0, "phase-4", Done, &[]),
        prow("land2", 0, "phase-4", Done, &[]),
        integ("phase-4", 1, Done, &["p4a", "land2"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::AllDone);
    // 段階を持たない行（最終レビューの repair WU）・退役した行は段階の統合に関係しない。
    units.push(row("repair-1", Done, &[]));
    units.push(prow("land", 0, "phase-4", Superseded, &[]));
    assert_eq!(settle_phase(&units), PhaseSettle::AllDone);
}

/// ADR-0079 付記「R7-9」D3: 統合済みの段階に後から足された unit の run が done で終わっても、計画の完了
/// （`plan_complete` / `WorkerDone`）にしない（本番では `land2` の done でそのまま最終レビューに出た）。
#[test]
fn completing_a_unit_added_to_an_integrated_stage_does_not_complete_the_plan() {
    use WorkUnitStatus::*;
    let land2 = prow("land2", 0, "phase-4", Running, &[]);
    let units = vec![
        prow("p4a", 0, "phase-4", Done, &[]),
        land2.clone(),
        integ("phase-4", 1, Done, &["p4a"]),
    ];
    let d = decide(
        RunEnd::Completed,
        "run-1",
        &land2,
        &units,
        ContinuationInputs::default(),
        WuLimits {
            max_continuations: 1,
            no_progress_limit: 1,
            max_retries: 0,
        },
    );
    assert_eq!(d.updated.status, Done);
    assert!(!d.plan_complete);
    assert_eq!(
        d.trigger,
        Trigger::Continue {
            why: ContinueWhy::Advance
        }
    );
}

#[test]
fn settle_waits_while_a_sibling_is_running_then_reports_the_question() {
    use WorkUnitStatus::*;
    let mut q = prow("a", 0, "build", Blocked, &[]);
    q.blocked_reason = Some(WorkUnitBlockedReason::Question);
    let units = vec![
        q.clone(),
        prow("b", 1, "build", Running, &[]),
        integ("build", 2, Pending, &["a", "b"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::Wait);
    let units = vec![
        q,
        prow("b", 1, "build", Done, &[]),
        integ("build", 2, Pending, &["a", "b"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::Question("id-a".into()));
}

#[test]
fn settle_reports_a_failure_after_in_flight_reaches_zero() {
    use WorkUnitStatus::*;
    let units = vec![
        prow("a", 0, "build", Failed, &[]),
        prow("b", 1, "build", Done, &[]),
        prow("c", 2, "build", Ready, &[]),
        integ("build", 3, Pending, &["a", "b", "c"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::Failure("id-a".into()));
}

#[test]
fn settle_advances_integrates_and_finishes() {
    use WorkUnitStatus::*;
    let units = vec![
        prow("a", 0, "build", Done, &[]),
        prow("b", 1, "build", Ready, &[]),
        integ("build", 2, Pending, &["a", "b"]),
        prow("c", 3, "verify", Pending, &["a"]),
        integ("verify", 4, Pending, &["c"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::Advance);
    let units = vec![
        prow("a", 0, "build", Done, &[]),
        prow("b", 1, "build", Done, &[]),
        integ("build", 2, Pending, &["a", "b"]),
        prow("c", 3, "verify", Pending, &["a"]),
        integ("verify", 4, Pending, &["c"]),
    ];
    assert_eq!(
        settle_phase(&units),
        PhaseSettle::Integrate("id-integrate-build".into())
    );
    let units = vec![
        prow("a", 0, "build", Done, &[]),
        integ("build", 1, Done, &["a"]),
    ];
    assert_eq!(settle_phase(&units), PhaseSettle::AllDone);
}

fn limits() -> WuLimits {
    WuLimits {
        max_continuations: 3,
        no_progress_limit: 2,
        max_retries: 2,
    }
}

fn no_ci() -> ContinuationInputs<'static> {
    ContinuationInputs::default()
}

#[test]
fn completing_the_last_work_unit_marks_the_plan_complete_and_triggers_worker_done() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let d = decide(RunEnd::Completed, "r1", &a, &units, no_ci(), limits());
    assert_eq!(d.updated.status, WorkUnitStatus::Done);
    assert_eq!(d.trigger, Trigger::WorkerDone);
    assert!(d.plan_complete);
}

#[test]
fn completing_a_work_unit_with_more_pending_advances_and_promotes_dependents() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let b = row("b", WorkUnitStatus::Pending, &["a"]);
    let units = vec![a.clone(), b.clone()];
    let d = decide(RunEnd::Completed, "r1", &a, &units, no_ci(), limits());
    assert_eq!(
        d.trigger,
        Trigger::Continue {
            why: ContinueWhy::Advance
        }
    );
    assert!(!d.plan_complete);
    assert_eq!(d.newly_ready.len(), 1);
    assert_eq!(d.newly_ready[0].key, "b");
    assert_eq!(d.newly_ready[0].status, WorkUnitStatus::Ready);
}

#[test]
fn a_retryable_failure_within_the_limit_retries_the_same_work_unit() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let d = decide(
        RunEnd::Failed { retryable: true },
        "r1",
        &a,
        &units,
        no_ci(),
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Ready);
    assert_eq!(d.updated.retries, 1);
    assert_eq!(
        d.trigger,
        Trigger::Continue {
            why: ContinueWhy::WorkUnitRetry
        }
    );
}

#[test]
fn a_failure_at_the_retry_limit_fails_the_work_unit_and_blocks_dependents() {
    let mut a = row("a", WorkUnitStatus::Running, &[]);
    a.retries = 2; // already at max_retries
    let b = row("b", WorkUnitStatus::Pending, &["a"]);
    let c = row("c", WorkUnitStatus::Pending, &["b"]);
    let units = vec![a.clone(), b, c];
    let d = decide(
        RunEnd::Failed { retryable: true },
        "r1",
        &a,
        &units,
        no_ci(),
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Failed);
    assert_eq!(d.trigger, Trigger::WorkerError { retryable: false });
    assert_eq!(d.outcome_override.as_deref(), Some("work unit a failed"));
    let mut blocked_keys: Vec<&str> = d.newly_blocked.iter().map(|u| u.key.as_str()).collect();
    blocked_keys.sort();
    assert_eq!(blocked_keys, vec!["b", "c"]);
    assert!(
        d.newly_blocked
            .iter()
            .all(|u| u.blocked_reason == Some(WorkUnitBlockedReason::DependencyFailed))
    );
}

#[test]
fn a_non_retryable_failure_fails_immediately_regardless_of_retries_so_far() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let d = decide(
        RunEnd::Failed { retryable: false },
        "r1",
        &a,
        &units,
        no_ci(),
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Failed);
}

#[test]
fn a_question_blocks_the_work_unit_and_the_task() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let d = decide(RunEnd::Question, "r1", &a, &units, no_ci(), limits());
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(
        d.updated.blocked_reason,
        Some(WorkUnitBlockedReason::Question)
    );
    assert_eq!(d.trigger, Trigger::WorkerQuestion);
}

fn checkpoint(completed: usize) -> Checkpoint {
    Checkpoint {
        schema: task_core::CHECKPOINT_SCHEMA.into(),
        task_id: "t".into(),
        work_unit: Some("a".into()),
        run_id: "r".into(),
        run_seq: 1,
        end: task_core::CheckpointEnd::BudgetExhausted,
        source: task_core::CheckpointSource::Mechanical,
        completed: (0..completed).map(|i| format!("c{i}")).collect(),
        remaining: vec![],
        decisions: vec![],
        files_changed: vec![],
        tests_run: vec![],
        known_failures: vec![],
        artifact_refs: vec![],
        next_action: "next".into(),
        open_questions: vec![],
        plan_issue: None,
        repo_state: None,
        recent_activity: vec![],
        created_at: "2026-09-24T00:00:00Z".into(),
    }
}

/// ADR-0072 D17 3.（Phase E4b 項目2）: checkpoint に `plan_issue` があれば、まだ continuation の
/// 上限に達していなくても（`continuations = 0`）`blocked(plan_issue)` になる。`limit`/`continue` の
/// 判定より優先される。
#[test]
fn a_plan_issue_in_the_checkpoint_blocks_the_work_unit_even_within_budget() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let mut cp = checkpoint(1);
    cp.plan_issue = Some("migration M is needed before this work unit".into());
    let d = decide(
        RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
        },
        "r1",
        &a,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            prev_checkpoint: None,
            no_progress_before: 0,
        },
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(
        d.updated.blocked_reason,
        Some(WorkUnitBlockedReason::PlanIssue)
    );
    assert_eq!(d.reason, "plan_issue");
    assert_eq!(d.trigger, Trigger::WorkerQuestion);
    // continuations は増えない（continuation の判定に入る前に分岐する）。
    assert_eq!(d.updated.continuations, 0);
}

#[test]
fn budget_exhausted_within_limits_moves_to_needs_continuation() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let cp = checkpoint(1);
    let d = decide(
        RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
        },
        "r1",
        &a,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            prev_checkpoint: None,
            no_progress_before: 0,
        },
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::NeedsContinuation);
    assert_eq!(d.updated.continuations, 1);
    assert_eq!(
        d.trigger,
        Trigger::Continue {
            why: ContinueWhy::Continue
        }
    );
}

#[test]
fn hitting_the_continuation_limit_blocks_with_a_question() {
    let mut a = row("a", WorkUnitStatus::Running, &[]);
    a.continuations = 3; // at max_continuations
    let units = vec![a.clone()];
    let cp = checkpoint(1);
    let d = decide(
        RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
        },
        "r1",
        &a,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            prev_checkpoint: None,
            no_progress_before: 0,
        },
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(d.updated.blocked_reason, Some(WorkUnitBlockedReason::Limit));
    assert_eq!(d.trigger, Trigger::WorkerQuestion);
}

#[test]
fn no_progress_at_the_limit_blocks_even_under_the_continuation_cap() {
    let a = row("a", WorkUnitStatus::Running, &[]);
    let units = vec![a.clone()];
    let cp = checkpoint(1);
    let prev = checkpoint(1); // same `completed` count => no progress
    let d = decide(
        RunEnd::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
        },
        "r1",
        &a,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            prev_checkpoint: Some(&prev),
            // already 1 consecutive no-progress; this one makes 2 = the limit
            no_progress_before: 1,
        },
        limits(),
    );
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(d.updated.blocked_reason, Some(WorkUnitBlockedReason::Limit));
}

#[test]
fn resume_after_answer_restores_needs_continuation_for_a_limit_block_and_ready_otherwise() {
    let mut a = row("a", WorkUnitStatus::Blocked, &[]);
    a.blocked_reason = Some(WorkUnitBlockedReason::Limit);
    assert_eq!(
        resume_after_answer(&a).status,
        WorkUnitStatus::NeedsContinuation
    );

    let mut b = row("b", WorkUnitStatus::Blocked, &[]);
    b.blocked_reason = Some(WorkUnitBlockedReason::Question);
    assert_eq!(resume_after_answer(&b).status, WorkUnitStatus::Ready);
}
