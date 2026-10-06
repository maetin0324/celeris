//! 案件の一覧・作成・詳細・変更と、廃止した途中目標の作成・変更（ADR-0033 D2、ADR-0079 D13）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use task_core::{ListFilter, ListOrder, Project, ProjectId, ProjectStatus, SqliteStore, TaskStore};

use crate::cos::operations::{Applied, OperationAudit};
use time::OffsetDateTime;

use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;
use crate::types::{
    MilestoneReviewView, MilestoneView, ProjectCreateBody, ProjectDetail, ProjectList,
    ProjectPatchBody, ProjectTaskView, ValidationError,
};

use super::{
    ApiResult, Params, json_response, no_query, parse_project_id, read_json, rfc3339,
    validated_workspace,
};

/// ADR-0033 D2: `GET /projects/{id}` が返す仕事の木の上限（GUI が一目で見る図なので十分に大きく取る）。
const PROJECT_TASKS_LIMIT: usize = 2_000;

/// `GET /projects`。ADR-0044 D6（Phase 55）: **アーカイブされた案件は既定で隠す**
/// （`?archived=1` で全部、`?archived=0` は既定と同じ）。
pub(super) async fn project_list(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["archived"])?;
    let show_archived = query.bool("archived")?.unwrap_or(false);
    let items = state
        .blocking(move |store| {
            let all = store.project_list().map_err(store_problem)?;
            Ok(if show_archived {
                all
            } else {
                all.into_iter()
                    .filter(|p| p.archived_at.is_none())
                    .collect()
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &ProjectList { items }))
}

/// 案件を作る。**管理系**（`token_file` 未設定でも 401）: 直後に秘書の run を起こす経路なので、
/// `POST /org/{id}/messages` と同じ規律にする（監査 M-4）。
pub(super) async fn create_project(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: ProjectCreateBody = read_json(body, false).await?;
    if create.title.trim().is_empty() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("title".into()),
            message: "title must not be blank".into(),
        }]));
    }
    if create.request.trim().is_empty() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("request".into()),
            message: "request must not be blank".into(),
        }]));
    }
    // ADR-0039 D1 / D5: 作業場所は `[[clusters]]` に無いクラスタを弾き、`Local` の `~` を展開して保存する。
    let workspace = match create.workspace {
        Some(spec) => Some(validated_workspace(&state, spec)?),
        None => None,
    };
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let conversation_genre = state.inner.conversation_genre.clone();
    let project = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let project = Project {
                auto_advance: false,
                slug: None,
                archived_at: None,
                paused_from: None,
                id: ProjectId::new(),
                title: create.title,
                request: create.request,
                // ADR-0033 D2: 作った直後は `proposed`（秘書が理解確認と方針を返すまで人の返事待ち）。
                status: ProjectStatus::Proposed,
                secretary_summary: None,
                workspace,
                created_at: now,
                updated_at: now,
            };
            store.project_create(&project).map_err(store_problem)?;
            // SPEC §7 / ADR-0033 D4: 案件を受けたら、秘書が最初に「理解の確認・大まかな方針・最初の
            // 途中目標の提案」を返す。ここは対話を 1 回起こすだけ（中身はプロンプトの仕事）。
            crate::conversation::greet_the_secretary(
                store,
                &project,
                &roles,
                &genres,
                &conversation_genre,
            );
            Ok(project)
        })
        .await?;
    let mut response = json_response(StatusCode::CREATED, &project);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/projects/{}", project.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

