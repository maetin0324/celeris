//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5: TS 制御層の領域ごとの mode の読み出しと切り替え。
//!
//! - `GET /script/domains` → 200 `ScriptDomainList`（領域ごとの mode・active generation・version）。
//! - `PUT /script/domains/{domain}/mode` → 200 `ScriptDomainModeResult`。本文は `ScriptDomainModeBody`。
//!   `rust`・`compare` はどの領域でも受ける。`ts` は `cos_digest` だけで受ける（付記 S4-14）。
//!   `canary` と `cos_digest` 以外の `ts` は 422 `script_mode_not_enabled`。
//!   `→ts` は人の API token の主体（`human:*`）だけが行える。CoS・外部 operator・MCP・`system:*` は
//!   403 `script_mode_requires_human`。`→ts` の元が同じ generation の `compare` でなければ 422
//!   `script_mode_ts_requires_compare`。戻し（`ts→rust`・`ts→compare`）は誰でも行える。
//!   `expected_version` が表の `version` と違えば 409 `script_domain_version_conflict`（どの拒否でも表を変えない）。
//!   同じ mode・同じ generation への変更は表も記録も変えずに 200（`changed: false`）。
//! - 本体は `task_core` の store（`script_domain_set_mode` の CAS）。CoS は `/cos/operations` の
//!   `script_domain.mode_set`（admin の ALLOWED）で同じ関数 [`set_mode_effect`] を呼ぶ。CoS の操作の
//!   `ts` は registry が effect の前に 403 で拒み、rejected として記録する（S4-14）。

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainError, ScriptDomainModeRow, ScriptMode, ScriptModeChange,
    ScriptModeChangeOutcome, actor_may_enable_ts,
};
use task_core::store::SqliteStore;

use crate::{
    handlers::{ApiResult, json_response, no_query, read_json},
    problem::{ApiProblem, store_problem},
    state::ApiState,
};

/// 未知の領域名（404）。
pub const SCRIPT_DOMAIN_NOT_FOUND: &str = "script_domain_not_found";
/// その領域で受けない mode（`canary`、`cos_digest` 以外の `ts`）。422。
pub const SCRIPT_MODE_NOT_ENABLED: &str = "script_mode_not_enabled";
/// `→ts` を人以外の主体が求めた（S4-14）。403。
pub const SCRIPT_MODE_REQUIRES_HUMAN: &str = "script_mode_requires_human";
/// `→ts` の元が同じ generation の `compare` でない（S4-14）。422。
pub const SCRIPT_MODE_TS_REQUIRES_COMPARE: &str = "script_mode_ts_requires_compare";
/// `expected_version` が表の `version` と違う（他の更新が先に入った）。409。
pub const SCRIPT_DOMAIN_VERSION_CONFLICT: &str = "script_domain_version_conflict";
/// generation の不備（指定が無い・余分・未登録・`loaded` でない・manifest にその領域が無い）。422。
pub const SCRIPT_GENERATION_INVALID: &str = "script_generation_invalid";

/// 領域 1 つの現在の mode（`GET /script/domains` の 1 行、変更の応答）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptDomainView {
    pub domain: ScriptDomain,
    pub mode: ScriptMode,
    /// active generation の id（`script_generations.id`）。`rust` では `null`。
    pub generation_id: Option<String>,
    /// active generation の bundle の sha256（64 桁 hex）。`rust` では `null`。
    pub bundle_sha256: Option<String>,
    /// CAS の版。変更の要求の `expected_version` に渡す。
    pub version: u64,
    pub updated_at: String,
    pub updated_by: String,
}

/// `GET /script/domains` の応答（`domain` の順）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptDomainList {
    pub domains: Vec<ScriptDomainView>,
}

