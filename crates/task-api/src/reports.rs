//! 報告の API（ADR-0033 D3。Phase 25。`docs/api/v1/api.md` §3.30）。
//!
//! - `GET /reports` — 一覧（新しい順、絞り込みつき）。読み取りなので通常の認証だけ。
//! - `GET /reports/{id}` — 1 件（`sources` の中身も展開する）。
//!
//! 既読と通知は受信箱（`GET /api/v1/notifications` と `POST /notifications/read-all`）が扱う。旧い
//! `POST /reports/read`・`POST /reports/notified` は ADR 2026-11-17（Console・旧 API・aider の撤去） D3 で消した。

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::report::{Report, ReportFilter, ReportId, ReportsLive};
use task_core::{ProjectId, SqliteStore};

use crate::handlers::{ApiResult, json_response};
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

/// ADR-0033 D3: スナップショットに載せる `reports`（未読の件数）。
pub(crate) fn reports_live(store: &SqliteStore) -> Option<ReportsLive> {
    let (unread_secretary, unread_bad_news) = store.report_unread_counts(0).ok()?;
    Some(ReportsLive {
        unread_secretary,
        unread_bad_news,
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

/// ADR 2026-10-15-verify-counts-without-page-limit D1: 全行数だけを返す応答（上限なし）。
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RowCount {
    pub count: u64,
}

/// `GET /api/v1/reports/count`: `reports` の全行数（release verify の件数照合用）。
pub(crate) async fn count_reports(State(state): State<ApiState>) -> ApiResult {
    let count = state
        .blocking(|store| store.report_count_all().map_err(store_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &RowCount { count }))
}

/// `GET /api/v1/messages/count`: `messages` の全行数（release verify の件数照合用）。
pub(crate) async fn count_messages(State(state): State<ApiState>) -> ApiResult {
    let count = state
        .blocking(|store| store.message_count_all().map_err(store_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &RowCount { count }))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/v1/reports", get(list))
        .route("/api/v1/reports/count", get(count_reports))
        .route("/api/v1/messages/count", get(count_messages))
        .route("/api/v1/reports/{id}", get(detail))
}
