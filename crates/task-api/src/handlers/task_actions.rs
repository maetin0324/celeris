//! タスクへの操作（approve/accept/reject/answer/cancel/retry、PATCH・コメント・reopen・rereview）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use task_core::TaskStore;
use time::OffsetDateTime;

use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::parse_task_id;
use crate::state::ApiState;
use crate::types::{
    AnswerBody, CancelBody, CommentBody, CommentList, DecisionBody, ReopenBody, RetryBody,
    ValidationError,
};

use super::{ApiResult, Params, json_response, no_query, read_json, validated_workspace};

/// PATCH /tasks/{id}: omitted hint leaves it unchanged; null or [] clears it.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskPatchBody {
    #[serde(flatten)]
    pub task: task_ops::edit::TaskEdit,
    #[serde(default, deserialize_with = "present_write_paths")]
    pub expected_write_paths: Option<Option<Vec<String>>>,
}

fn present_write_paths<'de, D>(deserializer: D) -> Result<Option<Option<Vec<String>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer).map(Some)
}

// ---- 13〜16. POST /tasks/{id}/{approve|reject|answer|cancel} ----
//
// ADR-0044 §5 Phase 53 追記（Phase 55）: **変更を伴う API はすべて管理系**（`token_file` 未設定でも 401）。
// この節の 4 つと `retry` / `POST /tasks` / `POST /plans` / `POST /replay`、案件・途中目標の変更系が
// この Phase で `require_admin` に揃った。認可は本文の検証より**先**（トークン無しの壊れた本文は 401）。

