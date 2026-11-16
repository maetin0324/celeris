//! ADR 2026-11-11 dispatcher-state-ownership D3: WU の run の終わりの判断（[`super::settle`] の後の WU の段）。
//!
//! WU の遷移（状態・カウンタ・依存の伝播・完了）の正本は `execution_scheduler` の遷移の関数で、ここはそれを呼び、
//! 結果から task の `Trigger`・outcome の文・auto-leaf の上限・merge の repair の退き方・integration repair の扱い・
//! WU の失敗の replan か質問（[`super::replan_or_ask`]）・worker の決定の要求による保留を決める。
//!
//! git の状態・checkpoint・永続化の行・auto-leaf の決定の要求・replan の上限の入力（[`super::ReplanLimits`]）は
//! worker 完了の経路が読んで入力に入れる。判断の結果で読むものが変わる所（replan の上限・worker の決定の要求）と
//! 副作用（integration repair の打ち切り・replan を使い切った後の決定の要求）は、提案の欄を見て worker 完了の経路が
//! 今と同じ順で行い、その結果をもう一度この module の関数に渡す。

use task_core::{
    ContinueWhy, DecisionRequest, Event, IntegrationRepairExhaustReason, RunEnd, Trigger,
    WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitStatus,
};

use super::{AskReason, ReplanLimits, ReplanOrAsk, describe_run_end, replan_or_ask};
use crate::execution_scheduler::{ContinuationInputs, WuDecision, WuLimits};

/// ADR 2026-10-29-phase-integration-auto-repair D2: 失敗した merge の repair WU を退かせたときの
/// `WorkUnitTransitioned.reason`。
const MERGE_REPAIR_FAILED_REASON: &str = "merge_repair_failed";

/// ADR-0072 D6 の表: 分類できない供給側・インフラの失敗（`end` が無い）は harness_error 相当として WU を ready に
/// 戻すだけにする。
pub(crate) fn work_unit_run_end(run_end: Option<RunEnd>) -> RunEnd {
    run_end.unwrap_or(RunEnd::HarnessError {
        class: task_core::HarnessErrorClass::Infra,
    })
}

/// WU の run の checkpoint を組み立てるか。continuation に回る終わり方で `[execution] continuation` が有効か
/// auto-leaf の上限を超えた run と、クラスタ job の wait（ADR-0090 D1。`continuation` に依らない）。
pub(crate) fn work_unit_saves_checkpoint(
    end: RunEnd,
    continuation: bool,
    auto_leaf_exceeded: bool,
) -> bool {
    (end.is_continuable() && (continuation || auto_leaf_exceeded)) || end == RunEnd::Waiting
}

/// [`settle_work_unit`] の入力。worker 完了の経路が [`super::settle`] の提案と、読んだ値を詰める。
pub(crate) struct WorkUnitRunInput<'a> {
    /// [`work_unit_run_end`] の終わり方。
    pub end: RunEnd,
    pub run_id: &'a str,
    /// いま `running` の WU の行。
    pub wu: &'a WorkUnitRow,
    /// この task の全 WU（`wu` を含む）。
    pub units: &'a [WorkUnitRow],
    /// 組み立てた checkpoint と、events から数えた進捗なし・continuation の回数。
    pub continuation: ContinuationInputs<'a>,
    /// `[execution] continuation`。
    pub continuation_enabled: bool,
    /// `[execution] continuation_hard_limit`。
    pub continuation_hard_limit: u32,
    /// `[execution] no_progress_limit`。
    pub no_progress_limit: u32,
    /// task の `max_retries`。
    pub max_retries: u32,
    /// auto-leaf で回数を測っている leaf か。測っている leaf は continuation と進捗なしの上限を外す
    /// （測った回数の上限が代わりに効く）。
    pub auto_leaf_measured: bool,
    /// 測った回数が上限を超えた leaf の決定の要求（超えていなければ `None`）。
    pub auto_leaf_request: Option<DecisionRequest>,
    /// integration repair の WU なら、その結果を信頼できるか（worktree の git の状態）。他の WU は `None`。
    pub integration_repair_trusted: Option<bool>,
    /// [`super::settle`] の提案の trigger と outcome の文。
    pub trigger: Trigger,
    pub outcome: String,
    /// outcome の文に足す check の不合格の要約（無ければ空）。
    pub check_failure_suffix: &'a str,
    /// check の不合格の失敗の署名（不合格でなければ `None`。outcome の文から作る）。
    pub check_failure_signature: Option<String>,
    /// worker が `result.json` を置き違えた run（WU の失敗から自動 replan を起こさない）。
    pub misplaced: bool,
}

