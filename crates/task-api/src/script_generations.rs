//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-1: TS 制御層の generation の登録・一覧・退役。
//!
//! - `GET /script/generations` → 200 `ScriptGenerationList`（store の `script_generations`、古い順）。
//! - `POST /script/generations`（本文 `{ "bundle_sha256": "<64 桁 hex>" }`）→ 200 `ScriptGenerationView`。
//!   daemon が [`ScriptGenerationControl`] で bundle を新しい isolate に load して検証し、`loaded` で記録する。
//!   同じ hash が `loaded`・`active`・`draining` なら何も変えずに今の行を返す。検証に落ちたら 422
//!   `script_generation_invalid`、配備先の無い hash は 409 `script_bundle_not_deployed`（`invalid` にしない。配備の後に
//!   登録し直せる）、`invalid` の hash は 422 `script_generation_invalidated`、生きた generation が 2 つなら
//!   409 `script_generation_limit`。
//! - `POST /script/generations/{id}/retire` → 200 `ScriptGenerationView`。`active` なら 409 `script_generation_active`。
//! - 登録と退役は挙動を変えないので、人・外部 operator・CoS operations（`script_generation.register`・
//!   `script_generation.retire`）が行える。
//! - [`ScriptGenerationControl`] が未接続（`[script]` の無い daemon、task-api の試験の state）なら登録・退役は 503
//!   `script_runtime_unavailable` で、表を変えない。
//!
//! この module は DTO と trait だけを持ち、`deno_core` に依存しない（(b) の依存の制約）。

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use task_core::script_compare::{ScriptDomainError, ScriptGeneration, ScriptGenerationState};
use task_core::store::SqliteStore;

use crate::{
    handlers::{ApiResult, json_response, no_query, read_json},
    problem::{ApiProblem, store_problem},
    script_domains::{SCRIPT_GENERATION_INVALID, domain_problem},
    state::ApiState,
};

/// runtime（daemon の generation の登録簿）が未接続。503。
pub const SCRIPT_RUNTIME_UNAVAILABLE: &str = "script_runtime_unavailable";
/// 登録する bundle がまだ配備されていない（配備先の dir が無い）。409。表を変えず、配備の後に同じ hash で登録し直せる。
pub const SCRIPT_BUNDLE_NOT_DEPLOYED: &str = "script_bundle_not_deployed";
/// 指定の id の generation が無い。404。
pub const SCRIPT_GENERATION_NOT_FOUND: &str = "script_generation_not_found";

