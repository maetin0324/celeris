//! モデルごとの複数役割・優先度の API（ADR 2026-10-06 model-role-assignments 付記）。
//! 集合の更新は `PUT …/assignments/roles/{tier}`、preview は `POST …/roles/{tier}/preview`。
//! 以下の単体 API は旧 client の互換用として保持する。
//!
//! - `GET /api/v1/llm/models/assignments` — 割り当て（実効の状態つき）と、全 `(source, tier)` の実効の枠（`effective`）。
//! - `PUT /api/v1/llm/models/assignments/{source}/{tier}` — 割り当てを置く → 200 `{ item, impact }`。
//! - `DELETE /api/v1/llm/models/assignments/{source}/{tier}` — 外す → 204（無ければ 404）。
//! - `POST /api/v1/llm/models/assignments/preview` — 書かずに影響（impact）だけ返す。
//!
//! 起源（origin）は割り当て表で決める。割り当てがあれば `assignment`、無ければ config の値（`config`）、
//! どちらも無ければ `null`。provider の config の値は provider の一覧（`tier_models[lane]` → `model`）から、
//! llm-proxy の lane は routing catalog の `legacy:*` deployment から取る。routing catalog の hook が割り当てを
//! 既に反映していても provider の値は変わらない。proxy の lane は割り当て中に config の値を判別できないとき、
//! 解除後の `after` を `null` にする（catalog の値が割り当てと違うときだけ示す）。
//! LLM は呼ばない。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::model_catalog::assignments::{
    AssignmentState, AssignmentView, EffectiveAssignment, LLM_PROXY_ADAPTER, RoleAssignment,
    RoleMember, WireRule, source_name_for_llm_source, tier_from_str, tier_str,
};
use task_core::model_catalog::{CatalogEntry, CatalogSource};
use task_core::{ModelCatalogStore, Tier};

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::model_catalog::{normalize_source, rfc3339};
use crate::problem::{ApiProblem, store_problem};
use crate::routing_catalog::RoutingCatalogView;
use crate::state::ApiState;
use crate::types::ProviderConfigView;

/// 割り当ての書き手（管理 API の呼び手。approvals の `who = "admin"` と同じ）。
const ACTOR: &str = "admin";