/// [`settle_work_unit`] の提案。worker 完了の経路は欄を見て副作用と追加の読み出しをし、最後に trigger・outcome の文・
/// WU の行（`decision`）を同じ transaction の書き込みに渡す。
#[derive(Debug)]
pub(crate) struct WorkUnitSettlement {
    pub trigger: Trigger,
    pub outcome: String,
    /// WU の遷移。
    pub decision: Option<WuDecision>,
    /// WU の状態だけを戻す run（harness_error・cancelled）。task の trigger と outcome の文は settle のまま。
    pub reset_only: bool,
    /// auto-leaf の上限で leaf を止めたときの決定の要求（記録して、この leaf を `blocked(decision)` にする）。
    pub auto_leaf_request: Option<DecisionRequest>,
    /// integration repair を打ち切る理由。worker 完了の経路が打ち切りを行い、結果を
    /// [`integration_repair_exhausted`] に渡す。
    pub integration_repair_exhaust: Option<IntegrationRepairExhaustReason>,
    /// 自動 replan か人に聞くかを決める WU の失敗。worker 完了の経路が replan の上限を読み、
    /// [`settle_work_unit_failure`] に渡す。
    pub failure: Option<WorkUnitFailure>,
    /// 自動 replan の記録（ADR 2026-10-22-stalled-auto-replan D1）。
    pub auto_replan: Option<Event>,
}

impl WorkUnitSettlement {
    /// worker の `result.json` の `decisions` を読む run か（done にした WU だけ。ADR-0079 D7）。
    pub(crate) fn reads_worker_decisions(&self) -> bool {
        !self.reset_only
            && self
                .decision
                .as_ref()
                .is_some_and(|d| d.reason == "completed")
    }
}

/// [`settle_work_unit_failure`] に渡す WU の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkUnitFailure {
    pub key: String,
    /// `failed`・`limit`・`plan_issue` のどれか。
    pub reason: &'static str,
    /// 失敗の署名（`task_ops::plan_gate::failure_signature` か check の不合格の署名）。
    pub failure: String,
    /// worker が申告した計画の問題の文（`plan_issue` でなければ空）。
    pub plan_issue: String,
    pub check_failure_suffix: String,
}

