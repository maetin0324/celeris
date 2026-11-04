//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-3・S5-6・S5-8: dispatcher の canary・ts。
//!
//! - 単位（`lane_rules`・`route_priority`・`execution_gate` は task id、`cos_digest` は pass id）が TS の側に選ばれたか（mode `ts` は
//!   全部、`canary` は [`task_core::script_compare::canary_selected`]）で分ける。Rust の側は compare と同じく TS を
//!   影で走らせて記録する。
//! - TS の側は TS を同期で待ち（1 回 `SCRIPT_CALL_DEADLINE`、1 tick の合計 [`SCRIPT_TICK_WAIT_BUDGET`]）、Rust の検証
//!   （出力の形・上限・天井・宣言済みの候補）を通った結果だけを使う。TS の失敗・検証落ち・安全違反・認可の拒否
//!   （`denied`）・予算切れ・呼べない generation では Rust の参照実装の結果を使い、記録の `fallback` に code を残す。
//!   どの場合も Rust の参照の出力と比べた記録を `script_compare_append_observed` で残す（計数も同じ transaction）。
//! - 戻しの書き込み（S5-6 の `script_effect_unverified`）に失敗した領域は process の中で `rust` と扱い、
//!   tick ごとに書き直す（fail-closed）。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use task_core::llm_route::{CandidateKey, ModelId, SourceId};
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, CosDigestFallback, RoutePriorityOutput,
    ScriptCompareRecord, ScriptDomain, ScriptMode,
};
use task_core::{TaskId, Tier};

use super::super::route_select::{RouteInput, RouteTrace};
use super::super::*;
use super::CompareTarget;
use crate::script_policy::{
    CompareDraft, LaneRulesOutput, PolicyScript, SCRIPT_CALL_DEADLINE, ScriptDomainStore, TsResult,
};

/// 付記 S5-8: 1 tick で TS を同期で待つ時間の合計の上限（`tick_ms` 既定 2000 ms の 10%）。
pub const SCRIPT_TICK_WAIT_BUDGET: Duration = Duration::from_millis(200);

/// 付記 S5-8: 集計に残す tick の数。
pub const SCRIPT_TICK_WAIT_WINDOW: usize = 1000;

/// 付記 S5-8: 1 tick の TS の待ちの予算と、直近 [`SCRIPT_TICK_WAIT_WINDOW`] tick の集計。時計は持たない
/// （待った時間は呼び出し側が測って [`Self::charge`] に渡す。試験は値を差し替えて決定的に確かめる）。
#[derive(Debug, Default, Clone)]
pub struct ScriptTickWait {
    used: Duration,
    budget_hits: u64,
    history: VecDeque<(u64, u64)>,
}

/// [`ScriptTickWait::summary`] の結果（S6 の overhead の報告と daemon の `script_tick_wait` の log 行が使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptTickWaitSummary {
    pub ticks: usize,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
    /// 窓の中の `tick_budget` の件数。
    pub tick_budget: u64,
}

impl ScriptTickWait {
    /// この tick の残りの予算。
    pub fn remaining(&self) -> Duration {
        SCRIPT_TICK_WAIT_BUDGET.saturating_sub(self.used)
    }

    /// TS を待った時間を足す。
    pub fn charge(&mut self, waited: Duration) {
        self.used = self.used.saturating_add(waited);
    }

    /// 予算切れで TS を呼ばなかった件数を 1 足す。
    pub fn budget_hit(&mut self) {
        self.budget_hits += 1;
    }

    /// tick の終わり: この tick の待ちを集計に入れ、予算を戻す。
    pub fn end_tick(&mut self) {
        let wait_us = u64::try_from(self.used.as_micros()).unwrap_or(u64::MAX);
        if self.history.len() == SCRIPT_TICK_WAIT_WINDOW {
            self.history.pop_front();
        }
        self.history.push_back((wait_us, self.budget_hits));
        self.used = Duration::ZERO;
        self.budget_hits = 0;
    }

