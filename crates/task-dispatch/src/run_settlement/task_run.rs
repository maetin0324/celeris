//! ADR 2026-11-11 dispatcher-state-ownership D3: task の段の判断（[`super::settle`] の後の atomic task の段と、WU の
//! 書き込みの後の段の進み方）。
//!
//! atomic task の run（WU を持たない run）は [`settle_task_run`] が continuation・子 task 待ち・進捗なしの連続・
//! continuation の固い上限の質問を決める。質問の文は `execution_scheduler` の質問の関数を呼び、写さない。
//! checkpoint の組み立て（git・`checkpoint.json`）・委譲した子の読み出し・events の読み出しは worker 完了の経路が行い、
//! 結果を入力に入れる。
//!
//! v2 の WU の run は、WU の行を書いた後に [`phase_step_after_write`] で task の段の進み方を決める（`execution_scheduler`
//! の段の関数を呼ぶ）。WU の行の読み直しと、兄弟が走っている間に止まった unit の trigger（events の追記を伴う）は
//! worker 完了の経路が提案を見て行う。

use task_core::{Checkpoint, CheckpointEnd, ContinueWhy, Event, RunEnd, Trigger, WorkUnitRow};

use super::describe_run_end;
use crate::execution_scheduler::{ContinuationFacts, PhaseSettle};

/// atomic task の run の checkpoint の終わり方。委譲した子を待つ run（ADR 2026-10-18-parent-waits-for-children）は
/// `Waiting` にし、continuation と進捗なしに数えない。
pub(crate) fn task_checkpoint_end(end: RunEnd, children_pending: bool) -> CheckpointEnd {
    if children_pending {
        CheckpointEnd::Waiting
    } else {
        end.as_checkpoint_end()
            .unwrap_or(CheckpointEnd::BudgetExhausted)
    }
}

/// [`settle_task_run`] の入力。worker 完了の経路が checkpoint を組み立て、events と委譲した子を読んで詰める。
/// 呼ぶのは `end.saves_checkpoint()` の run だけ。
pub(crate) struct TaskRunInput<'a> {
    pub end: RunEnd,
    /// この task の events（continuation の回数・直前の checkpoint・進捗なしの連続を数える）。
    pub events: &'a [(u64, Event)],
    /// この run の番号（`task_ops::derive::current_run_seq`）。
    pub run_seq: u32,
    /// この run の checkpoint（[`task_checkpoint_end`] の終わり方で組み立てたもの）。
    pub checkpoint: &'a Checkpoint,
    /// 委譲した子が未完了か、失敗した子に「再実行を待て」の印がある（`Waiting` の run では読まず `false`）。
    pub children_pending: bool,
    /// `[execution] continuation`。
    pub continuation_enabled: bool,
    /// `[execution] continuation_hard_limit`。
    pub continuation_hard_limit: u32,
    /// `[execution] no_progress_limit`。
    pub no_progress_limit: u32,
    /// [`super::settle`] の提案の trigger と outcome の文。
    pub trigger: Trigger,
    pub outcome: String,
}

/// [`settle_task_run`] の提案。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TaskRunSettlement {
    pub trigger: Trigger,
    pub outcome: String,
    /// checkpoint を `CheckpointSaved` と runs 索引に残すか（`[execution] continuation = false` の run は残さない）。
    pub saves_checkpoint: bool,
}

/// D3: atomic task の run（予算切れ・yield・クラスタ job の wait）の続け方を決める。
///
/// - クラスタ job の wait（ADR-0090 D1）と子 task 待ちは checkpoint を残し、continuation の上限・進捗なしに数えない。
///   wait の trigger は [`super::settle`] のまま、子 task 待ちは `Continue{children_wait}` にする。
/// - `[execution] continuation = false` なら `WorkerError{retryable:true}` に戻し、checkpoint を残さない（ADR-0072 §6 (f)）。
/// - 2026-10-20-continuation-limit-by-progress D2: 進捗がある間は回数で聞かない。聞くのは進捗なしの連続と固い上限
///   （暴走止め）だけで、いずれも失敗にせず人に聞く。
pub(crate) fn settle_task_run(input: TaskRunInput<'_>) -> TaskRunSettlement {
    let TaskRunInput {
        end,
        events,
        run_seq,
        checkpoint,
        children_pending,
        ..
    } = input;
    if end == RunEnd::Waiting || children_pending {
        let (trigger, outcome) = if children_pending {
            (
                Trigger::Continue {
                    why: ContinueWhy::ChildrenWait,
                },
                format!(
                    "continue: 委譲した子の完了を待ちます（{} の続き）",
                    describe_run_end(end)
                ),
            )
        } else {
            (input.trigger, input.outcome)
        };
        return TaskRunSettlement {
            trigger,
            outcome,
            saves_checkpoint: true,
        };
    }
    if !input.continuation_enabled {
        return TaskRunSettlement {
            trigger: Trigger::WorkerError { retryable: true },
            outcome: format!(
                "error(retryable=true): {} (continuation disabled)",
                describe_run_end(end)
            ),
            saves_checkpoint: false,
        };
    }
    let continuations = task_ops::derive::consecutive_continuations(events);
    let prev = task_ops::derive::latest_progress_checkpoint(events, None);
    let no_progress = if task_core::checkpoint_shows_progress(prev.as_ref(), checkpoint) {
        0
    } else {
        task_ops::derive::no_progress_streak(events, None) + 1
    };
    let (trigger, outcome) = if no_progress >= input.no_progress_limit {
        (
            Trigger::WorkerQuestion,
            crate::execution_scheduler::no_progress_question(None, no_progress, continuations),
        )
    } else if continuations >= input.continuation_hard_limit {
        let facts = ContinuationFacts::new(
            continuations,
            task_ops::derive::progress_checkpoints_since_answer(events, None),
            checkpoint,
        );
        (
            Trigger::WorkerQuestion,
            crate::execution_scheduler::continuation_cap_question(
                None,
                input.continuation_hard_limit,
                &facts,
            ),
        )
    } else {
        (
            Trigger::Continue {
                why: ContinueWhy::Continue,
            },
            format!(
                "continue: {} の続き（Run #{}）",
                describe_run_end(end),
                run_seq + 1
            ),
        )
    };
    TaskRunSettlement {
        trigger,
        outcome,
        saves_checkpoint: true,
    }
}