/// D3: WU の run の終わり方から、WU の遷移と task の trigger・outcome の文を決める。
pub(crate) fn settle_work_unit(input: WorkUnitRunInput<'_>) -> WorkUnitSettlement {
    let end = input.end;
    let wu = input.wu;
    let reset_only = matches!(end, RunEnd::HarnessError { .. } | RunEnd::Cancelled);
    let mut s = WorkUnitSettlement {
        trigger: input.trigger,
        outcome: input.outcome,
        decision: None,
        reset_only,
        auto_leaf_request: None,
        integration_repair_exhaust: None,
        failure: None,
        auto_replan: None,
    };
    // ADR-0072 §6 (f)・ADR 2026-11-15-research-adapter-deadline D3: continuation を使えない run（`[execution]
    // continuation = false`、または run の性質で続けられない）の予算切れ・yield は、checkpoint を残さない retryable な
    // 失敗として WU の retry（`max_retries` を使い切れば失敗）に回す。以前は WU の行を変えずに返したので、WU が
    // `running` のまま task だけが `ready` に戻り、次の WU を選べずに止まった。
    let end =
        if end.is_continuable() && !input.continuation_enabled && input.auto_leaf_request.is_none()
        {
            s.outcome = format!(
                "error(retryable=true): {} (continuation disabled)",
                describe_run_end(end)
            );
            RunEnd::Failed { retryable: true }
        } else {
            end
        };
    // auto-leaf で測っている leaf は、測った回数の上限が continuation と進捗なしの上限の代わりになる。失敗と
    // plan_issue の扱いは変えない。
    let limits = WuLimits {
        continuation_hard_limit: if input.auto_leaf_measured {
            u32::MAX
        } else {
            input.continuation_hard_limit
        },
        no_progress_limit: if input.auto_leaf_measured {
            u32::MAX
        } else {
            input.no_progress_limit
        },
        max_retries: input.max_retries,
    };
    let mut decision = crate::execution_scheduler::decide(
        end,
        input.run_id,
        wu,
        input.units,
        input.continuation,
        limits,
    );
    if decision.reason == "continue"
        && let Some(request) = input.auto_leaf_request
    {
        s.outcome = format!("decision: {}", request.question);
        hold_for_auto_leaf(&mut decision);
        s.auto_leaf_request = Some(request);
    }
    retire_failed_merge_repair(wu, &mut decision);
    if !reset_only {
        s.trigger = decision.trigger.clone();
        let plan_issue = input
            .continuation
            .checkpoint
            .and_then(|cp| cp.plan_issue.clone())
            .unwrap_or_default();
        work_unit_outcome(
            &mut s,
            &decision,
            wu,
            end,
            limits.max_retries,
            input.check_failure_suffix,
            &plan_issue,
        );
        // ADR-0072 D17（Phase E4）/ D17 3.（Phase E4b 項目2）: WU が failed、進捗なし/continuation の上限
        // （"limit"）、`plan_issue` の申告なら、replan の余地があれば task を failed/blocked にする代わりに replan の
        // planner run を起こす。integration repair の WU は replan せず打ち切る。
        let failed = matches!(decision.reason, "failed" | "limit" | "plan_issue");
        if let Some(trusted) = input.integration_repair_trusted
            && (failed || (decision.reason == "completed" && !trusted))
        {
            s.integration_repair_exhaust = Some(match decision.reason {
                "plan_issue" => IntegrationRepairExhaustReason::PlanIssue,
                "failed" => IntegrationRepairExhaustReason::WorkUnitFailed,
                "completed" => IntegrationRepairExhaustReason::ResultUntrusted,
                _ if matches!(end, RunEnd::BudgetExhausted { .. }) => {
                    IntegrationRepairExhaustReason::BudgetExhausted
                }
                _ => IntegrationRepairExhaustReason::WorkUnitFailed,
            });
        } else if !input.misplaced && failed {
            let failure = input.check_failure_signature.unwrap_or_else(|| {
                task_ops::plan_gate::failure_signature(&[decision.reason], &s.outcome)
            });
            s.failure = Some(WorkUnitFailure {
                key: wu.key.clone(),
                reason: decision.reason,
                failure,
                plan_issue,
                check_failure_suffix: input.check_failure_suffix.to_string(),
            });
        }
    }
    s.decision = Some(decision);
    s
}

/// WU の遷移の理由ごとの outcome の文（と v1 の task のクラスタ job の wait の trigger）。
fn work_unit_outcome(
    s: &mut WorkUnitSettlement,
    decision: &WuDecision,
    wu: &WorkUnitRow,
    end: RunEnd,
    max_retries: u32,
    check_failure_suffix: &str,
    plan_issue: &str,
) {
    match decision.reason {
        "failed" => {
            let msg = s
                .outcome
                .strip_prefix("error(retryable=false): ")
                .or_else(|| s.outcome.strip_prefix("error(retryable=true): "))
                .unwrap_or(s.outcome.as_str());
            s.outcome = format!("error(retryable=false): work unit {} failed: {msg}", wu.key);
        }
        "limit" | "continuation_cap" | MERGE_REPAIR_FAILED_REASON => {
            if let Some(question) = &decision.outcome_override {
                s.outcome = question.clone();
            }
        }
        "continue" => {
            s.outcome = format!(
                "continue: {} の続き（WorkUnit {}, Run #{}）",
                describe_run_end(end),
                wu.key,
                wu.runs + 1
            );
        }
        "retry" => {
            s.outcome = format!(
                "work_unit_retry: WorkUnit {} を最初からやり直します（{}/{}）{check_failure_suffix}",
                wu.key, decision.updated.retries, max_retries
            );
        }
        // ADR-0090 D2: unit はクラスタ job を待つ（`blocked(cluster_jobs)`）。v2 の task は兄弟を止めず `advance` で
        // 進み、段階の unit を順に並べる v1 の task は task ごと待つ。
        "cluster_jobs" => {
            s.outcome = format!("{}（WorkUnit {}）", s.outcome, wu.key);
            if wu.phase.is_none() {
                s.trigger = Trigger::ClusterJobWait;
            }
        }
        // ADR-0072 D17 3.（Phase E4b 項目2）: worker の checkpoint/result.json が `plan_issue` を書いた。
        "plan_issue" => {
            s.outcome = format!(
                "question: WorkUnit {} が計画の問題を申告しました: {plan_issue}",
                wu.key
            );
        }
        _ => {}
    }
}

