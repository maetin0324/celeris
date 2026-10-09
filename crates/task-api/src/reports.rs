//! 報告の API（ADR-0033 D3。Phase 25。`docs/api/v1/gui-api.md` §3.30）。
//!
//! - `GET /reports` — 一覧（新しい順、絞り込みつき）。読み取りなので通常の認証だけ。
//! - `GET /reports/{id}` — 1 件（`sources` の中身も展開する）。
//! - `POST /reports/read` — 既読（**管理系**。トークン必須）。
//! - `POST /reports/notified` — 前回の通知時刻を今に進める（**管理系**）。
//!
//! 通知の判定（`notify_now`）は `task_core::report::notify_now` の決定的な関数で、ここは値を組むだけ。
//! `last_notified_at` は DB に列が無い（migration を足さない）ので、**API プロセスのメモリ**に持つ
//! 観測値として扱う（celeris を再起動すると「まだ通知していない」状態に戻る）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::report::{self, Report, ReportFilter, ReportId, ReportStore, ReportsLive};
use task_core::{NoticeQuery, NoticeStore, ProjectId, SqliteStore};
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, json_response, read_json, rfc3339};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

/// 一覧の既定と上限（`limit`）。
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 500;

/// `GET /reports` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReportList {
    pub items: Vec<Report>,
}

/// `GET /reports/{id}` の応答（`sources` の中身も展開して返す）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReportDetail {
    pub report: Report,
    /// `report.sources` の順に引いた元の報告（見つからなかったものは飛ばす）。
    pub sources_expanded: Vec<Report>,
}

/// `POST /reports/read` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportsReadBody {
    pub ids: Vec<String>,
}

/// `POST /reports/read` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReportsReadResult {
    /// 未読から既読に変わった件数。
    pub updated: usize,
}

/// `POST /reports/notified` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReportsNotifiedResult {
    /// 進めた後の通知時刻（RFC 3339）。
    pub last_notified_at: String,
}

fn report_not_found(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "report_not_found",
        format!("no report {id}"),
    )
}

fn parse_report_id(raw: &str) -> Result<ReportId, ApiProblem> {
    raw.parse::<ReportId>().map_err(|_| report_not_found(raw))
}

/// ADR-0033 D3: スナップショットに載せる `reports`（未読の件数と通知の判定）。
/// ディスパッチャではなく API がここで組む（`last_notified_at` は API のメモリにあるため）。
pub(crate) fn reports_live(
    store: &SqliteStore,
    last_notified_at: Option<OffsetDateTime>,
) -> Option<ReportsLive> {
    let (unread_secretary, unread_bad_news) = store.report_unread_counts(0).ok()?;
    Some(ReportsLive {
        unread_secretary,
        unread_bad_news,
        last_notified_at: last_notified_at.map(rfc3339),
        notify_now: report::notify_now(
            unread_secretary,
            unread_bad_news,
            last_notified_at,
            OffsetDateTime::now_utc(),
        ),
    })
}

