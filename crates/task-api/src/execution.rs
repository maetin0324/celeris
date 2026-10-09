//! ADR-0072（Phase E2）: `POST /tasks/{id}/execution-plan`（origin human）、
//! `GET /tasks/{id}/execution-plan`。
//!
//! - `POST` は**管理系**（`token_file` 未設定でも 401）: 計画を作るのは人の判断なので、
//!   `POST /projects` / `POST /projects/{id}/repos` と同じ規律にする。
//! - 検証・採用そのものは `task_ops::execution::adopt_plan`（D14）。ハンドラは HTTP への写像だけ。
//! - 404（タスクが無い）、422（D14 の検証エラー）、409（既に `active` な計画がある。E2 は新規のみ、
//!   replan は E4）。
//!
//! ADR-0079 R5b-prep: `PUT` も `POST` と同じ（人の計画の採用）。検証の上限は daemon の実効の上限
//! （`[execution.tree]` は `ApiSettings.tree_limits`）で、/3 は planner の計画と同じ経路（unit の gate・決定の要求・
//! 木の上限・unit の `adopt`）を 1 トランザクションで通す（`task_ops::execution::adopt_human_plan`。承認〈PlanGate〉は
//! 挟まない）。`POST /tasks/{id}/tree/adopt`（管理系）は採用済みの計画への後からの採用（ADR-0079 D15）。
//!
//! ADR-0072 D19（Phase E5）: `GET /tasks/{id}/execution`（計画・WU 一覧・run 一覧・metrics・
//! ExecutionPhase を 1 つにまとめた深掘りビュー）と `GET /metrics/execution`（期間で集計した gate の
//! 判定分布・completion rate・continuation/repair/replan 頻度。集計そのものは `crate::stats`）。

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use task_core::{ExecutionLimits, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::query::{QueryParams, parse_task_id};
use crate::state::ApiState;
use crate::types::{ExecutionPlanView, TaskExecutionView};

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/tasks/{id}/execution-plan",
            axum::routing::get(get_execution_plan)
                .post(post_execution_plan)
                .put(put_execution_plan),
        )
        .route(
            "/api/v1/tasks/{id}/tree/adopt",
            axum::routing::post(post_tree_adopt),
        )
        .route(
            "/api/v1/tasks/{id}/execution",
            axum::routing::get(get_task_execution),
        )
        .route(
            "/api/v1/tasks/{id}/execution/phase-gate",
            axum::routing::post(post_phase_gate),
        )
        .route(
            "/api/v1/tasks/{id}/execution/plan-gate",
            axum::routing::post(post_plan_gate),
        )
        .route(
            "/api/v1/tasks/{id}/execution/decompose",
            axum::routing::post(post_decompose),
        )
        .route(
            "/api/v1/metrics/execution",
            axum::routing::get(get_execution_metrics),
        )
        // ADR-0079 D11（Phase R4a）: 木と roll-up（`GET /tasks/{id}/tree` は ADR-0043 D6 の作業ツリーの閲覧が
        // 既に使っているので `task-tree`。ADR-0079 付記「R4a 実装時の逸脱・明確化」）。
        .route(
            "/api/v1/tasks/{id}/task-tree",
            axum::routing::get(get_task_tree),
        )
}