/// ADR-0079 D7（Phase R3a）: atomic task の run が worker の `result.json` の `decisions` を読むか（done で終わった run だけ）。
pub(crate) fn task_reads_worker_decisions(trigger: &Trigger, end: Option<RunEnd>) -> bool {
    matches!(trigger, Trigger::WorkerDone) && end == Some(RunEnd::Completed)
}

/// ADR-0079 D7: `needed_before: self` の決定の要求を出した atomic task の run は、最終レビューに進めず `ready` に戻す
/// （`advance`。答えが出るまで run を起こさない）。返すのは置き換える trigger と outcome の文。
pub(crate) fn hold_task_for_worker_decisions(count: usize) -> (Trigger, String) {
    (
        Trigger::Continue {
            why: ContinueWhy::Advance,
        },
        format!("decision: 人の決定を待ちます（決定の要求 {count} 件。ADR-0079 D7）"),
    )
}

/// [`phase_step_after_write`] の提案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhaseStep {
    /// 書き込みの後の段の状態（記録に使う）。
    pub phase: PhaseSettle,
    /// task の trigger の置き換え（`None` なら今の trigger のまま）。
    pub trigger: Option<Trigger>,
    /// 人に見せる質問を捨てる（兄弟が走っている・統合を始める間。質問は in-flight が 0 になってから聞く）。
    pub clear_questions: bool,
    /// 兄弟が走っている間に question か失敗で止まった別の unit の id。worker 完了の経路が WU の行と events を読み直し、
    /// その unit の trigger と質問に置き換える。
    pub deferred_unit: Option<String>,
}

impl PhaseStep {
    /// task を遷移させない（兄弟の run か統合が走っている、または段の統合を始める）。events だけを残す。
    pub(crate) fn stays_running(&self) -> bool {
        matches!(self.phase, PhaseSettle::Wait | PhaseSettle::Integrate(_))
    }

    /// 始める統合 WU の id。
    pub(crate) fn integration(&self) -> Option<&str> {
        match &self.phase {
            PhaseSettle::Integrate(id) => Some(id),
            _ => None,
        }
    }
}

/// ADR-0074 D1.6（Phase F2b）: v2 の WU の run で WU の行を書いた後、task の段をどう進めるか。`units` は書き込みの後の
/// 全行、`unit_id` はこの run の WU、`trigger` はここまでの task の trigger。兄弟の WU（または統合）が走っていれば task は
/// 遷移させない。in-flight が 0 になったら段の状態（question → 失敗 → 起こせる WU → 統合）で決める。
pub(crate) fn phase_step_after_write(
    units: &[WorkUnitRow],
    unit_id: &str,
    trigger: &Trigger,
) -> PhaseStep {
    let phase = crate::execution_scheduler::settle_phase(units);
    let mut step = PhaseStep {
        phase: phase.clone(),
        trigger: None,
        clear_questions: false,
        deferred_unit: None,
    };
    match phase {
        PhaseSettle::Wait | PhaseSettle::Integrate(_) => step.clear_questions = true,
        PhaseSettle::Advance if matches!(trigger, Trigger::WorkerDone) => {
            step.trigger = Some(Trigger::Continue {
                why: ContinueWhy::Advance,
            });
        }
        PhaseSettle::Question(id) | PhaseSettle::Failure(id) if id != unit_id => {
            step.deferred_unit = Some(id);
        }
        PhaseSettle::AllDone => step.trigger = Some(Trigger::WorkerDone),
        _ => {}
    }
    step
}

#[cfg(test)]
mod tests;
