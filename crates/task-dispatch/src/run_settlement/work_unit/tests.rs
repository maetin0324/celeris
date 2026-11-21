//! [`super::settle_work_unit`] と後段の関数の単体試験。
//!
//! 既存の試験は、WU の遷移そのものを `execution_scheduler` の試験で、worker 完了の結果を `dispatcher/tests` の
//! 経路全体（store・worktree・config を組む）で見る。経路全体の試験は分岐ごとに 1 つの組み合わせしか通さず、
//! 遷移の後に task の trigger と outcome の文をどう決めるか（auto-leaf の上限・merge の repair・integration repair・
//! replan か質問・worker の決定の要求の保留）の境界を、遷移の関数の結果と並べて 1 か所で固定する試験は無かった。
//! ここでその契約を固定する。

use super::super::{AskReason, ReplanLimits, ReplanOrAsk, replan_or_ask};
use super::*;
use task_core::{
    BudgetKind, Checkpoint, CostOfReversal, DecisionKind, DecisionOrigin, DecisionRaisedBy,
    DecisionStatus, TaskId, WorkUnitContext, WorkUnitSpec,
};

fn row(key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.to_string(),
        kind,
        title: key.to_string(),
        objective: format!("objective {key}"),
        depends_on: vec![],
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
        "2026-11-13T00:00:00Z".into(),
    )
}

fn checkpoint(completed: usize, plan_issue: Option<&str>) -> Checkpoint {
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
        plan_issue: plan_issue.map(str::to_string),
        repo_state: None,
        recent_activity: vec![],
        created_at: "2026-11-13T00:00:00Z".into(),
    }
}

fn request() -> DecisionRequest {
    DecisionRequest {
        id: "d1".into(),
        key: "auto_leaf_budget:id-a:run-1".into(),
        kind: DecisionKind::LeafTooLarge,
        question: "leaf a の継続を確認してください".into(),
        options: vec![],
        recommended: "replan".into(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["a".into()],
        path: vec![],
        raised_by: DecisionRaisedBy {
            task_id: TaskId::new(),
            run_id: Some("run-1".into()),
            origin: DecisionOrigin::Daemon,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

fn budget() -> RunEnd {
    RunEnd::BudgetExhausted {
        kind: BudgetKind::Turns,
    }
}

fn input<'a>(
    end: RunEnd,
    wu: &'a WorkUnitRow,
    units: &'a [WorkUnitRow],
    continuation: ContinuationInputs<'a>,
    outcome: &str,
) -> WorkUnitRunInput<'a> {
    WorkUnitRunInput {
        end,
        run_id: "run-1",
        wu,
        units,
        continuation,
        continuation_enabled: true,
        continuation_hard_limit: 10,
        no_progress_limit: 1,
        max_retries: 0,
        auto_leaf_measured: false,
        auto_leaf_request: None,
        integration_repair_trusted: None,
        trigger: Trigger::WorkerError { retryable: false },
        outcome: outcome.to_string(),
        check_failure_suffix: "",
        check_failure_signature: None,
        misplaced: false,
    }
}

fn limits(stalled: u32, replans: u32, max_replans: u32) -> ReplanLimits {
    ReplanLimits {
        stalled,
        replans,
        max_replans,
        ban: None,
    }
}

/// 不変条件: `[execution] continuation = false`（または run の性質で続けられない）の continuation に回る run は、
/// retryable な失敗として WU の retry に回し（WU は `ready`、task は `work_unit_retry`）、auto-leaf の上限を超えた leaf
/// だけは遷移させて止める。以前の「WU の行を変えない」契約は WU を `running` のまま残して task を止めた
/// （`dispatcher::tests::research_deadline` の WU の試験が検出した）ので廃止した。この分岐は遷移の関数の入力を
/// 差し替えるので、`execution_scheduler` の試験では検出できない。
#[test]
fn continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it() {
    let wu = row("a", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let units = [wu.clone()];
    let cp = checkpoint(1, None);
    let ci = ContinuationInputs {
        checkpoint: Some(&cp),
        ..ContinuationInputs::default()
    };
    let mut disabled = input(budget(), &wu, &units, ci, "error(retryable=true): budget");
    disabled.continuation_enabled = false;
    disabled.max_retries = 1;
    let s = settle_work_unit(disabled);
    let d = s.decision.as_ref().expect("the unit is retried");
    assert_eq!(d.updated.status, WorkUnitStatus::Ready);
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: task_core::ContinueWhy::WorkUnitRetry
        }
    );
    assert_eq!(
        s.outcome,
        "work_unit_retry: WorkUnit a を最初からやり直します（1/1）"
    );

    let mut stopped = input(budget(), &wu, &units, ci, "error(retryable=true): budget");
    stopped.continuation_enabled = false;
    stopped.auto_leaf_measured = true;
    stopped.auto_leaf_request = Some(request());
    let s = settle_work_unit(stopped);
    let d = s.decision.as_ref().expect("the leaf transitions");
    assert_eq!(d.reason, "auto_leaf_budget");
    assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
    assert_eq!(
        d.updated.blocked_reason,
        Some(WorkUnitBlockedReason::Decision)
    );
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: ContinueWhy::Advance
        }
    );
    assert_eq!(s.outcome, "decision: leaf a の継続を確認してください");
    assert!(s.auto_leaf_request.is_some());
    // ADR 2026-11-21-worker-decisions-fail-closed 付記 2026-11-21: auto-leaf の上限で止めた run も worker の
    // `decisions` を読む（auto-leaf の要求と worker の決定を両方記録する）。
    assert!(s.reads_worker_decisions());
}

