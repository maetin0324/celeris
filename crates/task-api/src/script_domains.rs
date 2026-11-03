//! ADR 2026-11-02-ts-control-layer-boundary 付記 S2-4・S2-5・S5-4: TS 制御層の領域ごとの mode の読み出しと切り替え。
//!
//! - `GET /script/domains` → 200 `ScriptDomainList`（領域ごとの mode・active generation とその状態・`canary_bps`・
//!   version、script-guard の全体の停止）。
//! - `PUT /script/domains/{domain}/mode` → 200 `ScriptDomainModeResult`。本文は `ScriptDomainModeBody`。
//!   全領域で `rust`・`compare`・`canary`・`ts` を受ける（S5-4）。
//!   `→canary`（`canary_bps` の変更を含む）・`→ts` は人の API token の主体（`human:*`）だけ。CoS・外部 operator・MCP・
//!   `system:*` は 403 `script_mode_requires_human`。`canary` の元は同じ generation の `compare`、`ts` の元は同じ
//!   generation の `canary`（違えば 422 `script_mode_requires_previous_stage`）。昇格は script-guard の計数が (f) の
//!   条件を満たすときだけ（不足は 422 `script_canary_not_ready`）。戻し（`→rust`、`canary`・`ts`→`compare`）は
//!   認証された誰でも行える。全体の停止の間は `rust` 以外を 409 `script_guard_halted` で拒む。
//!   `expected_version` が表の `version` と違えば 409 `script_domain_version_conflict`（どの拒否でも表を変えない）。
//!   同じ mode・同じ generation・同じ `canary_bps` への変更は表も記録も変えずに 200（`changed: false`）。
//! - `POST /script/guard/clear` → 200 `ScriptGuardClearResult`。全体の停止を解く（人の主体だけ。S5-4）。
//! - 本体は `task_core` の store（`script_domain_set_mode`・`script_domain_set_canary` の CAS）。CoS は
//!   `/cos/operations` の `script_domain.mode_set`（admin の ALLOWED）で同じ関数 [`set_mode_effect`] を呼ぶ。CoS の操作の
//!   `canary`・`ts` は registry が effect の前に 403 で拒み、rejected として記録する（S5-4）。

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use task_core::script_compare::{
    ScriptDomain, ScriptDomainError, ScriptDomainModeRow, ScriptGenerationState, ScriptMode,
    ScriptModeChange, ScriptModeChangeOutcome, actor_may_enable_ts,
};
use task_core::store::SqliteStore;

use crate::{
    handlers::{ApiResult, json_response, no_query, read_json},
    problem::{ApiProblem, store_problem},
    state::ApiState,
};

/// 未知の領域名（404）。
pub const SCRIPT_DOMAIN_NOT_FOUND: &str = "script_domain_not_found";
/// `→canary`・`→ts`・停止の解除を人以外の主体が求めた（S5-4）。403。
pub const SCRIPT_MODE_REQUIRES_HUMAN: &str = "script_mode_requires_human";
/// `canary` の元が同じ generation の `compare` でない、`ts` の元が同じ generation の `canary` でない（S5-4）。422。
pub const SCRIPT_MODE_REQUIRES_PREVIOUS_STAGE: &str = "script_mode_requires_previous_stage";
/// 昇格の前提の計数（(f) の「進む条件」）が足りない（S5-4）。422。本文に計数。
pub const SCRIPT_CANARY_NOT_READY: &str = "script_canary_not_ready";
/// script-guard の全体の停止の間に `rust` 以外を求めた（S5-4）。409。
pub const SCRIPT_GUARD_HALTED: &str = "script_guard_halted";
/// `expected_version` が表の `version` と違う（他の更新が先に入った）。409。
pub const SCRIPT_DOMAIN_VERSION_CONFLICT: &str = "script_domain_version_conflict";
/// generation の不備（指定が無い・余分・未登録・`loaded`・`active` でない・manifest にその領域が無い・
/// `canary_bps` が 1〜9999 でない）。登録で検証に落ちた bundle も同じ code（S5-1）。422。
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
    /// active generation の状態（S5-1。ふつうは `active`）。`rust` では `null`。
    pub generation_state: Option<ScriptGenerationState>,
    /// `canary` の比率（1〜9999 の万分率、S5-3）。`canary` 以外では `null`。
    pub canary_bps: Option<u32>,
    /// CAS の版。変更の要求の `expected_version` に渡す。
    pub version: u64,
    pub updated_at: String,
    pub updated_by: String,
}

/// `GET /script/domains` の応答（`domain` の順）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptDomainList {
    pub domains: Vec<ScriptDomainView>,
    /// script-guard の全体の停止の理由（S5-7）。止まっていなければ `null`。止まっている間は `rust` 以外へ切り替えられない。
    pub guard_halted: Option<String>,
}

/// `POST /script/guard/clear` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptGuardClearBody {
    /// 解除の理由（`script_guard_cleared` の `reason`）。
    pub reason: String,
}

/// `POST /script/guard/clear` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptGuardClearResult {
    /// 停止を解いたか（止まっていなければ `false` で、表も記録も変えない）。
    pub cleared: bool,
}