/// auto-leaf の測った回数が上限を超えた leaf を、continuation の代わりに `blocked(decision)` で止める
/// （task は `Continue{advance}`。この leaf を待つ unit は ready にしない）。
fn hold_for_auto_leaf(decision: &mut WuDecision) {
    decision.updated.status = WorkUnitStatus::Blocked;
    decision.updated.blocked_reason = Some(WorkUnitBlockedReason::Decision);
    decision.reason = "auto_leaf_budget";
    decision.trigger = Trigger::Continue {
        why: ContinueWhy::Advance,
    };
    decision.plan_complete = false;
    decision.newly_ready.clear();
    decision.newly_blocked.clear();
}

/// merge の repair WU か（統合後の検査の repair `repair-<phase>-*` とは別）。
fn is_merge_repair(wu: &WorkUnitRow) -> bool {
    wu.kind == WorkUnitKind::Repair && wu.phase.is_some() && wu.key.starts_with("merge-")
}

/// ADR 2026-10-29-phase-integration-auto-repair D2: merge の repair WU が失敗した（再試行を使い切った・進まない・
/// 計画の問題を申告した）ら、replan せずにその WU を `cancelled` にして退かせ、統合 WU を `pending` に戻す（task は
/// `Continue{advance}`）。やり直した統合は同じ衝突でもう一度 repair WU を作り、上限（`MAX_MERGE_REPAIRS_PER_UNIT`）を
/// 超えたら理由を付けて統合の依頼にする（人の受信箱）。merge の repair WU でない・失敗でない決定は変えない。
fn retire_failed_merge_repair(wu: &WorkUnitRow, decision: &mut WuDecision) {
    if !is_merge_repair(wu) || !matches!(decision.reason, "failed" | "limit" | "plan_issue") {
        return;
    }
    let previous = decision.reason;
    decision.updated.status = WorkUnitStatus::Cancelled;
    decision.updated.blocked_reason = None;
    decision.reason = MERGE_REPAIR_FAILED_REASON;
    decision.trigger = Trigger::Continue {
        why: ContinueWhy::Advance,
    };
    decision.plan_complete = false;
    for row in decision
        .newly_blocked
        .iter_mut()
        .filter(|row| row.kind == WorkUnitKind::Integrate)
    {
        row.status = WorkUnitStatus::Pending;
        row.blocked_reason = None::<WorkUnitBlockedReason>;
        row.depends_on.retain(|d| d != &wu.key);
        row.spec.depends_on.retain(|d| d != &wu.key);
    }
    decision.outcome_override = Some(format!(
        "merge の repair WU {} が衝突を解けなかった（{previous}）。段の統合をやり直す（上限を超えたら統合の依頼として人に回す）",
        wu.key
    ));
}

/// integration repair を打ち切った後の task の trigger と outcome の文。`fallback` は打ち切りで task の branch を
/// repair の前の HEAD に戻せた（か、既にそこにある）か。戻せたら reviewer に同期していない HEAD を見せ、戻せなければ
/// 人に調べてもらう。
pub(crate) fn integration_repair_exhausted(key: &str, fallback: bool) -> (Trigger, String) {
    if fallback {
        (
            Trigger::WorkerDone,
            format!("integration repair {key} exhausted; review unsynced HEAD"),
        )
    } else {
        (
            Trigger::WorkerQuestion,
            format!("question: integration repair {key} needs manual inspection"),
        )
    }
}

/// [`settle_work_unit_failure`] が worker 完了の経路に返す、残りの仕事。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkUnitFailureStep {
    /// trigger と outcome の文を決め終えた（自動 replan・plan_issue の hint・何も足さない）。
    Settled,
    /// 同じ失敗の自動 replan が進展なしに続いたので人に聞く（`stalled` は続いた回数。記録の log に使う）。
    Stalled { stalled: u32 },
    /// replan を使い切った後の WU の失敗（ADR-0079 付記「R6-1」D3）。worker 完了の経路が、木の節点なら
    /// `limit:max_replans` の決定の要求、木でない task なら人への質問にする（`why` は失敗の文、`used` は数えた replan）。
    AskExhausted { why: String, used: u32 },
}