pub(crate) async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(
        raw.as_deref(),
        &["project", "node", "level", "unread", "limit"],
    )?;
    let project_id = match query.single("project")? {
        Some(raw) => Some(
            raw.parse::<ProjectId>()
                .map_err(|_| ApiProblem::bad_request("query parameter `project` must be a ULID"))?,
        ),
        None => None,
    };
    let node_id = query.single("node")?.map(str::to_string);
    let level = query
        .u64("level")?
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let unread_only = query.bool("unread")?.unwrap_or(false);
    let limit = query
        .u64("limit")?
        .map(|v| usize::try_from(v).unwrap_or(MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let filter = ReportFilter {
        project_id,
        node_id,
        level,
        // `GET /reports` はタスクでは絞らない（ADR-0044 D5 のタイムラインだけが使う）。
        task_id: None,
        unread_only,
        limit,
    };
    let items = state
        .blocking(move |store| store.report_list(&filter).map_err(store_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &ReportList { items }))
}

pub(crate) async fn detail(
    State(state): State<ApiState>,
    crate::handlers::Params(id): crate::handlers::Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    let report_id = parse_report_id(&id)?;
    let detail = state
        .blocking(move |store| {
            let Some(report) = store.report_get(report_id).map_err(store_problem)? else {
                return Err(report_not_found(&report_id.to_string()));
            };
            let mut sources_expanded = Vec::new();
            for source in &report.sources {
                if let Some(found) = store.report_get(*source).map_err(store_problem)? {
                    sources_expanded.push(found);
                }
            }
            Ok(ReportDetail {
                report,
                sources_expanded,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &detail))
}

/// Unread feed rows of the given reports: the old reports/read entry point also acknowledges them.
/// A bundle is acknowledged when its latest report is among the requested ids.
fn report_notices(
    store: &SqliteStore,
    ids: &[ReportId],
) -> Result<Vec<task_core::NoticeId>, ApiProblem> {
    let wanted: std::collections::HashSet<String> = ids.iter().map(ToString::to_string).collect();
    let mut offset = 0;
    let mut notice_ids = Vec::new();
    loop {
        let page = store
            .notice_list(&NoticeQuery {
                unread_only: true,
                limit: 500,
                offset,
                ..NoticeQuery::default()
            })
            .map_err(store_problem)?;
        let count = page.items.len();
        for notice in page.items {
            if notice
                .target
                .as_ref()
                .is_some_and(|t| t.kind == "report" && wanted.contains(&t.id))
            {
                notice_ids.push(notice.id);
            }
        }
        if count < 500 {
            break;
        }
        offset += count;
    }
    Ok(notice_ids)
}

/// `POST /reports/read` shared by the handler and `/cos/operations` (`report.read`, ADR
/// 2026-10-09-cos-operations-all-mutations D3): the reports and their feed rows are marked read in
/// one transaction with the audit record.
pub(crate) fn mark_read_op(
    store: &SqliteStore,
    read: ReportsReadBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<usize>, ApiProblem> {
    let reject = |problem| match audit {
        Some(audit) => audit.reject(store, "report", "-", problem),
        None => problem,
    };
    let mut ids = Vec::with_capacity(read.ids.len());
    for raw in &read.ids {
        ids.push(parse_report_id(raw).map_err(reject)?);
    }
    let notice_ids = report_notices(store, &ids).map_err(reject)?;
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let updated = store.report_mark_read(&ids, now).map_err(store_problem)?;
        for id in notice_ids {
            store.notice_mark_read(id, now).map_err(store_problem)?;
        }
        return Ok(Applied::Direct(updated));
    };
    let target_id = if ids.len() == 1 {
        ids[0].to_string()
    } else {
        format!("{} reports", ids.len())
    };
    let operation = audit.apply(store, "report", &target_id, "report.read", |tx| {
        let updated = SqliteStore::report_mark_read_tx(tx, &ids, now)?;
        let notices = SqliteStore::notice_mark_ids_read_tx(tx, &notice_ids, now)?;
        Ok(serde_json::json!({"updated": updated, "notices_read": notices}))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

pub(crate) async fn mark_read(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    require_admin(&state, &headers)?;
    let read: ReportsReadBody = read_json(body, false).await?;
    for raw in &read.ids {
        parse_report_id(raw)?;
    }
    let updated = state
        .blocking(move |store| mark_read_op(store, read, None)?.direct())
        .await?;
    tracing::info!(
        who = "admin",
        op = "reports_read",
        updated,
        "admin: reports marked as read"
    );
    Ok(crate::inbox_notifications::deprecated(
        json_response(StatusCode::OK, &ReportsReadResult { updated }),
        "</api/v1/notifications/read-all>; rel=\"successor-version\"",
    ))
}

/// Advance the in-memory notification time (no DB column; see the module doc).
fn advance_notified(state: &ApiState, now: OffsetDateTime) -> Result<(), ApiProblem> {
    match state.inner.last_notified_at.lock() {
        Ok(mut cell) => {
            *cell = Some(now);
            Ok(())
        }
        Err(_) => Err(ApiProblem::internal("the notification state is poisoned")),
    }
}

/// `POST /reports/notified` from `/cos/operations` (`report.notified`). The effect is the API
/// process's in-memory notification time, advanced only when the operation is first applied (a
/// resent request returns the record without advancing it again).
pub(crate) fn notified_op(
    store: &SqliteStore,
    state: &ApiState,
    audit: &OperationAudit,
) -> Result<task_core::chat::CosOperation, ApiProblem> {
    let now = OffsetDateTime::now_utc();
    audit.apply(store, "report", "notified", "report.notified", |_| {
        advance_notified(state, now)
            .map_err(|problem| task_core::chat::ChatError::Invalid(problem.detail().to_string()))?;
        Ok(serde_json::json!({"last_notified_at": rfc3339(now)}))
    })
}

pub(crate) async fn notified(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    require_admin(&state, &headers)?;
    let now = OffsetDateTime::now_utc();
    advance_notified(&state, now)?;
    tracing::info!(
        who = "admin",
        op = "reports_notified",
        "admin: notification time advanced"
    );
    Ok(crate::inbox_notifications::deprecated(
        json_response(
            StatusCode::OK,
            &ReportsNotifiedResult {
                last_notified_at: rfc3339(now),
            },
        ),
        "</api/v1/notifications>; rel=\"successor-version\"",
    ))
}

/// ハンドラ以外から使う（`GET /daemon` がスナップショットに `reports` を載せる）。
pub(crate) fn last_notified_at(state: &ApiState) -> Option<OffsetDateTime> {
    state
        .inner
        .last_notified_at
        .lock()
        .ok()
        .and_then(|cell| *cell)
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/reports", get(list))
        .route("/api/v1/reports/read", post(mark_read))
        .route("/api/v1/reports/notified", post(notified))
        .route("/api/v1/reports/{id}", get(detail))
}