    /// 直近の tick の待ちの p50・p95（最近傍順位）・最大と `tick_budget` の件数。
    pub fn summary(&self) -> ScriptTickWaitSummary {
        let mut waits: Vec<u64> = self.history.iter().map(|(w, _)| *w).collect();
        waits.sort_unstable();
        let rank = |p: usize| -> u64 {
            if waits.is_empty() {
                return 0;
            }
            let idx = (p * waits.len()).div_ceil(100).max(1) - 1;
            waits[idx.min(waits.len() - 1)]
        };
        ScriptTickWaitSummary {
            ticks: waits.len(),
            p50_us: rank(50),
            p95_us: rank(95),
            max_us: waits.last().copied().unwrap_or(0),
            tick_budget: self.history.iter().map(|(_, h)| *h).sum(),
        }
    }
}

/// 戻しの書き込みに失敗した領域（S5-6 の fail-closed）。
#[derive(Debug, Clone)]
pub(in crate::dispatcher) struct PendingRollback {
    pub domain: ScriptDomain,
    pub reason: String,
}

/// TS の側の 1 回の結果: 記録と、Rust の検証を通った TS の出力（使えないなら `None`）。
struct TsSide {
    record: Result<ScriptCompareRecord, serde_json::Error>,
    validated: Option<serde_json::Value>,
}

/// 付記 S5-3: 単位が TS の側か（`ts` は全部、`canary` は決定的な選択）。
pub(in crate::dispatcher) fn unit_selected(
    target: &CompareTarget,
    domain: ScriptDomain,
    unit_key: &str,
) -> bool {
    match target.mode {
        ScriptMode::Ts => true,
        ScriptMode::Canary => target.canary_bps.is_some_and(|bps| {
            task_core::script_compare::canary_selected(domain, &target.generation_id, unit_key, bps)
        }),
        ScriptMode::Rust | ScriptMode::Compare => false,
    }
}

/// TS の失敗の分類から fallback の code を作る（S5-6）。
fn fallback_of(record: &ScriptCompareRecord) -> CosDigestFallback {
    let message = record
        .error
        .as_ref()
        .map(|e| format!("{}: {}", e.kind, e.message))
        .unwrap_or_default();
    if !record.safety_violations.is_empty() {
        return CosDigestFallback::new(
            "safety_violation",
            &format!("{} {message}", record.safety_violations.join(",")),
        );
    }
    let code = match record.outcome {
        CompareOutcome::InvalidOutput => "invalid_output",
        CompareOutcome::Unavailable => "unavailable",
        _ => match record.error.as_ref().map(|e| e.kind.as_str()) {
            // 認可の拒否は許可側に倒さず、Rust の判定を使う（S5-6）。
            Some("denied") => "denied",
            _ => "ts_error",
        },
    };
    CosDigestFallback::new(code, &message)
}

/// 同期の tick から TS を呼ぶ（cos_digest の pass と同じく別の current-thread runtime で待つ）。
fn block_on_ts<F>(call: F) -> Option<(TsResult, Duration)>
where
    F: std::future::Future<Output = (TsResult, Duration)> + Send,
{
    super::super::cluster::run_cluster_hooks_off_async(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => Some(runtime.block_on(call)),
            Err(error) => {
                tracing::warn!(%error, "no runtime for the script call; Rust decides");
                None
            }
        }
    })
}

impl Dispatcher {
    /// tick の始め: tick を数え、前の tick の TS の待ちを集計して予算を戻す（S5-8）。失敗した戻しを書き直す（S5-6）。
    pub(in crate::dispatcher) fn begin_tick(&mut self) {
        self.ticks += 1;
        self.script_tick_rollover();
    }

    /// 付記 S5-8: tick の終わりに TS の待ちを集計し、予算を戻す。失敗した戻しを書き直す（S5-6）。
    pub(in crate::dispatcher) fn script_tick_rollover(&mut self) {
        self.script_compare.tick_wait.end_tick();
        self.retry_pending_rollbacks();
    }

