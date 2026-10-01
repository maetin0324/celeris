//! ADR-0072 D6/D11/D12/D15（Phase E2）: WorkUnit の状態遷移の決定。
//!
//! 純粋関数（I/O は無い。ADR-0001 D2）。`dispatcher.rs` が「この run がどう終わったか」（[`RunEnd`]・
//! checkpoint・進捗）を渡すと、ここが「この WorkUnit と Task はどうなるか」（[`WuDecision`]）を返す。
//! 実際に store へ書き込む（`work_unit_transition` の呼び出し・トランザクション）のは `dispatcher.rs`
//! の責務のまま。

use task_core::execution::RunEnd;
use task_core::{
    Checkpoint, ContinueWhy, Trigger, WorkUnitBlockedReason, WorkUnitRow, WorkUnitStatus,
    checkpoint_shows_progress, dependents_to_block, newly_ready,
};

/// D18 の上限（呼び出し側が `[execution]` の設定または既定値から組み立てる）。
#[derive(Debug, Clone, Copy)]
pub struct WuLimits {
    pub max_continuations: u32,
    pub no_progress_limit: u32,
    pub max_retries: u32,
}

/// [`decide`] の結果。`dispatcher.rs` はこれを見て `work_unit_transition`（更新された WU の行 +
/// `WorkUnitTransitioned`）と、Task レベルの `Trigger` を適用する。
#[derive(Debug, Clone, PartialEq)]
pub struct WuDecision {
    /// この WU の新しい行（`status`・カウンタを更新済み）。
    pub updated: WorkUnitRow,
    /// `WorkUnitTransitioned.reason`（決定的な静的文字列）。
    pub reason: &'static str,
    /// Task レベルの trigger。
    pub trigger: Trigger,
    /// `WorkerFinished.outcome` に付け足す接頭辞・文言（D12 の `"work unit <key> failed: "` 等。
    /// 無ければ `None` で、呼び出し側は通常どおりの `outcome_str` を使う）。
    pub outcome_override: Option<String>,
    /// D15: 失敗の伝播で `blocked(dependency_failed)` にする、他の WU の新しい行。
    pub newly_blocked: Vec<WorkUnitRow>,
    /// D15: この WU が `done` になったことで `pending → ready` に上がる、他の WU の新しい行。
    pub newly_ready: Vec<WorkUnitRow>,
    /// 計画の中で、この決定の後に非終端（有効）の WU がもう無い（＝ Task を完了させてよい）。
    pub plan_complete: bool,
}

/// D9/D18: continuation（予算切れ・yield）の判定に要る、checkpoint 周りの入力をまとめたもの。
#[derive(Debug, Clone, Copy, Default)]
pub struct ContinuationInputs<'a> {
    /// D8 で合成した確定値（`end.is_continuable()` のときだけ `Some`）。
    pub checkpoint: Option<&'a Checkpoint>,
    /// この WU の直前の checkpoint（進捗判定用。無ければ `None`）。
    pub prev_checkpoint: Option<&'a Checkpoint>,
    pub no_progress_before: u32,
}

/// D6: この WU の run が `end` で終わったとき、WU と Task がどうなるかを決める。
///
/// - `wu` は「いま `running` の WU」の現在の行（呼び出し側が `status = Running` であることを保証する）。
/// - `all_units` はこの Task の全 WU（`wu` 自身を含む。伝播・完了判定に使う）。
/// - `continuation` は D8/D18 の checkpoint 周りの入力（[`ContinuationInputs`]）。
/// - `limits` は D18 の上限。
pub fn decide(
    end: RunEnd,
    run_id: &str,
    wu: &WorkUnitRow,
    all_units: &[WorkUnitRow],
    continuation_inputs: ContinuationInputs<'_>,
    limits: WuLimits,
) -> WuDecision {
    // ADR-0072 D17 3.（Phase E4b 項目2）: `end` の種類に関わらず、合成した checkpoint が
    // `plan_issue` を持っていれば最優先する（continuation の判定より前）。checkpoint はいまのところ
    // `end.is_continuable()`（Yielded/BudgetExhausted）のときだけ合成される（D8/E2b の制約）ので、
    // 実際にこの分岐に入るのはその 2 つの終わり方だけ。
    if continuation_inputs
        .checkpoint
        .is_some_and(|cp| cp.plan_issue.is_some())
    {
        return blocked(wu, WorkUnitBlockedReason::PlanIssue);
    }
    match end {
        RunEnd::Completed => complete(wu, run_id, all_units),
        RunEnd::Yielded | RunEnd::BudgetExhausted { .. } => continuation(
            wu,
            continuation_inputs.checkpoint,
            continuation_inputs.prev_checkpoint,
            continuation_inputs.no_progress_before,
            limits,
        ),
        RunEnd::Question => blocked(wu, WorkUnitBlockedReason::Question),
        RunEnd::Failed { retryable } => failed(wu, all_units, retryable, limits),
        // ADR-0072 D6 表: harness_error は「ready、checkpoint があれば needs_continuation」。
        // checkpoint はこの分類では合成していない（`end.is_continuable()` が false のため）ので、
        // E2 では単純に ready へ戻す（`retries` は数えない。既存の Requeue/InfraRequeue の
        // カウンタが別に効く）。
        RunEnd::HarnessError { .. } | RunEnd::Cancelled => reset_to_ready(wu),
        // ADR-0090 D2: クラスタ job の終了待ち。unit は `blocked(cluster_jobs)`（continuation・retry に数えない）で、
        // task は `advance`（v2 は兄弟を止めない。v1 は呼び出し側が `ClusterJobWait` に差し替える）。
        RunEnd::Waiting => cluster_jobs_wait(wu),
    }
}

