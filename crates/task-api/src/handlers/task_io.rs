//! タスクの読み取り: イベント・run の出力・成果物（`/tasks/{id}/{events,runs,artifacts}`、`GET /events`）。

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_core::{EventRow, SqliteStore, StoreError, Task, TaskId, TaskStore};

use crate::files::{self, FileRequest, FileTarget, RunFile};
use crate::problem::{ApiProblem, store_problem};
use crate::query::{QueryParams, event_type_name, parse_task_id};
use crate::state::ApiState;
use crate::types::{ArtifactList, EventsPage, RunList, WorkUnitCheckLog};

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

// ---- 2026-10-04 統合の検査の進み具合 D3: GET /tasks/{id}/work-units/{wu_id}/check-log ----

/// 既定で返すログの末尾の大きさ。
const CHECK_LOG_DEFAULT_BYTES: u64 = 16 * 1024;
/// 返すログの末尾の上限（`bytes` はここで頭打ち）。
const CHECK_LOG_MAX_BYTES: u64 = 64 * 1024;

pub(super) async fn work_unit_check_log(
    State(state): State<ApiState>,
    Params((id, wu_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["index", "bytes"])?;
    let id = parse_task_id(&id)?;
    let index = query
        .u64("index")?
        .map(|i| {
            u32::try_from(i)
                .map_err(|_| ApiProblem::bad_request("query parameter `index` is too large"))
        })
        .transpose()?;
    let bytes = query
        .u64("bytes")?
        .unwrap_or(CHECK_LOG_DEFAULT_BYTES)
        .clamp(1, CHECK_LOG_MAX_BYTES);
    let log = state
        .blocking(move |store| {
            load_task(store, id)?;
            let events = store.events_for(id).map_err(store_problem)?;
            check_log_of(&events, &wu_id, index, bytes)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &log))
}

/// events（seq 順）から WU の検査の開始を引き、その `log_path` の末尾を読む。path は event から引き、要求からは取らない。
fn check_log_of(
    events: &[(u64, task_core::Event)],
    wu_id: &str,
    index: Option<u32>,
    bytes: u64,
) -> Result<WorkUnitCheckLog, ApiProblem> {
    use task_core::Event;
    // 2026-10-04 WU 検査の引き継ぎ D3: 葉の WU の受け入れ検査（`WorkUnitCheckStarted`）も同じく読む。
    let started = events.iter().rposition(|(_, e)| {
        matches!(e, Event::IntegrationCheckStarted { work_unit_id, index: i, .. }
            | Event::WorkUnitCheckStarted { work_unit_id, index: i, .. }
            if work_unit_id == wu_id && index.is_none_or(|want| want == *i))
    });
    let Some(pos) = started else {
        return Err(ApiProblem::file_not_found(format!(
            "work unit {wu_id} has no check{}",
            index
                .map(|i| format!(" with index {i}"))
                .unwrap_or_default()
        )));
    };
    let (Event::IntegrationCheckStarted {
        work_unit_id,
        key,
        index,
        total,
        cmd,
        log_path,
        started_at,
    }
    | Event::WorkUnitCheckStarted {
        work_unit_id,
        key,
        index,
        total,
        cmd,
        log_path,
        started_at,
        ..
    }) = &events[pos].1
    else {
        return Err(ApiProblem::not_found());
    };
    let finished = events[pos + 1..].iter().find_map(|(_, e)| match e {
        Event::IntegrationCheckFinished {
            work_unit_id: w,
            index: i,
            pass,
            exit,
            duration_ms,
            ..
        }
        | Event::WorkUnitCheckFinished {
            work_unit_id: w,
            index: i,
            pass,
            exit,
            duration_ms,
            ..
        } if w == work_unit_id && i == index => Some((*pass, *exit, *duration_ms)),
        _ => None,
    });
    let (size, truncated, tail) = read_log_tail(std::path::Path::new(log_path), bytes)
        .map_err(|e| ApiProblem::file_not_found(format!("cannot read the check log: {e}")))?;
    Ok(WorkUnitCheckLog {
        work_unit_id: work_unit_id.clone(),
        key: key.clone(),
        index: *index,
        total: *total,
        cmd: cmd.clone(),
        started_at: started_at.clone(),
        running: finished.is_none(),
        pass: finished.map(|f| f.0),
        exit: finished.and_then(|f| f.1),
        duration_ms: finished.map(|f| f.2),
        size,
        truncated,
        tail,
    })
}

/// ファイルの末尾 `bytes` を読む（無ければ空）。前を切ったときは UTF-8 の途中のバイトを捨てる。
fn read_log_tail(path: &std::path::Path, bytes: u64) -> std::io::Result<(u64, bool, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, false, String::new()));
        }
        Err(e) => return Err(e),
    };
    let size = file.metadata()?.len();
    let start = size.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.take(bytes).read_to_end(&mut buf)?;
    let mut skip = 0;
    if start > 0 {
        while skip < buf.len() && skip < 3 && (buf[skip] & 0b1100_0000) == 0b1000_0000 {
            skip += 1;
        }
    }
    Ok((
        size,
        start > 0,
        String::from_utf8_lossy(&buf[skip..]).into_owned(),
    ))
}

#[cfg(test)]
mod check_log_tests {
    use super::*;
    use task_core::Event;

    fn started(wu: &str, index: u32, path: &std::path::Path) -> Event {
        Event::IntegrationCheckStarted {
            work_unit_id: wu.into(),
            key: "integrate-p1".into(),
            index,
            total: 2,
            cmd: format!("check-{index}"),
            log_path: path.display().to_string(),
            started_at: "2026-10-04T02:09:00Z".into(),
        }
    }

    #[test]
    fn the_latest_started_check_is_running_until_its_finish_and_the_tail_is_utf8_safe() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("0.log");
        let p1 = dir.path().join("1.log");
        std::fs::write(&p0, "done-output\n").unwrap();
        // 「あ」は 3 バイト。末尾 4 バイトだと先頭が途中のバイトになる。
        std::fs::write(&p1, "xxあい").unwrap();
        let mut events = vec![
            (1, started("w", 0, &p0)),
            (
                2,
                Event::IntegrationCheckFinished {
                    work_unit_id: "w".into(),
                    key: "integrate-p1".into(),
                    index: 0,
                    total: 2,
                    cmd: "check-0".into(),
                    pass: false,
                    exit: Some(3),
                    timed_out: false,
                    duration_ms: 42,
                },
            ),
            (3, started("w", 1, &p1)),
        ];
        let running = check_log_of(&events, "w", None, 4).unwrap();
        assert_eq!(running.index, 1);
        assert!(running.running);
        assert!(running.truncated);
        assert_eq!(running.tail, "い");
        assert_eq!(running.size, 8);
        let first = check_log_of(&events, "w", Some(0), 1024).unwrap();
        assert!(!first.running);
        assert_eq!(
            (first.pass, first.exit, first.duration_ms),
            (Some(false), Some(3), Some(42))
        );
        assert_eq!(first.tail, "done-output\n");
        assert!(!first.truncated);
        // まだファイルが無い（出力がまだ無い）検査は空の末尾。
        events.push((4, started("w", 0, &dir.path().join("missing.log"))));
        let empty = check_log_of(&events, "w", None, 1024).unwrap();
        assert_eq!((empty.size, empty.tail.as_str()), (0, ""));
        assert!(check_log_of(&events, "other", None, 1024).is_err());
    }
}