/// 不変条件: auto-leaf で測っている leaf は進捗なしの上限で止めず（測った回数の上限が代わりに効く）、auto-leaf の
/// 決定の要求は continuation に進む遷移のときだけ使う（失敗の遷移では捨て、replan か質問の経路を通す）。上限を
/// 外す値と要求を使う条件は worker 完了の経路だけにあり、遷移の関数の試験では検出できない。
#[test]
fn auto_leaf_lifts_the_progress_limit_and_only_holds_a_continuing_leaf() {
    let wu = row("a", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let units = [wu.clone()];
    let cp = checkpoint(0, None);
    let prev = checkpoint(0, None);
    let ci = ContinuationInputs {
        checkpoint: Some(&cp),
        prev_checkpoint: Some(&prev),
        ..ContinuationInputs::default()
    };
    let limited = settle_work_unit(input(budget(), &wu, &units, ci, "error(retryable=true): b"));
    assert_eq!(limited.decision.as_ref().map(|d| d.reason), Some("limit"));
    assert_eq!(limited.trigger, Trigger::WorkerQuestion);

    let mut measured = input(budget(), &wu, &units, ci, "error(retryable=true): b");
    measured.auto_leaf_measured = true;
    let s = settle_work_unit(measured);
    assert_eq!(s.decision.as_ref().map(|d| d.reason), Some("continue"));
    assert_eq!(
        s.outcome,
        "continue: budget_exhausted(turns) の続き（WorkUnit a, Run #1）"
    );

    let mut failed = input(
        RunEnd::Failed { retryable: false },
        &wu,
        &units,
        ContinuationInputs::default(),
        "error(retryable=false): boom",
    );
    failed.auto_leaf_measured = true;
    failed.auto_leaf_request = Some(request());
    let s = settle_work_unit(failed);
    assert_eq!(s.decision.as_ref().map(|d| d.reason), Some("failed"));
    assert!(s.auto_leaf_request.is_none());
    assert_eq!(
        s.outcome,
        "error(retryable=false): work unit a failed: boom"
    );
    assert_eq!(s.failure.as_ref().map(|f| f.reason), Some("failed"));
}

/// 不変条件: 失敗した merge の repair WU は replan も質問もせず `cancelled` で退き、task は `advance` で統合を
/// やり直す。merge の repair の扱いは遷移の関数の後に当てるので、`execution_scheduler` の試験では検出できない。
#[test]
fn a_failed_merge_repair_retires_without_replanning() {
    let mut wu = row("merge-s1-a", WorkUnitKind::Repair, WorkUnitStatus::Running);
    wu.phase = Some("s1".into());
    let units = [wu.clone()];
    let s = settle_work_unit(input(
        RunEnd::Failed { retryable: false },
        &wu,
        &units,
        ContinuationInputs::default(),
        "error(retryable=false): conflict",
    ));
    let d = s.decision.as_ref().expect("transition");
    assert_eq!(d.reason, "merge_repair_failed");
    assert_eq!(d.updated.status, WorkUnitStatus::Cancelled);
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: ContinueWhy::Advance
        }
    );
    assert!(
        s.outcome
            .starts_with("merge の repair WU merge-s1-a が衝突を解けなかった（failed）")
    );
    assert!(s.failure.is_none());
}