fn cluster_jobs_wait(wu: &WorkUnitRow) -> WuDecision {
    let mut updated = now_wu(wu.clone(), WorkUnitStatus::Blocked);
    updated.blocked_reason = Some(WorkUnitBlockedReason::ClusterJobs);
    WuDecision {
        updated,
        reason: "cluster_jobs",
        trigger: Trigger::Continue {
            why: ContinueWhy::Advance,
        },
        outcome_override: None,
        newly_blocked: Vec::new(),
        newly_ready: Vec::new(),
        plan_complete: false,
    }
}

fn now_wu(mut wu: WorkUnitRow, status: WorkUnitStatus) -> WorkUnitRow {
    wu.status = status;
    // ADR-0074 D1.5（Phase F2）: `running` を離れたら WU の lease を外す（v1 では常に空のまま）。
    if status != WorkUnitStatus::Running {
        wu.clear_lease();
    }
    if status != WorkUnitStatus::Blocked {
        wu.blocked_reason = None;
    }
    wu
}

fn complete(wu: &WorkUnitRow, run_id: &str, all_units: &[WorkUnitRow]) -> WuDecision {
    let mut updated = now_wu(wu.clone(), WorkUnitStatus::Done);
    updated.last_run_id = Some(run_id.to_string());

    // D15: この WU が done になったので、`all_units` を仮に更新した上で依存の解決と完了判定を行う。
    let mut projected: Vec<WorkUnitRow> = all_units
        .iter()
        .map(|u| {
            if u.id == wu.id {
                updated.clone()
            } else {
                u.clone()
            }
        })
        .collect();
    let ready_ids = newly_ready(&projected);
    let mut ready_rows = Vec::new();
    for u in projected.iter_mut() {
        if ready_ids.contains(&u.id) {
            u.status = WorkUnitStatus::Ready;
            ready_rows.push(u.clone());
        }
    }

    // ADR-0079 付記「R7-9」D3: 統合済みの段階に統合されていない unit が残っていれば完了にしない（`Continue{advance}`。
    // 次の dispatch の gate がその段階の統合 WU を `pending` に戻して統合を走らせる）。
    let plan_complete = projected
        .iter()
        .filter(|u| u.status.is_active())
        .all(|u| u.status == WorkUnitStatus::Done)
        && task_core::stale_stage_integrations(&projected).is_empty();

    WuDecision {
        updated,
        reason: "completed",
        trigger: if plan_complete {
            Trigger::WorkerDone
        } else {
            Trigger::Continue {
                why: ContinueWhy::Advance,
            }
        },
        outcome_override: None,
        newly_blocked: Vec::new(),
        newly_ready: ready_rows,
        plan_complete,
    }
}

fn continuation(
    wu: &WorkUnitRow,
    checkpoint: Option<&Checkpoint>,
    prev_checkpoint: Option<&Checkpoint>,
    no_progress_before: u32,
    limits: WuLimits,
) -> WuDecision {
    let progressed = checkpoint_shows_progress(
        prev_checkpoint,
        checkpoint.expect(
            "continuation() is only called when RunEnd::is_continuable(); a checkpoint was merged",
        ),
    );
    let no_progress = if progressed {
        0
    } else {
        no_progress_before + 1
    };

    if wu.continuations >= limits.max_continuations || no_progress >= limits.no_progress_limit {
        let mut updated = now_wu(wu.clone(), WorkUnitStatus::Blocked);
        updated.blocked_reason = Some(WorkUnitBlockedReason::Limit);
        return WuDecision {
            updated,
            reason: "limit",
            trigger: Trigger::WorkerQuestion,
            outcome_override: Some(format!(
                "question: WorkUnit {} が進みません（continuation {} 回 / 進捗なし {} 回）。予算を増やして続ける／分割し直す（replan）／中止のいずれかを選んでください。",
                wu.key, wu.continuations, no_progress
            )),
            newly_blocked: Vec::new(),
            newly_ready: Vec::new(),
            plan_complete: false,
        };
    }

    let mut updated = now_wu(wu.clone(), WorkUnitStatus::NeedsContinuation);
    updated.continuations += 1;
    WuDecision {
        updated,
        reason: "continue",
        trigger: Trigger::Continue {
            why: ContinueWhy::Continue,
        },
        outcome_override: None,
        newly_blocked: Vec::new(),
        newly_ready: Vec::new(),
        plan_complete: false,
    }
}

