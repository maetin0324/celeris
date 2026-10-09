//! ADR-0131 D5: 定期実行（cron job）の API（`docs/api/cron-jobs.md`）。
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

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::problem::{ApiProblem, ops_problem};
use crate::query::QueryParams;
use crate::state::ApiState;
use task_core::chat::ChatError;

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

/// The audited form of a cron job write (ADR 2026-10-09-cos-operations-all-mutations D3).
type Audit<'a> = Option<&'a OperationAudit>;

fn reject_with(
    store: &SqliteStore,
    audit: Audit<'_>,
    target: &str,
    problem: ApiProblem,
) -> ApiProblem {
    match audit {
        Some(audit) => audit.reject(store, "cron_job", target, problem),
        None => problem,
    }
}

/// A store failure inside the audit transaction. A duplicate name stays a 409.
fn tx_error(error: StoreError) -> ChatError {
    match error {
        StoreError::InUse { kind, id, detail } => {
            ChatError::Conflict(format!("{kind} {id}: {detail}"))
        }
        other => ChatError::Store(other),
    }
}

fn gone(id: &str) -> ChatError {
    ChatError::NotFound {
        kind: "cron_job",
        id: id.to_string(),
    }
}

fn job_value(job: &CronJob) -> Result<serde_json::Value, ChatError> {
    Ok(serde_json::json!({"job": serde_json::to_value(job)?}))
}

fn resolve_for(store: &SqliteStore, audit: Audit<'_>, key: &str) -> Result<CronJob, ApiProblem> {
    resolve(store, key).map_err(|problem| reject_with(store, audit, key, problem))
}

/// `POST /cron-jobs` shared by the handler and `/cos/operations` (`cron_job.create`).
pub(crate) fn create_job_op(
    store: &SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    payload: CronJobCreateBody,
    audit: Audit<'_>,
) -> Result<Applied<CronJobView>, ApiProblem> {
    let ctx = CronFireContext {
        roles,
        genres,
        ..CronFireContext::default()
    };
    let target = payload.name.clone();
    let new = NewCronJob {
        name: payload.name,
        schedule: payload.schedule,
        timezone: payload.timezone,
        overlap: payload.overlap,
        catch_up: payload.catch_up,
        enabled: payload.enabled,
        template: payload.template,
    };
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let job = task_ops::cron_jobs::create_job(store, &ctx, new, now)
            .map_err(|e| cron_problem(store, e, "cron_job_create"))?;
        return Ok(Applied::Direct(view(store, job)?));
    };
    let job = task_ops::cron_jobs::prepare_create(store, &ctx, new, now).map_err(|e| {
        audit.reject(
            store,
            "cron_job",
            &target,
            cron_problem(store, e, "cron_job_create"),
        )
    })?;
    let operation = audit.apply(
        store,
        "cron_job",
        &job.id.to_string(),
        "cron_job.create",
        |tx| {
            task_core::cron::cron_job_insert_tx(tx, &job).map_err(tx_error)?;
            job_value(&job)
        },
    )?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `PATCH /cron-jobs/{id}` (`cron_job.update`).
