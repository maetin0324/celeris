//! [`super::settle_task_run`] と [`super::phase_step_after_write`] の単体試験。
//!
//! 既存の試験は、atomic task の continuation と段の進み方を `dispatcher/tests` の経路全体（store・worktree・config を
//! 組む）で見る。経路全体の試験は分岐ごとに 1 つの組み合わせしか通さず、子 task 待ちと wait を continuation に数えない
//! こと・進捗なしの質問が固い上限より先なこと・段の状態ごとの trigger の置き換えと質問の扱いの境界を、入力から 1 か所で
//! 固定する試験は無かった。ここでその契約を固定する。

use super::*;
use task_core::{
    BudgetKind, CheckpointSource, WorkUnitBlockedReason, WorkUnitContext, WorkUnitKind,
    WorkUnitSpec, WorkUnitStatus,
};

fn checkpoint(completed: usize) -> Checkpoint {
    Checkpoint {
        schema: task_core::CHECKPOINT_SCHEMA.into(),
        task_id: "t".into(),
        work_unit: None,
        run_id: "r".into(),
        run_seq: 1,
        end: CheckpointEnd::BudgetExhausted,
        source: CheckpointSource::Mechanical,
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
        created_at: "2026-11-13T00:00:00Z".into(),
    }
}

fn saved(seq: u64, cp: &Checkpoint) -> (u64, Event) {
    (
        seq,
        Event::CheckpointSaved {
            run_id: format!("run-{seq}"),
            work_unit_id: None,
            checkpoint: Box::new(cp.clone()),
        },
    )
}

fn input<'a>(
    end: RunEnd,
    events: &'a [(u64, Event)],
    cp: &'a Checkpoint,
    children_pending: bool,
    (continuation_enabled, hard, no_progress): (bool, u32, u32),
) -> TaskRunInput<'a> {
    TaskRunInput {
        end,
        events,
        run_seq: 2,
        checkpoint: cp,
        children_pending,
        continuation_enabled,
        continuation_hard_limit: hard,
        no_progress_limit: no_progress,
        trigger: Trigger::WorkerError { retryable: true },
        outcome: "from settle".into(),
    }
}

/// 不変条件: クラスタ job の wait と子 task 待ちは checkpoint を残し、`[execution] continuation = false` や上限 0 でも
/// continuation・進捗なしの判定に回さない（wait は settle の trigger のまま、子待ちは `Continue{children_wait}`）。
/// 子待ちの checkpoint は `Waiting` で残す（次の run の進捗の基準にしない）。
/// 既存の試験で検出できない理由: 経路全体の試験は continuation が有効な設定で子待ちを 1 通り見るだけで、無効の設定や
/// 上限 0 と子待ちの組み合わせ、checkpoint の終わり方を見ていない。
#[test]
fn settle_task_run_waits_for_children_and_cluster_jobs_without_counting() {
    let cp = checkpoint(0);
    let limits = (false, 0, 0);
    let mut wait = input(RunEnd::Waiting, &[], &cp, false, limits);
    wait.trigger = Trigger::ClusterJobWait;
    let s = settle_task_run(wait);
    assert_eq!(s.trigger, Trigger::ClusterJobWait);
    assert_eq!(s.outcome, "from settle");
    assert!(s.saves_checkpoint);

    let s = settle_task_run(input(RunEnd::Yielded, &[], &cp, true, limits));
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: ContinueWhy::ChildrenWait
        }
    );
    assert_eq!(
        s.outcome,
        "continue: 委譲した子の完了を待ちます（yielded の続き）"
    );
    assert!(s.saves_checkpoint);

    assert_eq!(
        task_checkpoint_end(RunEnd::Yielded, true),
        CheckpointEnd::Waiting
    );
    assert_eq!(
        task_checkpoint_end(RunEnd::Waiting, false),
        CheckpointEnd::Waiting
    );
    assert_eq!(
        task_checkpoint_end(
            RunEnd::BudgetExhausted {
                kind: BudgetKind::Turns
            },
            false
        ),
        CheckpointEnd::BudgetExhausted
    );
}

