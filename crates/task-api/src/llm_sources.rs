//! `GET /llm/sources`（ADR-0053 D4。API は Phase 65 で用意し、GUI 表示は Phase 66）。
//!
//! task-api は `llm-proxy`（`crates/llm-proxy`）を知らない（ADR-0017 M2 と同じ境界: task-api は
//! `task-worker`/`task-dispatch` に依存しない。celeris がトレイト実装として渡す）。
//! 到達性の probe はネットワーク I/O なので、`ReleaseSource` の同期版ではなく非同期トレイトにする。

use std::sync::Arc;

use axum::extract::{RawQuery, State};

use crate::handlers::{ApiResult, json_response};
use crate::problem::ApiProblem;
use crate::state::ApiState;
use crate::types::{
    LlmSourceBilledCostView, LlmSourceCostView, LlmSourceFreshnessView,
    LlmSourceOpportunityCostView, LlmSourceStateView, LlmSourcesView,
};
use task_core::model_router::cost::{CostEstimate, pressure as state_pressure};
use task_core::model_router::profiles::{Reachability, SourceState};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// celeris が `llm_proxy::ProxyState` を包んで渡す（`docs/guides/llm-source.md`）。
#[async_trait::async_trait]
pub trait LlmSourcesReader: Send + Sync + 'static {
    /// `now` は Unix 秒。
    async fn view(&self, now: i64) -> LlmSourcesView;
}

pub type SharedLlmSourcesReader = Arc<dyn LlmSourcesReader>;

/// `GET /llm/sources`。トークン必須（他の観測系と同じ）。`[llm_proxy]` が無効なら 409。
pub(crate) async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    let Some(reader) = state.inner.llm_sources.clone() else {
        return Err(ApiProblem::llm_proxy_unavailable());
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let view = reader.view(now).await;
    Ok(json_response(axum::http::StatusCode::OK, &view))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::get;
    axum::Router::new().route("/api/v1/llm/sources", get(list))
}

/// ADR 2026-10-04-multi-objective-model-routing Phase 2: `SourceState` と effective cost の見積もりを
/// `GET /llm/sources` の `deployments[]` に写す純粋関数（`now` は Unix 秒。同じ入力から同じ JSON）。
/// 未知は `null` のまま `unknown` に名前を残し、0 で埋めない。請求（cash）と機会費用
/// （shadow・resource）は別の欄に置く。`cost` が無ければ費用は全部未知。
pub fn source_state_view(
    state: &SourceState,
    cost: Option<&CostEstimate>,
    concurrency_limit: Option<u32>,
    now: i64,
) -> LlmSourceStateView {
    let parse = |raw: Option<&str>| raw.and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
    let observed = parse(state.observed_at.as_deref());
    let expires = parse(state.expires_at.as_deref());
    let age_secs = observed.map(|t| now - t.unix_timestamp());
    let stale = observed.is_none() || expires.is_some_and(|t| t.unix_timestamp() <= now);
    let quota_remaining = state
        .quota_windows
        .iter()
        .filter_map(|w| w.remaining)
        .reduce(f64::min)
        .or(state.quota_remaining);
    let quota_reset_at = state
        .quota_windows
        .iter()
        .filter_map(|w| {
            w.reset_at
                .as_deref()
                .and_then(|r| parse(Some(r)).map(|t| (t, r)))
        })
        .min_by_key(|(t, _)| *t)
        .map(|(_, r)| r.to_string());
    let pressure = match cost {
        Some(c) => c.pressure,
        None => state_pressure(state, concurrency_limit).ok().flatten(),
    };
    let mut unknown: Vec<String> = Vec::new();
    let mut mark = |known: bool, name: &str| {
        if !known {
            unknown.push(name.to_string());
        }
    };
    mark(observed.is_some(), "observed_at");
    mark(state.latency_ms.is_some(), "latency");
    mark(quota_remaining.is_some(), "quota");
    mark(quota_reset_at.is_some(), "quota_reset");
    mark(pressure.is_some(), "pressure");
    mark(cost.and_then(|c| c.cash_usd).is_some(), "cash");
    mark(
        cost.and_then(|c| c.subscription_shadow_usd).is_some(),
        "shadow",
    );
    mark(
        cost.and_then(|c| c.self_host_resource_usd).is_some(),
        "resource",
    );
    mark(cost.and_then(|c| c.effective_usd).is_some(), "effective");
    unknown.sort();
    LlmSourceStateView {
        deployment_id: state.deployment_id.clone(),
        freshness: LlmSourceFreshnessView {
            observed_at: state.observed_at.clone(),
            age_secs,
            expires_at: state.expires_at.clone(),
            stale,
        },
        reachability: match state.reachability {
            Reachability::Up => "up",
            Reachability::Down => "down",
            Reachability::Unknown => "unknown",
        }
        .to_string(),
        latency_ms: state.latency_ms,
        quota_remaining,
        quota_reset_at,
        pressure,
        unknown,
        cost: cost.map(|c| LlmSourceCostView {
            billed: LlmSourceBilledCostView {
                cash_usd: c.cash_usd,
            },
            opportunity: LlmSourceOpportunityCostView {
                shadow_usd: c.subscription_shadow_usd,
                resource_usd: c.self_host_resource_usd,
            },
            effective_usd: c.effective_usd,
            assumptions: c.assumptions.clone(),
        }),
    }
}
