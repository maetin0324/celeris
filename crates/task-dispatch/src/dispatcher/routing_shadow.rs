//! ADR 2026-10-04 multi-objective model routing §7.1・§10 Phase 4（dispatch-shadow）: decision shadow。
//!
//! - `mode = shadow` のとき primary は legacy の経路（`select_provider_for` と `select_tier`）のまま決め、
//!   同じ候補行（`legacy_provider_profiles`）と同じ source 状態（account 帳簿・cooldown・in-use）で
//!   候補 policy（Phase 2 の heuristic kernel = enforce の判断）を**純粋に**計算する。
//! - 計算は読むだけ: upstream・HTTP・health probe・worker 起動・reviewer 追加・run 再実行をしない。
//!   `full`・`unroutable`・警告の集合・アカウントの scan cache も書き換えない。ローカル行の health は
//!   primary の選択がこの tick に見た結果（`ProviderSelection.candidates`）だけを使う。
//! - 結果は `Event::RoutingShadowRecorded`（kind = decision）1 件。primary との差・除外理由・候補比較は
//!   `DecisionShadowComparison`（version 1）を JSON にして `ShadowRecord.detail` に入れる
//!   （本文・prompt・credential・tokens・費用は持たない）。mode = legacy / enforce では何も記録しない。

use super::*;
use crate::accounts::AccountCandidate;
use serde::{Deserialize, Serialize};
use task_core::model_router::{
    policy::RoutingMode,
    shadow::{ShadowKind, ShadowPolicy, ShadowRecord, ShadowStatus},
};
use task_core::model_routing::{ProviderCandidateOutcome, ProviderSelection};

/// `DecisionShadowComparison` の wire 版。
pub const DECISION_SHADOW_COMPARISON_VERSION: u32 = 1;

/// decision shadow の候補 policy の版（Phase 2 の enforce と同じ kernel）。
pub const DECISION_SHADOW_POLICY_VERSION: &str = "dispatch-enforce-heuristic-v1";

/// 候補 1 行の比較（設定順）。`excluded_reasons` が空なら候補 policy にとって適格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionShadowCandidate {
    pub deployment_id: String,
    pub model_profile_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_reasons: Vec<String>,
    /// 候補 policy がこの行を選んだ。
    #[serde(default)]
    pub selected: bool,
    /// primary（legacy）がこの行を選んだ。
    #[serde(default)]
    pub primary: bool,
}

/// primary と候補 policy の判断の比較（`ShadowRecord.detail` に JSON で入る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionShadowComparison {
    pub version: u32,
    pub primary_mode: RoutingMode,
    pub candidate_policy: String,
    pub requested_lane: Tier,
    pub primary_source: String,
    pub primary_model: String,
    pub primary_lane: Tier,
    /// 候補 policy が選んだ source（全候補が除外なら `None` = defer）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    /// 候補 policy は lane を下げない（requested のまま）。defer なら `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_lane: Option<Tier>,
    /// source・lane とも一致。
    pub agrees: bool,
    /// 差の種類（`source` / `model` / `lane` / `deferred`）。一致なら空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
    pub candidates: Vec<DecisionShadowCandidate>,
}

/// dispatch の 1 回で計算した候補 policy の判断（primary の model が決まる前に state を写しておく）。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DecisionShadowRound {
    pub requested_lane: Tier,
    pub candidates: Vec<DecisionShadowCandidate>,
}

impl DecisionShadowRound {
    fn selected(&self) -> Option<&DecisionShadowCandidate> {
        self.candidates.iter().find(|c| c.selected)
    }
}

/// `[model_routing.shadow]` の差し替えを受け取る側（daemon が llm-proxy の shadow queue を繋ぐ）。
/// `mode` は同じ時点の `DispatchRoutingSettings.mode`。呼び出し側を待たせない実装にする。
pub trait RoutingShadowListener: Send + Sync {
    fn reload(&self, mode: RoutingMode, policy: &ShadowPolicy);
}

impl Dispatcher {
    pub(super) fn shadow_active(&self) -> bool {
        self.dispatch_routing.mode == RoutingMode::Shadow
    }

    /// 検証済みの shadow policy を差し替え、listener へ同じ値を渡す。`set_dispatch_routing` の後に呼ぶ
    /// （listener は新しい mode と policy の組を 1 回で受け取る）。
    pub fn set_routing_shadow(&mut self, policy: ShadowPolicy) {
        self.routing_shadow_policy = policy;
        for listener in &self.routing_shadow_listeners {
            listener.reload(self.dispatch_routing.mode, &self.routing_shadow_policy);
        }
    }

    pub fn routing_shadow_policy(&self) -> &ShadowPolicy {
        &self.routing_shadow_policy
    }