const TIERS: [Tier; 3] = [Tier::Frontier, Tier::Standard, Tier::Cheap];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentStateView {
    Assigned,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotOrigin {
    Assignment,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImpactKind {
    Provider,
    Proxy,
}

/// 割り当て 1 件（実効の状態つき）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EffectiveAssignmentView {
    pub source: String,
    pub tier: Tier,
    pub model_id: String,
    pub priority: u32,
    pub state: AssignmentStateView,
    /// `override:disabled` か `catalog:unavailable`（`state = excluded` のとき）。
    pub excluded_reason: Option<String>,
    pub note: Option<String>,
    /// RFC3339（UTC）。
    pub updated_at: String,
    pub updated_by: String,
}

/// 1 つの `(source, tier)` の枠の実効。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RoleSlotView {
    pub source: String,
    pub tier: Tier,
    pub model_id: Option<String>,
    pub priority: u32,
    pub origin: Option<SlotOrigin>,
    pub excluded_reason: Option<String>,
    /// この枠を使う provider の id（昇順）。
    pub providers: Vec<String>,
    /// llm-proxy の lane がこの枠を使うか。
    pub proxy: bool,
    /// catalog に行があるときだけ。
    pub available: Option<bool>,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ImpactChangeView {
    pub kind: ImpactKind,
    pub id: String,
    pub tier: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub excluded_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ImpactView {
    pub changes: Vec<ImpactChangeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AssignmentList {
    pub items: Vec<EffectiveAssignmentView>,
    pub effective: Vec<RoleSlotView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AssignmentPutResponse {
    pub item: EffectiveAssignmentView,
    pub impact: ImpactView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct AssignmentPreviewResponse {
    pub impact: ImpactView,
}

/// `PUT …/assignments/{source}/{tier}` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentPutBody {
    pub model_id: String,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST …/assignments/preview` の本文。`model_id` が `null`（省略）なら解除した場合の影響。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentPreviewBody {
    pub source: String,
    pub tier: Tier,
    #[serde(default)]
    pub model_id: Option<String>,
}

/// Complete membership of one role, in all sources. An empty list disables the role.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoleMembersBody {
    pub members: Vec<RoleMember>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RoleMembersResponse {
    pub before: Vec<RoleMember>,
    pub after: Vec<RoleMember>,
    pub impact: ImpactView,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/llm/models/assignments", get(list_assignments))
        .route("/api/v1/llm/models/assignments/preview", post(preview))
        .route(
            "/api/v1/llm/models/assignments/roles/{tier}",
            axum::routing::put(replace_role),
        )
        .route(
            "/api/v1/llm/models/assignments/roles/{tier}/preview",
            post(preview_role),
        )
        .route(
            "/api/v1/llm/models/assignments/{source}/{tier}",
            axum::routing::put(put_assignment).delete(delete_assignment),
        )
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

fn parse_tier(raw: &str) -> Result<Tier, ApiProblem> {
    tier_from_str(raw).ok_or_else(|| {
        ApiProblem::bad_request(format!(
            "unknown tier {raw:?} (expected frontier, standard or cheap)"
        ))
    })
}

fn model_not_in_catalog(source: &CatalogSource, model_id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::BAD_REQUEST,
        "model_not_in_catalog",
        format!(
            "model not in catalog: {model_id:?} is not a known model of {source:?}; run discovery first"
        ),
    )
}

/// CoS の `PUT …/assignments/{source}/{tier}`（ADR 2026-10-09 D3/D5、`model_assignment.put`）。人の `PUT` と
/// 同じ検証（source・tier・空でない model_id・catalog にある model）の後、割り当てと
/// `ModelRoleAssignmentChanged`（actor = cos）を監査と同じ transaction で書く。
pub(crate) fn put_assignment_audited(
    store: &task_core::store::SqliteStore,
    raw_source: &str,
    raw_tier: &str,
    payload: AssignmentPutBody,
    audit: &crate::cos::operations::OperationAudit,
) -> Result<task_core::chat::CosOperation, ApiProblem> {
    let target_id = format!("{raw_source}/{raw_tier}");
    let reject = |problem| audit.reject(store, "model_assignment", &target_id, problem);
    let source = parse_source(raw_source).map_err(reject)?;
    let tier = parse_tier(raw_tier).map_err(reject)?;
    if payload.model_id.trim().is_empty() {
        return Err(reject(ApiProblem::bad_request("model_id is empty")));
    }
    let entries = store
        .model_catalog_list()
        .map_err(|e| reject(store_problem(e)))?;
    if !entries
        .iter()
        .any(|e| e.source == source && e.model_id == payload.model_id)
    {
        return Err(reject(model_not_in_catalog(&source, &payload.model_id)));
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    audit.apply(
        store,
        "model_assignment",
        &target_id,
        "model_assignment.put",
        |tx| {
            let item = task_core::store::SqliteStore::model_role_assignment_set_tx(
                tx,
                &source,
                tier,
                &payload.model_id,
                payload.note.as_deref(),
                "cos",
                now,
            )?;
            Ok(serde_json::json!({
                "source": item.source.as_str(),
                "tier": tier_str(tier),
                "model_id": item.model_id,
            }))
        },
    )
}

/// CoS の `DELETE …/assignments/{source}/{tier}`（ADR 2026-10-09 D3/D5、`model_assignment.delete`）。無い割り当ては
/// 人の経路と同じく 404 で、理由付きの rejected 行として記録する。
pub(crate) fn delete_assignment_audited(
    store: &task_core::store::SqliteStore,
    raw_source: &str,
    raw_tier: &str,
    audit: &crate::cos::operations::OperationAudit,
) -> Result<task_core::chat::CosOperation, ApiProblem> {
    let target_id = format!("{raw_source}/{raw_tier}");
    let reject = |problem| audit.reject(store, "model_assignment", &target_id, problem);
    let source = parse_source(raw_source).map_err(reject)?;
    let tier = parse_tier(raw_tier).map_err(reject)?;
    let present = store
        .model_role_assignments()
        .map_err(|e| reject(store_problem(e)))?
        .iter()
        .any(|a| a.source == source && a.tier == tier);
    if !present {
        return Err(reject(ApiProblem::new(
            StatusCode::NOT_FOUND,
            "model_assignment_not_found",
            "no assignment for that source and tier",
        )));
    }
    audit.apply(store, "model_assignment", &target_id, "model_assignment.delete", |tx| {
        if !task_core::store::SqliteStore::model_role_assignment_delete_tx(tx, &source, tier, "cos")? {
            return Err(task_core::chat::ChatError::NotFound {
                kind: "model_assignment",
                id: target_id.clone(),
            });
        }
        Ok(serde_json::json!({"source": source.as_str(), "tier": tier_str(tier), "deleted": true}))
    })
}

fn assignment_item(a: &EffectiveAssignment) -> EffectiveAssignmentView {
    let (state, excluded_reason) = match a.state {
        AssignmentState::Assigned => (AssignmentStateView::Assigned, None),
        AssignmentState::Excluded { reason } => {
            (AssignmentStateView::Excluded, Some(reason.to_string()))
        }
    };
    EffectiveAssignmentView {
        source: a.source.0.clone(),
        tier: a.tier,
        model_id: a.model_id.clone(),
        priority: a.priority,
        state,
        excluded_reason,
        note: a.note.clone(),
        updated_at: rfc3339(a.updated_at),
        updated_by: a.updated_by.clone(),
    }
}

/// 1 つの枠を使う側（provider 1 件か proxy の lane）と、その config 由来の wire（接頭辞付きのまま）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Participant {
    kind: ImpactKind,
    id: String,
    /// 行の adapter（proxy は `LLM_PROXY_ADAPTER`、一覧に無い provider は空 = 不明）。
    adapter: String,
    config_model: Option<String>,
}

impl Participant {
    /// 付記 2026-10-07 wire-prefix: dispatcher・llm-proxy・routing catalog と同じ規則で、割り当ての `model_id` から
    /// この枠の実行用のモデル名を作る。
    fn wire<'a>(&'a self, source: &'a str) -> WireRule<'a> {
        WireRule::new(source, &self.adapter, self.config_model.as_deref())
    }
}

fn provider_source(p: &ProviderConfigView) -> Option<String> {
    p.llm_source
        .as_ref()
        .and_then(|r| source_name_for_llm_source(&r.source))
}

/// provider 行の config 由来の wire（`tier_models[lane].model_id`（無ければ `name`）→ `model`）。接頭辞は外さない。
fn provider_config_model(p: &ProviderConfigView, tier: Tier) -> Option<String> {
    let from_lane = p
        .tier_models
        .get(&tier)
        .filter(|b| b.unavailable_reason.is_none())
        .and_then(|b| b.model_id.clone().or_else(|| Some(b.name.clone())));
    from_lane
        .or_else(|| p.model.clone())
        .filter(|m| !m.is_empty())
}

/// `(source, tier)` を使う側を求める。providers は provider の一覧（`llm_source` と `tiers`）と
/// routing catalog の `provider:<id>[/<lane>]` deployment、proxy は `legacy:*` deployment。
fn participants(
    source: &str,
    tier: Tier,
    providers: &[ProviderConfigView],
    catalog: Option<&RoutingCatalogView>,
) -> Vec<Participant> {
    use std::collections::BTreeMap;
    // provider id → config 由来の model。provider の一覧（`tier_models[lane]` → `model`）から求める。
    // routing catalog の hook は割り当てを既に反映していることがあるので、provider の config 値には使わない。
    let mut provider_models: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for p in providers {
        if provider_source(p).as_deref() == Some(source) && p.tiers.contains(&tier) {
            provider_models.insert(
                p.id.clone(),
                (p.adapter.clone(), provider_config_model(p, tier)),
            );
        }
    }
    let mut proxies = Vec::new();
    if let Some(catalog) = catalog {
        for dep in &catalog.deployments {
            if normalize_source(&dep.source_ref) != source || !dep.allowed_lanes.contains(&tier) {
                continue;
            }
            if let Some(rest) = dep.id.strip_prefix("provider:") {
                // 一覧に無い provider（reload 前など）は id だけ足す。config の値は分からない。
                let pid = rest.split_once('/').map_or(rest, |(pid, _)| pid);
                provider_models
                    .entry(pid.to_string())
                    .or_insert((String::new(), None));
            } else if dep.id.starts_with("legacy:") {
                proxies.push(Participant {
                    kind: ImpactKind::Proxy,
                    id: dep.id.clone(),
                    adapter: LLM_PROXY_ADAPTER.to_string(),
                    config_model: Some(dep.upstream_model.clone()),
                });
            }
        }
    }
    let mut out: Vec<Participant> = provider_models
        .into_iter()
        .map(|(id, (adapter, config_model))| Participant {
            kind: ImpactKind::Provider,
            id,
            adapter,
            config_model,
        })
        .collect();
    proxies.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(proxies);
    out
}

type SlotValue = (Option<String>, Option<SlotOrigin>, Option<String>, u32);

fn effective_slots(
    view: &AssignmentView,
    entries: &[CatalogEntry],
    providers: &[ProviderConfigView],
    catalog: Option<&RoutingCatalogView>,
) -> Vec<RoleSlotView> {
    let mut sources: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    sources.extend(entries.iter().map(|e| e.source.0.clone()));
    sources.extend(providers.iter().filter_map(provider_source));
    if let Some(catalog) = catalog {
        sources.extend(
            catalog
                .deployments
                .iter()
                .map(|d| normalize_source(&d.source_ref)),
        );
    }
    sources.extend(view.items.iter().map(|a| a.source.0.clone()));
    let mut out = Vec::new();
    for source in sources
        .iter()
        .filter(|s| CatalogSource::new(s.as_str()).is_valid())
    {
        for tier in TIERS {
            let parts = participants(source, tier, providers, catalog);
            let mut members: Vec<SlotValue> = view
                .members(source, tier)
                .iter()
                .map(|a| {
                    (
                        Some(a.model_id.clone()),
                        Some(SlotOrigin::Assignment),
                        match a.state {
                            AssignmentState::Assigned => None,
                            AssignmentState::Excluded { reason } => Some(reason.to_string()),
                        },
                        a.priority,
                    )
                })
                .collect();
            if !view.manages(source, tier) {
                for part in &parts {
                    // config の wire から接頭辞を外した catalog の model_id（`qwen-local/qwen3.8-27b` → `qwen3.8-27b`）。
                    if let Some(model) =
                        part.config_model.as_deref().map(WireRule::catalog_model_id)
                        && !members
                            .iter()
                            .any(|(m, _, _, _)| m.as_deref() == Some(model))
                    {
                        members.push((Some(model.to_string()), Some(SlotOrigin::Config), None, 0));
                    }
                }
            }
            if members.is_empty() {
                members.push((None, None, None, 0));
            }
            for (model_id, origin, excluded_reason, priority) in members {
                let entry = model_id.as_ref().and_then(|m| {
                    entries
                        .iter()
                        .find(|e| e.source.as_str() == source && &e.model_id == m)
                });
                out.push(RoleSlotView {
                    source: source.clone(),
                    tier,
                    model_id,
                    priority,
                    origin,
                    excluded_reason,
                    providers: parts
                        .iter()
                        .filter(|p| p.kind == ImpactKind::Provider)
                        .map(|p| p.id.clone())
                        .collect(),
                    proxy: parts.iter().any(|p| p.kind == ImpactKind::Proxy),
                    available: entry.map(|e| e.available),
                    last_seen: entry.map(|e| rfc3339(e.last_seen)),
                });
            }
        }
    }
    out
}

/// `new_model`（`None` は解除）にしたときの影響。`current` は今の割り当て（あれば）。
#[allow(clippy::too_many_arguments)]
fn compute_impact(
    source: &CatalogSource,
    tier: Tier,
    new_model: Option<&str>,
    current: Option<&EffectiveAssignment>,
    entries: &[CatalogEntry],
    overrides: &[task_core::model_catalog::CatalogOverrideRow],
    providers: &[ProviderConfigView],
    catalog: Option<&RoutingCatalogView>,
) -> ImpactView {
    let excluded_reason = new_model.and_then(|m| {
        let synthetic = RoleAssignment {
            priority: 0,
            source: source.clone(),
            tier,
            model_id: m.to_string(),
            note: None,
            updated_at: 0,
            updated_by: ACTOR.to_string(),
        };
        AssignmentView::build(&[synthetic], entries, overrides)
            .items
            .first()
            .and_then(|a| match a.state {
                AssignmentState::Assigned => None,
                AssignmentState::Excluded { reason } => Some(reason.to_string()),
            })
    });
    let changes = participants(source.as_str(), tier, providers, catalog)
        .into_iter()
        .map(|p| {
            // 付記 2026-10-07 wire-prefix: before / after は枠が実際に受け取る実行用のモデル名（provider 行は
            // config の `<prefix>/` 付き、proxy は接頭辞なし）。
            let rule = p.wire(source.as_str());
            let before = match current {
                Some(a) => Some(rule.model(None, &a.model_id)),
                None => p.config_model.clone(),
            };
            let after = match new_model {
                Some(m) => Some(rule.model(None, m)),
                // 解除後は config の値。provider は provider の一覧の値で常に分かる。proxy の lane は
                // routing catalog の値しか無く、割り当てを既に反映している（割り当てと同じ値）ときは config の
                // 値を判別できないので `null`。
                None => match p.kind {
                    ImpactKind::Provider => p.config_model.clone(),
                    ImpactKind::Proxy => p
                        .config_model
                        .clone()
                        .filter(|m| current.is_none_or(|a| &a.model_id != m)),
                },
            };
            ImpactChangeView {
                kind: p.kind,
                id: p.id,
                tier: tier_str(tier).to_string(),
                before,
                after,
                excluded_reason: excluded_reason.clone(),
            }
        })
        .collect();
    ImpactView { changes }
}

fn current_providers(state: &ApiState) -> Vec<ProviderConfigView> {
    let snapshot = state.snapshot();
    crate::handlers::providers::current_providers(state, snapshot.as_ref())
}

async fn list_assignments(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let routing = state.inner.routing_catalog.as_ref().map(|r| r.view());
    let providers = current_providers(&state);
    let list = state
        .blocking(move |store| {
            let view = store.model_role_assignment_view().map_err(store_problem)?;
            let entries = store.model_catalog_list().map_err(store_problem)?;
            Ok(AssignmentList {
                items: view.items.iter().map(assignment_item).collect(),
                effective: effective_slots(&view, &entries, &providers, routing.as_ref()),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn put_assignment(
    State(state): State<ApiState>,
    Params((source, tier)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let source = parse_source(&source)?;
    let tier = parse_tier(&tier)?;
    let payload: AssignmentPutBody = read_json(body, false).await?;
    if payload.model_id.trim().is_empty() {
        return Err(ApiProblem::bad_request("model_id is empty"));
    }
    let routing = state.inner.routing_catalog.as_ref().map(|r| r.view());
    let providers = current_providers(&state);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let response = state
        .blocking(move |store| {
            let entries = store.model_catalog_list().map_err(store_problem)?;
            if !entries
                .iter()
                .any(|e| e.source == source && e.model_id == payload.model_id)
            {
                return Err(model_not_in_catalog(&source, &payload.model_id));
            }
            let overrides = store.model_catalog_overrides().map_err(store_problem)?;
            let before = store.model_role_assignment_view().map_err(store_problem)?;
            let impact = compute_impact(
                &source,
                tier,
                Some(&payload.model_id),
                before.get(source.as_str(), tier),
                &entries,
                &overrides,
                &providers,
                routing.as_ref(),
            );
            store
                .model_role_assignment_set(
                    &source,
                    tier,
                    &payload.model_id,
                    payload.note.as_deref(),
                    ACTOR,
                    now,
                )
                .map_err(store_problem)?;
            let after = store.model_role_assignment_view().map_err(store_problem)?;
            let item = after
                .get(source.as_str(), tier)
                .map(assignment_item)
                .ok_or_else(|| ApiProblem::internal("assignment missing after write"))?;
            Ok(AssignmentPutResponse { item, impact })
        })
        .await?;
    tracing::info!(who = "admin", op = "model_role_assignment_set", source = %response.item.source, tier = tier_str(tier), model_id = %response.item.model_id, "admin: model role assigned");
    Ok(json_response(StatusCode::OK, &response))
}

async fn delete_assignment(
    State(state): State<ApiState>,
    Params((source, tier)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let source = parse_source(&source)?;
    let tier = parse_tier(&tier)?;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let deleted = state
        .blocking(move |store| {
            store
                .model_role_assignment_delete(&source, tier, ACTOR, now)
                .map_err(store_problem)
        })
        .await?;
    if !deleted {
        return Err(ApiProblem::new(
            StatusCode::NOT_FOUND,
            "model_assignment_not_found",
            "no assignment for that source and tier",
        ));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn preview(State(state): State<ApiState>, RawQuery(raw): RawQuery, body: Body) -> ApiResult {
    no_query(&raw)?;
    let payload: AssignmentPreviewBody = read_json(body, false).await?;
    let source = parse_source(&payload.source)?;
    let tier = payload.tier;
    let routing = state.inner.routing_catalog.as_ref().map(|r| r.view());
    let providers = current_providers(&state);
    let response = state
        .blocking(move |store| {
            let entries = store.model_catalog_list().map_err(store_problem)?;
            if let Some(m) = &payload.model_id
                && !entries
                    .iter()
                    .any(|e| e.source == source && &e.model_id == m)
            {
                return Err(model_not_in_catalog(&source, m));
            }
            let overrides = store.model_catalog_overrides().map_err(store_problem)?;
            let view = store.model_role_assignment_view().map_err(store_problem)?;
            Ok(AssignmentPreviewResponse {
                impact: compute_impact(
                    &source,
                    tier,
                    payload.model_id.as_deref(),
                    view.get(source.as_str(), tier),
                    &entries,
                    &overrides,
                    &providers,
                    routing.as_ref(),
                ),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &response))
}

async fn replace_role(
    State(state): State<ApiState>,
    Params(tier): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    edit_role(
        state,
        parse_tier(&tier)?,
        read_json(body, false).await?,
        false,
    )
    .await
}

async fn preview_role(
    State(state): State<ApiState>,
    Params(tier): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    edit_role(
        state,
        parse_tier(&tier)?,
        read_json(body, false).await?,
        true,
    )
    .await
}

async fn edit_role(
    state: ApiState,
    tier: Tier,
    payload: RoleMembersBody,
    preview: bool,
) -> ApiResult {
    let routing = state.inner.routing_catalog.as_ref().map(|r| r.view());
    let providers = current_providers(&state);
    let response = state
        .blocking(move |store| {
            let entries = store.model_catalog_list().map_err(store_problem)?;
            let view = store.model_role_assignment_view().map_err(store_problem)?;
            let slots = effective_slots(&view, &entries, &providers, routing.as_ref());
            let before: Vec<RoleMember> = slots
                .iter()
                .filter(|s| s.tier == tier)
                .filter_map(|s| {
                    s.model_id.as_ref().map(|model| RoleMember {
                        source: CatalogSource::new(&s.source),
                        model_id: model.clone(),
                        priority: s.priority,
                    })
                })
                .collect();
            let mut unique = std::collections::HashSet::new();
            for m in &payload.members {
                parse_source(m.source.as_str())?;
                if !unique.insert((m.source.as_str(), m.model_id.as_str())) {
                    return Err(ApiProblem::bad_request("duplicate role membership"));
                }
                // Already configured models remain editable before their first discovery.
                if !entries
                    .iter()
                    .any(|e| e.source == m.source && e.model_id == m.model_id)
                    && !before
                        .iter()
                        .any(|b| b.source == m.source && b.model_id == m.model_id)
                {
                    return Err(model_not_in_catalog(&m.source, &m.model_id));
                }
            }
            let sources: std::collections::BTreeSet<String> = slots
                .iter()
                .map(|s| s.source.clone())
                .chain(payload.members.iter().map(|m| m.source.0.clone()))
                .collect();
            let mut impact = ImpactView::default();
            for source in &sources {
                let list = |members: &[RoleMember]| -> Option<String> {
                    let mut items: Vec<_> = members
                        .iter()
                        .filter(|m| m.source.as_str() == source)
                        .collect();
                    items.sort_by_key(|m| (m.priority, &m.model_id));
                    (!items.is_empty()).then(|| {
                        items
                            .iter()
                            .map(|m| m.model_id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                };
                for part in participants(source, tier, &providers, routing.as_ref()) {
                    impact.changes.push(ImpactChangeView {
                        kind: part.kind,
                        id: part.id,
                        tier: tier_str(tier).into(),
                        before: list(&before),
                        after: list(&payload.members),
                        excluded_reason: None,
                    });
                }
            }
            if !preview {
                store
                    .model_role_members_replace(
                        tier,
                        &payload.members,
                        &sources
                            .into_iter()
                            .map(CatalogSource::new)
                            .collect::<Vec<_>>(),
                        ACTOR,
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .map_err(store_problem)?;
            }
            Ok(RoleMembersResponse {
                before,
                after: payload.members,
                impact,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &response))
}
