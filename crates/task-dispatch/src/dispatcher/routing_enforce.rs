//! ADR 2026-10-04 multi-objective model routing Phase 2（p2-dispatch-enforce）。
//!
//! - 既存の account 帳簿（残量・reset・cooldown・rejected）・in-use・provider の cooldown・self-host の
//!   load の取り込み口から `SourceState` を組む（新しい保存先は作らない。未知は None のまま）。
//! - `mode = enforce`（heuristic のみ・既定 off）の dispatcher 経路: 候補 allowlist を作り、task/組織固有の
//!   制約を proxy に渡せない経路は除外理由を残して外す。quota の枯渇・圧迫は lane を下げず、同じ lane の
//!   別 source を先に試し、無ければ defer する（ADR §7 の「暗黙降格の廃止」）。
//! - legacy は従来どおり `select_tier` の結果。cheap-local-first（`prefer_local`）の前段は変えない。

use super::*;
use crate::accounts::AccountState;
use task_core::accounts::{RateLimitObservation, RateWindow};
use task_core::model_router::{
    policy::{Constraints, FreshnessPolicy, RoutingMode},
    profiles::{QuotaWindow, SourceState},
};

/// 観測の鮮度（`accounts::measured_remaining` と同じ 300 秒）。
const OBSERVATION_TTL_SECS: i64 = 300;

/// dispatcher 側の model routing 設定。既定は legacy（enforce は人の opt-in）。
#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRoutingSettings {
    pub mode: RoutingMode,
    /// task/組織固有の追加制約。既定（空）なら制約なし。
    pub constraints: Constraints,
    pub freshness: FreshnessPolicy,
    /// subscription 窓 id（`five_hour`・`seven_day` 等）→ 設定の reserve_value（USD）。
    /// 載っていない窓の `reserve_value_usd` は None（unknown）のまま。
    pub window_reserves: std::collections::BTreeMap<String, f64>,
    /// Phase 3: context 長の余裕 token 数。
    pub context_safety_margin: Option<u64>,
    /// Phase 3: run 間 escalation の閾値と上限。
    pub escalation: task_core::EscalationThresholds,
}

impl Default for DispatchRoutingSettings {
    fn default() -> Self {
        Self {
            mode: RoutingMode::Legacy,
            constraints: Constraints::default(),
            freshness: FreshnessPolicy::default(),
            window_reserves: std::collections::BTreeMap::new(),
            context_safety_margin: None,
            escalation: task_core::EscalationThresholds::default(),
        }
    }
}

/// self-host の queue と GPU load の取り込み口（観測者が `Dispatcher::set_self_host_load` で渡す）。
/// 未観測の欄は None。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SelfHostLoad {
    pub queue_depth: Option<u32>,
    pub queue_limit: Option<u32>,
    pub gpu_utilization: Option<f64>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// 観測時刻（Unix 秒）。
    pub observed_at: Option<i64>,
}

fn rfc3339(unix: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
}

fn account_window(
    account_id: &str,
    window_id: &str,
    duration_secs: i64,
    obs: &RateLimitObservation,
    window: RateWindow,
    now: i64,
) -> QuotaWindow {
    QuotaWindow {
        account_id: account_id.to_string(),
        window_id: window_id.to_string(),
        unit: "fraction".into(),
        // 古い・未来・期限切れ・範囲外は None（0 にしない）。
        remaining: crate::accounts::window_remaining(obs, now, Some(window)),
        limit: Some(1.0),
        reset_at: rfc3339(window.resets_at),
        measured: true,
        observed_at: rfc3339(obs.observed_at),
        window_duration_s: Some(duration_secs as f64),
        estimated_consumption: None,
        reserve_value_usd: None,
    }
}

/// 1 か月窓の長さ（30 日。ADR 2026-10-06 D1）。
const ONE_MONTH_SECS: i64 = 30 * 24 * 3600;

/// pool の 1 アカウントの `SourceState`。帳簿の観測（5h / 7d / 1 か月窓・rejected の reset）・cooldown・in-use
/// だけを写す。帳簿に無い量（latency・rpm/tpm・queue・GPU）は None。
pub fn source_state_from_account(
    deployment_id: &str,
    account_id: &str,
    state: Option<&AccountState>,
    in_use: Option<u32>,
    now: i64,
) -> SourceState {
    let mut s = SourceState::unobserved(deployment_id);
    s.in_use = in_use;
    s.capacity_scope = Some(format!("account:{account_id}"));
    let Some(state) = state else {
        return s;
    };
    if let Some(cd) = state.cooldown {
        s.cooldown = cd.until > now;
        s.cooldown_until = rfc3339(cd.until);
    }
    if let Some(obs) = &state.usage {
        s.observed_at = rfc3339(obs.observed_at);
        s.expires_at = rfc3339(obs.observed_at + OBSERVATION_TTL_SECS);
        s.provenance = Some(format!(
            "account_book:{}",
            match state.source {
                Some(crate::accounts::ObservationSource::Run) => "run",
                Some(crate::accounts::ObservationSource::Check) => "check",
                None => "unknown",
            }
        ));
        if obs.status.as_deref() == Some("rejected") {
            s.rate_limited_until = obs.resets_at.and_then(rfc3339);
        }
        if let Some(w) = obs.five_hour {
            s.quota_windows.push(account_window(
                account_id,
                "five_hour",
                crate::accounts::FIVE_HOUR_SECS,
                obs,
                w,
                now,
            ));
        }
        if let Some(w) = obs.seven_day {
            s.quota_windows.push(account_window(
                account_id,
                "seven_day",
                crate::accounts::SEVEN_DAY_SECS,
                obs,
                w,
                now,
            ));
        }
        if let Some(w) = obs.one_month {
            s.quota_windows.push(account_window(
                account_id,
                "one_month",
                ONE_MONTH_SECS,
                obs,
                w,
                now,
            ));
        }
        s.quota_remaining = crate::accounts::measured_remaining(obs, now);
    }
    s
}

