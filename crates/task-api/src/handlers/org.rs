//! 組織のノードの一覧と編集（`GET/POST /org`、`PATCH/DELETE /org/{id}`。ADR-0033 D1）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use task_core::{NodeSessionStore, OrgNode, TaskStore};
use time::OffsetDateTime;

use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;
use crate::types::{OrgCreateBody, OrgList, OrgPatchBody, ValidationError};

use super::{ApiResult, Params, json_response, load_org_node, no_query, read_json};

/// PATCH /org/{id}/browser-settings. Only browser-related profile fields are editable.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserSettingsPatch {
    pub allowed_domains: Option<Vec<String>>,
    /// ADR 2026-10-08-browser-click-download-approval-policy D1: business actions that need a
    /// per-run human approval in every task of this grant (default empty = click/download run
    /// without approval). `credential_use` is always approved regardless of this list.
    pub approval_actions: Option<Vec<task_core::BrowserAction>>,
    pub credential_policy_ids: Option<Vec<String>>,
    pub credential_identity_ids: Option<BTreeMap<String, String>>,
    /// ADR 2026-10-08-browser-prod-enablement D3: `true` は grant の `allowed_actions` に `credential_use` を
    /// 加え（欄が無い grant は Phase 1 の集合を実体化してから）、`false` は外す。使用は毎回人の承認（変わらない）。
    pub credential_use: Option<bool>,
    pub harnesses: Option<task_core::HarnessPrefs>,
    pub budget: Option<task_core::BudgetPrefs>,
}

fn parse_browser_settings_json<T: serde::de::DeserializeOwned>(
    value: Value,
    domains: Option<&Value>,
) -> Result<T, ApiProblem> {
    if let Some(domains) = domains {
        let valid = domains.as_array().is_some_and(|values| {
            values.iter().all(|value| {
                value
                    .as_str()
                    .is_some_and(|origin| task_core::AllowedOrigin::parse(origin).is_ok())
            })
        });
        if !valid {
            return Err(ApiProblem::validation(vec![ValidationError {
                field: Some("browser.allowed_domains".into()),
                message: "expected valid browser origins".into(),
            }]));
        }
    }
    serde_json::from_value(value)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

pub(super) async fn patch_browser_settings(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let raw: Value = read_json(body, true).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = plan_browser_settings(store, &genres, &id, raw)?;
            org_commit(store, write, "org.browser_settings", None)?.direct()
        })
        .await?;
    Ok(json_response(StatusCode::OK, &node))
}

/// One org write planned by a route (validation and reads done), committed by [`org_commit`].
pub(crate) enum OrgWrite {
    /// Upsert the node; `browser` also records the browser settings audit row.
    Upsert {
        node: Box<OrgNode>,
        browser: bool,
    },
    Delete(String),
}