fn blocked(wu: &WorkUnitRow, reason: WorkUnitBlockedReason) -> WuDecision {
    let mut updated = now_wu(wu.clone(), WorkUnitStatus::Blocked);
    updated.blocked_reason = Some(reason);
    WuDecision {
        updated,
        reason: reason.as_str(),
        trigger: Trigger::WorkerQuestion,
        outcome_override: None,
        newly_blocked: Vec::new(),
        newly_ready: Vec::new(),
        plan_complete: false,
    }
}

fn reset_to_ready(wu: &WorkUnitRow) -> WuDecision {
    let updated = now_wu(wu.clone(), WorkUnitStatus::Ready);
    WuDecision {
        updated,
        reason: "harness_error",
        // Task レベルの trigger は呼び出し側（dispatcher.rs）が Requeue/InfraRequeue/WorkerError の
        // 既存の分類のまま使う。ここでは WU の状態だけを決める（`trigger` はダミーで上書きされる）。
        trigger: Trigger::Requeue,
        outcome_override: None,
        newly_blocked: Vec::new(),
        newly_ready: Vec::new(),
        plan_complete: false,
    }
}

/// D12 の 3 / D11 の retry: WU が失敗したとき、retry の余地があれば `ready` に戻し（retries+1）、
/// 無ければ `failed` にして依存先を `blocked(dependency_failed)` にする（E2 に planner は無いので
/// replan できず、Task は `WorkerError{retryable:false}` で `failed` にする。D12）。
fn failed(
    wu: &WorkUnitRow,
    all_units: &[WorkUnitRow],
    retryable: bool,
    limits: WuLimits,
) -> WuDecision {
    if retryable && wu.retries < limits.max_retries {
        let mut updated = now_wu(wu.clone(), WorkUnitStatus::Ready);
        updated.retries += 1;
        return WuDecision {
            updated,
            reason: "retry",
            trigger: Trigger::Continue {
                why: ContinueWhy::WorkUnitRetry,
            },
            outcome_override: None,
            newly_blocked: Vec::new(),
            newly_ready: Vec::new(),
            plan_complete: false,
        };
    }

    let updated = now_wu(wu.clone(), WorkUnitStatus::Failed);
    let mut projected: Vec<WorkUnitRow> = all_units
        .iter()
        .map(|u| {
            if u.id == wu.id {
                updated.clone()
            } else {
                u.clone()
            }
        })
        .collect();
    let blocked_ids = dependents_to_block(&projected, &wu.key);
    let mut blocked_rows = Vec::new();
    for u in projected.iter_mut() {
        if blocked_ids.contains(&u.id) {
            u.status = WorkUnitStatus::Blocked;
            u.blocked_reason = Some(WorkUnitBlockedReason::DependencyFailed);
            blocked_rows.push(u.clone());
        }
    }
    WuDecision {
        updated,
        reason: "failed",
        trigger: Trigger::WorkerError { retryable: false },
        outcome_override: Some(format!("work unit {} failed", wu.key)),
        newly_blocked: blocked_rows,
        newly_ready: Vec::new(),
        plan_complete: false,
    }
}

/// ADR-0074 D1.6（Phase F2b）: v2 の WU の run が終わって WU の行を書いた後（`units` はその後の
/// 全行）、Task をどうするか。純粋関数（`dispatcher.rs` がこれを見て遷移・統合を行う）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseSettle {
    /// 同じ Task の他の WU の run（または統合）が走っている。Task は遷移しない（兄弟は止めない）。
    Wait,
    /// 現在の工程に起こせる WU がある（走っているものは無い）。`Continue{advance}`（今と同じ）。
    Advance,
    /// 現在の工程の WU がすべて done。この統合 WU（id）を走らせる（Task は遷移しない）。
    Integrate(String),
    /// 現在の工程に `blocked(question)` の WU（id）がある。`WorkerQuestion`。
    Question(String),
    /// 現在の工程に failed / `blocked(limit|plan_issue|dependency_failed)` の WU（id）がある。
    /// replan できれば `Continue{replan}`、できなければ D12。
    Failure(String),
    /// 有効な WU がすべて done（最後の工程の統合まで済んだ）。`WorkerDone`。
    AllDone,
}

