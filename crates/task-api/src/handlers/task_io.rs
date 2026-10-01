//! タスクの読み取り: イベント・run の出力・成果物（`/tasks/{id}/{events,runs,artifacts}`、`GET /events`）。

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_core::{EventRow, SqliteStore, StoreError, Task, TaskId, TaskStore};

use crate::files::{self, FileRequest, FileTarget, RunFile};
use crate::problem::{ApiProblem, store_problem};
use crate::query::{QueryParams, event_type_name, parse_task_id};
use crate::state::ApiState;
use crate::types::{ArtifactList, EventsPage, RunList};

use super::{ApiResult, Params, json_response, no_query};

fn load_task(store: &SqliteStore, id: TaskId) -> Result<Task, ApiProblem> {
    store
        .get(id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::task_not_found(id))
}

// ---- 6. GET /tasks/{id}/events, 20. GET /events ----

/// `fetch(after, batch)` で読み進め、`keep` に合う行を `limit` 件まで集める。`key` は次の `after` にする値。
fn collect_events(
    limit: usize,
    filtered: bool,
    keep: impl Fn(&EventRow) -> bool,
    key: fn(&EventRow) -> u64,
    initial_after: Option<u64>,
    mut fetch: impl FnMut(Option<u64>, usize) -> Result<Vec<EventRow>, StoreError>,
) -> Result<EventsPage, StoreError> {
    let batch = if filtered {
        limit.saturating_add(1).max(1_000)
    } else {
        limit.saturating_add(1)
    };
    let mut items = Vec::new();
    let mut after = initial_after;
    loop {
        let rows = fetch(after, batch)?;
        let exhausted = rows.len() < batch;
        for row in rows {
            after = Some(key(&row));
            if keep(&row) {
                if items.len() == limit {
                    return Ok(EventsPage {
                        items,
                        has_more: true,
                    });
                }
                items.push(row);
            }
        }
        if exhausted {
            return Ok(EventsPage {
                items,
                has_more: false,
            });
        }
    }
}

pub(super) async fn task_events(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["after_seq", "limit", "types"])?;
    let id = parse_task_id(&id)?;
    let after_seq = match query.i64("after_seq")? {
        None | Some(-1) => None,
        Some(n) if n >= 0 => Some(n.unsigned_abs()),
        Some(_) => {
            return Err(ApiProblem::bad_request(
                "query parameter `after_seq` must be -1 or greater",
            ));
        }
    };
    let limit = query.limit("limit", 500, 5_000)?;
    let types = query.event_types()?;
    let page = state
        .blocking(move |store| {
            load_task(store, id)?;
            collect_events(
                limit,
                types.is_some(),
                |row| {
                    types
                        .as_ref()
                        .is_none_or(|t| t.contains(event_type_name(&row.event)))
                },
                |row| row.seq,
                after_seq,
                |after, batch| store.event_rows_for(id, after, batch),
            )
            .map_err(store_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &page))
}

pub(super) async fn events(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["after_id", "limit", "task_id", "types"])?;
    let after_id = query.u64("after_id")?.unwrap_or(0);
    let limit = query.limit("limit", 500, 5_000)?;
    let task_id = query.task_id("task_id")?;
    let types = query.event_types()?;
    let page = state
        .blocking(move |store| {
            let type_ok = |row: &EventRow| {
                types
                    .as_ref()
                    .is_none_or(|t| t.contains(event_type_name(&row.event)))
            };
            match task_id {
                Some(task_id) => collect_events(
                    limit,
                    true,
                    |row| row.id > after_id && type_ok(row),
                    |row| row.seq,
                    None,
                    |after, batch| store.event_rows_for(task_id, after, batch),
                ),
                None => collect_events(
                    limit,
                    types.is_some(),
                    type_ok,
                    |row| row.id,
                    Some(after_id),
                    |after, batch| store.events_since(after.unwrap_or(after_id), batch),
                ),
            }
            .map_err(store_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &page))
}

// ---- 7. GET /tasks/{id}/runs ----

