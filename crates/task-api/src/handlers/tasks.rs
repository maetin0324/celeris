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
}

fn created_task(task: &Task) -> Response {
    let mut response = json_response(StatusCode::CREATED, task);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/tasks/{}", task.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

// ---- 3. GET /tasks ----

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
    let NewTaskBody {
        mut task,
        expected_write_paths,
    } = read_json(body, false).await?;
    let paths = expected_write_paths
        .map(|paths| task_core::write_set::normalize_write_paths(&paths))
        .transpose()
        .map_err(ApiProblem::bad_request)?;
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
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let task = state
        .blocking(move |store| {
            task_ops::add::create_task_with_roles(
                store,
                task,
                &roles,
                &genres,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| ops_problem(store, e, None))
            .and_then(|created| {
                if let Some(paths) = &paths {
                    store
                        .set_task_expected_write_paths(
                            created.id,
                            Some(paths),
                            &OffsetDateTime::now_utc().to_string(),
                        )
                        .map_err(|e| ops_problem(store, OpsError::Store(e), None))?;
                }
                Ok(created)
            })
        })
        .await?;
    Ok(created_task(&task))
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