/// ADR-0074 D1.6: [`PhaseSettle`] を決める。
///
/// - 走っている WU（`running`。統合 WU を含む）があれば `Wait`（in-flight が 0 になってから決める）。
/// - 現在の工程（有効で終端でない行のうち `seq` 最小の行の工程。`runnable_work_units` と同じ）で、
///   question → 失敗（failed を先に、次に limit / plan_issue / dependency_failed）→ 起こせる WU →
///   統合の順に見る。人の入力（question）を先にするのは、replan で質問を捨てないため。
pub fn settle_phase(units: &[WorkUnitRow]) -> PhaseSettle {
    let active: Vec<&WorkUnitRow> = units.iter().filter(|u| u.status.is_active()).collect();
    // ADR-0079 D5（Phase R1b）: kind task の unit の `running` は子 task が走っていることの写しで、この
    // Task の run ではない（親は lease を持たずに待つ）。in-flight には数えない。
    if active
        .iter()
        .any(|u| u.status == WorkUnitStatus::Running && u.kind != task_core::WorkUnitKind::Task)
    {
        return PhaseSettle::Wait;
    }
    let in_play: Vec<&WorkUnitRow> = active
        .iter()
        .copied()
        .filter(|u| !u.status.is_terminal())
        .collect();
    let Some(current) = in_play.iter().min_by_key(|u| u.seq) else {
        // ADR-0079 付記「R7-9」D3: 統合済みの段階に統合 WU の依存に無い unit（統合の後に足された unit）が残って
        // いれば完了ではない。gate（`wu_dispatch_gate`）がその統合 WU を開き直すので、通常の経路に戻す。
        if !task_core::stale_stage_integrations(units).is_empty() {
            return PhaseSettle::Advance;
        }
        return PhaseSettle::AllDone;
    };
    let phase = current.phase.clone();
    let mut in_phase: Vec<&WorkUnitRow> = in_play
        .iter()
        .copied()
        .filter(|u| u.phase == phase)
        .collect();
    in_phase.sort_by_key(|u| u.seq);
    if let Some(q) = in_phase.iter().find(|u| {
        u.status == WorkUnitStatus::Blocked
            && u.blocked_reason == Some(WorkUnitBlockedReason::Question)
    }) {
        return PhaseSettle::Question(q.id.clone());
    }
    if let Some(f) = in_phase
        .iter()
        .find(|u| u.status == WorkUnitStatus::Failed)
        .or_else(|| {
            in_phase.iter().find(|u| {
                u.status == WorkUnitStatus::Blocked
                    && matches!(
                        u.blocked_reason,
                        Some(WorkUnitBlockedReason::Limit)
                            | Some(WorkUnitBlockedReason::PlanIssue)
                            | Some(WorkUnitBlockedReason::DependencyFailed)
                    )
            })
        })
    {
        return PhaseSettle::Failure(f.id.clone());
    }
    if in_phase.iter().any(|u| {
        u.kind != task_core::WorkUnitKind::Integrate
            && matches!(
                u.status,
                WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
            )
    }) {
        return PhaseSettle::Advance;
    }
    let phase_done = active
        .iter()
        .filter(|u| u.phase == phase && u.kind != task_core::WorkUnitKind::Integrate)
        .all(|u| u.status == WorkUnitStatus::Done);
    if phase_done
        && let Some(integ) = in_phase.iter().find(|u| {
            u.kind == task_core::WorkUnitKind::Integrate
                && matches!(u.status, WorkUnitStatus::Pending | WorkUnitStatus::Ready)
        })
    {
        return PhaseSettle::Integrate(integ.id.clone());
    }
    // ここに来るのは「pending だけが残り、依存も統合も進められない」一瞬の不整合だけ（通常の
    // 経路に戻して gate に任せる）。
    PhaseSettle::Advance
}

/// D18: 人の回答（`Trigger::Answer`）で `blocked` の WU を再開する（窓は 0 に戻す。D18「回答の時点
/// から数え直す」）。`blocked_reason = Limit` なら `needs_continuation`（continuation の続き）、
/// `Question` なら `ready`（最初から）に戻す。`DependencyFailed` は人の回答では戻らない（依存先の
/// 計画が直らない限り解消しない。E2 には該当しない: E2 の失敗した Task は即 `failed` になる）。
pub fn resume_after_answer(wu: &WorkUnitRow) -> WorkUnitRow {
    match wu.blocked_reason {
        Some(WorkUnitBlockedReason::Limit) => now_wu(wu.clone(), WorkUnitStatus::NeedsContinuation),
        _ => now_wu(wu.clone(), WorkUnitStatus::Ready),
    }
}

#[cfg(test)]
mod tests;