    /// listener を足す。足した時点の mode と policy をすぐ 1 回渡す。
    pub fn add_routing_shadow_listener(&mut self, listener: Arc<dyn RoutingShadowListener>) {
        listener.reload(self.dispatch_routing.mode, &self.routing_shadow_policy);
        self.routing_shadow_listeners.push(listener);
    }

    /// pool のアカウントを読むだけで選ぶ（`pick_account` と同じ規則。scan cache には書かない）。
    fn peek_account(&self, adapter: AccountAdapter, provider: &str, cos: bool) -> Option<String> {
        let cfg = self.config.accounts.as_ref()?;
        let dirs = match self.accounts_scan_cache.get(&adapter) {
            Some(dirs) => dirs.clone(),
            None => scan_accounts(cfg.root_for(adapter)?, adapter),
        };
        let requested = self
            .adapters
            .get(provider)
            .and_then(|a| a.account_id())
            .map(str::to_owned);
        let now = (self.now_unix_fn)();
        let book = self.account_book(adapter)?;
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<AccountCandidate<'_>> = dirs
            .iter()
            .filter(|d| requested.as_deref().is_none_or(|id| d.id == id))
            .map(|d| AccountCandidate {
                id: d.id.as_str(),
                logged_in: d.logged_in,
                in_use: self.account_in_use(adapter, &d.id),
            })
            .collect();
        if cos {
            let limit = crate::capacity::account_run_limit(cfg.max_runs_per_account, true);
            return crate::accounts::select_account_least_loaded(&candidates, &book, limit, now);
        }
        select_account(&candidates, &book, cfg.max_runs_per_account, now)
    }

    /// 候補 policy（enforce の kernel）の判断を純粋に計算する。順序は enforce の経路と同じ:
    /// 制約の allowlist → cheap はローカル行を設定順に先に → pool は残量 score 最大、無ければ設定順の
    /// 最初の行。選んだ source の状態（cooldown・rate limit・lane を保てない残量）で外れたら次を試す。
    pub(super) fn decision_shadow_round(
        &self,
        hint: &task_core::WorkerHint,
        cos: bool,
        primary_provider: &str,
        primary_account: Option<&(AccountAdapter, String)>,
        primary_selection: &ProviderSelection,
        now: Instant,
    ) -> DecisionShadowRound {
        let round = self.enforce_round(hint);
        let mut candidates: Vec<DecisionShadowCandidate> = round
            .candidates
            .iter()
            .map(|c| DecisionShadowCandidate {
                deployment_id: c.deployment_id.clone(),
                model_profile_id: c.model_profile_id.clone(),
                excluded_reasons: c.excluded_reasons.clone(),
                selected: false,
                primary: c.deployment_id == primary_provider
                    && self
                        .effective_lane_model(primary_provider, hint.tier)
                        .ok()
                        .flatten()
                        .is_none_or(|m| m == c.model_profile_id),
            })
            .collect();
        // 静的な適格性と state（pool はアカウント）を 1 回だけ集める。
        let mut states: Vec<
            Option<(
                Option<String>,
                task_core::model_router::profiles::SourceState,
            )>,
        > = Vec::with_capacity(candidates.len());
        for c in &mut candidates {
            let id = c.deployment_id.clone();
            if !c.excluded_reasons.is_empty() {
                states.push(None);
                continue;
            }
            if self.provider_full(&id, cos) {
                c.excluded_reasons.push("at_capacity".into());
                states.push(None);
                continue;
            }
            // ローカル行の health は primary がこの tick に見た結果だけ（probe を増やさない）。
            if let Some(seen) = primary_selection
                .candidates
                .iter()
                .find(|s| s.provider == id)
            {
                match seen.outcome {
                    ProviderCandidateOutcome::Down => {
                        c.excluded_reasons.push("unreachable".into());
                        states.push(None);
                        continue;
                    }
                    ProviderCandidateOutcome::Cooldown => {
                        c.excluded_reasons.push("cooldown".into());
                        states.push(None);
                        continue;
                    }
                    _ => {}
                }
            }
            if self.account_pool_providers.contains(&id) {
                let account = if id == primary_provider {
                    primary_account.cloned()
                } else {
                    self.policy
                        .adapter_of(&id)
                        .as_deref()
                        .and_then(|row| self.pool_adapter_of(&id, row))
                        .and_then(|a| self.peek_account(a, &id, cos).map(|acct| (a, acct)))
                };
                let Some(account) = account else {
                    c.excluded_reasons.push("no_account".into());
                    states.push(None);
                    continue;
                };
                let state = self.source_state_of(&id, Some(&account), now);
                states.push(Some((Some(account.1), state)));
            } else {
                let state = self.source_state_of(&id, None, now);
                states.push(Some((None, state)));
            }
        }
        let prefer_local = hint.tier == Tier::Cheap && !cos && !self.local_providers.is_empty();
        loop {
            let eligible = |i: &usize| states[*i].is_some();
            let local_pick = prefer_local
                .then(|| {
                    self.local_providers.iter().find_map(|l| {
                        (0..candidates.len()).filter(eligible).find(|i| {
                            candidates[*i].deployment_id == l.provider
                                && !self.account_pool_providers.contains(&l.provider)
                        })
                    })
                })
                .flatten();
            let pool_pick = (0..candidates.len())
                .filter(eligible)
                .filter(|i| {
                    self.account_pool_providers
                        .contains(&candidates[*i].deployment_id)
                })
                .map(|i| {
                    let score = states[i]
                        .as_ref()
                        .and_then(|(acct, _)| {
                            let adapter = self
                                .policy
                                .adapter_of(&candidates[i].deployment_id)
                                .as_deref()
                                .and_then(|row| {
                                    self.pool_adapter_of(&candidates[i].deployment_id, row)
                                })?;
                            Some(self.account_score(adapter, acct.as_deref()?))
                        })
                        .unwrap_or(f64::NEG_INFINITY);
                    (i, score)
                })
                // 同点は設定順（先の行）を残す。
                .fold(None::<(usize, f64)>, |best, (i, s)| match best {
                    Some((_, b)) if b >= s => best,
                    _ => Some((i, s)),
                })
                .map(|(i, _)| i);
            let pick = local_pick
                .or(pool_pick)
                .or_else(|| (0..candidates.len()).find(eligible));
            let Some(i) = pick else {
                break;
            };
            let Some((_, state)) = states[i].as_ref() else {
                break;
            };
            match self.enforce_check_source(hint.tier, state) {
                Ok(_) => {
                    candidates[i].selected = true;
                    break;
                }
                Err((codes, detail)) => {
                    candidates[i]
                        .excluded_reasons
                        .extend(codes.iter().map(|s| (*s).to_string()));
                    if let Some(d) = detail {
                        candidates[i].excluded_reasons.push(format!("detail:{d}"));
                    }
                    states[i] = None;
                }
            }
        }
        DecisionShadowRound {
            requested_lane: hint.tier,
            candidates,
        }
    }