/// Commit a planned org write: directly (the human route, actor `admin`) or, with `audit`, in the
/// CoS operation transaction (actor `cos`). The direct route returns the stored node (`None` for
/// a delete).
pub(crate) fn org_commit(
    store: &task_core::store::SqliteStore,
    write: OrgWrite,
    action: &str,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<Option<OrgNode>>, ApiProblem> {
    use crate::cos::operations::Applied;
    use task_core::store::SqliteStore;
    let Some(audit) = audit else {
        return match write {
            OrgWrite::Upsert {
                node,
                browser: true,
            } => store
                .org_upsert_browser_settings(&node, "admin")
                .map(|n| Applied::Direct(Some(n)))
                .map_err(store_problem),
            OrgWrite::Upsert { node, .. } => store
                .org_upsert(&node)
                .map(|n| Applied::Direct(Some(n)))
                .map_err(store_problem),
            OrgWrite::Delete(id) => {
                store.org_delete(&id).map_err(store_problem)?;
                Ok(Applied::Direct(None))
            }
        };
    };
    let target = match &write {
        OrgWrite::Upsert { node, .. } => node.id.clone(),
        OrgWrite::Delete(id) => id.clone(),
    };
    let operation = audit.apply_checked(store, "org", &target, action, |tx| match &write {
        OrgWrite::Upsert { node, browser } => {
            let stored = SqliteStore::org_upsert_tx(tx, node, browser.then_some("cos"))
                .map_err(store_problem)?;
            serde_json::to_value(&stored).map_err(|e| ApiProblem::internal(e.to_string()))
        }
        OrgWrite::Delete(id) => {
            if !SqliteStore::org_delete_tx(tx, id).map_err(store_problem)? {
                return Err(ApiProblem::org_node_not_found(id));
            }
            Ok(serde_json::json!({"id": id, "deleted": true}))
        }
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `PATCH /org/{id}/browser-settings`: the node with only the browser-related profile changed.
pub(crate) fn plan_browser_settings(
    store: &task_core::store::SqliteStore,
    genres: &[task_core::GenreSpec],
    id: &str,
    raw: Value,
) -> Result<OrgWrite, ApiProblem> {
    let patch: BrowserSettingsPatch =
        parse_browser_settings_json(raw.clone(), raw.get("allowed_domains"))?;
    let known = task_core::known_harness_ids(genres);
    let mut node = load_org_node(store, id)?;
    let browser = node.profile.browser.as_mut().ok_or_else(|| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("browser".into()),
            message: "node has no browser grant".into(),
        }])
    })?;
    if let Some(domains) = patch.allowed_domains {
        browser.allowed_domains = domains;
    }
    if let Some(actions) = patch.approval_actions {
        browser.approval_actions = actions;
    }
    if let Some(ids) = patch.credential_policy_ids {
        // ADR 2026-10-08 D3: 実在する site policy だけを grant に入れる。
        let mut missing = Vec::new();
        for id in &ids {
            if store
                .browser_site_policy_get(id)
                .map_err(store_problem)?
                .is_none()
            {
                missing.push(id.clone());
            }
        }
        if !missing.is_empty() {
            return Err(ApiProblem::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_site_policy",
                format!("unknown site policy: {}", missing.join(", ")),
            )
            .with_extra("policy_ids", missing));
        }
        browser.credential_policy_ids = ids;
    }
    if let Some(ids) = patch.credential_identity_ids {
        browser.credential_identity_ids = ids;
    }
    if let Some(enabled) = patch.credential_use {
        set_credential_use(browser, enabled);
    }
    if let Some(harnesses) = patch.harnesses {
        node.profile.harnesses = harnesses;
    }
    if let Some(budget) = patch.budget {
        node.profile.budget = budget;
    }
    task_core::validate_profile(&node.profile, &known).map_err(|e| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("profile".into()),
            message: e.to_string(),
        }])
    })?;
    node.updated_at = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(node),
        browser: true,
    })
}

/// grant の `allowed_actions` に `credential_use` を入れる・外す（他の action は変えない）。
fn set_credential_use(browser: &mut task_core::BrowserCapability, enabled: bool) {
    use task_core::BrowserAction;
    if !enabled && browser.allowed_actions.is_none() {
        // 欄が無い grant は Phase 1 の集合（`credential_use` を含まない）。
        return;
    }
    let actions = browser
        .allowed_actions
        .get_or_insert_with(|| BrowserAction::PHASE1.to_vec());
    actions.retain(|a| *a != BrowserAction::CredentialUse);
    if enabled {
        actions.push(BrowserAction::CredentialUse);
    }
}

/// 監査 L-1: 組織のノードの `genre` は設定の `[[genres]]` にあるものだけ（分野を 1 つも設定していない
/// 構成では検証しない。`POST /tasks` の `genre` と同じ規律。ADR-0027 D1）。
fn validate_genre(genres: &[task_core::GenreSpec], genre: Option<&str>) -> Result<(), ApiProblem> {
    let Some(genre) = genre else { return Ok(()) };
    if genres.is_empty() || genres.iter().any(|g| g.id == genre) {
        return Ok(());
    }
    Err(ApiProblem::validation(vec![ValidationError {
        field: Some("genre".into()),
        message: format!("unknown genre: {genre:?}"),
    }]))
}

/// ADR-0046 D1（Phase 59）: profile の決定的な検証（知らない道具・知らないハーネス・skill の綴り）。
/// ハーネスの集合は設定の `[[genres]]`（= `[[harnesses]]` の射影）＋ 組み込み。分野を 1 つも設定して
/// いない構成では検証しない（`validate_genre` と同じ規律）。
fn validate_profile_body(
    genres: &[task_core::GenreSpec],
    profile: Option<&task_core::Profile>,
) -> Result<(), ApiProblem> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let known = task_core::known_harness_ids(genres);
    task_core::validate_profile(profile, &known).map_err(|e| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("profile".into()),
            message: e.to_string(),
        }])
    })
}