pub(super) async fn task_runs(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let id = parse_task_id(&id)?;
    let root = state.inner.view.workspace_root.clone();
    let list = state
        .blocking(move |store| {
            let task = load_task(store, id)?;
            let rows = store
                .event_rows_for(id, None, usize::MAX)
                .map_err(store_problem)?;
            let mut runs = task_ops::view::runs(&rows);
            for run in &mut runs {
                run.files = Some(files::run_files(&task, &root, &run.run_id));
            }
            Ok(RunList { runs })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

// ---- 8〜10 + 32. GET /tasks/{id}/runs/{run_id}/{stdout|stderr|result|request} ----

pub(super) async fn run_stdout(
    State(state): State<ApiState>,
    Params(params): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    run_file(state, params, raw, headers, RunFile::Stdout).await
}

pub(super) async fn run_stderr(
    State(state): State<ApiState>,
    Params(params): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    run_file(state, params, raw, headers, RunFile::Stderr).await
}

pub(super) async fn run_result(
    State(state): State<ApiState>,
    Params(params): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    run_file(state, params, raw, headers, RunFile::Result).await
}

/// ADR-0023 M1: claude-code / codex が実際に渡したプロンプト文面。
pub(super) async fn run_prompt(
    State(state): State<ApiState>,
    Params(params): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    run_file(state, params, raw, headers, RunFile::Prompt).await
}

/// ADR-0023 D2: ワーカーに渡した `RunRequest`。
pub(super) async fn run_request(
    State(state): State<ApiState>,
    Params(params): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    run_file(state, params, raw, headers, RunFile::Request).await
}

async fn run_file(
    state: ApiState,
    (id, run_id): (String, String),
    raw: Option<String>,
    headers: HeaderMap,
    file: RunFile,
) -> ApiResult {
    let request = FileRequest::parse(raw.as_deref(), &headers)?;
    let id = parse_task_id(&id)?;
    let root = state.inner.view.workspace_root.clone();
    let target = state
        .blocking(move |store| {
            let task = load_task(store, id)?;
            let path = files::resolve_run_file(&task, &root, &run_id, file)?;
            let size = files::file_size(&path)?;
            Ok(FileTarget {
                path,
                size,
                recorded_sha256: None,
                current_sha256: None,
            })
        })
        .await?;
    files::respond_file(target, &request).await
}

// ---- 11. GET /tasks/{id}/artifacts, 12. GET /tasks/{id}/artifacts/{idx} ----

pub(super) async fn artifact_list(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let id = parse_task_id(&id)?;
    let root = state.inner.view.workspace_root.clone();
    let list = state
        .blocking(move |store| {
            let task = load_task(store, id)?;
            let rows = store
                .event_rows_for(id, None, usize::MAX)
                .map_err(store_problem)?;
            Ok(ArtifactList {
                items: files::artifact_views(&task, &root, &rows),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

pub(super) async fn artifact_body(
    State(state): State<ApiState>,
    Params((id, idx)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    let request = FileRequest::parse(raw.as_deref(), &headers)?;
    let id = parse_task_id(&id)?;
    let idx: usize = idx
        .parse()
        .map_err(|_| ApiProblem::bad_request("artifact index must be a non-negative integer"))?;
    let root = state.inner.view.workspace_root.clone();
    let target = state
        .blocking(move |store| {
            let task = load_task(store, id)?;
            let rows = store
                .event_rows_for(id, None, usize::MAX)
                .map_err(store_problem)?;
            let artifact = files::nth_artifact(&rows, idx)
                .ok_or_else(|| ApiProblem::artifact_not_found(idx))?;
            let ws = files::canonical_workspace(&task, &root)?;
            let path = files::resolve_artifact(&ws, &artifact.path)?;
            let size = files::file_size(&path)?;
            let current_sha256 = files::current_sha256(&path, size);
            Ok(FileTarget {
                path,
                size,
                recorded_sha256: Some(artifact.sha256.clone()),
                current_sha256,
            })
        })
        .await?;
    files::respond_file(target, &request).await
}
