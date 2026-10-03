//! ADR-0131 D5: 定期実行（cron job）の API（`docs/celeris-api-v1.md` の「定期実行」）。
//!
//! - `GET /cron-jobs` — 一覧（`name` 昇順。各 job に最後の履歴 `last_run`）。
//! - `POST /cron-jobs` — 作成 → 201 `CronJobView`（作成直後の `next_fire_at`）。`name` の重複は 409
//!   `cron_job_name_in_use`、式・タイムゾーン・雛形の誤りは 422。
//! - `GET /cron-jobs/{id}` — 詳細（雛形を含む）。`PATCH` で更新（書いた欄だけ）、`DELETE` で削除（204）。
//! - `POST /cron-jobs/{id}/pause` / `resume` — 一時停止・再開（D3 の再開規則）。
//! - `POST /cron-jobs/{id}/run` — 手動実行（`trigger = manual`、D2 の重ね掛けの規則）→ 200 `CronRunResult`。
//!   同じ時刻の二重押しは 409 `cron_run_conflict`。
//! - `GET /cron-jobs/{id}/runs?limit=` — 履歴（新しい順、既定 50・上限 500）。
//!
//! `{id}` は ULID か `name` のどちらでも引ける（無ければ 404 `cron_job_not_found`）。書き込み系は人の操作
//! （`POST /tasks` と同じく通常の認証）。ハンドラは HTTP への写像だけで、判断と発火は `task_ops::cron_jobs`
//! （決定的、LLM なし）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    CronCatchUp, CronJob, CronJobId, CronJobRun, CronJobStore, CronOverlap, CronTaskTemplate,
    SqliteStore, StoreError, TaskId, TaskStore,
};
use task_ops::cron_jobs::{CronFireContext, CronJobPatch, NewCronJob};
use time::OffsetDateTime;

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::problem::{ApiProblem, ops_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

/// `GET /cron-jobs/{id}/runs` の既定件数と上限。
pub const RUNS_DEFAULT_LIMIT: usize = 50;
pub const RUNS_MAX_LIMIT: usize = 500;

/// `POST /cron-jobs` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CronJobCreateBody {
    /// 人が読む識別子（一意。ULID の形は不可）。
    pub name: String,
    /// 5 欄の cron 式（`@daily` 等の別名も可）。
    pub schedule: String,
    /// IANA タイムゾーン名（例 `Asia/Tokyo`）。
    pub timezone: String,
    /// 前回の task が終わっていないときの扱い（既定 `skip`）。
    #[serde(default)]
    pub overlap: CronOverlap,
    /// 取りこぼした予定時刻の扱い（既定 `latest`）。
    #[serde(default)]
    pub catch_up: CronCatchUp,
    /// 既定 `true`。`false` なら一時停止の状態で作る。
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub template: CronTaskTemplate,
}

fn default_true() -> bool {
    true
}

/// `PATCH /cron-jobs/{id}` の本文（書いた欄だけ変える。有効/無効は `pause` / `resume` で変える）。
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CronJobPatchBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub overlap: Option<CronOverlap>,
    #[serde(default)]
    pub catch_up: Option<CronCatchUp>,
    /// 雛形は丸ごと置き換える。
    #[serde(default)]
    pub template: Option<CronTaskTemplate>,
}

/// job 1 件と最後の履歴（一覧・詳細・作成・更新・一時停止・再開の応答）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CronJobView {
    #[serde(flatten)]
    pub job: CronJob,
    /// 最後に記録した履歴（無ければ `null`）。
    pub last_run: Option<CronJobRun>,
}

/// `GET /cron-jobs` の応答。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CronJobList {
    pub items: Vec<CronJobView>,
}

/// `GET /cron-jobs/{id}/runs` の応答（新しい順）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CronJobRunList {
    pub job_id: CronJobId,
    pub items: Vec<CronJobRun>,
}