/// 不変条件: `[execution] continuation = false` の run は `WorkerError{retryable:true}` に戻し、checkpoint を残さない
/// （ADR-0072 §6 (f)）。
/// 既存の試験で検出できない理由: 経路全体の試験は文の接頭辞だけを見て、checkpoint を残さないことを入力の組で固定していない。
#[test]
fn settle_task_run_disabled_continuation_retries_without_a_checkpoint() {
    let cp = checkpoint(1);
    let s = settle_task_run(input(
        RunEnd::BudgetExhausted {
            kind: BudgetKind::WallClock,
        },
        &[],
        &cp,
        false,
        (false, 10, 10),
    ));
    assert_eq!(
        s,
        TaskRunSettlement {
            trigger: Trigger::WorkerError { retryable: true },
            outcome: "error(retryable=true): budget_exhausted(wall_clock) (continuation disabled)"
                .into(),
            saves_checkpoint: false,
        }
    );
}

/// 不変条件: 進捗がある間は回数で聞かず `Continue{continue}` で次の run（Run #run_seq+1）に進む。聞くのは進捗なしの連続
/// （`no_progress_limit` ちょうど）と固い上限（`continuation_hard_limit` ちょうど）で、両方に当たれば進捗なしの質問を出す。
/// 質問の文は `execution_scheduler` の質問の関数と同じ（2026-10-20-continuation-limit-by-progress D2・D3）。
/// 既存の試験で検出できない理由: 経路全体の試験は上限の片方ずつを多めの回数で見るだけで、ちょうどの境界と両方に当たる
/// ときの優先を見ていない。
#[test]
fn settle_task_run_asks_on_no_progress_before_the_hard_limit() {
    let cp = checkpoint(1);
    let s = settle_task_run(input(RunEnd::Yielded, &[], &cp, false, (true, 5, 5)));
    assert_eq!(
        s.trigger,
        Trigger::Continue {
            why: ContinueWhy::Continue
        }
    );
    assert_eq!(s.outcome, "continue: yielded の続き（Run #3）");
    assert!(s.saves_checkpoint);

    // 直前の checkpoint と同じ（進捗なし）: 1 回目で no_progress = 1。
    let events = vec![saved(1, &cp)];
    let s = settle_task_run(input(RunEnd::Yielded, &events, &cp, false, (true, 5, 1)));
    assert_eq!(s.trigger, Trigger::WorkerQuestion);
    assert_eq!(
        s.outcome,
        crate::execution_scheduler::no_progress_question(None, 1, 0)
    );
    let s = settle_task_run(input(RunEnd::Yielded, &events, &cp, false, (true, 5, 2)));
    assert!(matches!(s.trigger, Trigger::Continue { .. }));

    // 進捗あり・固い上限ちょうど（continuation 0 回で上限 0）。
    let s = settle_task_run(input(RunEnd::Yielded, &[], &cp, false, (true, 0, 5)));
    assert_eq!(s.trigger, Trigger::WorkerQuestion);
    assert_eq!(
        s.outcome,
        crate::execution_scheduler::continuation_cap_question(
            None,
            0,
            &ContinuationFacts::new(0, 0, &cp)
        )
    );

    // 両方に当たれば進捗なしの質問。
    let s = settle_task_run(input(RunEnd::Yielded, &events, &cp, false, (true, 0, 1)));
    assert_eq!(
        s.outcome,
        crate::execution_scheduler::no_progress_question(None, 1, 0)
    );
}

/// 不変条件: atomic task の run が worker の `decisions` を読むのは `WorkerDone` かつ `Completed` の run だけで、
/// `needed_before: self` なら `Continue{advance}` で最終レビューに進めない（ADR-0079 D7）。
/// 既存の試験で検出できない理由: 経路全体の試験は done の run の 1 通りだけで、質問・失敗・部をまたぐ質問の後の trigger で
/// 読まないことを見ていない。
#[test]
fn worker_decisions_hold_an_atomic_run_only_when_done() {
    assert!(task_reads_worker_decisions(
        &Trigger::WorkerDone,
        Some(RunEnd::Completed)
    ));
    assert!(!task_reads_worker_decisions(
        &Trigger::WorkerQuestion,
        Some(RunEnd::Completed)
    ));
    assert!(!task_reads_worker_decisions(&Trigger::WorkerDone, None));
    assert_eq!(
        hold_task_for_worker_decisions(2),
        (
            Trigger::Continue {
                why: ContinueWhy::Advance
            },
            "decision: 人の決定を待ちます（決定の要求 2 件。ADR-0079 D7）".to_string()
        )
    );
}