    /// 付記 S5-8: 直近の tick の TS の待ちの集計。
    pub fn script_tick_wait_summary(&self) -> ScriptTickWaitSummary {
        self.script_compare.tick_wait.summary()
    }

    /// 付記 S5-8（P1-C）: snapshot に載せる集計。tick をまだ数えていなければ `None`。
    pub fn script_tick_wait_live(&self) -> Option<task_ops::daemon::ScriptTickWaitLive> {
        let s = self.script_tick_wait_summary();
        (s.ticks > 0).then(|| task_ops::daemon::ScriptTickWaitLive {
            ticks: s.ticks as u64,
            p50_us: s.p50_us,
            p95_us: s.p95_us,
            max_us: s.max_us,
            tick_budget: s.tick_budget,
            budget_us: u64::try_from(SCRIPT_TICK_WAIT_BUDGET.as_micros()).unwrap_or(u64::MAX),
        })
    }

    /// この tick の TS の待ちの残り（S5-8。cos_digest の pass が使う）。
    pub(in crate::dispatcher) fn script_tick_wait_remaining(&self) -> Duration {
        self.script_compare.tick_wait.remaining()
    }

    pub(in crate::dispatcher) fn script_tick_charge(&mut self, waited: Duration) {
        self.script_compare.tick_wait.charge(waited);
    }

    pub(in crate::dispatcher) fn script_tick_budget_hit(&mut self) {
        self.script_compare.tick_wait.budget_hit();
    }

    /// 戻しの書き込みに失敗して process の中で `rust` と扱っている領域。
    pub fn script_domains_forced_rust(&self) -> Vec<ScriptDomain> {
        self.script_compare
            .pending_rollbacks
            .iter()
            .map(|p| p.domain)
            .collect()
    }

    /// 付記 S5-6: 領域を actor `system:script-guard` で `rust` に戻す。書けなければ process の中で `rust` と扱い、
    /// tick ごとに書き直す。
    pub(in crate::dispatcher) fn script_rollback(
        &mut self,
        domain: ScriptDomain,
        expected_version: u64,
        reason: &str,
    ) {
        let written = match &self.script_compare.store {
            Some(store) => store
                .script_domain_rollback(
                    domain,
                    expected_version,
                    reason,
                    time::OffsetDateTime::now_utc(),
                )
                .map_err(|e| e.to_string()),
            None => Err("no script domain store".to_owned()),
        };
        if let Err(error) = written {
            tracing::warn!(domain = domain.as_str(), %error, "script rollback write failed; treating the domain as rust");
            if !self
                .script_compare
                .pending_rollbacks
                .iter()
                .any(|p| p.domain == domain)
            {
                self.script_compare.pending_rollbacks.push(PendingRollback {
                    domain,
                    reason: reason.to_owned(),
                });
            }
        }
        // この tick の残りは読み直した行で判定する（戻した領域は `rust`）。
        self.script_compare.read_at_tick = None;
    }

    fn retry_pending_rollbacks(&mut self) {
        let Some(store) = self.script_compare.store.clone() else {
            return;
        };
        let pending = std::mem::take(&mut self.script_compare.pending_rollbacks);
        for p in pending {
            let row = store
                .script_domain_modes()
                .map_err(|e| e.to_string())
                .map(|rows| rows.into_iter().find(|r| r.domain == p.domain));
            let done = match row {
                Ok(Some(r)) if r.mode == ScriptMode::Rust => true,
                Ok(Some(r)) => store
                    .script_domain_rollback(
                        p.domain,
                        r.version,
                        &p.reason,
                        time::OffsetDateTime::now_utc(),
                    )
                    .is_ok(),
                Ok(None) | Err(_) => false,
            };
            if !done {
                self.script_compare.pending_rollbacks.push(p);
            }
        }
    }

