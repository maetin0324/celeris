//! ADR-0069 D6（Phase 114）: やり直し（retry）と lane のエスカレーションの決定的な policy。
//!
//! 純粋関数だけを置く（I/O・LLM 呼び出しはしない）。タスクを最終的に `failed` にするのは従来どおり
//! 状態機械（`transition` の `max_retries`）で、ここは「次の試行をどの lane で走らせるか」だけを決める。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Check, Event, Task, Tier, TierSource};
use crate::model_policy::{LaneCeiling, lane_rank, lane_up};

/// 1 回の試行の終わり方（イベントから決定的に分類する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// reviewer / human の条件で不合格。
    ReviewFailed,
    /// command / artifact / knowledge_page の決定的な検査で不合格。
    VerificationFailed,
    /// 旧予約値。worker の自己申告だけでは品質証拠にしない。
    LowQuality,
    /// ワーカーの失敗（予算切れ以外）。
    WorkerError,
    /// 供給側の失敗（レート制限・認証・枯渇・起動失敗。`requeue`）。試行に数えない。
    SupplySide,
    /// 予算切れ（wall-clock / max_turns / budget）。
    BudgetExhausted,
}

impl AttemptOutcome {
    /// 試行回数に数えるか（供給側失敗は attempts を消費しない。ADR-0010 D1）。
    pub fn counts(self) -> bool {
        !matches!(self, AttemptOutcome::SupplySide)
    }
}

/// 履歴の 1 件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttemptRecord {
    /// その試行で走った lane（`Event::RoutingDecided` が無い導入前の試行は `None`）。
    pub lane: Option<Tier>,
    pub outcome: AttemptOutcome,
}

/// 予算の見通し（budget guard）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BudgetState {
    #[default]
    Ok,
    /// 残量の層（`select_tier`）が実行を見送る状態。
    Defer,
    /// 予算を使い切った。
    Exhausted,
}

/// `EscalationPolicy::decide` の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RetryDecision {
    /// 同じ lane でやり直す（初回もこれ）。
    Retry { lane: Tier, reason: String },
    /// 1 段上げてやり直す。
    Escalate {
        from: Tier,
        to: Tier,
        reason: String,
    },
    /// これ以上上げない（`lane` は直前のまま）。タスクを失敗させるのは状態機械。
    Stop { lane: Tier, reason: String },
}

impl RetryDecision {
    pub fn lane(&self) -> Tier {
        match self {
            RetryDecision::Retry { lane, .. } | RetryDecision::Stop { lane, .. } => *lane,
            RetryDecision::Escalate { to, .. } => *to,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            RetryDecision::Retry { reason, .. }
            | RetryDecision::Escalate { reason, .. }
            | RetryDecision::Stop { reason, .. } => reason,
        }
    }

    /// 監査記録に残す 1 行。
    pub fn describe(&self) -> String {
        match self {
            RetryDecision::Retry { lane, reason } => format!("retry at {lane:?}: {reason}"),
            RetryDecision::Escalate { from, to, reason } => {
                format!("escalate {from:?} -> {to:?}: {reason}")
            }
            RetryDecision::Stop { lane, reason } => {
                format!("stop escalation at {lane:?}: {reason}")
            }
        }
    }
}

/// 既定: 同じ lane での（エスカレーション対象の）失敗がこの回数に達したら 1 段上げる。
pub const DEFAULT_MAX_ATTEMPTS_PER_LANE: u32 = 2;
/// 既定: 全体の試行回数の上限。
pub const DEFAULT_MAX_TOTAL_ATTEMPTS: u32 = 4;

/// Phase 3 の軌跡 escalation に渡す設定値。未指定の上限は制限しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EscalationThresholds {
    pub quality_failures_per_lane: u32,
    pub max_total_attempts: u32,
    pub org_ceiling: LaneCeiling,
    pub task_ceiling: LaneCeiling,
    pub work_unit_ceiling: LaneCeiling,
}