/// `PUT /script/domains/{domain}/mode` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptDomainModeBody {
    /// `rust` | `compare` | `ts`（`ts` は `cos_digest` だけ・人の主体だけ。`canary` は 422）。
    pub mode: ScriptMode,
    /// 読んだ時点の `version`。表の値と違えば 409。
    pub expected_version: u64,
    /// `compare`・`ts` で使う generation の `bundle_sha256`（必須。`ts` では今の `compare` と同じ値）。
    /// `rust` では書かない。
    #[serde(default)]
    pub generation: Option<String>,
    /// 変更の理由（`script_domain_mode_changed` の `reason`）。CoS の操作では省略すると操作の理由を使う。
    #[serde(default)]
    pub reason: Option<String>,
}

/// `PUT /script/domains/{domain}/mode` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptDomainModeResult {
    /// 表を変えたか（同じ mode・同じ generation なら `false`）。
    pub changed: bool,
    /// 追記した `script_domain_events` の id（`changed: false` では `null`）。
    pub event_id: Option<u64>,
    pub domain: ScriptDomainView,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/script/domains", axum::routing::get(list_domains))
        .route(
            "/api/v1/script/domains/{domain}/mode",
            axum::routing::put(put_mode),
        )
}

/// `GET /script/domains` → 200 `ScriptDomainList`。
async fn list_domains(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let list = state.blocking(list_view).await?;
    Ok(json_response(StatusCode::OK, &list))
}

/// `PUT /script/domains/{domain}/mode` → 200 `ScriptDomainModeResult`（actor は要求の主体）。
async fn put_mode(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    Path(domain): Path<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptDomainModeBody = read_json(body, false).await?;
    let actor = caller.actor();
    let result = state
        .blocking(move |store| set_mode_effect(store, &domain, body, &actor, ""))
        .await?;
    tracing::info!(
        op = "script_domain_mode_set",
        domain = %result.domain.domain.as_str(),
        mode = %result.domain.mode.as_str(),
        changed = result.changed,
        "script domain mode"
    );
    Ok(json_response(StatusCode::OK, &result))
}

pub(crate) fn list_view(store: &SqliteStore) -> Result<ScriptDomainList, ApiProblem> {
    let rows = store.script_domain_modes().map_err(store_problem)?;
    let domains = rows
        .into_iter()
        .map(|row| view_of(store, row))
        .collect::<Result<_, _>>()?;
    Ok(ScriptDomainList { domains })
}

fn view_of(store: &SqliteStore, row: ScriptDomainModeRow) -> Result<ScriptDomainView, ApiProblem> {
    let bundle_sha256 = match &row.generation_id {
        Some(id) => store
            .script_generation_get(id)
            .map_err(store_problem)?
            .map(|g| g.bundle_sha256),
        None => None,
    };
    Ok(ScriptDomainView {
        domain: row.domain,
        mode: row.mode,
        generation_id: row.generation_id,
        bundle_sha256,
        version: row.version,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
    })
}

/// mode を変える（route と CoS の操作が共有する）。`fallback_reason` は本文に `reason` が無いときの理由。
///
/// 検査の順: 領域名（404）→ その領域で受ける mode か（422）→ `ts` の主体（403）→ generation の解決
/// （422）→ store の CAS（409・422。`ts` の元の検査を含む）。どの失敗でも表と `script_domain_events` は変わらない。
pub(crate) fn set_mode_effect(
    store: &SqliteStore,
    domain: &str,
    body: ScriptDomainModeBody,
    actor: &str,
    fallback_reason: &str,
) -> Result<ScriptDomainModeResult, ApiProblem> {
    let Some(parsed) = ScriptDomain::parse(domain) else {
        return Err(domain_problem(ScriptDomainError::UnknownDomain(
            domain.to_owned(),
        )));
    };
    if !body.mode.accepted_for(parsed) {
        return Err(domain_problem(ScriptDomainError::ModeNotEnabled {
            domain: parsed,
            mode: body.mode,
        }));
    }
    if body.mode == ScriptMode::Ts && !actor_may_enable_ts(actor) {
        return Err(domain_problem(ScriptDomainError::TsRequiresHuman(
            actor.to_owned(),
        )));
    }
    let to_generation = match body.generation.as_deref() {
        None => None,
        Some(sha) => {
            let generation = store
                .script_generation_by_sha256(sha)
                .map_err(store_problem)?
                .ok_or_else(|| {
                    ApiProblem::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        SCRIPT_GENERATION_INVALID,
                        format!("no script generation has bundle_sha256 `{sha}`"),
                    )
                })?;
            Some(generation.id)
        }
    };
    let reason = body
        .reason
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| fallback_reason.to_owned());
    let change = ScriptModeChange {
        domain: domain.to_owned(),
        to_mode: body.mode,
        to_generation,
        expected_version: body.expected_version,
        actor: actor.to_owned(),
        reason,
    };
    let outcome = store
        .script_domain_set_mode(&change, time::OffsetDateTime::now_utc())
        .map_err(domain_problem)?;
    let (changed, event_id, row) = match outcome {
        ScriptModeChangeOutcome::Changed { row, event_id } => (true, Some(event_id), row),
        ScriptModeChangeOutcome::Unchanged { row } => (false, None, row),
    };
    Ok(ScriptDomainModeResult {
        changed,
        event_id,
        domain: view_of(store, row)?,
    })
}