/// D4 wu-failure: WU の失敗から自動 replan を起こすか、人に聞くか（[`replan_or_ask`]）。`limits` は worker 完了の
/// 経路が [`WorkUnitSettlement::failure`] の unit の key と失敗の署名で読んだ値。
pub(crate) fn settle_work_unit_failure(
    s: &mut WorkUnitSettlement,
    limits: ReplanLimits,
) -> WorkUnitFailureStep {
    let Some(f) = s.failure.clone() else {
        return WorkUnitFailureStep::Settled;
    };
    let why = s
        .outcome
        .strip_prefix("error(retryable=false): ")
        .unwrap_or(s.outcome.as_str())
        .to_string();
    match replan_or_ask(limits) {
        // ADR 2026-10-22-stalled-auto-replan D1 / D4: 同じ WU・同じ失敗の自動 replan が進展なしに続いたら、
        // `max_replans` に余りがあっても止めて人に聞く。
        ReplanOrAsk::Ask(AskReason::Stalled) => {
            s.trigger = Trigger::WorkerQuestion;
            s.outcome = format!(
                "question: {}",
                task_ops::plan_gate::replan_stalled_question(&f.key, limits.stalled, &why)
            );
            WorkUnitFailureStep::Stalled {
                stalled: limits.stalled,
            }
        }
        ReplanOrAsk::Replan => {
            let text = match f.reason {
                "failed" => format!("work unit {} failed{}", f.key, f.check_failure_suffix),
                "limit" => format!("work unit {} made no progress", f.key),
                _ => format!(
                    "work unit {} reported a plan issue: {}",
                    f.key, f.plan_issue
                ),
            };
            s.auto_replan = Some(Event::AutoReplanRequested {
                key: f.key,
                failure: f.failure,
                source: task_core::AutoReplanSource::WorkUnit,
            });
            s.trigger = Trigger::Continue {
                why: ContinueWhy::Replan,
            };
            s.outcome = format!("replan: {text}");
            WorkUnitFailureStep::Settled
        }
        // ADR-0079 付記「R6-1」D3: replan を使い切った後の WU の失敗で task を黙って `failed` にしない。
        ReplanOrAsk::Ask(_) if f.reason == "failed" => WorkUnitFailureStep::AskExhausted {
            why,
            used: limits.replans,
        },
        // ADR 2026-10-22-plan-issue-answer-replans D6: 同じ check での再開は同じ申告を繰り返し得るので、計画を
        // 直す答え方を質問に添える。
        ReplanOrAsk::Ask(_) if f.reason == "plan_issue" => {
            s.outcome.push_str(task_ops::gate::PLAN_ISSUE_REPLAN_HINT);
            WorkUnitFailureStep::Settled
        }
        ReplanOrAsk::Ask(_) => WorkUnitFailureStep::Settled,
    }
}

/// ADR-0079 D7（Phase R3a）: done にした leaf の worker が `needed_before: self` の決定の要求を出した。leaf を done に
/// せず `blocked(decision)` にし（それを待つ unit は ready にしない）、task が完了に進むなら `Continue{advance}` にする。
/// `count` は記録する決定の要求の数。
pub(crate) fn hold_for_worker_decisions(s: &mut WorkUnitSettlement, key: &str, count: usize) {
    if let Some(decision) = s.decision.as_mut() {
        decision.updated.status = WorkUnitStatus::Blocked;
        decision.updated.blocked_reason = Some(WorkUnitBlockedReason::Decision);
        decision.reason = "decision";
        decision.newly_blocked.clear();
        decision.newly_ready.clear();
    }
    if matches!(s.trigger, Trigger::WorkerDone) {
        s.trigger = Trigger::Continue {
            why: ContinueWhy::Advance,
        };
    }
    s.outcome = format!(
        "decision: WorkUnit {key} は人の決定を待ちます（決定の要求 {count} 件。ADR-0079 D7）"
    );
}

#[cfg(test)]
mod tests;
