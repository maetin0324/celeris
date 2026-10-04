//! ADR-0133 D5: two human-facing feeds. Classification lives in task-ops;
//! persistence and bundling live in task-core.
use axum::body::{Body, to_bytes};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::http::{HeaderValue, header};
use axum::routing::{get, post};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    Event, Notice, NoticeId, NoticeKind, NoticeQuery, NoticeStore, Status, TaskStore,
    WorkUnitBlockedReason, WorkUnitStatus,
};
use task_ops::human_inbox::{HumanInbox, InboxItem, InboxKind, KnowledgePending};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/inbox/items", get(items))
        .route("/api/v1/inbox/items/{id}", get(item))
        .route("/api/v1/inbox/items/{id}/answer", post(answer))
        .route("/api/v1/notifications", get(notifications))
        .route("/api/v1/notifications/unread-count", get(unread_count))
        .route("/api/v1/notifications/read-all", post(read_all))
        .route("/api/v1/notifications/{id}/read", post(read_one))
}

pub(crate) fn deprecated(
    mut response: axum::response::Response,
    successor: &'static str,
) -> axum::response::Response {
    response
        .headers_mut()
        .insert("deprecation", HeaderValue::from_static("true"));
    response
        .headers_mut()
        .insert(header::LINK, HeaderValue::from_static(successor));
    response
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HumanInboxView {
    #[serde(flatten)]
    pub feed: HumanInbox,
    /// ADR-0133 D4: attention items auto-closed by the ADR-0131 inbox-rules
    /// (`task_ops::inbox::attention_suppression`), counted per rule. Not narrowed by `project`/`kind`.
    pub suppressed: std::collections::BTreeMap<String, u32>,
}

fn gone(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "inbox-item-gone",
        format!("inbox item {id} is no longer open"),
    )
}

async fn human_feed(state: &ApiState) -> Result<HumanInbox, ApiProblem> {
    let snapshot = state.snapshot();
    let ctx = state.inner.view.clone();
    let knowledge_root = state.inner.knowledge_root.clone();
    state
        .blocking(move |store| {
            let knowledge = knowledge_root
                .as_deref()
                .filter(|r| task_ops::knowledge::exists(r))
                .map(|r| {
                    let candidates = task_ops::knowledge::inbox_list(r);
                    KnowledgePending {
                        count: candidates.len() as u32,
                        oldest_created: candidates.iter().filter_map(|c| c.created.clone()).min(),
                    }
                });
            let root = ctx.workspace_root.clone();
            task_ops::human_inbox::human_inbox(
                store,
                snapshot.as_ref(),
                &ctx,
                OffsetDateTime::now_utc(),
                &|task, run| crate::files::read_evidence(task, &root, run),
                knowledge.as_ref(),
            )
            .map_err(|e| ops_problem(store, e, None))
        })
        .await
}

async fn items(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let q = QueryParams::parse(raw.as_deref(), &["project", "kind"])?;
    let project = q.single("project")?;
    let kind = q.single("kind")?;
    if let Some(k) = kind.filter(|k| !InboxKind::ALL.iter().any(|x| x.as_str() == *k)) {
        return Err(ApiProblem::bad_request(format!("unknown inbox kind: {k}")));
    }
    let mut feed = human_feed(&state).await?;
    let suppressed = std::mem::take(&mut feed.suppressed);
    feed.items.retain(|x| {
        project.is_none_or(|p| x.project_id.as_deref() == Some(p))
            && kind.is_none_or(|k| x.kind.as_str() == k)
    });
    feed.counts.total = feed.items.len() as u32;
    feed.counts.by_kind.clear();
    for x in &feed.items {
        *feed
            .counts
            .by_kind
            .entry(x.kind.as_str().to_owned())
            .or_default() += 1;
    }
    Ok(json_response(
        StatusCode::OK,
        &HumanInboxView { feed, suppressed },
    ))
}