pub(super) async fn project_detail(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    // ADR-0079 D13 / U-R8（Phase R5a）: 途中目標（と案件計画の DAG）は凍結した履歴。既定では返さず
    // （`milestones: []`、件数だけ `milestones_frozen`）、`?include_frozen=true` のときだけ読み取り専用で返す。
    let query = QueryParams::parse(raw.as_deref(), &["include_frozen"])?;
    let include_frozen = query.bool("include_frozen")?.unwrap_or(false);
    let project_id = parse_project_id(&id)?;
    let detail = state
        .blocking(move |store| {
            let Some(project) = store.project_get(project_id).map_err(store_problem)? else {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            };
            let all_milestones = store.milestone_list(project_id).map_err(store_problem)?;
            let milestones_frozen = u32::try_from(all_milestones.len()).unwrap_or(u32::MAX);
            let milestones_frozen_open = u32::try_from(
                all_milestones
                    .iter()
                    .filter(|m| !m.status.is_terminal())
                    .count(),
            )
            .unwrap_or(u32::MAX);
            let all_milestones = if include_frozen {
                all_milestones
            } else {
                Vec::new()
            };
            // ADR-0038 D1 / D4（Phase 41）: 途中目標ごとに、秘書のレビューの返事と、提案された次の
            // 途中目標を添える（GUI のカードが「結果 → 提案 → ok / 議論 / ng」を出せるように）。
            let latest_proposal =
                task_ops::milestone_review::latest_proposal(store, project_id, None)
                    .map_err(|e| ops_problem(store, e, None))?;
            let mut milestones = Vec::new();
            for milestone in all_milestones {
                let review =
                    task_ops::milestone_review::review_state(store, project_id, milestone.id)
                        .map_err(store_problem)?
                        .reply
                        .map(|reply| MilestoneReviewView {
                            message_id: reply.id.to_string(),
                            text: reply.text,
                            at: rfc3339(reply.created_at),
                        });
                // 提案は「返事が付いた途中目標のカード」にだけ添える（自分自身は除く）。
                let proposal = match &review {
                    Some(_) => latest_proposal.clone().filter(|p| p.id != milestone.id),
                    None => None,
                };
                milestones.push(MilestoneView {
                    milestone,
                    review,
                    proposal,
                });
            }
            // ADR-0033 D2: 案件の仕事の木 = `tasks WHERE project_id = ?`（DAG は `parent_id` / `depends_on`）。
            let filter = ListFilter {
                project_id: Some(project_id),
                ..ListFilter::default()
            };
            let page = store
                .list_page(&filter, ListOrder::CreatedDesc, None, PROJECT_TASKS_LIMIT)
                .map_err(store_problem)?;
            // ADR-0079 D11（Phase R4a）: root task の合計（subtree の roll-up の和）。
            let root_totals = task_ops::tree_view::project_root_totals(store, &page.items)
                .map_err(|e| ops_problem(store, e, None))?;
            let tasks = page
                .items
                .into_iter()
                .map(|task| ProjectTaskView {
                    is_root_task: task_core::is_root_task(&task),
                    conversation: task_core::is_conversation(&task),
                    support: task_core::support_kind(&task).map(str::to_string),
                    id: task.id,
                    title: task.title,
                    status: task.status,
                    parent_id: task.parent_id,
                    depends_on: task.depends_on,
                    assignee: task.assignee,
                    milestone_id: task.milestone_id,
                })
                .collect();
            // ADR-0043 D1: この案件のリポジトリ（primary が先頭）。
            let repos = store.repo_list(project_id).map_err(store_problem)?;
            // ADR-0074 D3.5（Phase F4b (h)）: 案件計画の DAG（無ければ省略）。ADR-0079 D13（Phase R5a）: 凍結した
            // 履歴なので `include_frozen=true` のときだけ。
            let project_plan = if include_frozen {
                task_ops::project_plan::dag_view(store, &project)
                    .map_err(|e| ops_problem(store, e, None))?
            } else {
                None
            };
            Ok(ProjectDetail {
                project,
                repos,
                milestones,
                milestones_frozen,
                milestones_frozen_open,
                tasks,
                project_plan,
                root_totals: Some(root_totals),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &detail))
}

pub(super) async fn patch_project(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let patch: ProjectPatchBody = read_json(body, false).await?;
    // ADR-0079 D13（Phase R5a）: `auto_advance`（ADR-0074 D3.2 / ADR-0077 の途中目標の自動 reached）は廃止。
    // 列 `projects.auto_advance` は残すが書かない・読まない。
    if patch.auto_advance.is_some() {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("auto_advance".into()),
            message: "auto_advance was removed by ADR-0079 (projects have no plan; milestones are frozen)"
                .into(),
        }]));
    }
    if patch.status.is_none()
        && patch.workspace.is_none()
        && patch.slug.is_none()
        && patch.title.is_none()
        && patch.request.is_none()
    {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: None,
            message: "specify at least one of `status`, `workspace`, `slug`, `title` or `request`"
                .into(),
        }]));
    }
    let (title, request) = project_text_input(patch.title.as_deref(), patch.request.as_deref())?;
    // Phase K-1: slug の綴りは先に見る（422）。重複は store が 409 で返す。
    if let Some(slug) = patch.slug.as_deref()
        && !task_core::knowledge::is_valid_project_slug(slug.trim())
    {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("slug".into()),
            message: format!(
                "slug must be lowercase [a-z0-9-] (1..64 chars, no leading/trailing/double '-', not a project id): {slug:?}"
            ),
        }]));
    }
    // ADR-0044 D6（Phase 55）: `paused` / `cancelled` は**専用のエンドポイント**でしか入れない。
    // `PATCH` で入れると `paused_from`（`resume` の戻り先）が空のままになり、中止の連鎖
    //（属するタスクと途中目標を `cancelled` にする）も起きないので、状態だけが食い違う。
    if let Some(status @ (ProjectStatus::Paused | ProjectStatus::Cancelled)) = patch.status {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("status".into()),
            message: format!(
                "use POST /projects/{{id}}/{} instead of PATCH to set {:?} (it also records paused_from and cascades)",
                if status == ProjectStatus::Paused {
                    "pause"
                } else {
                    "cancel"
                },
                status.as_str()
            ),
        }]));
    }
    // ADR-0039 D1 / D5: 作業場所を書き換えるなら、先に検証と `~` の展開をする（422 はここで返す）。
    let workspace = match patch.workspace {
        Some(Some(spec)) => Some(Some(validated_workspace(&state, spec)?)),
        Some(None) => Some(None),
        None => None,
    };
    let project = state
        .blocking(move |store| {
            if let Some(status) = patch.status
                && !store
                    .project_set_status(project_id, status)
                    .map_err(store_problem)?
            {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            }
            if let Some(spec) = &workspace
                && !store
                    .project_set_workspace(project_id, spec.as_ref())
                    .map_err(store_problem)?
            {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            }
            if let Some(slug) = patch.slug.as_deref()
                && !store
                    .project_set_slug(project_id, slug)
                    .map_err(store_problem)?
            {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            }
            // ADR-0072「Phase F6 実装時の決定」: 変わった欄だけを書き、監査用に名前を返す。
            let (text_fields, old_title) = set_project_text_op(
                store,
                project_id,
                title.as_deref(),
                request.as_deref(),
                None,
            )?
            .direct()?;
            let project = store
                .project_get(project_id)
                .map_err(store_problem)?
                .ok_or_else(|| ApiProblem::project_not_found(&project_id.to_string()))?;
            Ok((project, text_fields, old_title))
        })
        .await?;
    let (project, text_fields, old_title) = project;
    if !text_fields.is_empty() {
        // 案件には events の列が無い（events は Task ごと）ので、監査は管理系の構造化ログに残す
        // （ADR-0072「Phase F6 実装時の決定」P4）。
        tracing::info!(who = "admin", op = "project_updated", project_id = %project_id, fields = ?text_fields, old_title = %old_title, title = %project.title, "admin: project updated");
    }
    Ok(json_response(StatusCode::OK, &project))
}