impl Default for EscalationThresholds {
    fn default() -> Self {
        Self {
            quality_failures_per_lane: DEFAULT_MAX_ATTEMPTS_PER_LANE,
            max_total_attempts: DEFAULT_MAX_TOTAL_ATTEMPTS,
            org_ceiling: LaneCeiling::default(),
            task_ceiling: LaneCeiling::default(),
            work_unit_ceiling: LaneCeiling::default(),
        }
    }
}

/// Run 間の lane 決定。selected_lane を次の optimizer の floor として使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EscalationAudit {
    pub requested_lane: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_lane: Option<Tier>,
    pub selected_lane: Tier,
    pub reason: String,
    pub counted_failures: u32,
    pub interval_id: String,
}

/// ADR-0069 D6: fallback / retry / escalation policy。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationPolicy {
    pub max_attempts_per_lane: u32,
    pub max_total_attempts: u32,
    pub escalate_on: Vec<AttemptOutcome>,
    pub never_escalate_on: Vec<AttemptOutcome>,
    /// 天井（組織の `allowed_tiers` / `budget.max_lane`）。
    pub ceiling: LaneCeiling,
}

impl Default for EscalationPolicy {
    fn default() -> Self {
        Self {
            max_attempts_per_lane: DEFAULT_MAX_ATTEMPTS_PER_LANE,
            max_total_attempts: DEFAULT_MAX_TOTAL_ATTEMPTS,
            escalate_on: vec![
                AttemptOutcome::ReviewFailed,
                AttemptOutcome::VerificationFailed,
            ],
            never_escalate_on: vec![AttemptOutcome::SupplySide, AttemptOutcome::BudgetExhausted],
            ceiling: LaneCeiling::default(),
        }
    }
}

impl EscalationPolicy {
    /// event から作った現在の区間だけを評価する。明示 lane と各 ceiling は必ず優先する。
    /// `interval_id` は `attempt_history_with_interval` の値を渡す。
    pub fn decide_trajectory(
        &self,
        history: &[AttemptRecord],
        requested_lane: Tier,
        source: TierSource,
        budget: BudgetState,
        thresholds: &EscalationThresholds,
        interval_id: &str,
    ) -> EscalationAudit {
        let previous_lane = history.iter().rev().find_map(|r| r.lane);
        let current = previous_lane
            .filter(|lane| lane_rank(*lane) >= lane_rank(requested_lane))
            .unwrap_or(requested_lane);
        let counted_failures = history
            .iter()
            .rev()
            .take_while(|r| {
                r.lane.unwrap_or(requested_lane) == current
                    && (self.escalate_on.contains(&r.outcome)
                        || r.outcome == AttemptOutcome::SupplySide)
            })
            .filter(|r| self.escalate_on.contains(&r.outcome))
            .count() as u32;
        let (selected_lane, reason) = if !source.policy_decides() {
            (requested_lane, "explicit lane; escalation disabled".into())
        } else {
            let mut policy = self.clone();
            policy.max_attempts_per_lane = thresholds.quality_failures_per_lane.max(1);
            policy.max_total_attempts = policy
                .max_total_attempts
                .min(thresholds.max_total_attempts.max(1));
            // `decide` keeps the existing retry/stop semantics. Enforce all three
            // independent ceilings on the candidate before accepting an escalation.
            let decision = policy.decide(history, requested_lane, budget);
            let selected = decision.lane();
            let ceilings = [
                &policy.ceiling,
                &thresholds.org_ceiling,
                &thresholds.task_ceiling,
                &thresholds.work_unit_ceiling,
            ];
            if ceilings.iter().all(|ceiling| ceiling.permits(selected)) {
                (selected, decision.reason().to_owned())
            } else {
                let permitted = crate::model_policy::LANES
                    .iter()
                    .rev()
                    .copied()
                    .find(|lane| {
                        lane_rank(*lane) <= lane_rank(selected)
                            && ceilings.iter().all(|ceiling| ceiling.permits(*lane))
                    })
                    .unwrap_or(requested_lane);
                (
                    permitted,
                    "org, task, or work unit ceiling limits lane".into(),
                )
            }
        };
        EscalationAudit {
            requested_lane,
            previous_lane,
            selected_lane,
            reason,
            counted_failures,
            interval_id: interval_id.to_owned(),
        }
    }

