//! ADR 2026-10-28-cos-review-back-thread D4'/D6: the review thread's report to the person and the
//! read-only lookup of the review thread.
//!
//! - `chat.review_report` (`POST /api/v1/cos/review-report`, a registered CoS operation): the run
//!   of the review thread writes one assistant message to the general thread per review input.
//!   The message and the `cos_operations` row are one transaction ([`report_op`]). The caller's
//!   thread comes from the verified run credential; a run of any other thread is refused (422
//!   `cos_review_report_invalid`, recorded as rejected).
//! - The route itself is called only through `POST /api/v1/cos/operations`: a direct call is 422
//!   (a CoS run is told to use the operation envelope and the refusal is audited; a person,
//!   operator or MCP token is refused as not the review run).
//! - `GET /api/v1/cos/review-thread`: the review thread, 404 when it does not exist yet (never
//!   created here).

use axum::Extension;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use task_core::chat::{self, ChatError, ChatMessage, ChatThreadResponse, CosOperation};
use task_core::store::SqliteStore;

use super::operations::{OperationAudit, unprocessable};
use super::{CosCaller, cos_problem, record_direct_rejection};
use crate::handlers::{ApiResult, json_response, no_query};
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// The registered route of `chat.review_report`.
pub(crate) const REVIEW_REPORT_PATH: &str = "/api/v1/cos/review-report";
/// Problem code of a refused report (D4').
pub const REVIEW_REPORT_INVALID: &str = "cos_review_report_invalid";

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(REVIEW_REPORT_PATH, post(report_direct))
        .route("/api/v1/cos/review-thread", get(review_thread))
}

/// Body of `chat.review_report`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewReportBody {
    /// The review input (`role=user`, key `cron-cos-review:…`) of the review thread this report
    /// answers.
    pub review_message_id: String,
    /// 1 to 4000 characters, written to the general thread as is.
    pub text: String,
}

/// Result of `chat.review_report` (`operation.result`).
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReviewReportResult {
    /// The report in the general thread (`provenance.kind = cos_review_report`).
    pub message: ChatMessage,
    /// `false` when the review input already had its report: the first one is returned and
    /// nothing is written.
    pub created: bool,
}

fn invalid(detail: impl Into<String>) -> ApiProblem {
    unprocessable(REVIEW_REPORT_INVALID, detail)
}

/// The id of the thread of `kind` (`general` or `review`), if any.
fn thread_of_kind(conn: &rusqlite::Connection, kind: &str) -> Result<Option<String>, ChatError> {
    Ok(conn
        .query_row("SELECT id FROM chat_threads WHERE kind=?1", [kind], |row| {
            row.get(0)
        })
        .optional()?)
}

/// `chat.review_report` inside the CoS operation transaction (D4'). The caller's thread (from
/// the credential) must be the review thread; the report and the audit row commit together.
pub(crate) fn report_op(
    store: &SqliteStore,
    audit: &OperationAudit,
    body: ReviewReportBody,
) -> Result<CosOperation, ApiProblem> {
    let now = time::OffsetDateTime::now_utc();
    let caller_thread = audit.ctx.thread_id.clone();
    let run_id = audit.ctx.run_id.clone();
    let operation_id = audit.ctx.operation_id.clone();
    audit.apply_checked(
        store,
        // The general thread is named as `chat.thread_create` names it (it may not exist yet;
        // the report opens it).
        "chat_thread",
        "general",
        "chat.review_report",
        |tx| {
            let review = thread_of_kind(tx, "review").map_err(cos_problem)?;
            if review.as_deref() != Some(caller_thread.as_str()) {
                return Err(invalid(format!(
                    "only the run of the review thread reports; this run belongs to thread \
                     {caller_thread}"
                )));
            }
            let reported = chat::cos_review_report_tx(
                tx,
                &body.review_message_id,
                &body.text,
                Some(&run_id),
                &operation_id,
                now,
            )
            .map_err(|error| match error {
                ChatError::Invalid(detail) => invalid(detail),
                other => crate::chat::chat_problem(other),
            })?;
            serde_json::to_value(ReviewReportResult {
                message: reported.message,
                created: reported.created,
            })
            .map_err(|e| ApiProblem::internal(e.to_string()))
        },
    )
}

/// A direct `POST /cos/review-report` (outside the operation envelope) is refused: the report
/// needs the audit context of `POST /api/v1/cos/operations`.
async fn report_direct(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let Some(Extension(caller)) = caller else {
        return Err(invalid(
            "only the CoS run of the review thread reports, through POST /api/v1/cos/operations",
        ));
    };
    state
        .blocking(move |store| {
            record_direct_rejection(
                store,
                &caller,
                "POST",
                REVIEW_REPORT_PATH,
                "chat.review_report runs only inside POST /api/v1/cos/operations",
            )
            .map_err(|error| ApiProblem::internal(format!("audit record failed: {error}")))
        })
        .await?;
    Err(ApiProblem::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "cos_audit_context_required",
        format!("POST {REVIEW_REPORT_PATH} needs the audit context of POST /api/v1/cos/operations"),
    )
    .with_extra("instead", "/api/v1/cos/operations"))
}

/// D6 `GET /cos/review-thread`: the review thread, or 404 when the first review has not run yet.
async fn review_thread(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let thread = state
        .blocking(|store| store.cos_review_thread_get().map_err(cos_problem))
        .await?;
    match thread {
        Some(thread) => Ok(json_response(
            StatusCode::OK,
            &ChatThreadResponse { thread },
        )),
        None => Err(cos_problem(ChatError::NotFound {
            kind: "thread",
            id: "review".into(),
        })),
    }
}
