//! ADR 2026-11-21-chat-discussion-mode D5.1: a person edits (`PATCH …/messages/{m}`) or retracts
//! (`POST …/messages/{m}/retract`) a message they wrote, and anyone who reads the thread reads the
//! previous bodies (`GET …/messages/{m}/revisions`). Only a `human:` token may change a message,
//! and the store checks that it wrote it. These are not CoS operations (not in the
//! `/cos/operations` allowlist) and do not bump `human_epoch`: a change is not a new instruction.

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_core::chat::{ChatMessageEditRequest, ChatMessageResponse, ChatMessageRetractRequest};
use time::OffsetDateTime;

use super::{authorize, changed, chat_json, chat_problem};
use crate::http_io::{ApiResult, Params, json_response, no_query};
use crate::principal::{Caller, Principal};
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// D5.1: the `human:<name>` that may change messages, from the authentication only. An operator,
/// an MCP client, a CoS run and an unauthenticated caller are refused (403).
pub(crate) fn editor(caller: &Caller) -> Result<String, ApiProblem> {
    match &caller.0 {
        Principal::Human { .. } => Ok(caller.actor()),
        Principal::Operator { .. }
        | Principal::Mcp { .. }
        | Principal::Cos { .. }
        | Principal::Unauthenticated => Err(chat_problem(
            task_core::chat::ChatError::EditForbidden(format!(
                "{} cannot edit or retract chat messages; only the person who wrote one can",
                caller.actor()
            )),
        )),
    }
}

pub(super) async fn edit_message(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params((t, m)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let by = editor(&caller)?;
    authorize(&state, &headers)?;
    let req: ChatMessageEditRequest = chat_json(body).await?;
    let message = state
        .blocking(move |store| {
            store
                .chat_message_edit(&t, &m, &by, &req, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        StatusCode::OK,
        &ChatMessageResponse { message },
    ))
}

pub(super) async fn retract_message(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params((t, m)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let by = editor(&caller)?;
    authorize(&state, &headers)?;
    let req: ChatMessageRetractRequest = chat_json(body).await?;
    let message = state
        .blocking(move |store| {
            store
                .chat_message_retract(&t, &m, &by, &req, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        StatusCode::OK,
        &ChatMessageResponse { message },
    ))
}

pub(super) async fn list_revisions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((t, m)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let page = state
        .blocking(move |store| store.chat_message_revisions(&t, &m).map_err(chat_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &page))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D5.1: whatever the store would say, only a `Principal::Human` passes the API check; the
    /// other principals get 403 `chat_message_edit_forbidden` (the middleware refuses MCP and CoS
    /// writes even earlier, so this guards the handler if that ever changes).
    #[test]
    fn chat_message_edit_forbidden_for_non_human_principals_at_handler() {
        let human = Caller(Principal::Human {
            name: "alice".into(),
        });
        assert_eq!(editor(&human).ok().as_deref(), Some("human:alice"));
        for p in [
            Principal::Operator {
                name: "claude-code".into(),
            },
            Principal::Mcp {
                client_id: "c1".into(),
            },
            Principal::Cos {
                thread_id: "t".into(),
                run_id: "r".into(),
            },
            Principal::Unauthenticated,
        ] {
            let problem = editor(&Caller(p.clone())).expect_err("refused");
            assert_eq!(problem.status(), StatusCode::FORBIDDEN, "{p:?}");
            assert_eq!(problem.code(), "chat_message_edit_forbidden", "{p:?}");
        }
    }
}