    /// TS の側の 1 回（S5-6・S5-8）: 予算・deadline の内で TS を待ち、Rust の検証を通った出力を返す。
    fn ts_side(
        &mut self,
        mut draft: CompareDraft,
        call: Result<Arc<dyn PolicyScript>, String>,
    ) -> TsSide {
        draft.canary_selected = true;
        let unavailable = |mut draft: CompareDraft, code: &str, reason: &str| {
            draft.applied_by = Some(CosDigestAppliedBy::Rust);
            draft.fallback = Some(CosDigestFallback::new(code, reason));
            TsSide {
                record: draft.unavailable(reason),
                validated: None,
            }
        };
        let script = match call {
            Ok(script) => script,
            Err(reason) => return unavailable(draft, "unavailable", &reason),
        };
        let remaining = self.script_compare.tick_wait.remaining();
        if remaining.is_zero() {
            self.script_compare.tick_wait.budget_hit();
            return unavailable(
                draft,
                "tick_budget",
                &format!(
                    "tick wait budget {} ms used up",
                    SCRIPT_TICK_WAIT_BUDGET.as_millis()
                ),
            );
        }
        let deadline = remaining.min(SCRIPT_CALL_DEADLINE);
        let call_draft = draft.clone();
        let Some((result, elapsed)) =
            block_on_ts(async move { call_draft.call(script.as_ref(), deadline).await })
        else {
            return unavailable(draft, "unavailable", "no runtime for the script call");
        };
        self.script_compare.tick_wait.charge(elapsed.min(deadline));
        // 予算で縮めた deadline の超えは TS の timeout に数えない（S5-8）。
        if matches!(result, TsResult::DeadlineExceeded(_)) && deadline < SCRIPT_CALL_DEADLINE {
            self.script_compare.tick_wait.budget_hit();
            return unavailable(
                draft,
                "tick_budget",
                &format!(
                    "tick wait budget {} ms used up during the call",
                    SCRIPT_TICK_WAIT_BUDGET.as_millis()
                ),
            );
        }
        let mut record = match draft.finish(&result, elapsed) {
            Ok(r) => r,
            Err(e) => {
                return TsSide {
                    record: Err(e),
                    validated: None,
                };
            }
        };
        let valid = matches!(
            record.outcome,
            CompareOutcome::Match | CompareOutcome::Mismatch | CompareOutcome::Explained
        ) && record.safety_violations.is_empty();
        let validated = record.ts.clone().filter(|_| valid);
        if validated.is_some() {
            record.applied_by = Some(CosDigestAppliedBy::Ts);
        } else {
            record.applied_by = Some(CosDigestAppliedBy::Rust);
            record.fallback = Some(fallback_of(&record));
        }
        TsSide {
            record: Ok(record),
            validated,
        }
    }

    /// 付記 S5-3: `lane_rules` の canary・ts。TS の側なら TS の lane（天井で丸めた後）を返す。Rust の側・
    /// compare は影の比較を走らせて `None`（Rust の lane のまま）。
    pub(in crate::dispatcher) fn canary_lane_rules(
        &mut self,
        task_id: TaskId,
        features: &task_core::model_policy::TaskFeatures,
        rust_lane: Tier,
        ceiling: &task_core::model_policy::LaneCeiling,
    ) -> Option<Tier> {
        let target = self.script_target(
            ScriptDomain::LaneRules,
            &[ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts],
        )?;
        let selected = unit_selected(&target, ScriptDomain::LaneRules, &task_id.to_string());
        let mut draft = CompareDraft::lane_rules(
            target.generation_id.clone(),
            target.bundle_sha256.clone(),
            target.host_api.clone(),
            features,
            rust_lane,
            ceiling,
        );
        if !selected {
            self.shadow_compare(task_id, &target, draft);
            return None;
        }
        draft.canary_bps = target.canary_bps;
        let store = Arc::clone(&target.store);
        let side = self.ts_side(draft, target.call.map(|(s, _)| s));
        let lane = side
            .validated
            .and_then(|v| serde_json::from_value::<LaneRulesOutput>(v).ok())
            .map(|ts| ceiling.clamp(ts.proposed).0);
        append_observed(store.as_ref(), task_id, side.record, true);
        if let Some(lane) = lane {
            self.script_compare.lane_canary.insert(task_id, lane);
        }
        lane
    }