async fn item(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let found = human_feed(&state)
        .await?
        .items
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| gone(&id))?;
    Ok(json_response(StatusCode::OK, &found))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboxAnswerBody {
    pub option: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InboxAnswerResult {
    pub removed: bool,
    pub item_id: String,
    pub result: serde_json::Value,
}

// Build a request to the existing domain endpoint so validation, audit, and state
// transitions remain in one place. The path comes only from the derived InboxItem.
fn delegated_request(
    item: &InboxItem,
    input: &InboxAnswerBody,
) -> Result<(Method, String, serde_json::Value), ApiProblem> {
    let task = item.task.as_ref().map(|t| t.id.to_string());
    let native = item.answer.native.as_ref();
    let base = native.map(|n| n.path.clone());
    let note = input.note.clone().unwrap_or_default();
    let (method, path, body) = match item.kind {
        InboxKind::Decision => (
            Method::POST,
            base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?,
            serde_json::json!({"option":input.option,"note":input.note}),
        ),
        InboxKind::Authorization => (
            Method::POST,
            base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?,
            serde_json::json!({"decision":input.option,"answer":note,"scope":input.payload.as_ref().and_then(|p|p.get("scope"))}),
        ),
        InboxKind::Question => (
            Method::POST,
            base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?,
            serde_json::json!({"answer":note}),
        ),
        InboxKind::AcceptanceCheck => {
            let p = base
                .ok_or_else(|| {
                    ApiProblem::new(
                        StatusCode::CONFLICT,
                        "native_action_required",
                        "this item has no direct domain action",
                    )
                })?
                .replace(
                    "/approve",
                    if input.option == "reject" {
                        "/reject"
                    } else {
                        "/approve"
                    },
                );
            (Method::POST, p, serde_json::json!({"note":input.note}))
        }
        InboxKind::DraftAccept => {
            let p = base.ok_or_else(|| {
                ApiProblem::bad_request("answer each draft through its task endpoint")
            })?;
            (
                Method::POST,
                p.replace(
                    "/accept",
                    if input.option == "cancel" {
                        "/cancel"
                    } else {
                        "/accept"
                    },
                ),
                serde_json::json!({}),
            )
        }
        InboxKind::Failed => {
            let p = format!(
                "/api/v1/tasks/{}/{}",
                task.ok_or_else(|| gone(&item.id))?,
                input.option
            );
            (Method::POST, p, serde_json::json!({}))
        }
        InboxKind::PlanGate | InboxKind::PhaseGate => (
            Method::POST,
            base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?,
            serde_json::json!({"action":input.option,"note":input.note}),
        ),
        InboxKind::ProjectPlan => (
            Method::POST,
            base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?,
            serde_json::json!({"decision":input.option,"note":input.note}),
        ),
        InboxKind::BrowserWait => {
            let p = base.ok_or_else(|| {
                ApiProblem::new(
                    StatusCode::CONFLICT,
                    "native_action_required",
                    "this item has no direct domain action",
                )
            })?;
            let p = if input.option == "registered" {
                p.replace("/credential", "/registered")
            } else {
                p.replace("/credential", "/decision")
            };
            let mut payload = input
                .payload
                .clone()
                .and_then(|v| v.as_object().cloned())
                .ok_or_else(|| {
                    ApiProblem::bad_request(
                        "browser answer requires payload with attestation and version",
                    )
                })?;
            if input.option != "registered" {
                payload.insert(
                    "decision".to_string(),
                    serde_json::json!(if input.option == "approve" {
                        "approve_once"
                    } else {
                        "deny"
                    }),
                );
            }
            (Method::POST, p, serde_json::Value::Object(payload))
        }
        InboxKind::KnowledgeReview => {
            let candidate = input
                .payload
                .as_ref()
                .and_then(|p| p.get("candidate_id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiProblem::bad_request("payload.candidate_id is required"))?;
            if !candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(ApiProblem::bad_request("invalid candidate_id"));
            }
            let mut payload = input
                .payload
                .clone()
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            payload.remove("candidate_id");
            (
                Method::POST,
                format!("/api/v1/knowledge/inbox/{candidate}/{}", input.option),
                serde_json::Value::Object(payload),
            )
        }
        InboxKind::Unroutable => {
            let id = task.ok_or_else(|| gone(&item.id))?;
            if input.option == "cancel" {
                (
                    Method::POST,
                    format!("/api/v1/tasks/{id}/cancel"),
                    serde_json::json!({}),
                )
            } else {
                let payload = input
                    .payload
                    .clone()
                    .filter(|v| v.is_object())
                    .ok_or_else(|| {
                        ApiProblem::bad_request("reassign requires a task edit in payload")
                    })?;
                (Method::PATCH, format!("/api/v1/tasks/{id}"), payload)
            }
        }
        InboxKind::IntegrationRequest | InboxKind::DeliverySkipped | InboxKind::ClusterLogin => {
            return Err(ApiProblem::new(
                StatusCode::CONFLICT,
                "native_action_required",
                "use the linked domain action",
            ));
        }
    };
    Ok((method, path, body))
}

async fn answer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let input: InboxAnswerBody = read_json(body, false).await?;
    let found = human_feed(&state)
        .await?
        .items
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| gone(&id))?;
    let selected = found
        .options
        .iter()
        .find(|o| o.key == input.option)
        .ok_or_else(|| ApiProblem::bad_request("option is not offered by this inbox item"))?;
    if selected.needs_note && input.note.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return Err(ApiProblem::bad_request("note is required for this option"));
    }
    if found.kind == InboxKind::IntegrationRequest {
        let task_id = found.task.as_ref().ok_or_else(|| gone(&id))?.id;
        let item_id = id.clone();
        let answer = input.option.clone();
        let note = input.note.clone();
        let request_id = state
            .blocking(move |store| {
                let row = store
                    .open_integration_requests()
                    .map_err(store_problem)?
                    .into_iter()
                    .find(|row| {
                        if row.task_id != task_id {
                            return false;
                        }
                        let Event::IntegrationRequested { request, .. } = &row.event else {
                            return false;
                        };
                        let id = request.id_for(task_id);
                        let safe_id: String = id
                            .chars()
                            .map(|c| {
                                if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                                    c
                                } else {
                                    '-'
                                }
                            })
                            .collect();
                        item_id == format!("integration_request-{safe_id}")
                    })
                    .ok_or_else(|| gone(&item_id))?;
                let Event::IntegrationRequested {
                    request, origin, ..
                } = row.event
                else {
                    unreachable!("open request rows contain requests only")
                };
                let request_id = request.id_for(task_id);
                // D4 付記（2026-10-04）: 統合 WU がこの依頼で止まっている（task blocked・WU blocked(question)）
                // ときだけ再開・cancel まで行う。既に別の経路（汎用の回答・replan）で再開・完了した WU の
                // 依頼は、回答の記録だけ行って受信箱から消す（410 にして残り続けさせない）。
                let phase_unit = if let Some(key) = origin.strip_prefix("phase:") {
                    let task = store
                        .get(task_id)
                        .map_err(store_problem)?
                        .ok_or_else(|| gone(&item_id))?;
                    store
                        .work_units_for(task_id)
                        .map_err(store_problem)?
                        .into_iter()
                        .find(|unit| {
                            unit.key == key && unit.kind == task_core::WorkUnitKind::Integrate
                        })
                        .filter(|unit| {
                            task.status == Status::Blocked
                                && unit.status == WorkUnitStatus::Blocked
                                && unit.blocked_reason == Some(WorkUnitBlockedReason::Question)
                        })
                } else {
                    None
                };
                if !store
                    .integration_request_answer(task_id, &request_id, &answer, note.as_deref())
                    .map_err(store_problem)?
                {
                    return Err(gone(&item_id));
                }
                if let Some(mut unit) = phase_unit {
                    if answer == "declined" {
                        task_ops::gate::cancel(store, task_id, None)
                            .map_err(|e| ops_problem(store, e, Some("cancel")))?;
                    } else {
                        let previous = unit.status;
                        unit.status = WorkUnitStatus::Pending;
                        unit.blocked_reason = None;
                        unit.updated_at = time::OffsetDateTime::now_utc()
                            .format(&Rfc3339)
                            .map_err(|_| ApiProblem::internal("timestamp encoding failed"))?;
                        store
                            .work_unit_transition(
                                task_id,
                                unit.clone(),
                                Event::WorkUnitTransitioned {
                                    work_unit_id: unit.id,
                                    key: unit.key,
                                    from: previous,
                                    to: WorkUnitStatus::Pending,
                                    reason: "integration_answer".to_string(),
                                    run_id: None,
                                },
                            )
                            .map_err(store_problem)?;
                        task_ops::gate::answer(
                            store,
                            task_id,
                            note.unwrap_or_else(|| answer.clone()),
                            Some(Status::Blocked),
                        )
                        .map_err(|e| ops_problem(store, e, Some("answer")))?;
                    }
                }
                Ok(request_id)
            })
            .await?;
        let removed = !human_feed(&state).await?.items.iter().any(|x| x.id == id);
        return Ok(json_response(
            StatusCode::OK,
            &InboxAnswerResult {
                removed,
                item_id: id,
                result: serde_json::json!({"request_id": request_id}),
            },
        ));
    }
    let requests = if found.kind == InboxKind::DraftAccept && found.answer.native.is_none() {
        found
            .blocking
            .tasks
            .iter()
            .map(|task| {
                (
                    Method::POST,
                    format!(
                        "/api/v1/tasks/{}/{}",
                        task.id,
                        if input.option == "cancel" {
                            "cancel"
                        } else {
                            "accept"
                        }
                    ),
                    serde_json::json!({}),
                )
            })
            .collect::<Vec<_>>()
    } else {
        vec![delegated_request(&found, &input)?]
    };
    if requests.is_empty() {
        return Err(gone(&id));
    }
    let mut paths = Vec::new();
    let mut last_status = StatusCode::OK;
    for (method, path, payload) in requests {
        let mut req = Request::builder()
            .method(method)
            .uri(&path)
            .header("content-type", "application/json");
        if let Some(auth) = headers.get("authorization") {
            req = req.header("authorization", auth);
        }
        if let Some(host) = headers.get("host") {
            req = req.header("host", host);
        }
        let req = req
            .body(Body::from(serde_json::to_vec(&payload).map_err(|_| {
                ApiProblem::internal("answer encoding failed")
            })?))
            .map_err(|_| ApiProblem::internal("answer request failed"))?;
        let response = crate::handlers::router(state.clone())
            .oneshot(req)
            .await
            .map_err(|_| ApiProblem::internal("answer routing failed"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiProblem::new(
                status,
                "delegated_action_failed",
                "the domain action rejected this answer",
            ));
        }
        let _ = to_bytes(response.into_body(), crate::MAX_BODY_BYTES).await;
        paths.push(path);
        last_status = status;
    }
    let removed = !human_feed(&state).await?.items.iter().any(|x| x.id == id);
    Ok(json_response(
        StatusCode::OK,
        &InboxAnswerResult {
            removed,
            item_id: id,
            result: serde_json::json!({"delegated_to": if paths.len() == 1 { serde_json::json!(paths[0]) } else { serde_json::json!(paths) }, "status":last_status.as_u16()}),
        },
    ))
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NotificationsView {
    pub items: Vec<Notice>,
    pub unread: u64,
    pub next_before: Option<String>,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct UnreadCountView {
    pub unread: u64,
    pub events: u64,
    pub by_kind: std::collections::BTreeMap<String, u64>,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct NoticeReadResult {
    pub id: String,
    pub read_at: String,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct NoticeReadAllResult {
    pub marked: u64,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadAllBody {
    pub before: Option<String>,
    pub kind: Option<NoticeKind>,
    pub project: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

fn parse_notice(id: &str) -> Result<NoticeId, ApiProblem> {
    id.parse()
        .map_err(|_| ApiProblem::bad_request("invalid notice id"))
}
fn parse_time(raw: &str) -> Result<OffsetDateTime, ApiProblem> {
    OffsetDateTime::parse(raw, &Rfc3339)
        .map_err(|_| ApiProblem::bad_request("before must be RFC 3339"))
}
fn str_time(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

async fn notifications(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let q = QueryParams::parse(
        raw.as_deref(),
        &["unread", "kind", "project", "limit", "before"],
    )?;
    let kind = q
        .single("kind")?
        .map(|s| NoticeKind::parse(s).ok_or_else(|| ApiProblem::bad_request("unknown notice kind")))
        .transpose()?;
    let project = q.single("project")?.map(str::to_owned);
    let before = q.single("before")?.map(parse_time).transpose()?;
    let limit = q.limit("limit", 50, 500)?;
    let unread = q.bool("unread")?.unwrap_or(false);
    let (items, unread_count) = state
        .blocking(move |store| {
            let count = store.notice_unread_count().map_err(store_problem)?.total;
            let mut offset = 0;
            let mut found = Vec::new();
            loop {
                let page = store
                    .notice_list(&NoticeQuery {
                        unread_only: unread,
                        kinds: kind.into_iter().collect(),
                        limit: 500,
                        offset,
                    })
                    .map_err(store_problem)?;
                let n = page.items.len();
                found.extend(
                    page.items
                        .into_iter()
                        .filter(|x| {
                            project
                                .as_deref()
                                .is_none_or(|p| x.project_id.as_deref() == Some(p))
                                && before.is_none_or(|b| x.last_at < b)
                        })
                        .take(limit - found.len()),
                );
                if found.len() == limit || n < 500 {
                    break;
                }
                offset += n;
            }
            Ok((found, count))
        })
        .await?;
    let next_before = (items.len() == limit)
        .then(|| items.last().map(|x| str_time(x.last_at)))
        .flatten();
    Ok(json_response(
        StatusCode::OK,
        &NotificationsView {
            items,
            unread: unread_count,
            next_before,
        },
    ))
}

async fn unread_count(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let result = state
        .blocking(|store| {
            let c = store.notice_unread_count().map_err(store_problem)?;
            let mut offset = 0;
            let mut events = 0u64;
            loop {
                let p = store
                    .notice_list(&NoticeQuery {
                        unread_only: true,
                        limit: 500,
                        offset,
                        ..NoticeQuery::default()
                    })
                    .map_err(store_problem)?;
                let n = p.items.len();
                events += p.items.iter().map(|x| x.count as u64).sum::<u64>();
                if n < 500 {
                    break;
                }
                offset += n;
            }
            Ok(UnreadCountView {
                unread: c.total,
                events,
                by_kind: c.by_kind,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn read_one(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let _: EmptyBody = read_json(body, true).await?;
    let notice_id = parse_notice(&id)?;
    let result = state
        .blocking(move |store| {
            let old = store
                .notice_get(notice_id)
                .map_err(store_problem)?
                .ok_or_else(|| {
                    ApiProblem::new(
                        StatusCode::NOT_FOUND,
                        "notice_not_found",
                        "notice not found",
                    )
                })?;
            if old.read_at.is_none() {
                store
                    .notice_mark_read(notice_id, OffsetDateTime::now_utc())
                    .map_err(store_problem)?;
            }
            let current = store
                .notice_get(notice_id)
                .map_err(store_problem)?
                .ok_or_else(|| {
                    ApiProblem::new(
                        StatusCode::NOT_FOUND,
                        "notice_not_found",
                        "notice not found",
                    )
                })?;
            Ok(NoticeReadResult {
                id,
                read_at: str_time(current.read_at.unwrap_or_else(OffsetDateTime::now_utc)),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn read_all(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let filter: ReadAllBody = read_json(body, true).await?;
    let before = filter.before.as_deref().map(parse_time).transpose()?;
    let result = state
        .blocking(move |store| {
            let mut offset = 0;
            let mut ids = Vec::new();
            loop {
                let p = store
                    .notice_list(&NoticeQuery {
                        unread_only: true,
                        kinds: filter.kind.into_iter().collect(),
                        limit: 500,
                        offset,
                    })
                    .map_err(store_problem)?;
                let n = p.items.len();
                ids.extend(
                    p.items
                        .into_iter()
                        .filter(|x| {
                            before.is_none_or(|t| x.last_at <= t)
                                && filter
                                    .project
                                    .as_deref()
                                    .is_none_or(|p| x.project_id.as_deref() == Some(p))
                        })
                        .map(|x| x.id),
                );
                if n < 500 {
                    break;
                }
                offset += n;
            }
            let mut marked = 0;
            for id in ids {
                if store
                    .notice_mark_read(id, OffsetDateTime::now_utc())
                    .map_err(store_problem)?
                {
                    marked += 1;
                }
            }
            Ok(NoticeReadAllResult { marked })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}