pub(super) async fn org_list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let (items, lead_sessions) = state
        .blocking(|store| {
            let items = store.org_list().map_err(store_problem)?;
            // ADR-0054 D3（Phase 68）: 部門長（`OrgKind::Department`）の継続セッションがあれば拾う
            // （無いノードは含めない。CoS はここに出さない。`crate::console::new_conversation` と同じ
            // `NodeSessionStore` を薄く読むだけ）。
            let mut lead_sessions = Vec::new();
            for node in items
                .iter()
                .filter(|n| n.kind == task_core::OrgKind::Department)
            {
                if let Some(session) = store
                    .node_session_active(&node.id, task_core::SessionKind::Lead, None)
                    .map_err(store_problem)?
                {
                    lead_sessions.push(crate::types::NodeSessionSummary {
                        node_id: session.node_id,
                        turns: session.turns,
                        approx_tokens: session.approx_tokens,
                        last_used_at: session.last_used_at,
                    });
                }
            }
            Ok((items, lead_sessions))
        })
        .await?;
    // ADR-0046 D1: 継いだ後の実効 profile も一緒に返す（計算は純粋関数。DB には保存しない）。
    let effective_profiles = items
        .iter()
        .map(|n| task_core::resolve_profile(&items, &n.id))
        .collect();
    Ok(json_response(
        StatusCode::OK,
        &OrgList {
            items,
            effective_profiles,
            lead_sessions,
        },
    ))
}

pub(super) async fn create_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: OrgCreateBody = read_json(body, false).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = plan_create(store, &genres, create)?;
            org_commit(store, write, "org.create", None)?.direct()
        })
        .await?
        .ok_or_else(|| ApiProblem::internal("org create returned no node"))?;
    tracing::info!(who = "admin", op = "org_create", org_id = %node.id, "admin: org node created");
    let mut response = json_response(StatusCode::CREATED, &node);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/org/{}", node.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// `POST /org`: validate the body and build the new node (409 when the id exists).
pub(crate) fn plan_create(
    store: &task_core::store::SqliteStore,
    genres: &[task_core::GenreSpec],
    create: OrgCreateBody,
) -> Result<OrgWrite, ApiProblem> {
    // 監査 L-1: `genre` は `[[genres]]` にあるものだけ受ける（`genres` が空の設定では検証しない）。
    validate_genre(genres, create.genre.as_deref())?;
    validate_profile_body(genres, create.profile.as_ref())?;
    if store.org_get(&create.id).map_err(store_problem)?.is_some() {
        return Err(ApiProblem::org_node_exists(&create.id));
    }
    let now = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(OrgNode {
            id: create.id,
            parent_id: create.parent_id,
            name: create.name,
            kind: create.kind,
            genre: create.genre,
            brief: create.brief.unwrap_or_default(),
            // ADR-0046 D1（Phase 59）: 省略時は空の profile。
            profile: create.profile.unwrap_or_default(),
            position: create.position.unwrap_or(0),
            created_at: now,
            updated_at: now,
        }),
        browser: false,
    })
}

pub(super) async fn patch_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let raw: Value = read_json(body, true).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = plan_patch(store, &genres, &id, raw)?;
            org_commit(store, write, "org.update", None)?.direct()
        })
        .await?
        .ok_or_else(|| ApiProblem::internal("org patch returned no node"))?;
    tracing::info!(who = "admin", op = "org_patch", org_id = %node.id, "admin: org node updated");
    Ok(json_response(StatusCode::OK, &node))
}

/// `PATCH /org/{id}`: the node with the patch applied (profile is replaced whole).
pub(crate) fn plan_patch(
    store: &task_core::store::SqliteStore,
    genres: &[task_core::GenreSpec],
    id: &str,
    raw: Value,
) -> Result<OrgWrite, ApiProblem> {
    let domains = raw.pointer("/profile/browser/allowed_domains").cloned();
    let patch: OrgPatchBody = parse_browser_settings_json(raw, domains.as_ref())?;
    if let Some(genre) = &patch.genre {
        validate_genre(genres, genre.as_deref())?;
    }
    validate_profile_body(genres, patch.profile.as_ref())?;
    let mut node = load_org_node(store, id)?;
    if let Some(name) = patch.name {
        node.name = name;
    }
    if let Some(kind) = patch.kind {
        node.kind = kind;
    }
    if let Some(parent_id) = patch.parent_id {
        node.parent_id = Some(parent_id);
    }
    if let Some(genre) = patch.genre {
        node.genre = genre;
    }
    if let Some(brief) = patch.brief {
        node.brief = brief;
    }
    if let Some(position) = patch.position {
        node.position = position;
    }
    // ADR-0046 D1（Phase 59）: profile は**丸ごと差し替え**（書かなければ今のまま）。
    let browser = patch
        .profile
        .as_ref()
        .is_some_and(|profile| profile.browser != node.profile.browser);
    if let Some(profile) = patch.profile {
        node.profile = profile;
    }
    node.updated_at = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(node),
        browser,
    })
}

pub(super) async fn delete_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    state
        .blocking(move |store| {
            load_org_node(store, &id)?;
            org_commit(store, OrgWrite::Delete(id.clone()), "org.delete", None)?.direct()?;
            tracing::info!(who = "admin", op = "org_delete", org_id = %id, "admin: org node deleted");
            Ok(())
        })
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
