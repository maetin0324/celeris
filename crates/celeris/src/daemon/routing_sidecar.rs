//! ADR 2026-10-04 §7.3・§10 Phase 5（daemon-wire）: `[model_routing.estimator.sidecar]` を llm-proxy の
//! estimator shadow（[`EstimatorShadow`]）へ配線する。
//!
//! - 既定（`enabled = false`）では sidecar client も estimator shadow も作らない（送信 0）。
//!   `enabled = true` かつ `shadow_only = true`（config 検証で必須）のときだけ組んで proxy の差し替え口
//!   （[`EstimatorShadowSlot`]）に入れる。heuristic の primary 判断は変えない（決定権なし）。
//! - 評価は proxy の要求の後に proxy 自身が spawn する（daemon の評価キュー）。CLI 直結の run も同じ
//!   proxy を通るので同じ経路で評価される。dispatcher の tick は sidecar の HTTP を呼ばない。
//! - reload: [`EstimatorSidecarControl::apply`] が新しい設定で組み直して差し替える（allowlist・上限を
//!   締める、off へ戻す、off から有効にする、のどれも再起動なしで効く）。差し替え前に始まった評価は
//!   古い設定のまま最後まで走る。組めなければ fail-closed（off）にする。
//! - 日次上限は sidecar 用に daemon 内で数える（[`SidecarDailyBudget`]。UTC 日・reload を跨いで保持）。
//!   実行 shadow の共有 DB 予約（migration 0049）は種類を区別せず数えるので、estimator の呼び出しで
//!   実行 shadow の上限を食わないよう分けている。
//! - prompt: `send_prompt = true` のときだけ descriptor が prompt を要すると宣言し、`prompt_allowlist` の
//!   対象だけ prompt を渡す。対象外は client の gate で送る前に止まる（送信 0、`privacy` で dropped）。
//! - 外部依存: daemon は `allow_external_dependencies = false` で組む。宣言と違う依存を返した応答は
//!   評価不能（`dependency_mismatch`）で、比較にも使わない。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use llm_proxy::estimator_shadow::{EstimatorShadow, EstimatorShadowConfig, EstimatorShadowSlot};
use llm_proxy::estimator_sidecar::{MonotonicClock, SidecarClientConfig, SidecarEstimatorClient};
use llm_proxy::legacy_catalog::LegacyCatalog;
use llm_proxy::reservation::Clock;
use llm_proxy::shadow::{ShadowBudget, ShadowSink};
use task_core::model_router::estimator::sidecar::{EstimatorDependencies, EstimatorDescriptor};
use task_core::model_router::shadow::{
    ShadowDailyCaps, ShadowPolicy, ShadowReason, ShadowReservation, ShadowReservationRequest,
    ShadowSettlement,
};
use time::OffsetDateTime;

use crate::config::SidecarEntry;

/// sidecar 1 回を日次上限に数える名目の effective 費用（USD）。推論費用は計測できないので、
/// request 上限と同じ数で上限に当たるよう 1 回 = この値で数える（ゼロにしない）。
pub(crate) const SIDECAR_NOMINAL_CALL_USD: f64 = 0.000_001;

/// 配線に要るもの（試験は偽の時計を渡す）。
pub(crate) struct SidecarDeps {
    pub sink: Arc<dyn ShadowSink>,
    pub clock: Arc<dyn Clock>,
    pub monotonic: Arc<dyn MonotonicClock>,
    /// 記録の持ち主（監査用）。
    pub owner: String,
}

/// proxy の estimator shadow の差し替え（起動時と reload）。
pub struct EstimatorSidecarControl {
    slot: Arc<EstimatorShadowSlot>,
    deps: SidecarDeps,
    budget: Arc<SidecarDailyBudget>,
    /// 直近に適用した設定（同じ設定の reload では作り直さず、circuit の状態を保つ）。
    applied: Mutex<Option<SidecarEntry>>,
}

impl std::fmt::Debug for EstimatorSidecarControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EstimatorSidecarControl")
            .field("active", &self.active())
            .finish_non_exhaustive()
    }
}

