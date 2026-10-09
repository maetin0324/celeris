//! タスクの一覧・作成・詳細（`GET/POST /tasks`、`GET /tasks/{id}`）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use schemars::JsonSchema;
use serde::Deserialize;
use task_core::{
    ListFilter, ListOrder, MilestoneId, ProjectId, Status, StoreError, Task, TaskKind,
};
use task_ops::OpsError;
use task_ops::add::NewTaskSpec;
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::files;
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::{QueryParams, parse_snake, parse_task_id};
use crate::state::ApiState;

use super::{ApiResult, Params, json_response, no_query, read_json};

const TITLE_QUERY_MAX_CHARS: usize = 200;

/// POST /tasks accepts the task specification and an optional write path hint.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTaskBody {
    #[serde(flatten)]
    pub task: NewTaskSpec,
    #[serde(default)]
    pub expected_write_paths: Option<Vec<String>>,
    /// ADR 2026-10-07 cos-live-fixes D1: chat attachment ids to pin to the new task
    /// (`owner_kind: "task"`) in the transaction that creates it, so the first run's input manifest
    /// already has them. An unknown, deleted, expired or duplicate id (or, from a CoS run, one from
    /// another thread) is 422 `invalid_attachment` and creates nothing.
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

fn created_task(task: &Task) -> Response {
    let mut response = json_response(StatusCode::CREATED, task);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/tasks/{}", task.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

// ---- 3. GET /tasks ----

/// `GET /tasks/counts` は一覧の重い行取得をせず、状態別件数だけを返す。
pub(super) async fn task_counts(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let counts = state
        .blocking(|store| {
            task_ops::view::task_status_counts(store).map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &counts))
}

pub(super) async fn list_tasks(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(
        raw.as_deref(),
        &[
            "status",
            "kind",
            "genre",
            "parent",
            "project",
            "root_only",
            "q",
            "order",
            "limit",
            "cursor",
            // ADR-0044 D4（Phase 53）: ボードと検索のフィルタ。複数指定は AND。
            "label",
            "category",
            "assignee",
            "milestone",
            "tier",
            "priority",
            // ADR-0044 D6（Phase 55）: アーカイブされた案件のタスクは既定で隠す。
            "archived",
        ],
    )?;
    let mut filter = ListFilter::default();
    for status in query.list("status") {
        filter
            .statuses
            .push(parse_snake::<Status>("status", status)?);
    }
    for kind in query.list("kind") {
        filter.kinds.push(parse_snake::<TaskKind>("kind", kind)?);
    }
    // ADR-0027 D1: `genre` は自由記述なので `kind` と違って列挙型の検証はしない（完全一致だけ）。
    for genre in query.list("genre") {
        filter.genres.push(genre.to_string());
    }
    filter.parent_id = query.task_id("parent")?;
    // ADR-0033 D2: 案件で絞る（案件の仕事の木。`GET /projects/{id}` は同じ絞り込みを使う）。
    if let Some(raw) = query.single("project")? {
        filter.project_id =
            Some(raw.parse::<ProjectId>().map_err(|_| {
                ApiProblem::bad_request("query parameter `project` must be a ULID")
            })?);
    }
    filter.root_only = query.bool("root_only")?.unwrap_or(false);
    // ADR-0044 D6（Phase 55）: `?archived=1` を付けたときだけアーカイブされた案件のタスクも返す。
    filter.hide_archived = !query.bool("archived")?.unwrap_or(false);
    // ---- ADR-0044 D4（Phase 53）----
    for label in query.list("label") {
        if !task_core::is_valid_label(label) {
            return Err(ApiProblem::bad_request(format!(
                "query parameter `label` must match [a-z0-9-] (got {label:?})"
            )));
        }
        filter.labels.push(label.to_string());
    }
    for category in query.list("category") {
        let Some(parsed) = task_core::TaskCategory::parse(category) else {
            return Err(ApiProblem::bad_request(format!(
                "unknown category `{category}`"
            )));
        };
        filter.categories.push(parsed);
    }
    if let Some(assignee) = query.single("assignee")?.filter(|a| !a.is_empty()) {
        filter.assignee = Some(assignee.to_string());
    }
    if let Some(raw) = query.single("milestone")? {
        filter.milestone_id =
            Some(raw.parse::<MilestoneId>().map_err(|_| {
                ApiProblem::bad_request("query parameter `milestone` must be a ULID")
            })?);
    }
    for tier in query.list("tier") {
        filter
            .tiers
            .push(parse_snake::<task_core::Tier>("tier", tier)?);
    }
    for priority in query.list("priority") {
        // `P0`〜`P3` でも生の整数でも受ける（`priority_label` と対）。
        let value = match task_core::priority_from_label(priority) {
            Some(v) => v,
            None => priority
                .parse::<i32>()
                .map_err(|_| ApiProblem::bad_request(format!("unknown priority `{priority}`")))?,
        };
        filter.priorities.push(value);
    }
    if let Some(text) = query.single("q")? {
        if text.chars().count() > TITLE_QUERY_MAX_CHARS {
            return Err(ApiProblem::bad_request(
                "query parameter `q` must be at most 200 characters",
            ));
        }
        if !text.is_empty() {
            filter.text_contains = Some(text.to_string());
            // ADR-0044 D4: `GET /tasks?q=` は title / objective に加えて**コメント本文**も見る。
            filter.text_includes_comments = true;
        }
    }
    let order = match query.single("order")? {
        None | Some("updated_desc") => ListOrder::UpdatedDesc,
        Some("dispatch") => ListOrder::Dispatch,
        Some("created_desc") => ListOrder::CreatedDesc,
        Some(other) => return Err(ApiProblem::bad_request(format!("unknown order `{other}`"))),
    };
    let limit = query.limit("limit", 100, 500)?;
    let cursor = query
        .single("cursor")?
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    let ctx = state.inner.view.clone();
    let list = state
        .blocking(move |store| {
            task_ops::view::task_list(
                store,
                &filter,
                order,
                cursor.as_deref(),
                limit,
                &ctx,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| match e {
                OpsError::Store(StoreError::Invalid(message))
                    if message.starts_with("invalid cursor") =>
                {
                    ApiProblem::bad_request("invalid cursor")
                }
                other => ops_problem(store, other, None),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

// ---- 4. POST /tasks ----

pub(super) async fn create_task(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let body: NewTaskBody = read_json(body, false).await?;
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let task = state
        .blocking(move |store| create_task_op(store, &roles, &genres, body, None)?.direct())
        .await?;
    let mut response = created_task(&task);
    // ADR 2026-10-09-cos-task-repository-required D2: 人の起票は拒否せず、header で警告する
    // （後から `PATCH /tasks/{id}` の `project_id`・`repos` で付けられる）。
    if task_ops::repo_requirement::missing_repository_reason(&task).is_some() {
        response.headers_mut().insert(
            REPOSITORY_WARNING_HEADER,
            HeaderValue::from_static(REPOSITORY_WARNING),
        );
    }
    Ok(response)
}

/// ADR 2026-10-09-cos-task-repository-required D2: 人の `POST /tasks` への警告の header。
pub const REPOSITORY_WARNING_HEADER: &str = "celeris-warning";
const REPOSITORY_WARNING: &str = "repository_required: the task uses a repository but has no project_id/repos; \
     attach them with PATCH /tasks/{id} (project_id, repos) before it runs";

/// `POST /tasks` の本体。handler（`audit = None`）と CoS の `/cos/operations`（ADR 2026-10-05 D3。
/// `audit` の transaction で task・`cos_operations`・監査 event を一緒に書く）が共有する。
pub(crate) fn create_task_op(
    store: &task_core::store::SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    body: NewTaskBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<Task>, ApiProblem> {
    let NewTaskBody {
        mut task,
        expected_write_paths,
        attachment_ids,
    } = body;
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "task", "new", problem),
        None => problem,
    };
    // repo 名だけがあり案件が無い要求にも、CoS へは同じ修正可能な理由を返す。
    if audit.is_some() && task.project_id.is_none() && !task.repos.is_empty() {
        return Err(reject(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            task_ops::repo_requirement::REPOSITORY_REQUIRED_CODE,
            task_ops::repo_requirement::REPOSITORY_REQUIRED,
        )));
    }
    let paths = expected_write_paths
        .map(|paths| task_core::write_set::normalize_write_paths(&paths))
        .transpose()
        .map_err(|e| reject(ApiProblem::bad_request(e)))?;
    // ADR-0044 D1（Phase 53）: **人が作ったタスクは `ready`**（人は Go を出す側なので draft を挟まない）。
    // `draft` にしたければ `status: "draft"` を明示する。計画・委譲で作られる子（`draft` → Go）の経路は
    // ここを通らないので変わらない。
    if task.status.is_none() {
        task.status = Some(task_core::Status::Ready);
    }
    // ADR-0016 M3 / ADR-0027 D1: 省略された tier / adapter / 予算は `[[roles]]` の既定 → `[[genres]]` の
    // `default_role` の既定 → 全体の既定で埋める。API は常に完全な設定を持つので、`genres` が設定されて
    // いれば知らない `genre` / `genre` と `role` の不整合は常に検証する（celerisctl の「`--config` 無し」の
    // 緩さはここには無い）。
    let built =
        task_ops::add::build_task_with_roles(store, task, roles, genres, OffsetDateTime::now_utc())
            .map_err(|e| reject(ops_problem(store, e, None)))?;
    // ADR 2026-10-09-cos-task-repository-required D1: CoS の起票は、リポジトリを使う task に案件・
    // リポジトリが無ければ 422（worker の workspace が空のまま走らせない）。
    if audit.is_some()
        && let Some(reason) = task_ops::repo_requirement::missing_repository_reason(&built)
    {
        return Err(reject(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            task_ops::repo_requirement::REPOSITORY_REQUIRED_CODE,
            reason,
        )));
    }
    let paths = paths.filter(|paths| !paths.is_empty());
    let thread = audit.map(|audit| audit.ctx.thread_id.clone());
    // ADR 2026-10-07 cos-live-fixes D1: the task row, its `ready` status and the attachment pins are
    // one transaction, so the dispatcher never picks the task up before its pins exist.
    let mut failure = None;
    let mut write = |tx: &rusqlite::Transaction<'_>| -> Result<(), StoreError> {
        if paths.is_some() {
            task_core::store::SqliteStore::set_task_expected_write_paths_tx(
                tx,
                built.id,
                paths.clone(),
                &OffsetDateTime::now_utc().to_string(),
            )?;
        }
        if let Err(problem) = pin_attachments_tx(tx, &attachment_ids, built.id, thread.as_deref()) {
            let detail = problem.detail().to_string();
            failure = Some(problem);
            return Err(StoreError::Invalid(detail));
        }
        Ok(())
    };
    let Some(audit) = audit else {
        let outcome = store.create_task_with(&built, vec![], write);
        if let Some(problem) = failure {
            return Err(problem);
        }
        outcome.map_err(|e| ops_problem(store, OpsError::Store(e), None))?;
        return Ok(Applied::Direct(built));
    };
    let target_id = built.id.to_string();
    let outcome = audit.apply(store, "task", &target_id, "task.create", |tx| {
        task_core::store::SqliteStore::create_task_tx(tx, &built, None, vec![])?;
        write(tx).map_err(|e| task_core::chat::ChatError::Invalid(e.to_string()))?;
        Ok(serde_json::json!({
            "task_id": built.id.to_string(),
            "attachment_ids": attachment_ids,
        }))
    });
    let operation = outcome.map_err(|problem| failure.take().unwrap_or(problem))?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// Pin `ids` to the task being created inside its creation transaction (ADR 2026-10-07
/// cos-live-fixes D1). Any invalid id fails the whole request with 422 `invalid_attachment`.
fn pin_attachments_tx(
    tx: &rusqlite::Transaction<'_>,
    ids: &[String],
    task: task_core::TaskId,
    thread: Option<&str>,
) -> Result<(), ApiProblem> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = OffsetDateTime::now_utc();
    let internal =
        |e: task_core::chat::attachments::AttachmentError| ApiProblem::internal(e.to_string());
    if let Some(why) =
        task_core::chat::attachments::task_pin_problem_tx(tx, ids, thread, now).map_err(internal)?
    {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_attachment",
            why,
        ));
    }
    let owner = task.to_string();
    for id in ids {
        task_core::chat::attachments::add_ref_tx(tx, id, "task", &owner, now).map_err(internal)?;
    }
    Ok(())
}

// ---- 5. GET /tasks/{id} ----

pub(super) async fn task_detail(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let id = parse_task_id(&id)?;
    let ctx = state.inner.view.clone();
    let detail = state
        .blocking(move |store| {
            let mut detail =
                task_ops::view::task_detail(store, id, &ctx, OffsetDateTime::now_utc())
                    .map_err(|e| ops_problem(store, e, None))?;
            let task = &detail.task;
            for run in &mut detail.runs {
                run.files = Some(files::run_files(task, &ctx.workspace_root, &run.run_id));
            }
            Ok(detail)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &detail))
}