/// daemon が実装する generation の操作（S5-1 の 2〜6）。task-api は blocking の thread から呼ぶ。
///
/// 実装は bundle を `<state_dir>/script-bundles/<bundle_sha256>/` から新しい isolate に load して検証し、
/// 結果を store に書く（`script_generation_record`・`script_generation_record_invalid`・
/// `script_generation_transition`）。表と isolate の食い違いを作らないよう、store が拒んだら isolate を捨てる。
pub trait ScriptGenerationControl: Send + Sync {
    /// bundle を新しい isolate で load・検証して登録する。返すのは store の行。
    fn register(&self, bundle_sha256: &str) -> Result<ScriptGeneration, ScriptGenerationError>;
    /// generation を退役させる（`loaded`・`draining` → `retired`、isolate を捨てる）。
    fn retire(
        &self,
        generation_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<ScriptGeneration, ScriptGenerationError>;
    /// `lane_rules` の全入力を流す網羅を始める（ADR 2026-11-04-lane-rules-exhaustive-evidence D7）。
    /// 返すのは `running` の行で、実行は裏で続く。既定の実装は `Unavailable`（runtime が無い）。
    /// 実際の runner は daemon の `ScriptGenerations` が上書きする。
    fn exhaustive_start(
        &self,
        _domain: task_core::script_compare::ScriptDomain,
        _bundle_sha256: &str,
        _actor: &str,
        _reason: &str,
    ) -> Result<task_core::script_exhaustive::ScriptExhaustiveRun, ScriptGenerationError> {
        Err(ScriptGenerationError::Unavailable(
            "exhaustive runs need the script runtime".into(),
        ))
    }
}

/// [`ScriptGenerationControl`] の共有の型（`ApiState` が持つ）。
pub type SharedScriptGenerationControl = Arc<dyn ScriptGenerationControl>;

/// generation の操作の失敗。
#[derive(Debug, thiserror::Error)]
pub enum ScriptGenerationError {
    /// 検証に落ちた（bundle が無い・hash が違う・host_api・capability・selftest など）。実装は `invalid` を記録済み。
    #[error("script bundle `{bundle_sha256}` failed validation: {reason}")]
    Invalid {
        bundle_sha256: String,
        reason: String,
    },
    /// 配備先（`<state_dir>/script-bundles/<sha256>/`）が無い（P1-C）。`invalid` を記録しない。409
    /// `script_bundle_not_deployed`。`script-deploy` の後に同じ hash で登録し直せる。
    #[error("script bundle `{bundle_sha256}` is not deployed yet: {reason}")]
    NotDeployed {
        bundle_sha256: String,
        reason: String,
    },
    /// store の拒否（上限・`invalid` の hash・`active` の退役・未知の id など）。
    #[error(transparent)]
    Domain(#[from] ScriptDomainError),
    /// runtime の内部の失敗（actor の停止など）。500。
    #[error("script runtime failed: {0}")]
    Runtime(String),
    /// runtime はあるが今は登録・退役を受けない（standby の daemon、起動時の復元の前）。503
    /// `script_runtime_unavailable`。表を変えない。
    #[error("script runtime is not ready: {0}")]
    Unavailable(String),
}

/// generation 1 つ（一覧・登録・退役の応答）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptGenerationView {
    pub id: String,
    pub bundle_sha256: String,
    pub host_api: String,
    pub schema_sha256: String,
    pub state: ScriptGenerationState,
    /// manifest の `domains`。
    pub domains: Vec<String>,
    /// manifest の `checkpoint_versions`（S5-5）。
    pub checkpoint_versions: Vec<u32>,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /script/generations` の応答（古い順）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptGenerationList {
    pub generations: Vec<ScriptGenerationView>,
}

/// `POST /script/generations` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptGenerationRegisterBody {
    /// `script-deploy` が置いた bundle の sha256（64 桁の小文字 hex）。
    pub bundle_sha256: String,
}

/// `POST /script/generations/{id}/retire` の本文。
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptGenerationRetireBody {
    /// 退役の理由（`script_generation_retired` の `reason`）。CoS の操作では省略すると操作の理由を使う。
    #[serde(default)]
    pub reason: Option<String>,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/script/generations",
            axum::routing::get(list_generations).post(register),
        )
        .route(
            "/api/v1/script/generations/{id}/retire",
            axum::routing::post(retire),
        )
}

async fn list_generations(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let list = state
        .blocking(|store| {
            let generations = store
                .script_generations()
                .map_err(store_problem)?
                .into_iter()
                .map(view_of)
                .collect();
            Ok(ScriptGenerationList { generations })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn register(State(state): State<ApiState>, RawQuery(raw): RawQuery, body: Body) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptGenerationRegisterBody = read_json(body, false).await?;
    let control = state.script_generations.clone();
    let view = state
        .blocking(move |store| register_effect(store, control.as_deref(), &body.bundle_sha256))
        .await?;
    tracing::info!(op = "script_generation_register", id = %view.id, state = %view.state.as_str(), "script generation");
    Ok(json_response(StatusCode::OK, &view))
}

async fn retire(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptGenerationRetireBody = read_json(body, true).await?;
    let control = state.script_generations.clone();
    let actor = caller.actor();
    let view = state
        .blocking(move |store| retire_effect(store, control.as_deref(), &id, body, &actor, ""))
        .await?;
    tracing::info!(op = "script_generation_retire", id = %view.id, "script generation");
    Ok(json_response(StatusCode::OK, &view))
}

/// 登録（route と CoS の操作が共有する）。hash の形を確かめ、store の行を見てから runtime に渡す。
/// 生きた行（`loaded`・`active`・`draining`）はそのまま返し、`invalid` は 422 `script_generation_invalidated`（S5-1 の 6）。
/// どちらも isolate を作らない。
pub(crate) fn register_effect(
    store: &SqliteStore,
    control: Option<&dyn ScriptGenerationControl>,
    bundle_sha256: &str,
) -> Result<ScriptGenerationView, ApiProblem> {
    let control = control.ok_or_else(unavailable)?;
    if !is_sha256_hex(bundle_sha256) {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_GENERATION_INVALID,
            format!("bundle_sha256 must be 64 lowercase hex digits (got `{bundle_sha256}`)"),
        ));
    }
    match store
        .script_generation_by_sha256(bundle_sha256)
        .map_err(store_problem)?
    {
        Some(existing) if existing.state.is_live() => return Ok(view_of(existing)),
        Some(existing) if existing.state == ScriptGenerationState::Invalid => {
            return Err(domain_problem(ScriptDomainError::GenerationInvalidated(
                existing.id,
            )));
        }
        _ => {}
    }
    control
        .register(bundle_sha256)
        .map(view_of)
        .map_err(generation_problem)
}

/// 退役（route と CoS の操作が共有する）。未知の id は runtime に渡す前に 404。
pub(crate) fn retire_effect(
    store: &SqliteStore,
    control: Option<&dyn ScriptGenerationControl>,
    id: &str,
    body: ScriptGenerationRetireBody,
    actor: &str,
    fallback_reason: &str,
) -> Result<ScriptGenerationView, ApiProblem> {
    let control = control.ok_or_else(unavailable)?;
    if store
        .script_generation_get(id)
        .map_err(store_problem)?
        .is_none()
    {
        return Err(ApiProblem::new(
            StatusCode::NOT_FOUND,
            SCRIPT_GENERATION_NOT_FOUND,
            format!("script generation `{id}` not found"),
        ));
    }
    let reason = body
        .reason
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| fallback_reason.to_owned());
    control
        .retire(id, actor, &reason)
        .map(view_of)
        .map_err(generation_problem)
}

fn unavailable() -> ApiProblem {
    ApiProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        SCRIPT_RUNTIME_UNAVAILABLE,
        "the script runtime is not wired into this daemon ([script] is not configured)",
    )
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn generation_problem(err: ScriptGenerationError) -> ApiProblem {
    match err {
        ScriptGenerationError::Invalid { .. } => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            SCRIPT_GENERATION_INVALID,
            err.to_string(),
        ),
        ScriptGenerationError::NotDeployed { .. } => ApiProblem::new(
            StatusCode::CONFLICT,
            SCRIPT_BUNDLE_NOT_DEPLOYED,
            format!("{err}; deploy it with script-deploy and register the same hash again"),
        ),
        ScriptGenerationError::Domain(ScriptDomainError::GenerationNotFound(id)) => {
            ApiProblem::new(
                StatusCode::NOT_FOUND,
                SCRIPT_GENERATION_NOT_FOUND,
                format!("script generation `{id}` not found"),
            )
        }
        ScriptGenerationError::Domain(e) => domain_problem(e),
        ScriptGenerationError::Unavailable(m) => ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            SCRIPT_RUNTIME_UNAVAILABLE,
            format!("the script runtime is not ready: {m}"),
        ),
        ScriptGenerationError::Runtime(m) => {
            ApiProblem::internal(format!("script runtime failed: {m}"))
        }
    }
}

pub(crate) fn view_of(g: ScriptGeneration) -> ScriptGenerationView {
    let domains = serde_json::from_str::<serde_json::Value>(&g.manifest_json)
        .ok()
        .and_then(|v| v.get("domains").cloned())
        .and_then(|d| d.as_array().cloned())
        .map(|ds| {
            ds.iter()
                .filter_map(|d| d.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let checkpoint_versions = g.checkpoint_versions();
    ScriptGenerationView {
        id: g.id,
        bundle_sha256: g.bundle_sha256,
        host_api: g.host_api,
        schema_sha256: g.schema_sha256,
        state: g.state,
        domains,
        checkpoint_versions,
        created_at: g.created_at,
        updated_at: g.updated_at,
    }
}
