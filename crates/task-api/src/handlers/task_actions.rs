//! タスクへの操作（approve/accept/reject/answer/cancel/retry、PATCH・コメント・reopen・rereview）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use task_core::TaskStore;
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::parse_task_id;
use crate::state::ApiState;
use crate::types::{
    AnswerBody, CancelBody, CommentBody, CommentList, DecisionBody, ReopenBody, RetryBody,
    ValidationError,
};

use super::{ApiResult, Params, json_response, no_query, read_json};

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

/// Audited counterpart of the gate actions. The state and expected-revision checks are repeated
/// inside the audited write transaction by `apply_transition_tx`; the read below selects whether
/// `approve` means accepting a draft or deciding an Approval task.
pub(crate) fn gate_action_op(
    store: &task_core::store::SqliteStore,
    id: task_core::TaskId,
    action: &str,
    note: Option<String>,
    expected: Option<task_core::Status>,
    audit: &OperationAudit,
) -> Result<Applied<()>, ApiProblem> {
    let target_id = id.to_string();
    let reject = |problem| audit.reject(store, "task", &target_id, problem);
    let task = store
        .get(id)
        .map_err(|error| reject(crate::problem::store_problem(error)))?
        .ok_or_else(|| {
            reject(ops_problem(
                store,
                task_ops::OpsError::NotFound(id),
                Some(action),
            ))
        })?;
    if let Some(expected) = expected
        && task.status != expected
    {
        return Err(reject(ops_problem(
            store,
            task_ops::OpsError::Conflict {
                expected,
                actual: task.status,
            },
            Some(action),
        )));
    }
    let (trigger, extra) = match action {
        "task.accept" if task.status == task_core::Status::Draft => {
            (task_core::Trigger::Accept, Vec::new())
        }
        "task.approve" if task.status == task_core::Status::Draft => {
            (task_core::Trigger::Accept, Vec::new())
        }
        "task.approve"
            if task.kind == task_core::TaskKind::Approval
                && task.status == task_core::Status::Ready =>
        {
            (
                task_core::Trigger::Approve,
                vec![task_core::Event::ApprovalDecided {
                    by: "cos".to_string(),
                    approved: true,
                    note,
                }],
            )
        }
        "task.reject"
            if task.kind == task_core::TaskKind::Approval
                && task.status == task_core::Status::Ready =>
        {
            (
                task_core::Trigger::Reject,
                vec![task_core::Event::ApprovalDecided {
                    by: "cos".to_string(),
                    approved: false,
                    note,
                }],
            )
        }
        "task.cancel"
            if !matches!(
                task.status,
                task_core::Status::Done | task_core::Status::Cancelled
            ) =>
        {
            (task_core::Trigger::Cancel, Vec::new())
        }
        _ => {
            return Err(reject(ops_problem(
                store,
                task_ops::OpsError::InvalidState {
                    id,
                    context: format!("kind={:?}, status={:?}", task.kind, task.status),
                    action: action.to_string(),
                },
                Some(action),
            )));
        }
    };
    let operation = audit.apply(store, "task", &target_id, action, |tx| {
        let outcome = task_core::store::SqliteStore::apply_transition_tx(tx, id, trigger, extra)?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "from": task.status,
            "to": outcome.next,
            "reason": outcome.reason.to_string(),
        }))
    })?;
    if action == "task.cancel" && operation.id == audit.ctx.operation_id {
        crate::browser_control::stop_task(store, &target_id)?;
    }
    Ok(Applied::Audited(Box::new(operation)))
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
    let body: RetryBody = read_json(body, true).await?;
    let clusters = super::cluster_ids(&state);
    let result = state
        .blocking(move |store| retry_op(store, &clusters, id, body, None)?.direct())
        .await?;
    let mut response = json_response(StatusCode::CREATED, &result);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/tasks/{}", result.task_id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// `POST /tasks/{id}/retry` の本体。handler（`audit = None`、`source = human`）と CoS の