/// pool を使わない provider 行の `SourceState`。provider の cooldown 終了時刻（Unix 秒）・in-use・
/// self-host の load の取り込み口だけを写す。
pub fn source_state_for_provider(
    deployment_id: &str,
    cooldown_until: Option<i64>,
    in_use: Option<u32>,
    load: Option<&SelfHostLoad>,
    now: i64,
) -> SourceState {
    let mut s = SourceState::unobserved(deployment_id);
    s.in_use = in_use;
    s.capacity_scope = Some(format!("provider:{deployment_id}"));
    if let Some(until) = cooldown_until {
        s.cooldown = until > now;
        s.cooldown_until = rfc3339(until);
    }
    if let Some(load) = load {
        s.queue_depth = load.queue_depth;
        s.queue_limit = load.queue_limit;
        s.gpu_utilization = load.gpu_utilization;
        s.vram_used = load.vram_used;
        s.vram_total = load.vram_total;
        if let Some(at) = load.observed_at {
            s.observed_at = rfc3339(at);
            s.expires_at = rfc3339(at + OBSERVATION_TTL_SECS);
            s.provenance = Some("self_host_load".into());
        }
    }
    s
}

/// enforce の quota 判定。`select_tier` が lane を保てば Ok（理由）、下げるか defer なら Err（理由）。
/// enforce では lane を下げず、Err の source は除外して同じ lane の別 source を試す。
pub fn enforce_quota_verdict(lane: Tier, remaining: Option<f64>) -> Result<String, String> {
    match task_core::model_routing::select_tier(lane, remaining) {
        Ok((selected, reason)) if selected == lane => Ok(format!("{reason}; enforce")),
        Ok((selected, _)) => Err(format!(
            "measured quota remaining={:.1}% would lower lane {lane:?} -> {selected:?}; \
             enforce keeps the lane",
            remaining.unwrap_or_default() * 100.0
        )),
        Err(reason) => Err(reason),
    }
}

/// enforce の候補 1 行の静的な属性（allowlist の判定材料）。
#[derive(Debug, Clone, PartialEq)]
pub struct EnforceSource<'a> {
    pub provider: &'a str,
    pub source_ref: &'a str,
    /// llm-proxy 経由の行（`llm_source` を持つ）。Phase 2 では task の context を proxy に渡せない。
    pub proxy_routed: bool,
    pub external_network: bool,
    pub retains_data: Option<bool>,
    pub region: Option<&'a str>,
}

/// task/組織固有の制約による除外の理由コード（空なら適格）。制約の未知は通さない。
pub fn constraint_exclusions(c: &Constraints, src: &EnforceSource<'_>) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if *c == Constraints::default() {
        return reasons;
    }
    // ADR §2: 制約を proxy に渡せない経路は enforce 候補にしない（黙って捨てない）。
    if src.proxy_routed {
        reasons.push("context_transport_unsupported");
    }
    if c.allowed_deployments
        .as_ref()
        .is_some_and(|l| !l.iter().any(|d| d == src.provider))
    {
        reasons.push("deployment");
    }
    if c.allowed_sources
        .as_ref()
        .is_some_and(|l| !l.iter().any(|s| s == src.source_ref))
    {
        reasons.push("source");
    }
    if (c.external_network_allowed == Some(false) && src.external_network)
        || (c.data_retention_allowed == Some(false) && src.retains_data != Some(false))
    {
        reasons.push("privacy");
    }
    if c.required_region
        .as_deref()
        .is_some_and(|r| src.region != Some(r))
    {
        reasons.push("locality");
    }
    // dispatcher は費用・latency を見積もらない（未知は制約を満たすと示せない）。
    if c.max_cost_usd.is_some() {
        reasons.push("cost");
    }
    if c.max_latency_ms.is_some() {
        reasons.push("latency");
    }
    reasons
}

impl Dispatcher {
    /// dispatcher の model routing 設定（既定 legacy）。
    pub fn set_dispatch_routing(&mut self, settings: DispatchRoutingSettings) {
        self.dispatch_routing = settings;
    }

    pub fn dispatch_routing(&self) -> &DispatchRoutingSettings {
        &self.dispatch_routing
    }