    /// primary の結果（source・model・lane）と比べた decision shadow の記録。
    pub(super) fn decision_shadow_record(
        round: &DecisionShadowRound,
        primary_decision_id: &str,
        run_id: &str,
        primary_source: &str,
        primary_model: &str,
        primary_lane: Tier,
    ) -> ShadowRecord {
        let selected = round.selected();
        let candidate_source = selected.map(|c| c.deployment_id.clone());
        let candidate_model = selected.map(|c| c.model_profile_id.clone());
        let candidate_lane = selected.map(|_| round.requested_lane);
        let mut differences = Vec::new();
        match selected {
            None => differences.push("deferred".to_string()),
            Some(c) => {
                if c.deployment_id != primary_source {
                    differences.push("source".into());
                }
                if c.model_profile_id != primary_model {
                    differences.push("model".into());
                }
                if round.requested_lane != primary_lane {
                    differences.push("lane".into());
                }
            }
        }
        let agrees = selected.is_some_and(|c| c.deployment_id == primary_source)
            && round.requested_lane == primary_lane;
        let comparison = DecisionShadowComparison {
            version: DECISION_SHADOW_COMPARISON_VERSION,
            primary_mode: RoutingMode::Legacy,
            candidate_policy: DECISION_SHADOW_POLICY_VERSION.into(),
            requested_lane: round.requested_lane,
            primary_source: primary_source.into(),
            primary_model: primary_model.into(),
            primary_lane,
            candidate_source: candidate_source.clone(),
            candidate_model: candidate_model.clone(),
            candidate_lane,
            agrees,
            differences,
            candidates: round.candidates.clone(),
        };
        ShadowRecord {
            shadow_id: ulid::Ulid::new().to_string(),
            primary_decision_id: primary_decision_id.into(),
            kind: ShadowKind::Decision,
            status: ShadowStatus::Completed,
            reason: None,
            detail: serde_json::to_string(&comparison).ok(),
            policy_version: DECISION_SHADOW_POLICY_VERSION.into(),
            run_id: Some(run_id.into()),
            request_id: None,
            candidate_model,
            candidate_source,
            input_tokens: None,
            output_tokens: None,
            output_sha256: None,
            cash_usd: None,
            effective_usd: None,
            latency_ms: None,
            reservation_id: None,
        }
    }
}