/// `PUT /script/domains/{domain}/mode` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptDomainModeBody {
    /// `rust` | `compare` | `canary` | `ts`（`canary`・`ts` は人の主体だけ）。
    pub mode: ScriptMode,
    /// 読んだ時点の `version`。表の値と違えば 409。
    pub expected_version: u64,
    /// `compare`・`canary`・`ts` で使う generation の `bundle_sha256`（必須。`canary` では今の `compare`、`ts` では今の
    /// `canary` と同じ値）。`rust` では書かない。
    #[serde(default)]
    pub generation: Option<String>,
    /// `canary` の比率（1〜9999 の万分率。`canary` では必須、他の mode では書かない。S5-3）。
    #[serde(default)]
    pub canary_bps: Option<u32>,
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
        .route(
            "/api/v1/script/guard/clear",
            axum::routing::post(clear_guard),
        )
}

/// `POST /script/guard/clear` → 200 `ScriptGuardClearResult`（人の主体だけ。他は 403 で表も記録も変えない）。
async fn clear_guard(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptGuardClearBody = read_json(body, false).await?;
    let actor = caller.actor();
    let cleared = state
        .blocking(move |store| {
            store
                .script_guard_clear(&actor, &body.reason, time::OffsetDateTime::now_utc())
                .map_err(domain_problem)
        })
        .await?;
    tracing::info!(op = "script_guard_clear", cleared, "script guard");
    Ok(json_response(
        StatusCode::OK,
        &ScriptGuardClearResult { cleared },
    ))
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
    let guard_halted = store.script_guard_halted().map_err(store_problem)?;
    Ok(ScriptDomainList {
        domains,
        guard_halted,
    })
}

fn view_of(store: &SqliteStore, row: ScriptDomainModeRow) -> Result<ScriptDomainView, ApiProblem> {
    let generation = match &row.generation_id {
        Some(id) => store.script_generation_get(id).map_err(store_problem)?,
        None => None,
    };
    let (bundle_sha256, generation_state) = generation
        .map(|g| (Some(g.bundle_sha256), Some(g.state)))
        .unwrap_or_default();
    Ok(ScriptDomainView {
        domain: row.domain,
        mode: row.mode,
        generation_id: row.generation_id,
        bundle_sha256,
        generation_state,
        canary_bps: row.canary_bps,
        version: row.version,
        updated_at: row.updated_at,
        updated_by: row.updated_by,
    })
}

/// mode を変える（route と CoS の操作が共有する）。`fallback_reason` は本文に `reason` が無いときの理由。
///
/// 検査の順: 領域名（404）→ `canary`・`ts` の主体（403）→ generation の解決（422）→ store の CAS（409・422。
/// 元の段・計数・`canary_bps` の検査を含む）。どの失敗でも表と `script_domain_events` は変わらない。
pub(crate) fn set_mode_effect(
    store: &SqliteStore,
    domain: &str,
    body: ScriptDomainModeBody,
    actor: &str,
    fallback_reason: &str,
) -> Result<ScriptDomainModeResult, ApiProblem> {
    if ScriptDomain::parse(domain).is_none() {
        return Err(domain_problem(ScriptDomainError::UnknownDomain(
            domain.to_owned(),
        )));
    }
    if !actor_may_enable_ts(actor) {
        match body.mode {
            ScriptMode::Ts => {
                return Err(domain_problem(ScriptDomainError::TsRequiresHuman(
                    actor.to_owned(),
                )));
            }
            ScriptMode::Canary => {
                return Err(domain_problem(ScriptDomainError::CanaryRequiresHuman(
                    actor.to_owned(),
                )));
            }
            ScriptMode::Rust | ScriptMode::Compare => {}
        }
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
    let now = time::OffsetDateTime::now_utc();
    let outcome = match (change.to_mode, body.canary_bps) {
        (ScriptMode::Canary, Some(bps)) => store.script_domain_set_canary(&change, bps, now),
        (ScriptMode::Canary, None) => Err(ScriptDomainError::CanaryBpsInvalid(None)),
        (_, Some(bps)) => Err(ScriptDomainError::CanaryBpsInvalid(Some(bps))),
        (_, None) => store.script_domain_set_mode(&change, now),
    }
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

/// store の失敗を API の problem に写す（S2-5・S5-4）。generation の登録・退役（`script_generations`）も使う。
pub(crate) fn domain_problem(err: ScriptDomainError) -> ApiProblem {
    let detail = err.to_string();
    match err {
        ScriptDomainError::UnknownDomain(_) => {
            ApiProblem::new(StatusCode::NOT_FOUND, SCRIPT_DOMAIN_NOT_FOUND, detail)
        }
        ScriptDomainError::TsRequiresHuman(_)
        | ScriptDomainError::CanaryRequiresHuman(_)
        | ScriptDomainError::GuardClearRequiresHuman(_) => {
            ApiProblem::new(StatusCode::FORBIDDEN, SCRIPT_MODE_REQUIRES_HUMAN, detail)
        }
        ScriptDomainError::RequiresPreviousStage { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_MODE_REQUIRES_PREVIOUS_STAGE,
            detail,
        ),
        ScriptDomainError::VersionConflict {
            expected, found, ..
        } => ApiProblem::new(StatusCode::CONFLICT, SCRIPT_DOMAIN_VERSION_CONFLICT, detail)
            .with_extra("expected_version", expected)
            .with_extra("version", found),
        ScriptDomainError::GuardHalted(reason) => {
            ApiProblem::new(StatusCode::CONFLICT, SCRIPT_GUARD_HALTED, detail)
                .with_extra("halted", reason)
        }
        ScriptDomainError::CanaryNotReady { detail: counts, .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_CANARY_NOT_READY,
            detail,
        )
        .with_extra("readiness", counts),
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