pub(crate) fn update_job_op(
    store: &SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    key: &str,
    payload: CronJobPatchBody,
    audit: Audit<'_>,
) -> Result<Applied<CronJobView>, ApiProblem> {
    let job = resolve_for(store, audit, key)?;
    let ctx = CronFireContext {
        roles,
        genres,
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
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let job = task_ops::cron_jobs::update_job(store, &ctx, job.id, patch, now)
            .map_err(|e| cron_problem(store, e, "cron_job_update"))?;
        return Ok(Applied::Direct(view(store, job)?));
    };
    let target = job.id.to_string();
    let job =
        task_ops::cron_jobs::prepare_update(store, &ctx, job.id, patch, now).map_err(|e| {
            audit.reject(
                store,
                "cron_job",
                &target,
                cron_problem(store, e, "cron_job_update"),
            )
        })?;
    let operation = audit.apply(store, "cron_job", &target, "cron_job.update", |tx| {
        if !task_core::cron::cron_job_update_tx(tx, &job).map_err(tx_error)? {
            return Err(gone(&target));
        }
        job_value(&job)
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `DELETE /cron-jobs/{id}` (`cron_job.delete`).
pub(crate) fn delete_job_op(
    store: &SqliteStore,
    key: &str,
    audit: Audit<'_>,
) -> Result<Applied<CronJobId>, ApiProblem> {
    let job = resolve_for(store, audit, key)?;
    let Some(audit) = audit else {
        return match task_ops::cron_jobs::delete_job(store, job.id) {
            Ok(true) => Ok(Applied::Direct(job.id)),
            Ok(false) => Err(cron_job_not_found(key)),
            Err(e) => Err(cron_problem(store, e, "cron_job_delete")),
        };
    };
    let target = job.id.to_string();
    let operation = audit.apply(store, "cron_job", &target, "cron_job.delete", |tx| {
        if !task_core::cron::cron_job_delete_tx(tx, job.id)? {
            return Err(gone(&target));
        }
        Ok(serde_json::json!({"job_id": target, "name": job.name}))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `POST /cron-jobs/{id}/pause` (`cron_job.pause`).
pub(crate) fn pause_job_op(
    store: &SqliteStore,
    key: &str,
    audit: Audit<'_>,
) -> Result<Applied<CronJobView>, ApiProblem> {
    let job = resolve_for(store, audit, key)?;
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let job = task_ops::cron_jobs::pause_job(store, job.id, now)
            .map_err(|e| cron_problem(store, e, "cron_job_pause"))?;
        return Ok(Applied::Direct(view(store, job)?));
    };
    let target = job.id.to_string();
    let (job, queued) = task_ops::cron_jobs::prepare_pause(store, job.id, now).map_err(|e| {
        audit.reject(
            store,
            "cron_job",
            &target,
            cron_problem(store, e, "cron_job_pause"),
        )
    })?;
    let operation = audit.apply(store, "cron_job", &target, "cron_job.pause", |tx| {
        if let Some((run_id, update)) = &queued {
            task_core::cron::cron_job_run_update_tx(tx, *run_id, update)?;
        }
        if !task_core::cron::cron_job_update_tx(tx, &job).map_err(tx_error)? {
            return Err(gone(&target));
        }
        job_value(&job)
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `POST /cron-jobs/{id}/resume` (`cron_job.resume`). An enabled job is recorded unchanged.
pub(crate) fn resume_job_op(
    store: &SqliteStore,
    key: &str,
    audit: Audit<'_>,
) -> Result<Applied<CronJobView>, ApiProblem> {
    let job = resolve_for(store, audit, key)?;
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let job = task_ops::cron_jobs::resume_job(store, job.id, now)
            .map_err(|e| cron_problem(store, e, "cron_job_resume"))?;
        return Ok(Applied::Direct(view(store, job)?));
    };
    let target = job.id.to_string();
    let (job, changed) = task_ops::cron_jobs::prepare_resume(store, job.id, now).map_err(|e| {
        audit.reject(
            store,
            "cron_job",
            &target,
            cron_problem(store, e, "cron_job_resume"),
        )
    })?;
    let operation = audit.apply(store, "cron_job", &target, "cron_job.resume", |tx| {
        if changed && !task_core::cron::cron_job_update_tx(tx, &job).map_err(tx_error)? {
            return Err(gone(&target));
        }
        job_value(&job)
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// `POST /cron-jobs/{id}/run` (`cron_job.run`). A manual run creates a task through several
/// independent commits, so the CoS form is the external-effect protocol (C): the pending record
/// is persisted first, the run happens once, and a resent request returns the recorded
/// operation without firing again. A run that fails after `begin_external` stays pending and is
/// moved to needs_remediation by the stale-pending sweep (its partial effect is unknown).
pub(crate) fn run_job_op(
    store: &SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    key: &str,
    audit: Audit<'_>,
    now: OffsetDateTime,
) -> Result<Applied<CronRunResult>, ApiProblem> {
    let job = resolve_for(store, audit, key)?;
    let ctx = CronFireContext {
        roles,
        genres,
        ..CronFireContext::default()
    };
    let run = || -> Result<CronRunResult, ApiProblem> {
        let out = task_ops::cron_jobs::run_now_with(store, &ctx, job.id, now)
            .map_err(|e| cron_problem(store, e, "cron_job_run"))?;
        Ok(CronRunResult {
            job_id: out.job_id,
            job_name: out.job_name,
            runs: out.runs,
            task_id: out.task_id,
        })
    };
    let Some(audit) = audit else {
        return Ok(Applied::Direct(run()?));
    };
    let target = job.id.to_string();
    let (operation, fresh) =
        audit.begin_external(store, "cron_job", &target, "cron_job.run", now)?;
    if !fresh {
        return Ok(Applied::Audited(Box::new(operation)));
    }
    let result = run()?;
    let value = serde_json::to_value(&result)
        .map_err(|e| ApiProblem::internal(format!("cron run result: {e}")))?;
    let operation = audit.finish_external(store, &value, OffsetDateTime::now_utc())?;
    Ok(Applied::Audited(Box::new(operation)))
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
        .blocking(move |store| create_job_op(store, &roles, &genres, payload, None)?.direct())
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
        .blocking(move |store| update_job_op(store, &roles, &genres, &id, payload, None)?.direct())
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
        .blocking(move |store| delete_job_op(store, &id, None)?.direct())
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
        .blocking(move |store| pause_job_op(store, &id, None)?.direct())
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
        .blocking(move |store| resume_job_op(store, &id, None)?.direct())
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
            run_job_op(store, &roles, &genres, &id, None, OffsetDateTime::now_utc())?.direct()
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