impl EstimatorSidecarControl {
    pub(crate) fn new(deps: SidecarDeps) -> Self {
        let budget = Arc::new(SidecarDailyBudget::new());
        Self {
            slot: Arc::new(EstimatorShadowSlot::default()),
            deps,
            budget,
            applied: Mutex::new(None),
        }
    }

    /// proxy に渡す差し替え口（`ProxyState::with_estimator_shadow_slot`）。
    pub(crate) fn slot(&self) -> Arc<EstimatorShadowSlot> {
        Arc::clone(&self.slot)
    }

    /// estimator shadow が入っているか（client があるか）。
    pub(crate) fn active(&self) -> bool {
        self.slot.current().is_some()
    }

    /// 設定を適用する。off（`enabled = false`）なら外し、有効なら組み直して差し替える。組めなければ
    /// warn を出して外す（fail-closed）。戻り値は適用後に有効か。
    pub(crate) fn apply(&self, entry: &SidecarEntry, catalog: Arc<LegacyCatalog>) -> bool {
        let mut applied = self.applied.lock().unwrap_or_else(|e| e.into_inner());
        if !entry.enabled {
            self.slot.set(None);
            *applied = Some(entry.clone());
            return false;
        }
        if applied.as_ref() == Some(entry) && self.active() {
            return true;
        }
        match self.build(entry, catalog) {
            Ok(shadow) => {
                self.slot.set(Some(shadow));
                *applied = Some(entry.clone());
                true
            }
            Err(error) => {
                tracing::warn!(%error, "model_routing.estimator.sidecar: not enabled");
                self.slot.set(None);
                *applied = None;
                false
            }
        }
    }

    fn build(
        &self,
        entry: &SidecarEntry,
        catalog: Arc<LegacyCatalog>,
    ) -> Result<Arc<EstimatorShadow>, String> {
        let (config, client_config) = shadow_settings(entry)?;
        let caps = config
            .policy
            .daily_caps()
            .ok_or("estimator sidecar policy has no daily caps")?;
        self.budget.set_caps(caps);
        let client = SidecarEstimatorClient::new(client_config, Arc::clone(&self.deps.monotonic))?;
        EstimatorShadow::new(
            config,
            Arc::new(client),
            catalog,
            self.deps.owner.clone(),
            Arc::clone(&self.budget) as Arc<dyn ShadowBudget>,
            Arc::clone(&self.deps.sink),
            Arc::clone(&self.deps.clock),
        )
        .map_err(|e| e.to_string())
    }
}

/// 検証済みの `SidecarEntry` から estimator shadow と client の設定を組む。
pub(crate) fn shadow_settings(
    entry: &SidecarEntry,
) -> Result<(EstimatorShadowConfig, SidecarClientConfig), String> {
    if !entry.shadow_only {
        return Err("shadow_only must be true".into());
    }
    let endpoint = entry
        .endpoint
        .as_deref()
        .ok_or("enabled requires endpoint")?;
    let endpoint = reqwest::Url::parse(endpoint).map_err(|e| format!("invalid endpoint: {e}"))?;
    let (Some(id), Some(version)) = (&entry.estimator_id, &entry.estimator_version) else {
        return Err("enabled requires estimator_id and estimator_version".into());
    };
    let max_requests = entry
        .daily_max_requests
        .filter(|n| *n > 0)
        .ok_or("enabled requires positive daily_max_requests")?;
    let max_inflight = entry.max_inflight.max(1);
    let descriptor = EstimatorDescriptor {
        estimator_id: id.clone(),
        version: version.clone(),
        protocol_version: entry.protocol_version,
        // prompt を送ってよい設定のときだけ「prompt を要する」と宣言する。対象外の要求は送る前に止まる。
        needs_prompt: entry.send_prompt,
        dependencies: EstimatorDependencies {
            needs_network: false,
            external_embeddings: false,
        },
    };
    let mut client = SidecarClientConfig::new(endpoint, descriptor);
    client.timeout = Duration::from_millis(entry.timeout_ms);
    client.max_inflight = usize::try_from(max_inflight).unwrap_or(usize::MAX);
    client.max_payload_bytes = entry.max_payload_bytes;
    client.send_prompt = entry.send_prompt;
    client.allow_external_dependencies = false;
    let policy = ShadowPolicy {
        execute: true,
        allowlist: entry.allowlist.clone(),
        sample_rate: 1.0,
        daily_max_requests: Some(max_requests),
        daily_max_tokens: Some(max_requests),
        daily_max_effective_usd: Some(max_requests as f64 * SIDECAR_NOMINAL_CALL_USD),
        max_concurrency: Some(max_inflight),
        max_queue_depth: Some(max_inflight),
        timeout_ms: Some(entry.timeout_ms),
    };
    let config = EstimatorShadowConfig {
        shadow_only: true,
        policy,
        worst_call_effective_usd: Some(SIDECAR_NOMINAL_CALL_USD),
        worst_call_tokens: 1,
        resource_group: None,
        prompt_allowlist: if entry.send_prompt {
            entry.prompt_allowlist.clone()
        } else {
            Default::default()
        },
    };
    Ok((config, client))
}

