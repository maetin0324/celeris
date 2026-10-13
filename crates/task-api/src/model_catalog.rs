//! ADR 2026-10-06 D5: モデル catalog の API。
//!
//! - `GET /api/v1/llm/models` — catalog 全件（source・model_id 昇順）と上書き・source ごとの最終発見記録。
//! - `PUT /api/v1/llm/models/{source}/{model_id}/override` — 上書き（`disabled` / `tier` / `alias` / `note`）→ 200 項目。
//! - `DELETE /api/v1/llm/models/{source}/{model_id}/override` — 上書きを消す → 204（無ければ 404）。
//! - `POST /api/v1/llm/models/discover` — 発見を今すぐ走らせる（任意で `source`）→ 202。発見そのもの
//!   （コマンド実行・HTTP）は daemon の `ModelDiscoveryHook` が行う。無ければ `results: []`・`unavailable: true`。
//!
//! `model_id` に `/` が入る場合（self-host の `Qwen/Qwen3` 等）は `%2F` で符号化する。LLM は呼ばない。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::model_catalog::{
    CatalogDelta, CatalogEntry, CatalogOverride, CatalogOverrideRow, CatalogSource, DiscoveryRecord,
};
use task_core::{ModelCatalogStore, Tier};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;

/// 上書きの本文・応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalogOverrideView {
    #[serde(default)]
    pub disabled: bool,
    /// catalog 上の lane annotation（無ければ `null`）。候補選択の順序には使わない。
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ModelCatalogItem {
    pub source: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub available: bool,
    /// RFC3339（UTC）。
    pub first_seen: String,
    pub last_seen: String,
    pub capabilities: serde_json::Value,
    /// 人の上書き（無ければ `null`）。
    #[serde(rename = "override")]
    pub override_: Option<ModelCatalogOverrideView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DiscoveryRecordView {
    pub source: String,
    pub at: String,
    pub ok: bool,
    pub error: Option<String>,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ModelCatalogView {
    pub items: Vec<ModelCatalogItem>,
    pub last_discovery: Vec<DiscoveryRecordView>,
}

/// 発見 1 source の結果の要約。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DiscoverySummaryView {
    pub source: String,
    pub ok: bool,
    pub count: u32,
    pub error: Option<String>,
    /// 失敗したときは空の delta。
    pub delta: CatalogDelta,
}

/// `POST /llm/models/discover` の本文。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoverBody {
    /// 1 source だけ走らせる（省略で全 source）。
    #[serde(default)]
    pub source: Option<String>,
}

/// `POST /llm/models/discover` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DiscoverResponse {
    pub results: Vec<DiscoverySummaryView>,
    /// daemon が発見の係を渡していない（この process では発見できない）とき `true`。
    pub unavailable: bool,
}

/// daemon が持つ発見の実行（コマンド実行・HTTP は celeris 側。task-api は呼ぶだけ）。
pub trait ModelDiscoveryHook: Send + Sync + 'static {
    fn discover<'a>(
        &'a self,
        source: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<DiscoverySummaryView>> + Send + 'a>>;
}

pub type SharedModelDiscoveryHook = Arc<dyn ModelDiscoveryHook>;

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post, put};
    axum::Router::new()
        .route("/api/v1/llm/models", get(list_models))
        .route("/api/v1/llm/models/discover", post(discover))
        .route(
            "/api/v1/llm/models/{source}/{model_id}/override",
            put(put_override).delete(delete_override),
        )
}

pub(crate) fn rfc3339(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| unix.to_string())
}

fn override_view(value: &CatalogOverride) -> ModelCatalogOverrideView {
    ModelCatalogOverrideView {
        disabled: value.disabled,
        tier: value.tier,
        alias: value.alias.clone(),
        note: value.note.clone(),
    }
}

fn item(entry: &CatalogEntry, overrides: &[CatalogOverrideRow]) -> ModelCatalogItem {
    let ov = overrides
        .iter()
        .find(|o| o.source == entry.source && o.model_id == entry.model_id);
    ModelCatalogItem {
        source: entry.source.0.clone(),
        model_id: entry.model_id.clone(),
        display_name: entry.display_name.clone(),
        available: entry.available,
        first_seen: rfc3339(entry.first_seen),
        last_seen: rfc3339(entry.last_seen),
        capabilities: entry.capabilities.clone(),
        override_: ov.map(|o| override_view(&o.value)),
    }
}

fn record_view(r: &DiscoveryRecord) -> DiscoveryRecordView {
    DiscoveryRecordView {
        source: r.source.0.clone(),
        at: rfc3339(r.at),
        ok: r.ok,
        error: r.error.clone(),
        count: r.count,
    }
}

fn parse_source(raw: &str) -> Result<CatalogSource, ApiProblem> {
    let source = CatalogSource::new(raw);
    if source.is_valid() {
        Ok(source)
    } else {
        Err(ApiProblem::bad_request(format!(
            "unknown model source {raw:?} (expected claude-oauth, codex-oauth, opencode-go or openai-compatible:<id>)"
        )))
    }
}