    /// WU の lane の上限（`decide_lane_for_work_unit`）に、この tick の `lane_rules` の TS の lane を使う。
    pub(in crate::dispatcher) fn lane_canary_for(&self, task_id: TaskId) -> Option<Tier> {
        self.script_compare.lane_canary.get(&task_id).copied()
    }

    /// 付記 S5-3: `route_priority` の canary・ts。TS の側で TS の候補列が検証を通り Rust と違えば、その列を返す
    /// （呼び出し側はその列で選び直す）。それ以外は `None`。
    pub(in crate::dispatcher) fn canary_route_priority(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        input: &RouteInput,
        trace: &RouteTrace,
    ) -> Option<(task_core::llm_route::HarnessId, Vec<CandidateKey>)> {
        if input.cos {
            return None;
        }
        let harness = trace.harness.clone()?;
        let target = self.script_target(
            ScriptDomain::RoutePriority,
            &[ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts],
        )?;
        let selected = unit_selected(&target, ScriptDomain::RoutePriority, &task_id.to_string());
        let mut draft = CompareDraft::route_priority(
            target.generation_id.clone(),
            target.bundle_sha256.clone(),
            target.host_api.clone(),
            Some(run_id.to_owned()),
            &self.llm_route_registry,
            &harness,
            input.lane.as_deref(),
            false,
            input.harness.execution_class.as_ref(),
        );
        if !selected {
            self.shadow_compare(task_id, &target, draft);
            return None;
        }
        draft.canary_bps = target.canary_bps;
        let store = Arc::clone(&target.store);
        let side = self.ts_side(draft, target.call.map(|(s, _)| s));
        let mismatch = side
            .record
            .as_ref()
            .is_ok_and(|r| r.outcome == CompareOutcome::Mismatch);
        let candidates = side
            .validated
            .and_then(|v| serde_json::from_value::<RoutePriorityOutput>(v).ok())
            .filter(|_| mismatch)
            .map(|ts| {
                ts.candidates
                    .into_iter()
                    .map(|c| CandidateKey {
                        source: SourceId(c.source),
                        model: ModelId(c.model),
                    })
                    .collect::<Vec<_>>()
            });
        append_observed(store.as_ref(), task_id, side.record, true);
        candidates.map(|c| (harness, c))
    }

    /// ADR 2026-11-04-ts-execution-gate-domain: Rust の gate の判定。対象外の規則と人の明示に当たらない task だけ、
    /// 規則表の部分を領域 `execution_gate` と比べる。canary の TS の側と `ts` では Rust の検証を通った TS の出力を使う。
    /// 閾値・`source`・`shadow`・`depth`・`policy_version` は Rust の値のまま。
    pub(in crate::dispatcher) fn execution_gate_decide(
        &mut self,
        task: &Task,
        features: &task_core::TaskFeatures,
        human_execution: Option<task_core::ExecutionMode>,
        cos_hint_compound: bool,
        shadow: bool,
        at: task_core::GateThreshold,
    ) -> task_core::ExecutionGateDecision {
        let inputs = task_core::ExecutionGateInputs::default();
        let mut decision = task_core::decide_execution_gate_at(
            task,
            features,
            human_execution,
            cos_hint_compound,
            inputs,
            shadow,
            at,
        );
        if human_execution.is_none() && task_core::out_of_scope_rule(task).is_none() {
            let input = task_core::execution_gate::ExecutionGateInput::from_task(
                task,
                features,
                cos_hint_compound,
                inputs,
                at.threshold,
            );
            if let Some(ts) = self.canary_execution_gate(task.id, input) {
                decision.signals = ts.gate_signals();
                decision.mode = ts.mode;
                decision.score = ts.score;
                decision.rule_id = ts.rule_id;
            }
        }
        decision
    }