/// ADR-0079 D11 / §7 R4a: `GET /tasks/{id}/task-tree?root=true|false`（読み取り。トークンは要らない）。
/// 既定は問い合わせた task の subtree、`root=true` なら木の root から。節点ごとに段階・unit・自分の分と
/// subtree の roll-up、view の根が木の root なら木の上限の使用。木の無い task は 1 節点（深さ 1）。
/// 不明な task は 404 `task_not_found`、知らないクエリ・`root` が真偽値でなければ 400。
async fn get_task_tree(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["root"])?;
    let from_root = query.bool("root")?.unwrap_or(false);
    let task_id = parse_task_id(&id)?;
    let limits = state.inner.tree_limits;
    let view = state
        .blocking(move |store| {
            task_ops::tree_view::task_tree(store, task_id, from_root, &limits)
                .map_err(|e| ops_problem(store, e, Some("task_tree")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

fn no_active_plan(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "execution_plan_not_found",
        format!("task {id} has no execution plan"),
    )
}

async fn get_execution_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let task_id = parse_task_id(&id)?;
    let view = state
        .blocking(move |store| {
            match task_ops::execution::active_plan(store, task_id)
                .map_err(|e| ops_problem(store, e, Some("execution_plan_get")))?
            {
                Some(view) => {
                    // ADR-0072 D17（Phase E4）: 版の履歴（`versions`）も添える（監査用）。
                    let versions = store
                        .execution_plan_list(task_id)
                        .map_err(crate::problem::store_problem)?;
                    Ok(ExecutionPlanView::new(view.plan, view.work_units, versions))
                }
                None => Err(no_active_plan(&task_id.to_string())),
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

async fn post_execution_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let spec: task_core::ExecutionPlanSpec = read_json(body, false).await?;
    // ADR-0079 R5b-prep: daemon と同じ実効の上限（`[execution.tree]` 以外は既定。celeris の `dispatch_config` と同じ）。
    let limits = ExecutionLimits {
        tree: state.inner.tree_limits,
        ..ExecutionLimits::default()
    };
    let view = state
        .blocking(move |store| {
            let adopted = execution_plan_adopt_op(store, task_id, spec, limits, None)?.direct()?;
            if let Err(e) = task_ops::regate::record_human_plan_gate(
                store,
                task_id,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %task_id, error = %e, "failed to record the human plan's gate decision");
            }
            let work_units = store
                .work_units_for(task_id)
                .map_err(crate::problem::store_problem)?;
            let versions = store
                .execution_plan_list(task_id)
                .map_err(crate::problem::store_problem)?;
            let mut view = ExecutionPlanView::new(adopted.plan, work_units, versions);
            view.adoptions = adopted.adoptions;
            view.decisions_raised = adopted.decisions_raised;
            Ok(view)
        })
        .await?;
    tracing::info!(who = "admin", op = "execution_plan_adopt", task_id = %task_id, plan_id = %view.id, work_units = view.work_units.len(), adoptions = view.adoptions.len(), decisions = view.decisions_raised, "admin: execution plan adopted");
    Ok(json_response(StatusCode::CREATED, &view))
}

pub(crate) fn execution_plan_adopt_op(
    store: &task_core::store::SqliteStore,
    task_id: task_core::TaskId,
    spec: task_core::ExecutionPlanSpec,
    limits: task_core::ExecutionLimits,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<task_ops::execution::HumanPlanAdopted>, ApiProblem> {
    let target = task_id.to_string();
    let prepared = task_ops::execution::prepare_human_plan_adoption(
        store,
        task_id,
        spec,
        limits,
        "human",
        OffsetDateTime::now_utc(),
    )
    .map_err(|e| {
        let problem = ops_problem(store, e, Some("execution_plan_adopt"));
        if let Some(audit) = audit {
            audit.reject(store, "task", &target, problem)
        } else {
            problem
        }
    })?;
    if let Some(audit) = audit {
        let result = serde_json::json!({
            "plan_id": prepared.result.plan.id,
            "adoptions": prepared.result.adoptions,
            "decisions_raised": prepared.result.decisions_raised,
        });
        let op = audit.apply(store, "task", &target, "execution.plan_adopt", |tx| {
            let ok = task_core::store::SqliteStore::execution_plan_adopt_tree_tx(
                tx,
                task_id,
                prepared.result.plan.clone(),
                prepared.work_units,
                prepared.extra_events,
                prepared.event,
                prepared.after_events,
                prepared.adoptions,
            )?;
            if !ok {
                return Err(task_core::chat::ChatError::Conflict(
                    "plan adoption changed concurrently".into(),
                ));
            }
            Ok(result)
        })?;
        task_ops::execution::record_human_plan_notice(
            store,
            &prepared.result,
            "human",
            OffsetDateTime::now_utc(),
        );
        Ok(crate::cos::operations::Applied::Audited(Box::new(op)))
    } else {
        if !store
            .execution_plan_adopt_tree(
                task_id,
                prepared.result.plan.clone(),
                prepared.work_units,
                prepared.extra_events,
                prepared.event,
                prepared.after_events,
                prepared.adoptions,
            )
            .map_err(crate::problem::store_problem)?
        {
            return Err(ApiProblem::new(
                StatusCode::CONFLICT,
                "adopt_conflict",
                "plan adoption changed concurrently",
            ));
        }
        task_ops::execution::record_human_plan_notice(
            store,
            &prepared.result,
            "human",
            OffsetDateTime::now_utc(),
        );
        Ok(crate::cos::operations::Applied::Direct(prepared.result))
    }
}

/// ADR-0079 R5b-fix1: `PUT /tasks/{id}/execution-plan`（管理系）。有効な計画が無ければ `POST` と同じ（新規の採用、201）。
/// 有効な計画があれば人の replan（`task_ops::execution::replan`、origin human。本文は計画の**全体**で、差分の形は
/// 受け付けない）で 200。done の WU は同じ key・`kind`・`phase`（/3 の段階）・`depends_on` で残す必要があり、spec の
/// ほかの欄（`checks` など）は上書きできる（`replan.overridden_done`、`Event::WorkUnitSpecOverridden`）。
async fn put_execution_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let spec: task_core::ExecutionPlanSpec = read_json(body, false).await?;
    let limits = ExecutionLimits {
        tree: state.inner.tree_limits,
        ..ExecutionLimits::default()
    };
    let (status, view) = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let has_active = store
                .execution_plan_active(task_id)
                .map_err(crate::problem::store_problem)?
                .is_some();
            let (status, plan, adoptions, decisions_raised, diff) = if has_active {
                let (plan, diff) = task_ops::execution::replan(
                    store,
                    task_id,
                    spec,
                    "replan (human PUT)".to_string(),
                    task_core::PlanOrigin::Human,
                    None,
                    limits,
                    now,
                )
                .map_err(|e| ops_problem(store, e, Some("execution_plan_replan")))?;
                (StatusCode::OK, plan, Vec::new(), 0, Some(diff))
            } else {
                let adopted = task_ops::execution::adopt_human_plan(
                    store, task_id, spec, limits, "human", now,
                )
                .map_err(|e| ops_problem(store, e, Some("execution_plan_adopt")))?;
                // ADR-0079「R5b-fix3」: POST と同じく、人の計画の採用は gate の記録を人の compound にする。
                if let Err(e) = task_ops::regate::record_human_plan_gate(store, task_id, now) {
                    tracing::warn!(task_id = %task_id, error = %e, "failed to record the human plan's gate decision");
                }
                (
                    StatusCode::CREATED,
                    adopted.plan,
                    adopted.adoptions,
                    adopted.decisions_raised,
                    None,
                )
            };
            let work_units = store
                .work_units_for(task_id)
                .map_err(crate::problem::store_problem)?;
            let versions = store
                .execution_plan_list(task_id)
                .map_err(crate::problem::store_problem)?;
            let mut view = ExecutionPlanView::new(plan, work_units, versions);
            view.adoptions = adoptions;
            view.decisions_raised = decisions_raised;
            view.replan = diff;
            Ok((status, view))
        })
        .await?;
    match &view.replan {
        Some(diff) => {
            tracing::info!(who = "admin", op = "execution_plan_replan", task_id = %task_id, plan_id = %view.id, version = view.version, added = diff.added.len(), changed = diff.changed.len(), removed = diff.removed.len(), overridden_done = ?diff.overridden_done, "admin: execution plan replanned by a human")
        }
        None => {
            tracing::info!(who = "admin", op = "execution_plan_adopt", task_id = %task_id, plan_id = %view.id, work_units = view.work_units.len(), adoptions = view.adoptions.len(), decisions = view.decisions_raised, "admin: execution plan adopted")
        }
    }
    Ok(json_response(status, &view))
}

/// CoS の `PUT /tasks/{id}/execution-plan`（ADR 2026-10-09 D3/D5、`execution.put_plan`）。人の `PUT` と同じ
/// 検証（`task_ops::execution::plan_replan`）で組み立てた版の置き換えを、監査と同じ transaction で書く。
/// active な計画が無い task への初回の採用（`adopt_human_plan`）は `execution.adopt_plan` と同じく未登録で、
/// 理由付きの 422 で記録する。
pub(crate) fn put_plan_audited(
    store: &task_core::store::SqliteStore,
    task_id: task_core::TaskId,
    spec: task_core::ExecutionPlanSpec,
    tree: task_core::TreeLimits,
    audit: &crate::cos::operations::OperationAudit,
) -> Result<task_core::chat::CosOperation, ApiProblem> {
    let target_id = task_id.to_string();
    let limits = ExecutionLimits {
        tree,
        ..ExecutionLimits::default()
    };
    let has_active = store
        .execution_plan_active(task_id)
        .map_err(|e| audit.reject(store, "task", &target_id, store_problem(e)))?
        .is_some();
    if !has_active {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            crate::cos::operations::unprocessable(
                "cos_operation_not_allowed",
                "PUT /api/v1/tasks/{id}/execution-plan without an active plan adopts a first plan, \
                 which is not a registered CoS operation yet (pending with execution.adopt_plan); \
                 only a replan of an active plan is",
            ),
        ));
    }
    let now = OffsetDateTime::now_utc();
    let (write, diff) = task_ops::execution::plan_replan(
        store,
        task_id,
        spec,
        "replan (cos PUT)".to_string(),
        task_core::PlanOrigin::Human,
        None,
        limits,
        now,
    )
    .map_err(|e| {
        audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(store, e, Some("execution_plan_replan")),
        )
    })?;
    audit.apply(store, "task", &target_id, "execution.put_plan", |tx| {
        let result = serde_json::json!({
            "task_id": target_id,
            "plan_id": write.new_plan.id,
            "version": write.new_plan.version,
            "replan": diff,
        });
        task_core::store::SqliteStore::execution_plan_replan_tx(
            tx,
            write.task_id,
            write.old_plan_id,
            write.new_plan,
            write.updated_work_units,
            write.new_work_units,
            write.extra_events,
            write.plan_event,
        )?;
        Ok(result)
    })
}