/// 不変条件: integration repair の WU は失敗・進まない・計画の問題、または信頼できない done で打ち切り（replan
/// しない）、信頼できる done は打ち切らない。打ち切りの後は戻せたら reviewer、戻せなければ人に聞く。経路全体の
/// 試験は理由ごとに 1 件しか通さず、信頼の有無と理由の写し方の組を検出できない。
#[test]
fn integration_repair_exhausts_on_failure_or_an_untrusted_completion() {
    let wu = row("repair-s1-1", WorkUnitKind::Repair, WorkUnitStatus::Running);
    let units = [wu.clone()];
    let mut done = input(
        RunEnd::Completed,
        &wu,
        &units,
        ContinuationInputs::default(),
        "done: ok",
    );
    done.integration_repair_trusted = Some(true);
    assert_eq!(settle_work_unit(done).integration_repair_exhaust, None);

    let mut untrusted = input(
        RunEnd::Completed,
        &wu,
        &units,
        ContinuationInputs::default(),
        "done: ok",
    );
    untrusted.integration_repair_trusted = Some(false);
    let s = settle_work_unit(untrusted);
    assert_eq!(
        s.integration_repair_exhaust,
        Some(IntegrationRepairExhaustReason::ResultUntrusted)
    );
    assert!(s.failure.is_none());

    let cp = checkpoint(0, None);
    let prev = checkpoint(0, None);
    let mut stuck = input(
        budget(),
        &wu,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            prev_checkpoint: Some(&prev),
            ..ContinuationInputs::default()
        },
        "error(retryable=true): b",
    );
    stuck.integration_repair_trusted = Some(true);
    let s = settle_work_unit(stuck);
    assert_eq!(
        s.integration_repair_exhaust,
        Some(IntegrationRepairExhaustReason::BudgetExhausted)
    );
    assert!(s.failure.is_none());

    assert_eq!(
        integration_repair_exhausted("r", true),
        (
            Trigger::WorkerDone,
            "integration repair r exhausted; review unsynced HEAD".to_string()
        )
    );
    assert_eq!(
        integration_repair_exhausted("r", false).0,
        Trigger::WorkerQuestion
    );
}

/// 不変条件: WU の失敗は `replan_or_ask` の判定で自動 replan（`Continue{replan}` と記録）、stalled の質問、
/// 使い切りの後の人への確認（失敗）か hint（計画の問題）に分かれ、置き違えた `result.json` の run は replan の
/// 判定に回さない。上限の判定は `run_settlement` の試験が見るが、判定の結果から trigger と outcome の文を作る分岐は
/// 経路全体の試験でしか通らず、理由ごとの組を検出できない。
#[test]
fn a_work_unit_failure_replans_asks_or_hints() {
    let wu = row("a", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let units = [wu.clone()];
    let failed = || {
        let mut i = input(
            RunEnd::Failed { retryable: false },
            &wu,
            &units,
            ContinuationInputs::default(),
            "error(retryable=false): boom",
        );
        i.check_failure_suffix = ": check x failed";
        settle_work_unit(i)
    };

    let mut s = failed();
    assert_eq!(replan_or_ask(limits(0, 0, 1)), ReplanOrAsk::Replan);
    assert_eq!(
        settle_work_unit_failure(&mut s, limits(0, 0, 1)),
        WorkUnitFailureStep::Settled
    );
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: ContinueWhy::Replan
        }
    );
    assert_eq!(s.outcome, "replan: work unit a failed: check x failed");
    assert!(matches!(
        s.auto_replan,
        Some(Event::AutoReplanRequested { ref key, .. }) if key == "a"
    ));

    let stalled = task_ops::plan_gate::STALLED_REPLAN_LIMIT;
    let mut s = failed();
    assert_eq!(
        replan_or_ask(limits(stalled, 0, 9)),
        ReplanOrAsk::Ask(AskReason::Stalled)
    );
    assert_eq!(
        settle_work_unit_failure(&mut s, limits(stalled, 0, 9)),
        WorkUnitFailureStep::Stalled { stalled }
    );
    assert_eq!(s.trigger, Trigger::WorkerQuestion);
    assert!(s.auto_replan.is_none());

    let mut s = failed();
    assert_eq!(
        settle_work_unit_failure(&mut s, limits(0, 2, 2)),
        WorkUnitFailureStep::AskExhausted {
            why: "work unit a failed: boom".into(),
            used: 2
        }
    );

    let cp = checkpoint(0, Some("split it"));
    let mut s = settle_work_unit(input(
        budget(),
        &wu,
        &units,
        ContinuationInputs {
            checkpoint: Some(&cp),
            ..ContinuationInputs::default()
        },
        "error(retryable=true): b",
    ));
    assert_eq!(
        s.outcome,
        "question: WorkUnit a が計画の問題を申告しました: split it"
    );
    assert_eq!(
        settle_work_unit_failure(&mut s, limits(0, 1, 1)),
        WorkUnitFailureStep::Settled
    );
    assert!(s.outcome.ends_with(task_ops::gate::PLAN_ISSUE_REPLAN_HINT));

    let mut misplaced = input(
        RunEnd::Failed { retryable: false },
        &wu,
        &units,
        ContinuationInputs::default(),
        "error(retryable=false): misplaced",
    );
    misplaced.misplaced = true;
    assert!(settle_work_unit(misplaced).failure.is_none());
}