fn unit(key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
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
        phase: Some("p1".into()),
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

fn blocked(key: &str, reason: WorkUnitBlockedReason) -> WorkUnitRow {
    let mut u = unit(key, WorkUnitKind::Implement, WorkUnitStatus::Blocked);
    u.blocked_reason = Some(reason);
    u
}

/// 不変条件: WU の行を書いた後の段の進み方は `execution_scheduler::settle_phase` の結果で決まり、兄弟か統合が走る間は
/// task を遷移させず質問を捨てる。統合を始める段では統合 WU の id を返す。起こせる WU がある段では done の trigger だけを
/// `Continue{advance}` に替え、全部済めば `WorkerDone` にする。止まった別の unit は適用側に読み直させ、この run の unit
/// が止まった段では trigger を替えない。
/// 既存の試験で検出できない理由: `settle_phase` の試験は段の状態だけを見て、task の trigger の置き換えと質問の扱いは
/// 経路全体の試験が段の状態ごとに 1 通りずつ見るだけだった。
#[test]
fn phase_step_after_write_keeps_the_task_running_or_replaces_the_trigger() {
    let done = |k: &str| unit(k, WorkUnitKind::Implement, WorkUnitStatus::Done);
    let running = unit("b", WorkUnitKind::Implement, WorkUnitStatus::Running);
    let step = phase_step_after_write(&[done("a"), running], "id-a", &Trigger::WorkerDone);
    assert!(step.stays_running());
    assert!(step.clear_questions);
    assert_eq!((&step.trigger, step.integration()), (&None, None));

    let integ = unit("i", WorkUnitKind::Integrate, WorkUnitStatus::Pending);
    let step = phase_step_after_write(&[done("a"), integ], "id-a", &Trigger::WorkerDone);
    assert!(step.stays_running());
    assert_eq!(step.integration(), Some("id-i"));

    let ready = unit("b", WorkUnitKind::Implement, WorkUnitStatus::Ready);
    let step = phase_step_after_write(&[done("a"), ready.clone()], "id-a", &Trigger::WorkerDone);
    assert!(!step.stays_running() && !step.clear_questions);
    assert_eq!(
        step.trigger,
        Some(Trigger::Continue {
            why: ContinueWhy::Advance
        })
    );
    let error = Trigger::WorkerError { retryable: true };
    let step = phase_step_after_write(&[done("a"), ready], "id-a", &error);
    assert_eq!(step.trigger, None);

    let step = phase_step_after_write(&[done("a"), done("b")], "id-a", &Trigger::WorkerDone);
    assert_eq!(step.phase, PhaseSettle::AllDone);
    assert_eq!(step.trigger, Some(Trigger::WorkerDone));

    let asked = blocked("b", WorkUnitBlockedReason::Question);
    let step = phase_step_after_write(&[done("a"), asked.clone()], "id-a", &Trigger::WorkerDone);
    assert_eq!(step.deferred_unit.as_deref(), Some("id-b"));
    assert_eq!(step.trigger, None);
    let step = phase_step_after_write(&[asked], "id-b", &Trigger::WorkerQuestion);
    assert_eq!(step.deferred_unit, None);
    let failed = blocked("b", WorkUnitBlockedReason::Limit);
    let step = phase_step_after_write(&[done("a"), failed], "id-a", &Trigger::WorkerDone);
    assert_eq!(step.phase, PhaseSettle::Failure("id-b".into()));
    assert_eq!(step.deferred_unit.as_deref(), Some("id-b"));
}