/// sidecar 呼び出しの日次上限（UTC 日、daemon 内）。予約した時点で 1 回と数え、結果に関わらず戻さない。
pub struct SidecarDailyBudget {
    state: Mutex<BudgetState>,
}

#[derive(Default)]
struct BudgetState {
    caps: Option<ShadowDailyCaps>,
    day: Option<time::Date>,
    requests: u64,
    next_id: u64,
}

impl SidecarDailyBudget {
    fn new() -> Self {
        Self {
            state: Mutex::new(BudgetState::default()),
        }
    }

    fn set_caps(&self, caps: ShadowDailyCaps) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).caps = Some(caps);
    }

    /// 直近に数えた UTC 日の呼び出し数。
    #[cfg(test)]
    pub(crate) fn used(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
    }
}

impl ShadowBudget for SidecarDailyBudget {
    fn reserve(
        &self,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> ShadowReservation {
        if request
            .worst_effective_usd
            .is_none_or(|usd| !usd.is_finite() || usd < 0.0)
        {
            return ShadowReservation::Denied(ShadowReason::UnknownCost);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(caps) = state.caps else {
            return ShadowReservation::Denied(ShadowReason::CapExceeded);
        };
        let today = now.to_offset(time::UtcOffset::UTC).date();
        if state.day != Some(today) {
            state.day = Some(today);
            state.requests = 0;
        }
        if state.requests.saturating_add(1) > caps.max_requests {
            return ShadowReservation::Denied(ShadowReason::CapExceeded);
        }
        state.requests += 1;
        state.next_id += 1;
        ShadowReservation::Reserved {
            reservation_id: format!("sidecar-{today}-{}", state.next_id),
            day: today.to_string(),
        }
    }

    fn settle(&self, _reservation_id: &str, _settlement: ShadowSettlement) {
        // 予約時に最悪値（1 回）で数え済み。結果で戻さない。
    }
}

/// proxy の catalog: config の検証済み routing catalog（無ければ legacy config から導く）。
pub(crate) fn proxy_catalog(config: &crate::Config) -> Arc<LegacyCatalog> {
    Arc::new(match &config.routing_catalog_snapshot {
        Some(catalog) => LegacyCatalog {
            models: catalog.models.clone(),
            deployments: catalog.deployments.clone(),
            policies: catalog.policies.clone(),
        },
        None => llm_proxy::legacy_catalog::normalize_legacy_config(&config.llm_proxy),
    })
}

/// reload で新しい `[model_routing.estimator.sidecar]` を適用する（proxy に配線していなければ何もしない）。
pub(crate) fn reload_estimator_sidecar(config: &crate::Config) {
    if let Some(control) = &config.model_routing.estimator_sidecar_control {
        let active = control.apply(
            &config.model_routing.estimator.sidecar,
            proxy_catalog(config),
        );
        tracing::info!(active, "model_routing.estimator.sidecar reloaded");
    }
}

#[cfg(test)]
#[path = "routing_sidecar_tests.rs"]
mod tests;