/// `title` / `request` の入力検証（前後の空白を除き、空と長すぎるものは 422）。
fn project_text_input(
    title: Option<&str>,
    request: Option<&str>,
) -> Result<(Option<String>, Option<String>), ApiProblem> {
    // ADR-0072「Phase F6 実装時の決定」: 名前と説明（依頼文）。前後の空白を除き、空と長すぎるものは 422。
    let title = title.map(str::trim).map(str::to_string);
    let request = request.map(str::trim).map(str::to_string);
    let mut text_errors = Vec::new();
    for (field, value, max) in [
        (
            "title",
            title.as_deref(),
            crate::types::PROJECT_TITLE_MAX_CHARS,
        ),
        (
            "request",
            request.as_deref(),
            crate::types::PROJECT_REQUEST_MAX_CHARS,
        ),
    ] {
        let Some(value) = value else { continue };
        if value.is_empty() {
            text_errors.push(ValidationError {
                field: Some(field.into()),
                message: format!("{field} must not be blank"),
            });
        } else if value.chars().count() > max {
            text_errors.push(ValidationError {
                field: Some(field.into()),
                message: format!("{field} must be at most {max} characters"),
            });
        }
    }
    if !text_errors.is_empty() {
        return Err(ApiProblem::validation(text_errors));
    }
    Ok((title, request))
}