    /// タスクと実効 profile から作る。`max_total_attempts` はタスクの `max_retries + 1`
    /// （状態機械が許す試行回数）と profile の `max_attempts` を超えない。profile の
    /// `review.escalate_on_fail = false` ならレビュー不合格では上げない。
    pub fn for_task(task: &Task, profile: Option<&crate::profile::EffectiveProfile>) -> Self {
        Self::for_task_with_thresholds(task, profile, &EscalationThresholds::default())
    }

    /// 設定値を先に反映し、task/profile の試行上限で丸める。
    pub fn for_task_with_thresholds(
        task: &Task,
        profile: Option<&crate::profile::EffectiveProfile>,
        thresholds: &EscalationThresholds,
    ) -> Self {
        let mut policy = EscalationPolicy {
            max_attempts_per_lane: thresholds.quality_failures_per_lane.max(1),
            max_total_attempts: thresholds.max_total_attempts.max(1),
            ..EscalationPolicy::default()
        };
        let task_limit = task.budget.max_retries.saturating_add(1);
        policy.max_total_attempts = policy.max_total_attempts.min(task_limit);
        if let Some(p) = profile {
            if let Some(max) = p.max_attempts {
                policy.max_total_attempts = policy.max_total_attempts.min(max.max(1));
            }
            if p.review_escalate_on_fail == Some(false) {
                policy.escalate_on.retain(|o| {
                    !matches!(o, AttemptOutcome::ReviewFailed | AttemptOutcome::LowQuality)
                });
            }
            policy.ceiling = p.lane_ceiling();
        }
        policy
    }

    /// 次の試行の lane を決める。`base` は policy（または人の明示）が出した lane。
    pub fn decide(
        &self,
        history: &[AttemptRecord],
        base: Tier,
        budget: BudgetState,
    ) -> RetryDecision {
        let counted: Vec<&AttemptRecord> = history.iter().filter(|r| r.outcome.counts()).collect();
        let Some(last) = history.last() else {
            return RetryDecision::Retry {
                lane: base,
                reason: "first attempt".into(),
            };
        };
        // 直前に走った lane（エスカレーション済みならそれを保つ。base より下には戻さない）。
        let current = history
            .iter()
            .rev()
            .find_map(|r| r.lane)
            .filter(|l| lane_rank(*l) >= lane_rank(base))
            .unwrap_or(base);
        if counted.len() as u32 >= self.max_total_attempts {
            return RetryDecision::Stop {
                lane: current,
                reason: format!(
                    "max_total_attempts {} reached (attempts so far {})",
                    self.max_total_attempts,
                    counted.len()
                ),
            };
        }
        if budget == BudgetState::Exhausted || last.outcome == AttemptOutcome::BudgetExhausted {
            return RetryDecision::Stop {
                lane: current,
                reason: "budget exhausted; never escalate".into(),
            };
        }
        if self.never_escalate_on.contains(&last.outcome) {
            return RetryDecision::Retry {
                lane: current,
                reason: format!("{:?} is never escalated", last.outcome),
            };
        }
        if !self.escalate_on.contains(&last.outcome) {
            return RetryDecision::Retry {
                lane: current,
                reason: format!("{:?} does not trigger escalation", last.outcome),
            };
        }
        // 今の lane で続いたエスカレーション対象の失敗の数（末尾から数える）。
        let failures_here = counted
            .iter()
            .rev()
            .take_while(|r| {
                r.lane.unwrap_or(base) == current && self.escalate_on.contains(&r.outcome)
            })
            .count() as u32;
        if failures_here < self.max_attempts_per_lane {
            return RetryDecision::Retry {
                lane: current,
                reason: format!(
                    "{:?} at {current:?} ({failures_here}/{} before escalation)",
                    last.outcome, self.max_attempts_per_lane
                ),
            };
        }
        let Some(next) = lane_up(current) else {
            return RetryDecision::Retry {
                lane: current,
                reason: "already at the top lane".into(),
            };
        };
        if !self.ceiling.permits(next) {
            return RetryDecision::Retry {
                lane: current,
                reason: format!("escalation to {next:?} is above the org ceiling"),
            };
        }
        if budget == BudgetState::Defer {
            return RetryDecision::Stop {
                lane: current,
                reason: "budget guard: quota defers; not escalating".into(),
            };
        }
        RetryDecision::Escalate {
            from: current,
            to: next,
            reason: format!(
                "{failures_here} consecutive {:?} at {current:?}",
                last.outcome
            ),
        }
    }
}