pub(super) async fn approve(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let DecisionBody {
        note,
        expected_status,
    } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            task_ops::gate::approve(store, id, note, expected_status)
                .map_err(|e| ops_problem(store, e, Some("approve")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// ADR-0070 D2 追記（Phase 116。本番で確認: `retry` の `accept` を明示しないと `draft` のまま止まり、
/// `draft` を `ready` にする専用の道具が `approve` しか無くわかりにくかった）: `status == Draft` だけを
/// 許す `task_ops::gate::accept` の薄いラッパー。
pub(super) async fn accept(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let ReopenBody { expected_status } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            task_ops::gate::accept(store, id, expected_status)
                .map_err(|e| ops_problem(store, e, Some("accept")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

pub(super) async fn reject(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let DecisionBody {
        note,
        expected_status,
    } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            task_ops::gate::reject(store, id, note, expected_status)
                .map_err(|e| ops_problem(store, e, Some("reject")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

pub(super) async fn answer(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let AnswerBody {
        answer,
        expected_status,
    } = read_json(body, false).await?;
    if answer.trim().is_empty() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("answer".to_string()),
            message: "answer must not be blank".to_string(),
        }]));
    }
    let result = state
        .blocking(move |store| {
            task_ops::gate::answer(store, id, answer, expected_status)
                .map_err(|e| ops_problem(store, e, Some("answer")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

pub(super) async fn cancel(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let CancelBody { expected_status } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            let result = task_ops::gate::cancel(store, id, expected_status)
                .map_err(|e| ops_problem(store, e, Some("cancel")))?;
            crate::browser_control::stop_task(store, &id.to_string())?;
            Ok(result)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// Phase 31（実機の事故、2026-09-18）: `failed`/`cancelled` を複製してやり直す。
pub(super) async fn retry(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let RetryBody {
        accept,
        workspace,
        execution,
    } = read_json(body, true).await?;
    // ADR-0062 Phase 108: `workspace` の検証は `PATCH /tasks/{id}` と同じ（先に 422 を返す）。
    let workspace = workspace
        .map(|spec| validated_workspace(&state, spec))
        .transpose()?;
    let result = state
        .blocking(move |store| {
            task_ops::retry::retry_task_with_execution(
                store,
                id,
                accept,
                workspace,
                execution,
                "human",
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| ops_problem(store, e, Some("retry")))
        })
        .await?;
    let mut response = json_response(StatusCode::CREATED, &result);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/tasks/{}", result.task_id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

// ---- ADR-0044 D1/D2（Phase 53）: 編集・コメント・再開 ----

/// `PATCH /tasks/{id}`（**管理系**。ADR-0044 D1）。書いた項目だけを変える。終端のタスクは 409。
/// `running` / `reviewing` は受け付けるが**次の run から効く**（走っている run は止めない）。
pub(super) async fn patch_task(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let TaskPatchBody {
        mut task,
        expected_write_paths,
    } = read_json(body, true).await?;
    let paths = expected_write_paths
        .map(|paths| {
            paths
                .map(|paths| task_core::write_set::normalize_write_paths(&paths))
                .transpose()
        })
        .transpose()
        .map_err(ApiProblem::bad_request)?;
    if task.is_empty() && paths.is_none() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: None,
            message: "at least one field must be given".to_string(),
        }]));
    }
    // ADR-0062 Phase 108: `workspace` の検証（`Remote.cluster` が設定にあること、`~` の展開）は
    // `PATCH /projects/{id}` と同じ `validated_workspace` を使う（422 はここで返す）。
    if let Some(spec) = task.workspace.take() {
        task.workspace = Some(validated_workspace(&state, spec)?);
    }
    let genres = state.inner.genres.clone();
    let result = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let result = if task.is_empty() {
                let existing = store
                    .get(id)
                    .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))?
                    .ok_or_else(|| ApiProblem::task_not_found(id))?;
                if let Some(expected) = task.expected_status
                    && expected != existing.status
                {
                    return Err(ops_problem(
                        store,
                        task_ops::OpsError::Conflict {
                            expected,
                            actual: existing.status,
                        },
                        Some("edit"),
                    ));
                }
                if existing.status.is_terminal() {
                    return Err(ops_problem(
                        store,
                        task_ops::OpsError::InvalidState {
                            id,
                            context: format!("status={:?}", existing.status),
                            action: "edited; terminal tasks cannot be edited".into(),
                        },
                        Some("edit"),
                    ));
                }
                task_ops::edit::EditResult {
                    task: existing,
                    fields: vec![],
                }
            } else {
                task_ops::edit::edit_task(store, id, task, &genres, now)
                    .map_err(|e| ops_problem(store, e, Some("edit")))?
            };
            if let Some(paths) = &paths {
                store
                    .set_task_expected_write_paths(id, paths.as_deref(), &now.to_string())
                    .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))?;
            }
            Ok(result)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// `GET /tasks/{id}/comments`（読み取り。古い順）。
pub(super) async fn list_comments(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let id = parse_task_id(&id)?;
    let items = state
        .blocking(move |store| {
            task_ops::comment::list_comments(store, id).map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &CommentList { items }))
}

/// `POST /tasks/{id}/comments`（**管理系**。ADR-0044 D2）。人のコメントは状態に応じて
/// 割り込み（`running`/`reviewing`）・回答（`blocked`）・記録（その他）になる。
pub(super) async fn create_comment(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let CommentBody { body } = read_json(body, false).await?;
    let result = state
        .blocking(move |store| {
            task_ops::comment::post_human_comment(store, id, body, OffsetDateTime::now_utc())
                .map_err(|e| ops_problem(store, e, Some("comment")))
        })
        .await?;
    Ok(json_response(StatusCode::CREATED, &result))
}

/// `POST /tasks/{id}/reopen`（**管理系**。ADR-0044 D2）。`done` / `failed` を `ready` に戻す
/// （attempts は 0）。`cancelled` は worktree を消してあるので 409（`retry` を使う）。
pub(super) async fn reopen(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let ReopenBody { expected_status } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            task_ops::comment::reopen(store, id, expected_status)
                .map_err(|e| ops_problem(store, e, Some("reopen")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

pub(super) async fn rereview(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let id = parse_task_id(&id)?;
    let ReopenBody { expected_status } = read_json(body, true).await?;
    let result = state
        .blocking(move |store| {
            task_ops::comment::rereview(store, id, expected_status)
                .map_err(|e| ops_problem(store, e, Some("rereview")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}