/// CoS の `PATCH /api/v1/projects/{id}` の本文（ADR 2026-10-05 D3）。CoS が変えられるのは名前と説明だけ
/// （状態・作業場所・slug は人の管理操作に残す）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CosProjectPatchBody {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) request: Option<String>,
}

/// 案件の名前・説明の書き換え。`PATCH /projects/{id}` の handler（`audit = None`）と CoS の
/// `/cos/operations` が共有する。変わった欄の名前と元の名前を返す（監査ありでは書き込み・
/// `cos_operations`・監査 envelope・card が 1 transaction）。
pub(crate) fn set_project_text_op(
    store: &SqliteStore,
    project_id: ProjectId,
    title: Option<&str>,
    request: Option<&str>,
    audit: Option<&OperationAudit>,
) -> Result<Applied<(Vec<&'static str>, String)>, ApiProblem> {
    let target_id = project_id.to_string();
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "project", &target_id, problem),
        None => problem,
    };
    let before = store
        .project_get(project_id)
        .map_err(|e| reject(store_problem(e)))?
        .ok_or_else(|| reject(ApiProblem::project_not_found(&target_id)))?;
    let new_title = title.filter(|t| *t != before.title);
    let new_request = request.filter(|r| *r != before.request);
    let mut text_fields: Vec<&'static str> = Vec::new();
    if new_title.is_some() {
        text_fields.push("title");
    }
    if new_request.is_some() {
        text_fields.push("request");
    }
    let Some(audit) = audit else {
        if !text_fields.is_empty()
            && !store
                .project_set_text(project_id, new_title, new_request)
                .map_err(store_problem)?
        {
            return Err(ApiProblem::project_not_found(&target_id));
        }
        return Ok(Applied::Direct((text_fields, before.title)));
    };
    let operation = audit.apply(store, "project", &target_id, "project.update", |tx| {
        if !SqliteStore::project_set_text_tx(tx, project_id, new_title, new_request)? {
            return Err(task_core::chat::ChatError::NotFound {
                kind: "project",
                id: target_id.clone(),
            });
        }
        Ok(serde_json::json!({
            "project_id": target_id,
            "fields": text_fields,
            "old_title": before.title,
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// CoS 経路の入口: 本文を検証してから [`set_project_text_op`] に渡す。
pub(crate) fn cos_patch_project(
    store: &SqliteStore,
    project_id: ProjectId,
    body: CosProjectPatchBody,
    audit: &OperationAudit,
) -> Result<Applied<(Vec<&'static str>, String)>, ApiProblem> {
    let target_id = project_id.to_string();
    if body.title.is_none() && body.request.is_none() {
        return Err(audit.reject(
            store,
            "project",
            &target_id,
            ApiProblem::validation(vec![ValidationError {
                field: None,
                message: "specify at least one of `title` or `request`".into(),
            }]),
        ));
    }
    let (title, request) = project_text_input(body.title.as_deref(), body.request.as_deref())
        .map_err(|problem| audit.reject(store, "project", &target_id, problem))?;
    set_project_text_op(
        store,
        project_id,
        title.as_deref(),
        request.as_deref(),
        Some(audit),
    )
}

/// ADR-0079 D13（Phase R5a）: 途中目標の作成は 410（途中目標は root task の段階で表す）。
pub(super) async fn create_milestone(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        crate::milestones::MILESTONE_GONE,
        "POST /api/v1/tasks with project_id and stages_hint (the stages of a root task)",
    ))
}

/// ADR-0079 D13（Phase R5a）: 途中目標の状態の変更は 410（既存の行は凍結）。
pub(super) async fn patch_milestone(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        crate::milestones::MILESTONE_GONE,
        "a stage with review: human in the root task's plan",
    ))
}