    /// `execution_gate` の compare・canary・ts。TS の側で Rust の検証を通った出力を返す。それ以外は `None`
    /// （Rust の判定のまま）。単位は task id。
    fn canary_execution_gate(
        &mut self,
        task_id: TaskId,
        input: task_core::execution_gate::ExecutionGateInput,
    ) -> Option<task_core::execution_gate::ExecutionGateOutput> {
        let target = self.script_target(
            ScriptDomain::ExecutionGate,
            &[ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts],
        )?;
        let selected = unit_selected(&target, ScriptDomain::ExecutionGate, &task_id.to_string());
        let oversized = input.title.len().saturating_add(input.objective.len())
            > crate::script_policy::EXECUTION_GATE_TEXT_MAX;
        let mut draft = CompareDraft::execution_gate(
            target.generation_id.clone(),
            target.bundle_sha256.clone(),
            target.host_api.clone(),
            input,
        );
        if oversized {
            // ADR D9: 大きすぎる本文は TS に渡さない。Rust の判定を使い、`unavailable` を残す。
            let reason = "the task text is over the execution_gate input limit";
            if target.mode != ScriptMode::Compare {
                draft.applied_by = Some(CosDigestAppliedBy::Rust);
                draft.canary_bps = target.canary_bps;
                draft.canary_selected = selected;
                draft.fallback = selected.then(|| CosDigestFallback::new("unavailable", reason));
            }
            append_observed(
                target.store.as_ref(),
                task_id,
                draft.unavailable(reason),
                selected,
            );
            return None;
        }
        if !selected {
            self.shadow_compare(task_id, &target, draft);
            return None;
        }
        draft.canary_bps = target.canary_bps;
        let store = Arc::clone(&target.store);
        let side = self.ts_side(draft, target.call.map(|(s, _)| s));
        let output = side.validated.and_then(|v| serde_json::from_value(v).ok());
        append_observed(store.as_ref(), task_id, side.record, true);
        output
    }

    /// compare と canary の Rust の側: TS を影で走らせ、記録だけを残す（挙動は Rust のまま）。
    fn shadow_compare(&mut self, task_id: TaskId, target: &CompareTarget, mut draft: CompareDraft) {
        if target.mode != ScriptMode::Compare {
            draft.applied_by = Some(CosDigestAppliedBy::Rust);
            draft.canary_bps = target.canary_bps;
        }
        let store = Arc::clone(&target.store);
        let domain = draft.script_domain();
        let (script, runtime) = match &target.call {
            Err(reason) => {
                append_observed(store.as_ref(), task_id, draft.unavailable(reason), false);
                return;
            }
            Ok((s, r)) => (Arc::clone(s), r.clone()),
        };
        self.spawn_compare(domain, &runtime, async move {
            let (result, elapsed) = draft.call(script.as_ref(), SCRIPT_CALL_DEADLINE).await;
            append_observed(
                store.as_ref(),
                task_id,
                draft.finish(&result, elapsed),
                false,
            );
        });
    }
}

/// 記録を追記し、同じ transaction で計数する（S5-7）。失敗しても dispatch は続ける（warn の log だけ。S2-6）。
pub(in crate::dispatcher) fn append_observed(
    store: &dyn ScriptDomainStore,
    task_id: TaskId,
    record: Result<ScriptCompareRecord, serde_json::Error>,
    ts_side: bool,
) {
    let record = match record {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%task_id, error = %e, "script_compare_recorded build failed");
            return;
        }
    };
    match store.script_compare_append_observed(
        task_id,
        &record,
        ts_side,
        time::OffsetDateTime::now_utc(),
    ) {
        Ok((_, Some(outcome))) if !outcome.rolled_back.is_empty() => {
            tracing::warn!(
                %task_id,
                domain = record.domain.as_str(),
                stop = ?outcome.stop,
                "script-guard rolled the domain back to rust"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%task_id, error = %e, "script_compare_recorded append failed"),
    }
}

/// tick ごとの `lane_rules` の TS の lane（WU の lane の上限に使う）。
pub(in crate::dispatcher) type LaneCanary = HashMap<TaskId, Tier>;