/// `/cos/operations`（ADR 2026-10-09 D3/D5。複製・依存の付け替え・監査を同じ transaction、`execution` の
/// 明示は commit 後）が共有する。
pub(crate) fn retry_op(
    store: &task_core::store::SqliteStore,
    clusters: &[String],
    id: task_core::TaskId,
    RetryBody {
        accept,
        workspace,
        execution,
    }: RetryBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::retry::RetryResult>, ApiProblem> {
    let target_id = id.to_string();
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "task", &target_id, problem),
        None => problem,
    };
    // ADR-0062 Phase 108: `workspace` の検証は `PATCH /tasks/{id}` と同じ（先に 422 を返す）。
    let workspace = workspace
        .map(|spec| super::validated_workspace_in(clusters, spec))
        .transpose()
        .map_err(reject)?;
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        return task_ops::retry::retry_task_with_execution(
            store, id, accept, workspace, execution, "human", now,
        )
        .map(Applied::Direct)
        .map_err(|e| ops_problem(store, e, Some("retry")));
    };
    let new_task =
        task_ops::retry::plan_retry(store, id, accept, workspace, execution.is_some(), now)
            .map_err(|e| reject(ops_problem(store, e, Some("retry"))))?;
    let new_id = new_task.id;
    let mut rewired = Vec::new();
    let operation = audit.apply(store, "task", &target_id, "task.retry", |tx| {
        rewired = task_core::store::SqliteStore::retry_task_tx(tx, id, &new_task)?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "new_task_id": new_id.to_string(),
            "rewired": rewired.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
        }))
    })?;
    if operation.id == audit.ctx.operation_id
        && let Err(error) =
            task_ops::retry::finish_retry(store, new_id, rewired, execution, "cos", now)
    {
        tracing::warn!(operation_id = %operation.id, %error, "cos retry follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
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
    let body: TaskPatchBody = read_json(body, true).await?;
    let genres = state.inner.genres.clone();
    let clusters = super::cluster_ids(&state);
    let result = state
        .blocking(move |store| patch_task_op(store, &genres, &clusters, id, body, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// `PATCH /tasks/{id}` の本体。handler（`audit = None`）と CoS の `/cos/operations`（ADR 2026-10-09 D3/D5。
/// `Edited.by = cos`、task・書き込み範囲の hint・監査を同じ transaction）が共有する。
pub(crate) fn patch_task_op(
    store: &task_core::store::SqliteStore,
    genres: &[task_core::GenreSpec],
    clusters: &[String],
    id: task_core::TaskId,
    TaskPatchBody {
        mut task,
        expected_write_paths,
    }: TaskPatchBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::edit::EditResult>, ApiProblem> {
    let target_id = id.to_string();
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "task", &target_id, problem),
        None => problem,
    };
    let paths = expected_write_paths
        .map(|paths| {
            paths
                .map(|paths| task_core::write_set::normalize_write_paths(&paths))
                .transpose()
        })
        .transpose()
        .map_err(|e| reject(ApiProblem::bad_request(e)))?;
    if task.is_empty() && paths.is_none() {
        return Err(reject(ApiProblem::validation(vec![ValidationError {
            field: None,
            message: "at least one field must be given".to_string(),
        }])));
    }
    // ADR-0062 Phase 108: `workspace` の検証（`Remote.cluster` が設定にあること、`~` の展開）は
    // `PATCH /projects/{id}` と同じ `validated_workspace` を使う（422 はここで返す）。
    if let Some(spec) = task.workspace.take() {
        task.workspace = Some(super::validated_workspace_in(clusters, spec).map_err(reject)?);
    }
    let now = OffsetDateTime::now_utc();
    let by = if audit.is_some() { "cos" } else { "human" };
    let plan = if task.is_empty() {
        let existing = store
            .get(id)
            .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))
            .map_err(reject)?
            .ok_or_else(|| reject(ApiProblem::task_not_found(id)))?;
        if let Some(expected) = task.expected_status
            && expected != existing.status
        {
            return Err(reject(ops_problem(
                store,
                task_ops::OpsError::Conflict {
                    expected,
                    actual: existing.status,
                },
                Some("edit"),
            )));
        }
        if existing.status.is_terminal() {
            return Err(reject(ops_problem(
                store,
                task_ops::OpsError::InvalidState {
                    id,
                    context: format!("status={:?}", existing.status),
                    action: "edited; terminal tasks cannot be edited".into(),
                },
                Some("edit"),
            )));
        }
        None
    } else {
        Some(
            task_ops::edit::plan_edit(store, id, task, genres, now, by)
                .map_err(|e| reject(ops_problem(store, e, Some("edit"))))?,
        )
    };
    let now_text = now.to_string();
    let Some(audit) = audit else {
        let result = match plan {
            None => task_ops::edit::EditResult {
                task: store
                    .get(id)
                    .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))?
                    .ok_or_else(|| ApiProblem::task_not_found(id))?,
                fields: vec![],
            },
            Some(plan) if plan.fields.is_empty() => task_ops::edit::EditResult {
                task: plan.task,
                fields: plan.fields,
            },
            Some(plan) => {
                let updated = store
                    .update_task(&plan.task, plan.event())
                    .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))?;
                task_ops::edit::finish_edit(store, plan, updated)
                    .map_err(|e| ops_problem(store, e, Some("edit")))?
            }
        };
        if let Some(paths) = &paths {
            store
                .set_task_expected_write_paths(id, paths.as_deref(), &now_text)
                .map_err(|e| ops_problem(store, task_ops::OpsError::Store(e), Some("edit")))?;
        }
        return Ok(Applied::Direct(result));
    };
    let mut updated = None;
    let operation = audit.apply(store, "task", &target_id, "task.update", |tx| {
        let mut fields = Vec::new();
        if let Some(plan) = plan.as_ref().filter(|plan| !plan.fields.is_empty()) {
            updated = Some(task_core::store::SqliteStore::edit_task_tx(
                tx,
                &plan.task,
                &plan.event(),
            )?);
            fields.extend(plan.fields.iter().cloned());
        }
        if let Some(paths) = &paths {
            task_core::store::SqliteStore::set_task_expected_write_paths_tx(
                tx,
                id,
                paths.clone(),
                &now_text,
            )?;
            fields.push("expected_write_paths".to_string());
        }
        Ok(serde_json::json!({"task_id": target_id, "fields": fields}))
    })?;
    if operation.id == audit.ctx.operation_id
        && let (Some(plan), Some(updated)) = (plan, updated)
        && let Err(error) = task_ops::edit::finish_edit(store, plan, updated)
    {
        tracing::warn!(operation_id = %operation.id, %error, "cos task edit follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
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
    let body: CommentBody = read_json(body, false).await?;
    let result = state
        .blocking(move |store| create_comment_op(store, id, body, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::CREATED, &result))
}

/// `POST /tasks/{id}/comments` の本体。handler（`audit = None`）と CoS の `/cos/operations`
/// （ADR 2026-10-05 D3。author は `cos`、コメント・遷移・監査を同じ transaction で書く）が共有する。
pub(crate) fn create_comment_op(
    store: &task_core::store::SqliteStore,
    id: task_core::TaskId,
    CommentBody { body }: CommentBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::comment::CommentResult>, ApiProblem> {
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        return task_ops::comment::post_human_comment(store, id, body, now)
            .map(Applied::Direct)
            .map_err(|e| ops_problem(store, e, Some("comment")));
    };
    let target_id = id.to_string();
    let plan = task_ops::comment::plan_human_comment(store, id, Some("cos".into()), body, now)
        .map_err(|e| {
            audit.reject(
                store,
                "task",
                &target_id,
                ops_problem(store, e, Some("comment")),
            )
        })?;
    let mut outcome = None;
    let operation = audit.apply(store, "task", &target_id, "comment.create", |tx| {
        outcome = task_core::store::SqliteStore::comment_add_tx(
            tx,
            &plan.comment,
            plan.transition.clone(),
        )?;
        Ok(serde_json::json!({"task_id": target_id, "comment_id": plan.comment.id.to_string()}))
    })?;
    if operation.id == audit.ctx.operation_id {
        // Follow-up writes (pending approval settlement) are idempotent and run after commit,
        // exactly as on the human path.
        if let Err(error) = task_ops::comment::finish_human_comment(store, plan, outcome) {
            tracing::warn!(operation_id = %operation.id, %error, "cos comment follow-up failed");
        }
    }
    Ok(Applied::Audited(Box::new(operation)))
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
    let body: ReopenBody = read_json(body, true).await?;
    let result = state
        .blocking(move |store| reopen_op(store, id, body, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// `POST /tasks/{id}/reopen` の本体。handler（`audit = None`）と CoS の `/cos/operations`
/// （ADR 2026-10-09 D3/D5。遷移・監査を同じ transaction、run 履歴の区切りは commit 後に冪等で）が共有する。
pub(crate) fn reopen_op(
    store: &task_core::store::SqliteStore,
    id: task_core::TaskId,
    ReopenBody { expected_status }: ReopenBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::gate::TransitionResult>, ApiProblem> {
    let Some(audit) = audit else {
        return task_ops::comment::reopen(store, id, expected_status)
            .map(Applied::Direct)
            .map_err(|e| ops_problem(store, e, Some("reopen")));
    };
    let target_id = id.to_string();
    let from = task_ops::comment::plan_reopen(store, id, expected_status).map_err(|e| {
        audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(store, e, Some("reopen")),
        )
    })?;
    let mut outcome = None;
    let operation = audit.apply(store, "task", &target_id, "task.reopen", |tx| {
        let applied = task_core::store::SqliteStore::apply_transition_tx(
            tx,
            id,
            task_core::Trigger::Reopen,
            Vec::new(),
        )?;
        let result = serde_json::json!({
            "task_id": target_id,
            "from": from,
            "to": applied.next,
            "reason": applied.reason.to_string(),
        });
        outcome = Some(applied);
        Ok(result)
    })?;
    if operation.id == audit.ctx.operation_id
        && let Some(outcome) = outcome
        && let Err(error) = task_ops::comment::finish_reopen(store, id, from, outcome)
    {
        tracing::warn!(operation_id = %operation.id, %error, "cos reopen follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
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
        .blocking(move |store| rereview_op(store, id, expected_status, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

pub(crate) fn rereview_op(
    store: &task_core::store::SqliteStore,
    id: task_core::TaskId,
    expected: Option<task_core::Status>,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::gate::TransitionResult>, ApiProblem> {
    let Some(audit) = audit else {
        return task_ops::comment::rereview(store, id, expected)
            .map(Applied::Direct)
            .map_err(|error| ops_problem(store, error, Some("rereview")));
    };
    let target_id = id.to_string();
    let task = store
        .get(id)
        .map_err(crate::problem::store_problem)?
        .ok_or_else(|| {
            audit.reject(
                store,
                "task",
                &target_id,
                ops_problem(store, task_ops::OpsError::NotFound(id), Some("rereview")),
            )
        })?;
    if let Some(expected) = expected
        && expected != task.status
    {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(
                store,
                task_ops::OpsError::Conflict {
                    expected,
                    actual: task.status,
                },
                Some("rereview"),
            ),
        ));
    }
    let events = store
        .events_for(id)
        .map_err(crate::problem::store_problem)?;
    if !task_ops::comment::can_rereview(&task, &events) {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(
                store,
                task_ops::OpsError::Validation("task is not eligible for rereview".into()),
                Some("rereview"),
            ),
        ));
    }
    let from = task.status;
    let operation = audit.apply(store, "task", &target_id, "task.rereview", |tx| {
        let outcome = task_core::store::SqliteStore::apply_transition_tx(
            tx,
            id,
            task_core::Trigger::Rereview,
            Vec::new(),
        )?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "from": from,
            "to": outcome.next,
            "reason": outcome.reason.to_string(),
            "cascaded": [],
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// ADR 2026-10-05 D3（D6 一次対応 A）: CoS が `POST /tasks/{id}/answer` を代わりに答える監査つきの
/// 操作。検証は人の経路と同じ `gate::plan_answer`、遷移は `cos_operation_apply` の transaction 内で書く。
/// 統合依頼を閉じる・未決の approvals を決める後始末は冪等で、人の経路と同じく commit 後に走らせる。
pub(crate) fn answer_op(
    store: &task_core::store::SqliteStore,
    id: task_core::TaskId,
    AnswerBody {
        answer,
        expected_status,
    }: AnswerBody,
    audit: &OperationAudit,
) -> Result<Applied<()>, ApiProblem> {
    let target_id = id.to_string();
    if answer.trim().is_empty() {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            ApiProblem::validation(vec![ValidationError {
                field: Some("answer".to_string()),
                message: "answer must not be blank".to_string(),
            }]),
        ));
    }
    let (from, question) =
        task_ops::gate::plan_answer(store, id, expected_status).map_err(|e| {
            audit.reject(
                store,
                "task",
                &target_id,
                ops_problem(store, e, Some("answer")),
            )
        })?;
    task_ops::gate::close_phase_integration_requests(store, id, &answer).map_err(|e| {
        audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(store, e, Some("answer")),
        )
    })?;
    let event = task_core::Event::Answered {
        question,
        answer: answer.clone(),
    };
    let operation = audit.apply(store, "task", &target_id, "question.answer", |tx| {
        let outcome = task_core::store::SqliteStore::apply_transition_tx(
            tx,
            id,
            task_core::Trigger::Answer,
            vec![event],
        )?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "from": from,
            "to": outcome.next,
            "reason": outcome.reason.to_string(),
        }))
    })?;
    if operation.id == audit.ctx.operation_id
        && let Err(error) = task_ops::gate::settle_pending_approvals(store, id, &answer)
    {
        tracing::warn!(operation_id = %operation.id, %error, "cos answer follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
}