/// `WorkerFinished.outcome` が予算切れか（wall-clock / max_turns / budget / context 超過の文言。
/// 決定的な字句判定）。ADR-0072 D7: 構造化した `RunEnd` を返さない古い経路・アダプタのための
/// 「二重の安全網」として `pub` にした（`task-dispatch` がここを呼ぶ）。
pub fn is_budget_outcome(outcome: &str) -> bool {
    let o = outcome.to_lowercase();
    [
        "wall-clock",
        "wall clock",
        "max_turns",
        "max turns",
        "turn limit",
        "budget",
    ]
    .iter()
    .any(|w| o.contains(w))
        || crate::execution::looks_like_context_exceeded(&o)
}

/// ADR-0069 D6: イベント列（古い順）から試行の履歴を作る。`reopen` で履歴はリセットする。
pub fn attempt_history(task: &Task, events: &[Event]) -> Vec<AttemptRecord> {
    attempt_history_with_interval(task, events).0
}

/// `reopen` ごとに履歴を区切り、event 列内の位置から安定した区間 ID を返す。
pub fn attempt_history_with_interval(
    task: &Task,
    events: &[Event],
) -> (Vec<AttemptRecord>, String) {
    let mut out: Vec<AttemptRecord> = Vec::new();
    let mut interval_id = "initial".to_owned();
    let mut lane: Option<Tier> = None;
    let mut last_finish: Option<String> = None;
    let mut failed_checks: Vec<usize> = Vec::new();
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::RoutingDecided { record, .. } => {
                lane = Some(record.resolution.lane.unwrap_or(record.decision.lane));
                failed_checks.clear();
            }
            Event::WorkerFinished {
                outcome,
                role: None,
                ..
            } => {
                last_finish = Some(outcome.clone());
            }
            Event::ReviewVerdict {
                criterion_idx,
                pass: false,
                ..
            } => failed_checks.push(*criterion_idx),
            Event::Transitioned { reason, .. } => {
                let outcome = match reason.as_str() {
                    "review_fail" => {
                        let only_verification = !failed_checks.is_empty()
                            && failed_checks.iter().all(|i| {
                                task.acceptance.get(*i).is_some_and(|c| {
                                    matches!(
                                        c.check,
                                        Check::Command { .. }
                                            | Check::ArtifactExists { .. }
                                            | Check::KnowledgePage { .. }
                                    )
                                })
                            });
                        Some(if only_verification {
                            AttemptOutcome::VerificationFailed
                        } else {
                            AttemptOutcome::ReviewFailed
                        })
                    }
                    "worker_error" | "lease_expired" => {
                        Some(if last_finish.as_deref().is_some_and(is_budget_outcome) {
                            AttemptOutcome::BudgetExhausted
                        } else {
                            AttemptOutcome::WorkerError
                        })
                    }
                    "requeue" => Some(AttemptOutcome::SupplySide),
                    "reopen" => {
                        out.clear();
                        lane = None;
                        last_finish = None;
                        failed_checks.clear();
                        interval_id = format!("reopen:{index}");
                        None
                    }
                    _ => None,
                };
                if let Some(outcome) = outcome {
                    out.push(AttemptRecord { lane, outcome });
                    failed_checks.clear();
                    last_finish = None;
                }
            }
            _ => {}
        }
    }
    (out, interval_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use AttemptOutcome::*;

    fn rec(lane: Tier, outcome: AttemptOutcome) -> AttemptRecord {
        AttemptRecord {
            lane: Some(lane),
            outcome,
        }
    }

    #[test]
    fn escalates_one_step_after_repeated_review_failures_and_is_bounded() {
        let p = EscalationPolicy::default();
        assert_eq!(
            p.decide(&[], Tier::Cheap, BudgetState::Ok).lane(),
            Tier::Cheap
        );
        let h1 = [rec(Tier::Cheap, ReviewFailed)];
        assert!(matches!(
            p.decide(&h1, Tier::Cheap, BudgetState::Ok),
            RetryDecision::Retry {
                lane: Tier::Cheap,
                ..
            }
        ));
        let h2 = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, VerificationFailed),
        ];
        assert_eq!(
            p.decide(&h2, Tier::Cheap, BudgetState::Ok),
            RetryDecision::Escalate {
                from: Tier::Cheap,
                to: Tier::Standard,
                reason: "2 consecutive VerificationFailed at Cheap".into()
            }
        );
        // 上げた後はその lane を保つ（base に戻らない）
        let h3 = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Standard, ReviewFailed),
        ];
        assert_eq!(
            p.decide(&h3, Tier::Cheap, BudgetState::Ok).lane(),
            Tier::Standard
        );
        // 全体の上限で止まる
        let h4 = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Standard, ReviewFailed),
            rec(Tier::Standard, ReviewFailed),
        ];
        assert!(matches!(
            p.decide(&h4, Tier::Cheap, BudgetState::Ok),
            RetryDecision::Stop {
                lane: Tier::Standard,
                ..
            }
        ));
        // 1 回に 1 段だけ（cheap から frontier へ飛ばない）
        for d in [
            p.decide(&h2, Tier::Cheap, BudgetState::Ok),
            p.decide(&h3, Tier::Cheap, BudgetState::Ok),
        ] {
            if let RetryDecision::Escalate { from, to, .. } = d {
                assert_eq!(lane_rank(to), lane_rank(from) + 1);
            }
        }
    }

    #[test]
    fn never_escalates_above_the_ceiling_or_on_supply_side_failures() {
        let p = EscalationPolicy {
            ceiling: LaneCeiling {
                allowed: vec![],
                max_lane: Some(Tier::Standard),
            },
            ..EscalationPolicy::default()
        };
        let h = [
            rec(Tier::Standard, ReviewFailed),
            rec(Tier::Standard, ReviewFailed),
        ];
        let d = p.decide(&h, Tier::Standard, BudgetState::Ok);
        assert_eq!(d.lane(), Tier::Standard);
        assert!(d.reason().contains("ceiling"), "{d:?}");

        let p = EscalationPolicy::default();
        let h = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, SupplySide),
        ];
        let d = p.decide(&h, Tier::Cheap, BudgetState::Ok);
        assert!(
            matches!(
                d,
                RetryDecision::Retry {
                    lane: Tier::Cheap,
                    ..
                }
            ),
            "{d:?}"
        );
        // 供給側失敗は試行に数えない
        let many = vec![rec(Tier::Cheap, SupplySide); 10];
        assert!(matches!(
            p.decide(&many, Tier::Cheap, BudgetState::Ok),
            RetryDecision::Retry { .. }
        ));
        // ワーカーの失敗では上げない
        let h = [rec(Tier::Cheap, WorkerError), rec(Tier::Cheap, WorkerError)];
        assert_eq!(
            p.decide(&h, Tier::Cheap, BudgetState::Ok).lane(),
            Tier::Cheap
        );
    }

    #[test]
    fn budget_guard_stops_escalation() {
        let p = EscalationPolicy::default();
        let h = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, ReviewFailed),
        ];
        assert!(matches!(
            p.decide(&h, Tier::Cheap, BudgetState::Defer),
            RetryDecision::Stop {
                lane: Tier::Cheap,
                ..
            }
        ));
        assert!(matches!(
            p.decide(&h, Tier::Cheap, BudgetState::Exhausted),
            RetryDecision::Stop { .. }
        ));
        let h = [rec(Tier::Cheap, BudgetExhausted)];
        let d = p.decide(&h, Tier::Cheap, BudgetState::Ok);
        assert!(matches!(d, RetryDecision::Stop { .. }), "{d:?}");
    }

    #[test]
    fn for_task_never_exceeds_max_retries_and_honours_the_profile() {
        let mut t = crate::model_policy::tests::task("x", vec![]);
        t.budget.max_retries = 1;
        assert_eq!(EscalationPolicy::for_task(&t, None).max_total_attempts, 2);
        t.budget.max_retries = 9;
        assert_eq!(
            EscalationPolicy::for_task(&t, None).max_total_attempts,
            DEFAULT_MAX_TOTAL_ATTEMPTS
        );
        let profile = crate::profile::EffectiveProfile {
            max_attempts: Some(3),
            review_escalate_on_fail: Some(false),
            max_lane: Some(Tier::Standard),
            ..Default::default()
        };
        let p = EscalationPolicy::for_task(&t, Some(&profile));
        assert_eq!(p.max_total_attempts, 3);
        assert!(!p.escalate_on.contains(&ReviewFailed));
        assert!(p.escalate_on.contains(&VerificationFailed));
        assert_eq!(p.ceiling.max_lane, Some(Tier::Standard));
        let thresholds = EscalationThresholds {
            max_total_attempts: 6,
            ..EscalationThresholds::default()
        };
        assert_eq!(
            EscalationPolicy::for_task_with_thresholds(&t, None, &thresholds).max_total_attempts,
            6
        );
    }

    #[test]
    fn history_is_derived_from_events() {
        use crate::model::{Criterion, Status};
        let mut t = crate::model_policy::tests::task("x", vec![]);
        t.acceptance = vec![
            Criterion {
                text: "tests".into(),
                check: Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            },
            Criterion {
                text: "judge".into(),
                check: Check::Reviewer,
            },
        ];
        let tr = |reason: &str| Event::Transitioned {
            from: Status::Reviewing,
            to: Status::Ready,
            reason: reason.into(),
        };
        let verdict = |idx: usize| Event::ReviewVerdict {
            run_id: "r".into(),
            criterion_idx: idx,
            pass: false,
            reason: "no".into(),
        };
        let finished = |outcome: &str| Event::WorkerFinished {
            run_id: "w".into(),
            outcome: outcome.into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        };
        let events = vec![
            verdict(0),
            tr("review_fail"),
            verdict(1),
            tr("review_fail"),
            tr("requeue"),
            finished("error(retryable=true): wall-clock budget exceeded"),
            tr("worker_error"),
            finished("error(retryable=true): crashed"),
            tr("worker_error"),
        ];
        let h = attempt_history(&t, &events);
        let outcomes: Vec<AttemptOutcome> = h.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                VerificationFailed,
                ReviewFailed,
                SupplySide,
                BudgetExhausted,
                WorkerError
            ]
        );
        assert!(h.iter().all(|r| r.lane.is_none()));
        let mut with_reopen = events.clone();
        with_reopen.push(tr("reopen"));
        assert!(attempt_history(&t, &with_reopen).is_empty());
    }

    #[test]
    fn routing_trajectory_escalates_one_lane_and_respects_caps() {
        use crate::model::Status;

        let policy = EscalationPolicy::default();
        let thresholds = EscalationThresholds::default();
        let failures = [
            rec(Tier::Cheap, ReviewFailed),
            rec(Tier::Cheap, VerificationFailed),
        ];
        let audit = policy.decide_trajectory(
            &failures,
            Tier::Cheap,
            TierSource::Default,
            BudgetState::Ok,
            &thresholds,
            "initial",
        );
        assert_eq!(audit.requested_lane, Tier::Cheap);
        assert_eq!(audit.previous_lane, Some(Tier::Cheap));
        assert_eq!(audit.selected_lane, Tier::Standard);
        assert_eq!(audit.counted_failures, 2);
        assert_eq!(audit.interval_id, "initial");

        for source in [TierSource::Human, TierSource::System] {
            let explicit = policy.decide_trajectory(
                &failures,
                Tier::Cheap,
                source,
                BudgetState::Ok,
                &thresholds,
                "initial",
            );
            assert_eq!(explicit.selected_lane, Tier::Cheap);
        }
        for cap in 0..3 {
            let mut limited = thresholds.clone();
            let ceiling = LaneCeiling {
                max_lane: Some(Tier::Cheap),
                ..LaneCeiling::default()
            };
            match cap {
                0 => limited.org_ceiling = ceiling,
                1 => limited.task_ceiling = ceiling,
                _ => limited.work_unit_ceiling = ceiling,
            }
            assert_eq!(
                policy
                    .decide_trajectory(
                        &failures,
                        Tier::Cheap,
                        TierSource::Default,
                        BudgetState::Ok,
                        &limited,
                        "initial",
                    )
                    .selected_lane,
                Tier::Cheap
            );
        }
        let mut tuned = thresholds.clone();
        tuned.quality_failures_per_lane = 3;
        assert_eq!(
            policy
                .decide_trajectory(
                    &failures,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &tuned,
                    "initial",
                )
                .selected_lane,
            Tier::Cheap
        );
        let supply = [failures[0], failures[1], rec(Tier::Cheap, SupplySide)];
        assert_eq!(
            policy
                .decide_trajectory(
                    &supply,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &thresholds,
                    "initial",
                )
                .selected_lane,
            Tier::Cheap
        );
        for outcome in [WorkerError, BudgetExhausted, LowQuality] {
            let history = [rec(Tier::Cheap, outcome), rec(Tier::Cheap, outcome)];
            assert_eq!(
                policy
                    .decide_trajectory(
                        &history,
                        Tier::Cheap,
                        TierSource::Default,
                        BudgetState::Ok,
                        &thresholds,
                        "initial",
                    )
                    .selected_lane,
                Tier::Cheap
            );
        }
        let second_lane = [failures[0], failures[1], rec(Tier::Standard, ReviewFailed)];
        assert_eq!(
            policy
                .decide_trajectory(
                    &second_lane,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &thresholds,
                    "initial",
                )
                .selected_lane,
            Tier::Standard
        );

        let task = crate::model_policy::tests::task("reopen", vec![]);
        let reopen = Event::Transitioned {
            from: Status::Done,
            to: Status::Ready,
            reason: "reopen".into(),
        };
        let review_fail = Event::Transitioned {
            from: Status::Reviewing,
            to: Status::Ready,
            reason: "review_fail".into(),
        };
        let events = [
            review_fail.clone(),
            review_fail.clone(),
            reopen,
            review_fail,
        ];
        let (history, interval_id) = attempt_history_with_interval(&task, &events);
        assert_eq!(interval_id, "reopen:2");
        assert_eq!(history.len(), 1);
        assert_eq!(
            policy
                .decide_trajectory(
                    &history,
                    Tier::Cheap,
                    TierSource::Default,
                    BudgetState::Ok,
                    &thresholds,
                    &interval_id,
                )
                .counted_failures,
            1
        );
    }
}