/// 不変条件: checkpoint を組み立てるのは continuation に回る終わり方で continuation が有効か auto-leaf の上限を
/// 超えた run と、クラスタ job の wait だけ。組み立ての条件は経路全体の試験でしか通らず、auto-leaf の上限で組み立てる
/// 組を検出できない。
#[test]
fn checkpoints_are_saved_for_continuations_and_cluster_waits() {
    assert!(work_unit_saves_checkpoint(budget(), true, false));
    assert!(work_unit_saves_checkpoint(budget(), false, true));
    assert!(!work_unit_saves_checkpoint(budget(), false, false));
    assert!(work_unit_saves_checkpoint(RunEnd::Waiting, false, false));
    assert!(!work_unit_saves_checkpoint(RunEnd::Completed, true, true));
}

/// self の決定の要求で WU を止める（worker 完了の経路と同じ呼び方）。
fn hold(s: &mut WorkUnitSettlement, wu: &WorkUnitRow, count: usize) {
    hold_for_worker_decisions(s, wu, count);
}

/// 不変条件（ADR 2026-11-21-worker-decisions-fail-closed D1・付記 2026-11-21、I1・I5）: worker の `decisions` を
/// 読むのは、状態だけを戻す run（harness_error・取り消し）と WU のクラスタ job の wait（`cluster_jobs`）を除く全ての
/// 遷移。遷移の理由を列挙しないので、continuation・auto-leaf の上限・進捗なし・固い上限・計画の問題・質問・再試行・
/// 失敗・merge の repair の失敗で終わった run の要求も人に届く。旧い `worker_decisions_hold_a_completed_leaf_only` は
/// done の遷移と状態だけを戻す run の 2 つしか見ず、他の遷移で読まないこと（取りこぼし）を検出できなかった。
#[test]
fn decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs() {
    let wu = row("a", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let units = [wu.clone()];
    let mut merge = row("merge-s1-a", WorkUnitKind::Repair, WorkUnitStatus::Running);
    merge.phase = Some("s1".into());
    let merge_units = [merge.clone()];
    let fresh = checkpoint(1, None);
    let stale = checkpoint(0, None);
    let issue = checkpoint(0, Some("split it"));
    let progressed = ContinuationInputs {
        checkpoint: Some(&fresh),
        ..ContinuationInputs::default()
    };
    let stuck = ContinuationInputs {
        checkpoint: Some(&stale),
        prev_checkpoint: Some(&stale),
        ..ContinuationInputs::default()
    };
    let capped = ContinuationInputs {
        checkpoint: Some(&fresh),
        continuations_since_answer: 10,
        ..ContinuationInputs::default()
    };
    let planned = ContinuationInputs {
        checkpoint: Some(&issue),
        ..ContinuationInputs::default()
    };
    let none = ContinuationInputs::default();
    let failed = |retryable| RunEnd::Failed { retryable };

    let mut auto_leaf = input(budget(), &wu, &units, progressed, "b");
    auto_leaf.auto_leaf_measured = true;
    auto_leaf.auto_leaf_request = Some(request());
    let mut retry = input(failed(true), &wu, &units, none, "error(retryable=true): x");
    retry.max_retries = 1;
    let cases = [
        (
            "completed",
            true,
            input(RunEnd::Completed, &wu, &units, none, "done: ok"),
        ),
        (
            "continue",
            true,
            input(budget(), &wu, &units, progressed, "b"),
        ),
        ("auto_leaf_budget", true, auto_leaf),
        ("limit", true, input(budget(), &wu, &units, stuck, "b")),
        (
            "continuation_cap",
            true,
            input(budget(), &wu, &units, capped, "b"),
        ),
        (
            "plan_issue",
            true,
            input(budget(), &wu, &units, planned, "b"),
        ),
        (
            "question",
            true,
            input(RunEnd::Question, &wu, &units, none, "question: q"),
        ),
        ("retry", true, retry),
        (
            "failed",
            true,
            input(
                failed(false),
                &wu,
                &units,
                none,
                "error(retryable=false): x",
            ),
        ),
        (
            "merge_repair_failed",
            true,
            input(
                failed(false),
                &merge,
                &merge_units,
                none,
                "error(retryable=false): c",
            ),
        ),
        (
            "harness_error",
            false,
            input(work_unit_run_end(None), &wu, &units, none, "infra"),
        ),
        (
            "harness_error",
            false,
            input(RunEnd::Cancelled, &wu, &units, none, "cancelled"),
        ),
        (
            "cluster_jobs",
            false,
            input(RunEnd::Waiting, &wu, &units, none, "waiting"),
        ),
    ];
    for (reason, reads, i) in cases {
        let s = settle_work_unit(i);
        assert_eq!(s.decision.as_ref().map(|d| d.reason), Some(reason));
        assert_eq!(s.reads_worker_decisions(), reads, "{reason}");
    }
    // 状態だけを戻す run は task の trigger と outcome の文を settle のまま残す（旧い試験から引き継いだ確かめ）。
    let reset = settle_work_unit(input(
        work_unit_run_end(None),
        &wu,
        &units,
        none,
        "infra_requeue: adapter: x",
    ));
    assert!(reset.reset_only);
    assert_eq!(reset.trigger, Trigger::WorkerError { retryable: false });
    assert_eq!(reset.outcome, "infra_requeue: adapter: x");
    assert_eq!(
        reset.decision.map(|d| d.updated.status),
        Some(WorkUnitStatus::Ready)
    );
}

/// 不変条件（ADR 2026-11-21-worker-decisions-fail-closed 付記 2026-11-21、I2・I7）: self の決定の要求で止めた WU は、
/// どの遷移でも `blocked(decision)`・task は `Continue{advance}` になり、WU の失敗（自動 replan・質問・replan の
/// 使い切りの決定）・integration repair の打ち切り・自動 replan の記録・失敗の伝播・待つ unit の ready を残さない
/// （1 つの leaf を止めるのは決定 1 か所）。止めた遷移が数えを動かす遷移でも行の数えは run の前の値のまま（replay は
/// `decision` の遷移で数えを動かさない）で、auto-leaf の上限の遷移は理由と continuation の数えを残す。質問で終わった
/// run は質問の文を outcome に残す。旧い試験は done の遷移の止めしか見ず、失敗の側の提案が止めの後にも残ることを
/// 検出できなかった。
#[test]
fn decision_fail_closed_hold_clears_failure_side_effects() {
    let wu = row("a", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let mut next = row("b", WorkUnitKind::Implement, WorkUnitStatus::Pending);
    next.depends_on = vec!["a".into()];
    let units = [wu.clone(), next];
    let advance = Trigger::Continue {
        why: ContinueWhy::Advance,
    };
    let held = |s: &WorkUnitSettlement, reason: &str| {
        let d = s.decision.as_ref().expect("transition");
        assert_eq!(d.reason, reason);
        assert_eq!(d.updated.status, WorkUnitStatus::Blocked);
        assert_eq!(
            d.updated.blocked_reason,
            Some(WorkUnitBlockedReason::Decision)
        );
        assert!(d.newly_ready.is_empty() && d.newly_blocked.is_empty());
        assert!(!d.plan_complete);
        assert!(s.failure.is_none() && s.integration_repair_exhaust.is_none());
        assert!(s.auto_replan.is_none());
    };

    // 失敗（待つ b は dependency_failed にならない）。止めた後の失敗の処理は何もしない。
    let mut s = settle_work_unit(input(
        RunEnd::Failed { retryable: false },
        &wu,
        &units,
        ContinuationInputs::default(),
        "error(retryable=false): boom",
    ));
    assert!(s.failure.is_some());
    hold(&mut s, &wu, 1);
    held(&s, "decision");
    assert_eq!(
        settle_work_unit_failure(&mut s, limits(0, 0, 1)),
        WorkUnitFailureStep::Settled
    );
    assert!(s.auto_replan.is_none());
    assert_eq!(s.trigger, advance);
    assert_eq!(
        s.outcome,
        "decision: WorkUnit a は人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）"
    );

    // 計画の問題（質問の文を残す）。
    let cp = checkpoint(0, Some("split it"));
    let planned = ContinuationInputs {
        checkpoint: Some(&cp),
        ..ContinuationInputs::default()
    };
    let mut s = settle_work_unit(input(budget(), &wu, &units, planned, "b"));
    assert_eq!(s.failure.as_ref().map(|f| f.reason), Some("plan_issue"));
    hold(&mut s, &wu, 1);
    held(&s, "decision");
    assert_eq!(s.trigger, advance);
    assert!(
        s.outcome
            .ends_with("止めた質問: WorkUnit a が計画の問題を申告しました: split it"),
        "{}",
        s.outcome
    );

    // worker の質問（質問の文を残す）。
    let mut s = settle_work_unit(input(
        RunEnd::Question,
        &wu,
        &units,
        ContinuationInputs::default(),
        "question: which vault?",
    ));
    hold(&mut s, &wu, 2);
    held(&s, "decision");
    assert_eq!(s.trigger, advance);
    assert_eq!(
        s.outcome,
        "decision: WorkUnit a は人の決定を待ちます（決定の要求 2 件。ADR-0079 D7）。止めた質問: which vault?"
    );

    // integration repair の失敗（打ち切らない）。
    let repair = row("repair-s1-1", WorkUnitKind::Repair, WorkUnitStatus::Running);
    let repair_units = [repair.clone()];
    let mut failed_repair = input(
        RunEnd::Failed { retryable: false },
        &repair,
        &repair_units,
        ContinuationInputs::default(),
        "error(retryable=false): boom",
    );
    failed_repair.integration_repair_trusted = Some(true);
    let mut s = settle_work_unit(failed_repair);
    assert!(s.integration_repair_exhaust.is_some());
    hold(&mut s, &repair, 1);
    held(&s, "decision");
    assert_eq!(s.trigger, advance);

    // done（待つ b を ready にせず、task の完了を advance に替える）。
    let mut s = settle_work_unit(input(
        RunEnd::Completed,
        &wu,
        &units,
        ContinuationInputs::default(),
        "done: ok",
    ));
    assert_eq!(s.decision.as_ref().map(|d| d.newly_ready.len()), Some(1));
    s.trigger = Trigger::WorkerDone;
    hold(&mut s, &wu, 2);
    held(&s, "decision");
    assert_eq!(s.trigger, advance);
    assert_eq!(
        s.outcome,
        "decision: WorkUnit a は人の決定を待ちます（決定の要求 2 件。ADR-0079 D7）"
    );

    // 再試行・continuation: 行の数えは run の前の値（`decision` の遷移は数えない）。
    let mut retry = input(
        RunEnd::Failed { retryable: true },
        &wu,
        &units,
        ContinuationInputs::default(),
        "error(retryable=true): x",
    );
    retry.max_retries = 1;
    let mut s = settle_work_unit(retry);
    hold(&mut s, &wu, 1);
    held(&s, "decision");
    assert_eq!(s.decision.as_ref().map(|d| d.updated.retries), Some(0));
    let fresh = checkpoint(1, None);
    let progressed = ContinuationInputs {
        checkpoint: Some(&fresh),
        ..ContinuationInputs::default()
    };
    let mut s = settle_work_unit(input(budget(), &wu, &units, progressed, "b"));
    hold(&mut s, &wu, 1);
    held(&s, "decision");
    assert_eq!(
        s.decision.as_ref().map(|d| d.updated.continuations),
        Some(0)
    );
    assert_eq!(s.trigger, advance);

    // auto-leaf の上限: 理由と continuation の数えを残す。
    let mut measured = input(budget(), &wu, &units, progressed, "b");
    measured.auto_leaf_measured = true;
    measured.auto_leaf_request = Some(request());
    let mut s = settle_work_unit(measured);
    hold(&mut s, &wu, 2);
    held(&s, "auto_leaf_budget");
    assert_eq!(
        s.decision.as_ref().map(|d| d.updated.continuations),
        Some(1)
    );
    assert_eq!(s.trigger, advance);
}