/// ADR-0079 D15（Phase R5b-prep）: `POST /tasks/{id}/tree/adopt {task_id, stage, unit_key}`（管理系 = 人だけ）。
/// 採用済みの /3 の計画の kind task の unit（`adopt: <task_id>` を持つ）に既存の task を結ぶ（`task_ops::tree_adopt::adopt`）。
/// 200 は `AdoptionOutcome`。404: task が無い。422: 木が無効・計画が /3 でない・unit が無い / kind task でない /
/// 段階が違う / `adopt` の id が違う・別の案件・祖先・対象の種類。409: 対象が終端でない / 中止済み / 他の木に属する・
/// unit が既に子を持つか pending / ready でない・計画を持つ task が終端・競合。
async fn post_tree_adopt(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let req: task_ops::tree_adopt::AdoptRequest = read_json(body, false).await?;
    let limits = state.inner.tree_limits;
    let outcome = state
        .blocking(move |store| tree_adopt_op(store, task_id, req, limits, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = "tree_adopt", task_id = %task_id, unit = %outcome.unit_key, adopted = %outcome.task_id, "admin: task adopted into the tree");
    Ok(json_response(StatusCode::OK, &outcome))
}

pub(crate) fn tree_adopt_op(
    store: &task_core::store::SqliteStore,
    task_id: task_core::TaskId,
    req: task_ops::tree_adopt::AdoptRequest,
    limits: task_core::TreeLimits,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<task_ops::tree_adopt::AdoptionOutcome>, ApiProblem> {
    let target_id = task_id.to_string();
    let prepared = task_ops::tree_adopt::prepare_adopt(
        store,
        task_id,
        &req,
        &limits,
        "human",
        OffsetDateTime::now_utc(),
    )
    .map_err(|e| {
        let problem = ops_problem(store, e, Some("tree_adopt"));
        if let Some(audit) = audit {
            audit.reject(store, "task", &target_id, problem)
        } else {
            problem
        }
    })?;
    if let Some(audit) = audit {
        let result = serde_json::to_value(&prepared.outcome)
            .map_err(|e| ApiProblem::internal(e.to_string()))?;
        let op = audit.apply(store, "task", &target_id, "tree.adopt", |tx| {
            let ok = task_core::store::SqliteStore::tree_adopt_apply_tx(
                tx,
                prepared.owner_id,
                &prepared.unit_id,
                prepared.expect_unit_status,
                prepared.updated,
                prepared.events,
                prepared.adoption,
            )?;
            if !ok {
                return Err(task_core::chat::ChatError::Conflict(
                    "tree adoption changed concurrently".into(),
                ));
            }
            Ok(result)
        })?;
        Ok(crate::cos::operations::Applied::Audited(Box::new(op)))
    } else {
        if !store
            .tree_adopt_apply(
                prepared.owner_id,
                &prepared.unit_id,
                prepared.expect_unit_status,
                prepared.updated,
                prepared.events,
                prepared.adoption,
            )
            .map_err(crate::problem::store_problem)?
        {
            return Err(ApiProblem::new(
                StatusCode::CONFLICT,
                "adopt_conflict",
                "tree adoption changed concurrently",
            ));
        }
        Ok(crate::cos::operations::Applied::Direct(prepared.outcome))
    }
}

/// ADR-0072 D19（Phase E5）: `GET /tasks/{id}/execution`。`TaskDetail.execution`（D20 の要約）と
/// 同じ材料（`task_ops::view::task_detail`）を使い、計画は `ExecutionPlanView`（全文の
/// `WorkUnitSpec` 付き）で、run はファイルの有無も添えて返す。
async fn get_task_execution(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let task_id = parse_task_id(&id)?;
    let ctx = state.inner.view.clone();
    let view = state
        .blocking(move |store| {
            let mut detail =
                task_ops::view::task_detail(store, task_id, &ctx, OffsetDateTime::now_utc())
                    .map_err(|e| ops_problem(store, e, Some("task_execution_get")))?;
            let task = detail.task.clone();
            for run in &mut detail.runs {
                run.files = Some(crate::files::run_files(
                    &task,
                    &ctx.workspace_root,
                    &run.run_id,
                ));
            }
            let plan = match task_ops::execution::active_plan(store, task_id)
                .map_err(|e| ops_problem(store, e, Some("task_execution_get")))?
            {
                Some(pv) => {
                    let versions = store.execution_plan_list(task_id).map_err(store_problem)?;
                    // ADR-0074 D4.3（Phase F3 quota）: WU ごとの quota 消費を差し込む。
                    let events = store.events_for(task_id).map_err(store_problem)?;
                    let event_list: Vec<task_core::Event> =
                        events.iter().map(|(_, e)| e.clone()).collect();
                    let by_wu = task_core::group_quota_by_work_unit(&event_list);
                    Some(
                        ExecutionPlanView::new(pv.plan, pv.work_units, versions)
                            .with_quota(&by_wu)
                            .with_serialized_reason(&event_list),
                    )
                }
                None => None,
            };
            let (gate, route, phase, metrics, phase_checkpoint, awaiting_children, plan_approval) =
                match detail.execution {
                    Some(e) => (
                        e.gate,
                        e.route,
                        e.phase,
                        e.metrics,
                        e.phase_checkpoint,
                        e.awaiting_children,
                        e.plan_approval,
                    ),
                    None => (
                        None,
                        None,
                        None,
                        // ADR-0130 D4: Execution 節の無い task でも behind の最後の snapshot は出す。
                        task_core::summarize_execution_metrics(&task, &[]).with_behind_target(
                            &task_ops::behind_target::behind_target_of(
                                store,
                                task_id,
                                time::OffsetDateTime::now_utc(),
                            )
                            .map_err(|e| ops_problem(store, e, Some("task_execution_get")))?,
                        ),
                        None,
                        Vec::new(),
                        None,
                    ),
                };
            Ok(TaskExecutionView {
                gate,
                route,
                phase,
                plan,
                runs: detail.runs,
                metrics,
                phase_checkpoint,
                awaiting_children,
                plan_approval,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// ADR-0074 D2.4（Phase F3 途中確認）: `POST /tasks/{id}/execution/phase-gate`（管理系 = 人だけ）。
/// 本文 `{"action": "continue" | "replan" | "withdraw", "note": "…"}`。awaiting_human でなければ 409、
/// `replan` で `note` が空なら 422。応答は `TransitionResult`。
async fn post_phase_gate(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let req: task_ops::phase_gate::PhaseGateRequest = read_json(body, false).await?;
    let action = req.action;
    let result = state
        .blocking(move |store| phase_gate_op(store, task_id, req, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = "phase_gate", task_id = %task_id, action = action.as_str(), to = ?result.to, "admin: phase gate");
    Ok(json_response(StatusCode::OK, &result))
}

fn phase_gate_problem(store: &task_core::SqliteStore, e: task_ops::OpsError) -> ApiProblem {
    match e {
        task_ops::OpsError::Validation(message) => {
            ApiProblem::validation(vec![crate::types::ValidationError {
                field: Some("note".to_string()),
                message,
            }])
        }
        other => ops_problem(store, other, Some("phase_gate")),
    }
}

/// `POST /tasks/{id}/execution/phase-gate` の本体。handler（`audit = None`）と CoS の
/// `/cos/operations`（ADR 2026-10-05 D3）が共有する。監査ありでは再開の遷移（と人のメモの
/// `Answered`）を `cos_operation_apply` の transaction 内で書く。`withdraw` は中止の連鎖が
/// 1 transaction に収まらないので CoS からは 422（人が受信箱で取り下げる）。
pub(crate) fn phase_gate_op(
    store: &task_core::SqliteStore,
    task_id: task_core::TaskId,
    req: task_ops::phase_gate::PhaseGateRequest,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<task_ops::gate::TransitionResult>, ApiProblem> {
    use crate::cos::operations::Applied;
    use task_ops::phase_gate::PhaseGatePlan;
    let Some(audit) = audit else {
        return task_ops::phase_gate::phase_gate(store, task_id, req.action, req.note)
            .map(Applied::Direct)
            .map_err(|e| phase_gate_problem(store, e));
    };
    let target_id = task_id.to_string();
    let plan = task_ops::phase_gate::plan_phase_gate(store, task_id, req.action, req.note)
        .map_err(|e| audit.reject(store, "task", &target_id, phase_gate_problem(store, e)))?;
    let PhaseGatePlan::Resume {
        trigger,
        extra_event,
        from,
    } = plan
    else {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            ApiProblem::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "cos_operation_not_allowed",
                "phase-gate withdraw cascades a cancellation and is left to a human",
            ),
        ));
    };
    let action = req.action.as_str();
    let operation = audit.apply(store, "task", &target_id, "execution.phase_gate", |tx| {
        let outcome = task_core::SqliteStore::apply_transition_tx(
            tx,
            task_id,
            trigger,
            extra_event.into_iter().map(|e| *e).collect(),
        )?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "action": action,
            "from": from,
            "to": outcome.next,
            "reason": outcome.reason.to_string(),
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// ADR-0079 D8（Phase R3b）: `POST /tasks/{id}/execution/plan-gate`（管理系 = 人だけ。回答の主体は `human`）。
/// 本文 `{"action": "approve" | "replan" | "withdraw", "note": "…"}`（`decision` は `action` の別名）。
/// root の計画の承認待ち（`awaiting_plan_approval`）でなければ 409、`replan` で `note` が空・note が長すぎるなら 422、
/// 無い task は 404。応答は `TransitionResult`。
async fn post_plan_gate(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let req: task_ops::plan_gate::PlanGateRequest = read_json(body, false).await?;
    let action = req.action;
    let result = state
        .blocking(move |store| {
            task_ops::plan_gate::plan_gate(store, task_id, req.action, req.note, "human").map_err(
                |e| match e {
                    task_ops::OpsError::Validation(message) => {
                        ApiProblem::validation(vec![crate::types::ValidationError {
                            field: Some("note".to_string()),
                            message,
                        }])
                    }
                    other => ops_problem(store, other, Some("plan_gate")),
                },
            )
        })
        .await?;
    tracing::info!(who = "admin", op = "plan_gate", task_id = %task_id, action = action.as_str(), to = ?result.to, "admin: plan gate");
    Ok(json_response(StatusCode::OK, &result))
}

/// ADR-0072「Phase F6 実装時の決定」: `POST /tasks/{id}/execution/decompose`（管理系 = 人だけ）。
/// 本文 `{"mode": "compound" | "atomic", "note": "…"}`。起票済みの Task の実行の形を人が決め直す
/// （`execution_hint = {mode, explicit: true}`、前の gate の判定を消す、`ExecutionHintSet{source: "human"}`）。
/// 次の dispatch で gate が `human/explicit` として判定し直す。計画を持つ Task への `compound` は
/// replan の依頼。応答は `DecomposeResult`（200）。409: `running` / `reviewing` / 終端 / 計画を持つ Task の
/// `atomic`。422: gate の対象外・`note` が長すぎる。
async fn post_decompose(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let req: task_ops::regate::DecomposeRequest = read_json(body, false).await?;
    let result = state
        .blocking(move |store| decompose_op(store, task_id, req, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = "execution_decompose", task_id = %task_id, mode = result.mode.as_str(), replan = result.replan, "admin: execution mode set by a human");
    Ok(json_response(StatusCode::OK, &result))
}

pub(crate) fn decompose_op(
    store: &task_core::store::SqliteStore,
    task_id: task_core::TaskId,
    req: task_ops::regate::DecomposeRequest,
    audit: Option<&crate::cos::operations::OperationAudit>,
) -> Result<crate::cos::operations::Applied<task_ops::regate::DecomposeResult>, ApiProblem> {
    use crate::cos::operations::Applied;
    let Some(audit) = audit else {
        return task_ops::regate::set_execution_mode(
            store,
            task_id,
            req.mode,
            "human",
            req.note,
            OffsetDateTime::now_utc(),
        )
        .map(Applied::Direct)
        .map_err(|error| ops_problem(store, error, Some("execution_decompose")));
    };
    let target_id = task_id.to_string();
    let (result, event) = task_ops::regate::plan_execution_mode(
        store,
        task_id,
        req.mode,
        "cos",
        req.note,
        OffsetDateTime::now_utc(),
    )
    .map_err(|error| {
        audit.reject(
            store,
            "task",
            &target_id,
            ops_problem(store, error, Some("execution_decompose")),
        )
    })?;
    let operation = audit.apply(store, "task", &target_id, "execution.decompose", |tx| {
        let updated = task_core::store::SqliteStore::edit_task_tx(tx, &result.task, &event)?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "mode": result.mode,
            "replan": result.replan,
            "previous_decision": result.previous_decision,
            "task": updated,
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

fn bad_group_by(group_by: &str) -> ApiProblem {
    ApiProblem::bad_request(format!(
        "`group_by` must be one of {} (got {group_by:?})",
        crate::stats::EXECUTION_METRICS_GROUP_BY.join("|")
    ))
}

/// ADR-0072 D19（Phase E5）: `GET /metrics/execution?since=&group_by=gate_mode|genre|assignee|lane`。
/// 集計は `crate::stats::execution_metrics_summary` が索引行から行い、索引にない情報だけ
/// 該当タスクの events で補完する。
async fn get_execution_metrics(
    axum::extract::State(state): axum::extract::State<ApiState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["since", "group_by"])?;
    let since = match query.single("since")? {
        Some(s) if !s.trim().is_empty() => Some(
            OffsetDateTime::parse(s.trim(), &Rfc3339)
                .map_err(|_| ApiProblem::bad_request("`since` must be an RFC 3339 timestamp"))?,
        ),
        _ => None,
    };
    let group_by = query.single("group_by")?.unwrap_or("gate_mode").to_string();
    if !crate::stats::EXECUTION_METRICS_GROUP_BY.contains(&group_by.as_str()) {
        return Err(bad_group_by(&group_by));
    }
    let mut summary = state
        .blocking(move |store| {
            crate::stats::execution_metrics_summary(store, since, &group_by).map_err(store_problem)
        })
        .await?;
    // ADR-0074 D4.3（Phase F3 quota）: 今のアカウントの残量を最上位に足す（`GET /llm/sources` と
    // 同じ値）。`[llm_proxy]` が無効なら空のまま（quota はここでは判断材料ではなく観測なので、
    // 409 にはしない。D4）。
    if let Some(reader) = state.inner.llm_sources.clone() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let sources = reader.view(now).await;
        summary.accounts_now = sources
            .sources
            .into_iter()
            .flat_map(|s| {
                let source = s.id;
                s.accounts
                    .into_iter()
                    .map(move |a| crate::types::AccountNowView {
                        source: source.clone(),
                        id: a.id,
                        remaining_short: a.remaining_short,
                        remaining_long: a.remaining_long,
                        cooldown_until: a.cooldown_until,
                    })
            })
            .collect();
    }
    Ok(json_response(StatusCode::OK, &summary))
}

/// ADR 2026-10-05 D3（D6 一次対応 A）: CoS が root の計画の承認（plan gate）を既承認の範囲で答える監査
/// つきの操作。検証は人の経路と同じ `plan_gate::plan_plan_gate`（回答の主体は `cos`）、遷移は
/// `cos_operation_apply` の transaction 内で書く。`withdraw` は中止の連鎖なので CoS からは 422。
pub(crate) fn plan_gate_op(
    store: &task_core::SqliteStore,
    task_id: task_core::TaskId,
    req: task_ops::plan_gate::PlanGateRequest,
    audit: &crate::cos::operations::OperationAudit,
) -> Result<crate::cos::operations::Applied<()>, ApiProblem> {
    use task_ops::plan_gate::PlanGatePlan;
    let target_id = task_id.to_string();
    let action = req.action.as_str();
    let plan = task_ops::plan_gate::plan_plan_gate(store, task_id, req.action, req.note, "cos")
        .map_err(|e| {
            let problem = match e {
                task_ops::OpsError::Validation(message) => {
                    ApiProblem::validation(vec![crate::types::ValidationError {
                        field: Some("note".to_string()),
                        message,
                    }])
                }
                other => ops_problem(store, other, Some("plan_gate")),
            };
            audit.reject(store, "task", &target_id, problem)
        })?;
    let PlanGatePlan::Resume {
        trigger,
        extra,
        from,
    } = plan
    else {
        return Err(audit.reject(
            store,
            "task",
            &target_id,
            ApiProblem::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "cos_operation_not_allowed",
                "plan-gate withdraw cascades a cancellation and is left to a human",
            ),
        ));
    };
    let operation = audit.apply(store, "task", &target_id, "execution.plan_gate", |tx| {
        let outcome = task_core::SqliteStore::apply_transition_tx(tx, task_id, trigger, extra)?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "action": action,
            "from": from,
            "to": outcome.next,
            "reason": outcome.reason.to_string(),
        }))
    })?;
    Ok(crate::cos::operations::Applied::Audited(Box::new(
        operation,
    )))
}