/// `POST /cron-jobs/{id}/run` の応答。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CronRunResult {
    pub job_id: CronJobId,
    pub job_name: String,
    /// この実行で書いた履歴（古い順。手動実行では 1 件）。
    pub runs: Vec<CronJobRun>,
    /// 作った task（`outcome = created` のときだけ）。
    pub task_id: Option<TaskId>,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/cron-jobs", get(list_jobs).post(create_job))
        .route(
            "/api/v1/cron-jobs/{id}",
            get(get_job).patch(patch_job).delete(delete_job),
        )
        .route("/api/v1/cron-jobs/{id}/pause", post(pause_job))
        .route("/api/v1/cron-jobs/{id}/resume", post(resume_job))
        .route("/api/v1/cron-jobs/{id}/run", post(run_job))
        .route("/api/v1/cron-jobs/{id}/runs", get(list_runs))
}

fn cron_job_not_found(key: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "cron_job_not_found",
        format!("cron job {key:?} not found"),
    )
}

/// cron job の操作の失敗 → HTTP。`StoreError::InUse` は名前の重複（`cron_job`）と同じ予定時刻の二重記録
/// （`cron_job_run`）を 409 に分け、他は既存の写像（検証は 422）に任せる。
fn cron_problem(store: &dyn TaskStore, err: task_ops::OpsError, trigger: &str) -> ApiProblem {
    match err {
        task_ops::OpsError::Store(StoreError::InUse { kind, id, detail })
            if kind == "cron_job" || kind == "cron_job_run" =>
        {
            let code = if kind == "cron_job" {
                "cron_job_name_in_use"
            } else {
                "cron_run_conflict"
            };
            ApiProblem::new(StatusCode::CONFLICT, code, format!("{kind} {id}: {detail}"))
        }
        other => ops_problem(store, other, Some(trigger)),
    }
}

/// `{id}`（ULID か `name`）を job に解決する。無ければ 404。
fn resolve(store: &SqliteStore, key: &str) -> Result<CronJob, ApiProblem> {
    task_ops::cron_jobs::resolve_job(store, key)
        .map_err(|e| cron_problem(store, e, "cron_job_get"))?
        .ok_or_else(|| cron_job_not_found(key))
}

fn view(store: &SqliteStore, job: CronJob) -> Result<CronJobView, ApiProblem> {
    let last_run = store
        .cron_job_runs(job.id, Some(1))
        .map_err(crate::problem::store_problem)?
        .into_iter()
        .next();
    Ok(CronJobView { job, last_run })
}

fn fire_context(state: &ApiState) -> (Vec<task_core::RoleSpec>, Vec<task_core::GenreSpec>) {
    (state.inner.roles.clone(), state.inner.genres.clone())
}

async fn list_jobs(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let list = state
        .blocking(move |store| {
            let jobs = store
                .cron_job_list()
                .map_err(crate::problem::store_problem)?;
            let items = jobs
                .into_iter()
                .map(|job| view(store, job))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CronJobList { items })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn create_job(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let payload: CronJobCreateBody = read_json(body, false).await?;
    let (roles, genres) = fire_context(&state);
    let created = state
        .blocking(move |store| {
            let ctx = CronFireContext {
                roles: &roles,
                genres: &genres,
                ..CronFireContext::default()
            };
            let new = NewCronJob {
                name: payload.name,
                schedule: payload.schedule,
                timezone: payload.timezone,
                overlap: payload.overlap,
                catch_up: payload.catch_up,
                enabled: payload.enabled,
                template: payload.template,
            };
            let job = task_ops::cron_jobs::create_job(store, &ctx, new, OffsetDateTime::now_utc())
                .map_err(|e| cron_problem(store, e, "cron_job_create"))?;
            view(store, job)
        })
        .await?;
    tracing::info!(op = "cron_job_create", cron_job_id = %created.job.id, name = %created.job.name, "cron job created");
    Ok(json_response(StatusCode::CREATED, &created))
}

async fn get_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let found = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            view(store, job)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &found))
}