    /// self-host の queue / GPU load の取り込み口（観測は揮発。保存しない）。
    pub fn set_self_host_load(&mut self, provider: impl Into<ProviderId>, load: SelfHostLoad) {
        self.self_host_loads.insert(provider.into(), load);
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountCooldown, AccountCooldownReason, ObservationSource};

    fn obs(u5: f64, u7: Option<f64>, observed_at: i64) -> RateLimitObservation {
        RateLimitObservation {
            one_month: None,
            five_hour: Some(RateWindow {
                utilization: u5,
                resets_at: observed_at + 3600,
            }),
            seven_day: u7.map(|u| RateWindow {
                utilization: u,
                resets_at: observed_at + 86_400,
            }),
            status: None,
            resets_at: None,
            observed_at,
        }
    }

    #[test]
    fn account_state_maps_windows_cooldown_and_keeps_unknowns_none() {
        let empty = source_state_from_account("p", "a", None, None, 10_000);
        assert_eq!(empty.quota_remaining, None);
        assert!(empty.quota_windows.is_empty());
        assert_eq!(empty.in_use, None);
        assert_eq!(empty.latency_ms, None);

        let state = AccountState {
            usage: Some(obs(0.4, Some(0.7), 10_000)),
            source: Some(ObservationSource::Run),
            cooldown: Some(AccountCooldown {
                until: 10_060,
                reason: AccountCooldownReason::Throttled,
            }),
            last_check: None,
        };
        let s = source_state_from_account("p", "a", Some(&state), Some(1), 10_000);
        assert!(s.cooldown);
        assert_eq!(s.cooldown_until.as_deref(), Some("1970-01-01T02:47:40Z"));
        assert_eq!(s.in_use, Some(1));
        assert_eq!(s.quota_windows.len(), 2);
        assert_eq!(s.quota_windows[0].window_id, "five_hour");
        assert!((s.quota_windows[0].remaining.unwrap() - 0.6).abs() < 1e-9);
        assert!((s.quota_remaining.unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(s.provenance.as_deref(), Some("account_book:run"));
        assert_eq!(s.gpu_utilization, None);

        // 古い観測（300 秒超）の残量は None（0 にしない）。7d 窓が無ければ総合残量も None。
        let stale = AccountState {
            usage: Some(obs(1.0, None, 10_000)),
            ..Default::default()
        };
        let s = source_state_from_account("p", "a", Some(&stale), Some(0), 10_400);
        assert_eq!(s.quota_windows[0].remaining, None);
        assert_eq!(s.quota_remaining, None);
        assert!(!s.cooldown);
    }

    #[test]
    fn provider_state_takes_self_host_load_and_cooldown() {
        let load = SelfHostLoad {
            queue_depth: Some(3),
            queue_limit: Some(8),
            gpu_utilization: Some(0.5),
            vram_used: None,
            vram_total: Some(80),
            observed_at: Some(9_990),
        };
        let s = source_state_for_provider("qwen", Some(10_005), Some(2), Some(&load), 10_000);
        assert!(s.cooldown);
        assert_eq!(s.queue_depth, Some(3));
        assert_eq!(s.vram_used, None);
        assert_eq!(s.provenance.as_deref(), Some("self_host_load"));
        let bare = source_state_for_provider("qwen", Some(9_000), None, None, 10_000);
        assert!(!bare.cooldown);
        assert_eq!(bare.queue_depth, None);
        assert_eq!(bare.observed_at, None);
    }

    #[test]
    fn enforce_quota_verdict_never_lowers_the_lane() {
        assert!(enforce_quota_verdict(Tier::Frontier, None).is_ok());
        assert!(enforce_quota_verdict(Tier::Frontier, Some(0.9)).is_ok());
        // legacy なら standard へ下げる残量。enforce は除外（同 lane の別 source か defer）。
        assert!(enforce_quota_verdict(Tier::Frontier, Some(0.2)).is_err());
        assert!(enforce_quota_verdict(Tier::Standard, Some(0.2)).is_ok());
        assert!(enforce_quota_verdict(Tier::Standard, Some(0.05)).is_err());
        assert!(enforce_quota_verdict(Tier::Cheap, Some(0.02)).is_err());
    }

    #[test]
    fn constraints_exclude_proxy_routes_and_unknowns_with_reasons() {
        let src = EnforceSource {
            provider: "qwen",
            source_ref: "openai_compatible:qwen",
            proxy_routed: true,
            external_network: false,
            retains_data: None,
            region: None,
        };
        assert!(constraint_exclusions(&Constraints::default(), &src).is_empty());
        let c = Constraints {
            external_network_allowed: Some(false),
            ..Default::default()
        };
        assert_eq!(
            constraint_exclusions(&c, &src),
            vec!["context_transport_unsupported"]
        );
        let direct = EnforceSource {
            provider: "claude",
            source_ref: "claude",
            proxy_routed: false,
            external_network: true,
            ..src.clone()
        };
        assert_eq!(constraint_exclusions(&c, &direct), vec!["privacy"]);
        let retention = Constraints {
            data_retention_allowed: Some(false),
            required_region: Some("jp".into()),
            ..Default::default()
        };
        assert_eq!(
            constraint_exclusions(&retention, &direct),
            vec!["privacy", "locality"]
        );
    }
}
