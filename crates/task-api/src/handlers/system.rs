//! daemon 全体の読み取りと管理（health・inbox・plans・replay・graph・daemon・metrics・config・schema）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use task_core::{Task, TaskStore};
use time::OffsetDateTime;

use crate::API_VERSION;
use crate::files;
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::query::QueryParams;
use crate::schema::API_V1_SCHEMA_JSON;
use crate::state::ApiState;
use crate::types::{DaemonView, DbInfo, Health};

use super::providers::current_providers;
use super::{ApiResult, json_response, no_query, now_rfc3339, read_json};

// ---- 1. GET /health ----

pub(super) async fn health(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let schema_version = state
        .blocking(|store| store.schema_version().map_err(store_problem))
        .await?;
    let inner = &state.inner;
    Ok(json_response(
        StatusCode::OK,
        &Health {
            api_version: API_VERSION.to_string(),
            schema_version,
            celeris_version: inner.celeris_version.clone(),
            instance_id: inner.instance_id.clone(),
            started_at: inner.started_at.clone(),
            now: now_rfc3339(),
            db: DbInfo {
                journal_mode: inner.journal_mode.clone(),
                busy_timeout_ms: inner.busy_timeout_ms,
                filesystem: inner.db_mount.as_ref().map(|m| m.fstype.clone()),
                device: inner.db_mount.as_ref().map(|m| m.source.clone()),
            },
            // ADR-0040 D3 / D4: 検証（`verify.sh`）と昇格（`promote.sh`）が「どの版がどの役割で動いて
            // いるか」をここだけで判定できるようにする。
            release: inner.release.clone(),
            mode: inner.mode.as_str().to_string(),
            role: inner.role.get().as_str().to_string(),
        },
    ))
}

// ---- 2. GET /inbox ----

pub(super) async fn inbox(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let snapshot = state.snapshot();
    let ctx = state.inner.view.clone();
    let inbox = state
        .blocking(move |store| {
            let root = ctx.workspace_root.clone();
            let mut inbox = task_ops::inbox::inbox(
                store,
                snapshot.as_ref(),
                &ctx,
                OffsetDateTime::now_utc(),
                &|task: &Task, run_id: &str| files::read_evidence(task, &root, run_id),
            )
            .map_err(|e| ops_problem(store, e, None))?;
            for item in &mut inbox.approvals {
                let (Some(parent), Some(run)) = (item.parent.as_ref(), item.last_run.as_mut())
                else {
                    continue;
                };
                if run.files.is_none()
                    && let Some(task) = store.get(parent.id).map_err(store_problem)?
                {
                    run.files = Some(files::run_files(&task, &root, &run.run_id));
                }
            }
            Ok(inbox)
        })
        .await?;
    Ok(crate::inbox_notifications::deprecated(
        json_response(StatusCode::OK, &inbox),
        "</api/v1/inbox/items>; rel=\"successor-version\"",
    ))
}

// ---- 17. POST /plans ----

/// ADR-0079 U-R6（Phase R5a）: ADR-0028 の `POST /plans`（Plan kind の分解）は 410。分解は root task の gate と
/// planner が行う（`POST /tasks` で root task を作る）。既存の `kind = plan` の行と子はそのまま読める。
pub(super) async fn create_plan(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        "ADR-0079: POST /plans は廃止。分解は root task の Complexity Gate と planner が行う",
        "POST /api/v1/tasks (a root task; name the stages in stages_hint)",
    ))
}

// ---- 18. POST /replay ----

/// `POST /replay` の本文は `{}`（空本体も可）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayBody {}

pub(super) async fn replay(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let ReplayBody {} = read_json(body, true).await?;
    let guard = state
        .try_begin_replay()
        .ok_or_else(ApiProblem::replay_in_progress)?;
    let report = state
        .blocking(move |store| {
            // 要求が切断されても replay が終わるまで枠を持つ。
            let _guard = guard;
            task_ops::replay::replay(store).map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &report))
}

// ---- 19. GET /graph ----

pub(super) async fn graph(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["root", "depth", "include_terminal"])?;
    let root = query.task_id("root")?;
    let depth = query
        .u64("depth")?
        .map(|d| {
            u32::try_from(d)
                .map_err(|_| ApiProblem::bad_request("query parameter `depth` is too large"))
        })
        .transpose()?;
    let include_terminal = query.bool("include_terminal")?.unwrap_or(true);
    let graph = state
        .blocking(move |store| {
            task_ops::graph::graph(store, root, depth, include_terminal)
                .map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &graph))
}

// ---- 24. GET /daemon, 25. GET /config, 26. GET /schema ----

pub(super) async fn daemon(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    Ok(json_response(
        StatusCode::OK,
        &DaemonView {
            now: now_rfc3339(),
            // ADR-0033 D3: `reports` だけは API が埋める（`last_notified_at` は API のメモリにある）。
            snapshot: daemon_snapshot_with_reports(&state),
        },
    ))
}

/// ADR-0075 D6（Phase G1）: `GET /metrics/scratch`。ディスパッチャが tick ごとに組んだ `DaemonSnapshot.scratch`
/// （`celeris.scratch-status/1`）をそのまま返す。スナップショットが無い（daemon が動いていない）・scratch を持たない
/// 構成（`shared_build_cache = false`）は 404 `scratch_unavailable`（`celerisctl scratch status` は daemon 無しでも出せる）。
pub(super) async fn metrics_scratch(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    match state.snapshot().and_then(|s| s.scratch) {
        Some(status) => Ok(json_response(StatusCode::OK, &status)),
        None => Err(ApiProblem::new(
            StatusCode::NOT_FOUND,
            "scratch_unavailable",
            "no scratch status yet (the daemon has not published a snapshot, or [workspace] shared_build_cache is off); use `celerisctl scratch status`",
        )),
    }
}

/// ADR-0033 D3 / D5: ディスパッチャのスナップショットに、秘書レベルの未読の報告・通知の判定・未決定の
/// 認可の件数を載せる。
fn daemon_snapshot_with_reports(state: &ApiState) -> Option<task_ops::daemon::DaemonSnapshot> {
    let mut snapshot = state.snapshot()?;
    snapshot.reports =
        crate::reports::reports_live(&state.inner.store, crate::reports::last_notified_at(state));
    snapshot.approvals_pending = crate::approvals::approvals_pending(&state.inner.store);
    snapshot.decisions_open =
        task_ops::decision::open_count(state.inner.store.as_ref()).unwrap_or(0);
    Some(snapshot)
}

pub(super) async fn config(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let snapshot = state.snapshot();
    let mut view = state.inner.config_view.clone();
    view.providers = current_providers(&state, snapshot.as_ref());
    Ok(json_response(StatusCode::OK, &view))
}

pub(super) async fn schema(RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let mut response = Response::new(Body::from(API_V1_SCHEMA_JSON));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/schema+json"),
    );
    Ok(response)
}
