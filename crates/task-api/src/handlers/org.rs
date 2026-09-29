//! 組織のノードの一覧と編集（`GET/POST /org`、`PATCH/DELETE /org/{id}`。ADR-0033 D1）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use task_core::{NodeSessionStore, OrgNode, TaskStore};
use time::OffsetDateTime;

use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;
use crate::types::{OrgCreateBody, OrgList, OrgPatchBody, ValidationError};

use super::{ApiResult, Params, json_response, load_org_node, no_query, read_json};

/// 監査 L-1: 組織のノードの `genre` は設定の `[[genres]]` にあるものだけ（分野を 1 つも設定していない
/// 構成では検証しない。`POST /tasks` の `genre` と同じ規律。ADR-0027 D1）。
fn validate_genre(state: &ApiState, genre: Option<&str>) -> Result<(), ApiProblem> {
    let Some(genre) = genre else { return Ok(()) };
    if state.inner.genres.is_empty() || state.inner.genres.iter().any(|g| g.id == genre) {
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
    state: &ApiState,
    profile: Option<&task_core::Profile>,
) -> Result<(), ApiProblem> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let known = task_core::known_harness_ids(&state.inner.genres);
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
    // 監査 L-1: `genre` は `[[genres]]` にあるものだけ受ける（`genres` が空の設定では検証しない）。
    validate_genre(&state, create.genre.as_deref())?;
    validate_profile_body(&state, create.profile.as_ref())?;
    let node = state
        .blocking(move |store| {
            if store.org_get(&create.id).map_err(store_problem)?.is_some() {
                return Err(ApiProblem::org_node_exists(&create.id));
            }
            let now = OffsetDateTime::now_utc();
            let node = OrgNode {
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
            };
            store.org_upsert(&node).map_err(store_problem)
        })
        .await?;
    tracing::info!(who = "admin", op = "org_create", org_id = %node.id, "admin: org node created");
    let mut response = json_response(StatusCode::CREATED, &node);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/org/{}", node.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
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
    let patch: OrgPatchBody = read_json(body, true).await?;
    if let Some(genre) = &patch.genre {
        validate_genre(&state, genre.as_deref())?;
    }
    validate_profile_body(&state, patch.profile.as_ref())?;
    let node = state
        .blocking(move |store| {
            let mut node = load_org_node(store, &id)?;
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
            if let Some(profile) = patch.profile {
                node.profile = profile;
            }
            node.updated_at = OffsetDateTime::now_utc();
            store.org_upsert(&node).map_err(store_problem)
        })
        .await?;
    tracing::info!(who = "admin", op = "org_patch", org_id = %node.id, "admin: org node updated");
    Ok(json_response(StatusCode::OK, &node))
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
            store.org_delete(&id).map_err(store_problem)?;
            tracing::info!(who = "admin", op = "org_delete", org_id = %id, "admin: org node deleted");
            Ok(())
        })
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