async fn list_models(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let view = state
        .blocking(move |store| {
            let entries = store.model_catalog_list().map_err(store_problem)?;
            let overrides = store.model_catalog_overrides().map_err(store_problem)?;
            let records = store
                .model_catalog_discovery_records()
                .map_err(store_problem)?;
            Ok(ModelCatalogView {
                items: entries.iter().map(|e| item(e, &overrides)).collect(),
                last_discovery: records.iter().map(record_view).collect(),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// `PUT /llm/models/{source}/{model_id}/override`, shared by the handler and CoS
/// (`model_override.put`, written with its audit record in one transaction).
pub(crate) fn put_override_op(
    store: &task_core::store::SqliteStore,
    raw_source: &str,
    model_id: &str,
    payload: ModelCatalogOverrideView,
    audit: Option<&OperationAudit>,
) -> Result<Applied<ModelCatalogItem>, ApiProblem> {
    let target = format!("{raw_source}/{model_id}");
    let checked = parse_source(raw_source).and_then(|source| {
        if model_id.trim().is_empty() {
            Err(ApiProblem::bad_request("model_id is empty"))
        } else {
            Ok(source)
        }
    });
    let source = match (checked, audit) {
        (Ok(source), _) => source,
        (Err(problem), Some(audit)) => {
            return Err(audit.reject(store, "model_override", &target, problem));
        }
        (Err(problem), None) => return Err(problem),
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let value = CatalogOverride {
        disabled: payload.disabled,
        tier: payload.tier,
        alias: payload.alias,
        note: payload.note,
    };
    let read_item =
        |store: &task_core::store::SqliteStore| -> Result<ModelCatalogItem, ApiProblem> {
            let overrides = store.model_catalog_overrides().map_err(store_problem)?;
            let entry = store
                .model_catalog_list()
                .map_err(store_problem)?
                .into_iter()
                .find(|e| e.source == source && e.model_id == model_id)
                // catalog に無いモデルにも上書きは置ける（発見前に決める）。その場合は未発見の行として返す。
                .unwrap_or_else(|| CatalogEntry {
                    source: source.clone(),
                    model_id: model_id.to_string(),
                    display_name: None,
                    first_seen: now,
                    last_seen: now,
                    available: false,
                    capabilities: serde_json::json!({}),
                });
            Ok(item(&entry, &overrides))
        };
    let Some(audit) = audit else {
        store
            .model_catalog_set_override(&source, model_id, &value, now)
            .map_err(store_problem)?;
        return Ok(Applied::Direct(read_item(store)?));
    };
    let operation = audit.apply_checked(store, "model_override", &target, "model_override.put", |tx| {
        task_core::store::SqliteStore::model_catalog_set_override_tx(tx, &source, model_id, &value, now)
            .map_err(store_problem)?;
        Ok(serde_json::json!({"source": source.as_str(), "model_id": model_id, "override": value}))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `DELETE /llm/models/{source}/{model_id}/override` (`model_override.delete`). No override is 404.
pub(crate) fn delete_override_op(
    store: &task_core::store::SqliteStore,
    raw_source: &str,
    model_id: &str,
    audit: Option<&OperationAudit>,
) -> Result<Applied<()>, ApiProblem> {
    let target = format!("{raw_source}/{model_id}");
    let missing = || {
        ApiProblem::new(
            StatusCode::NOT_FOUND,
            "model_override_not_found",
            "no override for that model",
        )
    };
    let Some(audit) = audit else {
        let source = parse_source(raw_source)?;
        return if store
            .model_catalog_delete_override(&source, model_id)
            .map_err(store_problem)?
        {
            Ok(Applied::Direct(()))
        } else {
            Err(missing())
        };
    };
    let source =
        parse_source(raw_source).map_err(|p| audit.reject(store, "model_override", &target, p))?;
    let operation = audit.apply_checked(store, "model_override", &target, "model_override.delete", |tx| {
        if !task_core::store::SqliteStore::model_catalog_delete_override_tx(tx, &source, model_id)
            .map_err(store_problem)?
        {
            return Err(missing());
        }
        Ok(serde_json::json!({"source": source.as_str(), "model_id": model_id, "deleted": true}))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

async fn put_override(
    State(state): State<ApiState>,
    Params((source, model_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    parse_source(&source)?;
    if model_id.trim().is_empty() {
        return Err(ApiProblem::bad_request("model_id is empty"));
    }
    let payload: ModelCatalogOverrideView = read_json(body, true).await?;
    let result = state
        .blocking(move |store| put_override_op(store, &source, &model_id, payload, None)?.direct())
        .await?;
    tracing::info!(op = "model_catalog_override_set", source = %result.source, model_id = %result.model_id, "model catalog override set");
    Ok(json_response(StatusCode::OK, &result))
}

async fn delete_override(
    State(state): State<ApiState>,
    Params((source, model_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    parse_source(&source)?;
    state
        .blocking(move |store| delete_override_op(store, &source, &model_id, None)?.direct())
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn discover(State(state): State<ApiState>, RawQuery(raw): RawQuery, body: Body) -> ApiResult {
    no_query(&raw)?;
    let payload: DiscoverBody = read_json(body, true).await?;
    let response = crate::cos::operations::effect_result(discover_effect(&state, payload).await)?;
    Ok(json_response(StatusCode::ACCEPTED, &response))
}

/// Run discovery now (shared with CoS `model_catalog.discover`, an external effect: the daemon's
/// hook runs commands and HTTP requests). An unknown source is refused before the hook runs.
pub(crate) async fn discover_effect(
    state: &ApiState,
    payload: DiscoverBody,
) -> crate::cos::operations::Effect<DiscoverResponse> {
    if let Some(source) = &payload.source
        && let Err(problem) = parse_source(source)
    {
        return Ok(Err(problem));
    }
    Ok(Ok(match &state.inner.model_discovery {
        Some(hook) => DiscoverResponse {
            results: hook.discover(payload.source).await,
            unavailable: false,
        },
        None => DiscoverResponse {
            results: Vec::new(),
            unavailable: true,
        },
    }))
}