/// store の失敗を API の problem に写す（S2-5）。
fn domain_problem(err: ScriptDomainError) -> ApiProblem {
    let detail = err.to_string();
    match err {
        ScriptDomainError::UnknownDomain(_) => {
            ApiProblem::new(StatusCode::NOT_FOUND, SCRIPT_DOMAIN_NOT_FOUND, detail)
        }
        ScriptDomainError::ModeNotEnabled { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_MODE_NOT_ENABLED,
            detail,
        ),
        ScriptDomainError::TsRequiresHuman(_) => {
            ApiProblem::new(StatusCode::FORBIDDEN, SCRIPT_MODE_REQUIRES_HUMAN, detail)
        }
        ScriptDomainError::CanaryRequiresHuman(_)
        | ScriptDomainError::GuardClearRequiresHuman(_) => {
            ApiProblem::new(StatusCode::FORBIDDEN, SCRIPT_MODE_REQUIRES_HUMAN, detail)
        }
        ScriptDomainError::RequiresPreviousStage { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_MODE_TS_REQUIRES_COMPARE,
            detail,
        ),
        ScriptDomainError::VersionConflict {
            expected, found, ..
        } => ApiProblem::new(StatusCode::CONFLICT, SCRIPT_DOMAIN_VERSION_CONFLICT, detail)
            .with_extra("expected_version", expected)
            .with_extra("version", found),
        ScriptDomainError::GuardHalted(_) => {
            ApiProblem::new(StatusCode::CONFLICT, "script_guard_halted", detail)
        }
        ScriptDomainError::CanaryNotReady { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "script_canary_not_ready",
            detail,
        ),
        ScriptDomainError::CheckpointIncompatible { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "script_checkpoint_incompatible",
            detail,
        ),
        ScriptDomainError::GenerationLimit { .. } => {
            ApiProblem::new(StatusCode::CONFLICT, "script_generation_limit", detail)
        }
        ScriptDomainError::GenerationInvalidated(_) => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "script_generation_invalidated",
            detail,
        ),
        ScriptDomainError::GenerationActive(_) => {
            ApiProblem::new(StatusCode::CONFLICT, "script_generation_active", detail)
        }
        ScriptDomainError::IllegalTransition { .. } => {
            ApiProblem::new(StatusCode::CONFLICT, "script_generation_transition", detail)
        }
        ScriptDomainError::CanaryBpsInvalid(_)
        | ScriptDomainError::GenerationRequired
        | ScriptDomainError::GenerationNotAllowed
        | ScriptDomainError::GenerationNotFound(_)
        | ScriptDomainError::GenerationNotLoaded(_)
        | ScriptDomainError::DomainNotDeclared { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_GENERATION_INVALID,
            detail,
        ),
        ScriptDomainError::Store(e) => store_problem(e),
    }
}
