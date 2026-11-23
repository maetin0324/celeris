//! ADR 2026-11-23-project-observability-publish D2.1: `POST /chat/threads/{t}/messages/{m}/review`. A person
//! adopts, rejects or comments on a chat proposal (an external LLM's propose, a person's import, a CoS strategy
//! draft). Only a `Principal::Human` passes; an operator (relay included), an MCP client, a CoS run and an
//! unauthenticated caller are 403 `proposal_verdict_requires_human`. Not a CoS operation (the registry's
//! `EXCLUDED` `human_proposal_verdict`) and not an MCP tool. `adopt` is the person's instruction to take the
//! proposal down 子 2's path (a task's artifact and the decision `strategy-approve-v<n>`); it is not an approval.

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_core::proposal_review::{ProposalReviewError, ProposalReviewRequest};
use time::OffsetDateTime;

use super::{authorize, changed, chat_json, chat_problem, require_cos_enabled};
use crate::http_io::{ApiResult, Params, json_response, no_query};
use crate::principal::{Caller, Principal};
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// D2.1: the problem code of a review by anyone but a person.
pub(crate) const PROPOSAL_VERDICT_REQUIRES_HUMAN: &str = "proposal_verdict_requires_human";

/// D2.1: the `human:<name>` who reviews, from the authentication only (the roadmap decide's `requires_human`).
pub(crate) fn reviewer(caller: &Caller) -> Result<String, ApiProblem> {
    match &caller.0 {
        Principal::Human { .. } => Ok(caller.actor()),
        Principal::Operator { .. }
        | Principal::Mcp { .. }
        | Principal::Cos { .. }
        | Principal::Unauthenticated => Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            PROPOSAL_VERDICT_REQUIRES_HUMAN,
            format!(
                "{} cannot adopt, reject or comment on a proposal; only a person's token can",
                caller.actor()
            ),
        )),
    }
}

fn review_problem(error: ProposalReviewError) -> ApiProblem {
    match error {
        ProposalReviewError::Chat(e) => chat_problem(e),
        other => {
            let status = if matches!(other, ProposalReviewError::Stale(_)) {
                StatusCode::CONFLICT
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            ApiProblem::new(status, other.code(), other.to_string())
        }
    }
}

pub(super) async fn post_review(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params((t, m)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let by = reviewer(&caller)?;
    authorize(&state, &headers)?;
    require_cos_enabled(&state)?;
    let req: ProposalReviewRequest = chat_json(body).await?;
    let human = crate::human_epoch::HumanEpoch::of(&caller);
    let posted = state
        .blocking(move |store| {
            store
                .chat_proposal_review(
                    &t,
                    &m,
                    &by,
                    &req,
                    OffsetDateTime::now_utc(),
                    |tx, posted| human.bump_posted_tx(tx, &t, posted),
                )
                .map_err(review_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(StatusCode::ACCEPTED, &posted.response))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant (D2.1): whatever the earlier layers do, only a `Principal::Human` passes the handler's check; the
    /// other four principals get 403 `proposal_verdict_requires_human` (the guard refuses MCP and CoS writes even
    /// earlier, so this guards the handler if that ever changes). Before this WU there was no handler.
    #[test]
    fn proposal_review_non_human_principals_are_403_at_handler() {
        let human = Caller(Principal::Human {
            name: "alice".into(),
        });
        assert_eq!(reviewer(&human).ok().as_deref(), Some("human:alice"));
        for p in [
            Principal::Operator {
                name: "claude-code".into(),
            },
            Principal::Mcp {
                client_id: "chatgpt".into(),
            },
            Principal::Cos {
                thread_id: "t".into(),
                run_id: "r".into(),
            },
            Principal::Unauthenticated,
        ] {
            let problem = reviewer(&Caller(p.clone())).expect_err("refused");
            assert_eq!(problem.status(), StatusCode::FORBIDDEN, "{p:?}");
            assert_eq!(problem.code(), PROPOSAL_VERDICT_REQUIRES_HUMAN, "{p:?}");
        }
    }
}
