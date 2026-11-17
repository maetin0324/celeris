//! ADR-0048 D2: `GET /tasks/{id}/runs/{run_id}/events`（run のログ画面が開く、その run の全行）。
//! Console の入口は ADR 2026-11-17（Console・旧 API・aider の撤去） D1 で消した。

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use task_core::{Event, EventRow};

use crate::handlers::{ApiResult, json_response};
use crate::problem::{ApiProblem, store_problem};
use crate::query::{QueryParams, parse_task_id};
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route(
        "/api/v1/tasks/{id}/runs/{run_id}/events",
        axum::routing::get(run_events),
    )
}

/// `GET /tasks/{id}/runs/{run_id}/events`（ADR-0048 D2: 折り畳んだ `progress` を開いたときの全行）。
/// その run に紐づくイベントだけを `seq` 昇順で返す（`GET /tasks/{id}/events` の絞り込み版）。
pub(crate) async fn run_events(
    State(state): State<ApiState>,
    crate::handlers::Params((id, run_id)): crate::handlers::Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["after_seq", "limit"])?;
    let task_id = parse_task_id(&id)?;
    let after_seq = query.u64("after_seq")?;
    let limit = query.limit("limit", 500, 5_000)?;
    let page = state
        .blocking(move |store| {
            if store.get(task_id).map_err(store_problem)?.is_none() {
                return Err(ApiProblem::task_not_found(task_id));
            }
            let rows = store
                .event_rows_for(task_id, after_seq, RUN_EVENTS_SCAN)
                .map_err(store_problem)?;
            let mut items: Vec<EventRow> = rows
                .into_iter()
                .filter(|row| run_of(&row.event) == Some(run_id.as_str()))
                .collect();
            let has_more = items.len() > limit;
            items.truncate(limit);
            Ok(crate::types::EventsPage { items, has_more })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &page))
}

/// `run_events` が 1 回になめるイベントの上限（タスクの全イベントは `GET /tasks/{id}/events`）。
const RUN_EVENTS_SCAN: usize = 20_000;

/// そのイベントが属する run（run に紐づかないイベントは `None`）。
fn run_of(event: &Event) -> Option<&str> {
    match event {
        Event::WorkerStarted { run_id, .. }
        | Event::WorkerProgress { run_id, .. }
        | Event::ArtifactProduced { run_id, .. }
        | Event::WorkerFinished { run_id, .. }
        | Event::ReviewVerdict { run_id, .. }
        | Event::ReviewCommandChecked { run_id, .. }
        | Event::ReviewTargetSynced {
            review_run: run_id, ..
        }
        | Event::RequiredChecksEvaluated {
            review_run: run_id, ..
        }
        | Event::ReviewTargetAdvanced {
            review_run: run_id, ..
        }
        | Event::ReviewerPassed {
            review_run: run_id, ..
        }
        | Event::ReviewerPassReused {
            review_run: run_id, ..
        }
        | Event::ReviewResyncHalted {
            review_run: run_id, ..
        }
        | Event::ReviewResyncResumed {
            review_run: run_id, ..
        }
        | Event::QuestionRaised { run_id, .. }
        | Event::Delegated { run_id, .. } => Some(run_id.as_str()),
        _ => None,
    }
}