async fn patch_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let payload: CronJobPatchBody = read_json(body, true).await?;
    let (roles, genres) = fire_context(&state);
    let updated = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            let ctx = CronFireContext {
                roles: &roles,
                genres: &genres,
                ..CronFireContext::default()
            };
            let patch = CronJobPatch {
                name: payload.name,
                schedule: payload.schedule,
                timezone: payload.timezone,
                overlap: payload.overlap,
                catch_up: payload.catch_up,
                template: payload.template,
            };
            let job = task_ops::cron_jobs::update_job(
                store,
                &ctx,
                job.id,
                patch,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| cron_problem(store, e, "cron_job_update"))?;
            view(store, job)
        })
        .await?;
    tracing::info!(op = "cron_job_update", cron_job_id = %updated.job.id, "cron job updated");
    Ok(json_response(StatusCode::OK, &updated))
}

async fn delete_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let deleted = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            match task_ops::cron_jobs::delete_job(store, job.id) {
                Ok(true) => Ok(job.id),
                Ok(false) => Err(cron_job_not_found(&id)),
                Err(e) => Err(cron_problem(store, e, "cron_job_delete")),
            }
        })
        .await?;
    tracing::info!(op = "cron_job_delete", cron_job_id = %deleted, "cron job deleted");
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn pause_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let paused = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            let job = task_ops::cron_jobs::pause_job(store, job.id, OffsetDateTime::now_utc())
                .map_err(|e| cron_problem(store, e, "cron_job_pause"))?;
            view(store, job)
        })
        .await?;
    tracing::info!(op = "cron_job_pause", cron_job_id = %paused.job.id, "cron job paused");
    Ok(json_response(StatusCode::OK, &paused))
}

async fn resume_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let resumed = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            let job = task_ops::cron_jobs::resume_job(store, job.id, OffsetDateTime::now_utc())
                .map_err(|e| cron_problem(store, e, "cron_job_resume"))?;
            view(store, job)
        })
        .await?;
    tracing::info!(op = "cron_job_resume", cron_job_id = %resumed.job.id, "cron job resumed");
    Ok(json_response(StatusCode::OK, &resumed))
}

async fn run_job(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let (roles, genres) = fire_context(&state);
    let result = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            let ctx = CronFireContext {
                roles: &roles,
                genres: &genres,
                ..CronFireContext::default()
            };
            let out =
                task_ops::cron_jobs::run_now_with(store, &ctx, job.id, OffsetDateTime::now_utc())
                    .map_err(|e| cron_problem(store, e, "cron_job_run"))?;
            Ok(CronRunResult {
                job_id: out.job_id,
                job_name: out.job_name,
                runs: out.runs,
                task_id: out.task_id,
            })
        })
        .await?;
    tracing::info!(op = "cron_job_run", cron_job_id = %result.job_id, task_id = ?result.task_id, "cron job run manually");
    Ok(json_response(StatusCode::OK, &result))
}

async fn list_runs(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["limit"])?;
    let limit = query.limit("limit", RUNS_DEFAULT_LIMIT, RUNS_MAX_LIMIT)?;
    let list = state
        .blocking(move |store| {
            let job = resolve(store, &id)?;
            let items = store
                .cron_job_runs(job.id, Some(limit))
                .map_err(crate::problem::store_problem)?;
            Ok(CronJobRunList {
                job_id: job.id,
                items,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_in_use_maps_to_distinct_conflict_codes() {
        let store = SqliteStore::open_in_memory().expect("store");
        let in_use = |kind: &'static str| {
            task_ops::OpsError::Store(StoreError::InUse {
                kind,
                id: "x".to_string(),
                detail: "already".to_string(),
            })
        };
        let name = cron_problem(&store, in_use("cron_job"), "t");
        assert_eq!(name.status(), StatusCode::CONFLICT);
        assert_eq!(name.code(), "cron_job_name_in_use");
        let run = cron_problem(&store, in_use("cron_job_run"), "t");
        assert_eq!(run.status(), StatusCode::CONFLICT);
        assert_eq!(run.code(), "cron_run_conflict");
    }
}
