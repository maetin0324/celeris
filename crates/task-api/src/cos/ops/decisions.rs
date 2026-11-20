//! decisions registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{audited, decode_optional, decode_problem};
use super::super::operations::{dispatch as dispatch_operation, unprocessable};
use crate::problem::ApiProblem;
use axum::http::StatusCode;
use serde_json::Value;
use task_core::chat::{ChatError, CosOperation};
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    ("POST", "/api/v1/decisions/{id}/answer", "decision.answer"),
    ("POST", "/api/v1/approvals/{id}/decide", "approval.decide"),
    ("POST", "/api/v1/knowledge/inbox", "knowledge.record"),
    ("PUT", "/api/v1/knowledge/page", "knowledge.page_put"),
    (
        "POST",
        "/api/v1/knowledge/inbox/{id}/reject",
        "knowledge.reject",
    ),
    ("POST", "/api/v1/inbox/items/{id}/answer", "inbox.answer"),
    ("POST", "/api/v1/decisions/{id}/revise", "decision.revise"),
    (
        "POST",
        "/api/v1/decisions/{id}/withdraw",
        "decision.withdraw",
    ),
    (
        "POST",
        "/api/v1/knowledge/inbox/{id}/accept",
        "knowledge.accept",
    ),
    (
        "POST",
        "/api/v1/notifications/read-all",
        "notification.read_all",
    ),
    (
        "POST",
        "/api/v1/notifications/{id}/read",
        "notification.read",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[];

pub(crate) fn dispatch(
    store: &SqliteStore,
    env: &DispatchEnv,
    audit: &OperationAudit,
    matched: Matched,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let decode = |error| decode_problem(store, audit, path, error);
    match matched.action {
        "decision.answer" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::decisions::answer_op(
                store,
                &decision_id,
                input,
                &audit.by(),
                crate::human_epoch::HumanEpoch::NotCounted,
                Some(audit),
            )?)
        }
        "decision.revise" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::decisions::revise_op(
                store,
                &decision_id,
                input,
                &audit.by(),
                crate::human_epoch::HumanEpoch::NotCounted,
                Some(audit),
            )?)
        }
        "decision.withdraw" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::decisions::withdraw_op(
                store,
                &decision_id,
                input,
                &audit.by(),
                crate::human_epoch::HumanEpoch::NotCounted,
                Some(audit),
            )?)
        }
        "knowledge.page_put" => {
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge_page", path, problem))?;
            let input: crate::knowledge::KnowledgePagePutBody =
                serde_json::from_value(body).map_err(decode)?;
            // plan の拒否は正規化の前の path で記録し、効果（git）は外部の効果として 1 回だけ走らせる。
            let edit = crate::knowledge::plan_page_put(&root, &input)
                .map_err(|problem| audit.reject(store, "knowledge_page", &input.path, problem))?;
            audit.external(
                store,
                "knowledge_page",
                &edit.path,
                "knowledge.page_put",
                time::OffsetDateTime::now_utc(),
                || match crate::knowledge::page_put_effect(&root, &edit)? {
                    Ok(result) => {
                        tracing::info!(
                            op = "knowledge_put",
                            path = %result.path,
                            unchanged = result.unchanged,
                            cos = true,
                            "knowledge page committed"
                        );
                        serde_json::to_value(&result)
                            .map(Ok)
                            .map_err(|e| ApiProblem::internal(format!("knowledge result: {e}")))
                    }
                    Err(refused) => Ok(Err(refused)),
                },
            )
        }
        "knowledge.accept" => {
            let raw_id = matched.id.unwrap_or_default();
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            let input: crate::knowledge::KnowledgeAcceptBody =
                decode_optional(body).map_err(decode)?;
            crate::knowledge::plan_accept(&root, &raw_id, &input)
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            // 取り込み（git）は `cos_operation_apply` の transaction の中。失敗すれば監査の行は rejected で、
            // 応答は効果の problem（status と code）のまま。
            let mut failure = None;
            let outcome = audit.apply(store, "knowledge", &raw_id, "knowledge.accept", |_tx| {
                match crate::knowledge::accept_effect(&root, &raw_id, &input) {
                    Ok(page) => {
                        Ok(serde_json::json!({"id": raw_id, "path": page.path, "sha": page.sha}))
                    }
                    Err(problem) => {
                        let detail = problem.detail().to_string();
                        failure = Some(problem);
                        Err(ChatError::Conflict(detail))
                    }
                }
            });
            outcome.map_err(|problem| failure.unwrap_or(problem))
        }
        "notification.read" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = crate::inbox_notifications::parse_notice(&raw_id)
                .map_err(|problem| audit.reject(store, "notification", &raw_id, problem))?;
            let _: crate::inbox_notifications::EmptyBody =
                serde_json::from_value(body).map_err(decode)?;
            audited(crate::inbox_notifications::notice_read_op(
                store,
                id,
                Some(audit),
            )?)
        }
        "notification.read_all" => {
            let input: crate::inbox_notifications::ReadAllBody =
                serde_json::from_value(body).map_err(decode)?;
            let before = input
                .before
                .as_deref()
                .map(crate::inbox_notifications::parse_time)
                .transpose()
                .map_err(|problem| audit.reject(store, "notification", "all", problem))?;
            audited(crate::inbox_notifications::notice_read_all_op(
                store,
                input,
                before,
                Some(audit),
            )?)
        }
        "approval.decide" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = raw_id.parse().map_err(|_| {
                audit.reject(
                    store,
                    "approval",
                    &raw_id,
                    ApiProblem::new(
                        StatusCode::NOT_FOUND,
                        "approval_not_found",
                        format!("no approval {raw_id}"),
                    ),
                )
            })?;
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::approvals::decide_op(
                store,
                id,
                input,
                &audit.by(),
                crate::human_epoch::HumanEpoch::NotCounted,
                Some(audit),
            )?)
        }
        "knowledge.reject" => {
            let raw_id = matched.id.unwrap_or_default();
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            if !body.is_null() && body != serde_json::json!({}) {
                return Err(audit.reject(
                    store,
                    "knowledge",
                    &raw_id,
                    unprocessable("validation", "request.body must be empty"),
                ));
            }
            crate::knowledge::plan_reject(&root, &raw_id)
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            // 取り下げ（git）は `cos_operation_apply` の transaction の中。候補が無ければ `NotFound`、
            // ほかの失敗は `Conflict` で rejected の行を書き、応答は効果の problem のまま。
            let mut failure = None;
            let outcome = audit.apply(store, "knowledge", &raw_id, "knowledge.reject", |_tx| {
                match crate::knowledge::reject_effect(&root, &raw_id) {
                    Ok(rejected) => Ok(serde_json::json!({"id": rejected.id, "sha": rejected.sha})),
                    Err(problem) => {
                        let error = if problem.code() == crate::knowledge::CANDIDATE_NOT_FOUND {
                            ChatError::NotFound {
                                kind: "knowledge candidate",
                                id: raw_id.clone(),
                            }
                        } else {
                            ChatError::Conflict(problem.detail().to_string())
                        };
                        failure = Some(problem);
                        Err(error)
                    }
                }
            });
            outcome.map_err(|problem| failure.unwrap_or(problem))
        }
        "knowledge.record" => {
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", "new", problem))?;
            let input = serde_json::from_value(body).map_err(decode)?;
            // 候補を git に書き、pin と監査を同じ transaction で書く。失敗したら候補を消す
            // （ADR 2026-11-18 knowledge-lifecycle-tx-boundary D3）。
            let recorded = crate::knowledge::record_candidate(store, &root, true, input)
                .map_err(|problem| audit.reject(store, "knowledge", "new", problem))
                .and_then(|result| {
                    let thread = audit.ctx.thread_id.as_str();
                    audit
                        .apply_checked(store, "knowledge", &result.id, "knowledge.record", |tx| {
                            crate::knowledge::pin_candidate_tx(
                                tx,
                                &result.attachment_ids,
                                &result.id,
                                Some(thread),
                            )?;
                            serde_json::to_value(&result)
                                .map_err(|e| ApiProblem::internal(e.to_string()))
                        })
                        .map_err(|problem| {
                            crate::knowledge::discard_candidate(&root, &result.id, problem)
                        })
                });
            super::super::knowledge_use::counted_record(store, audit, recorded)
        }
        "inbox.answer" => {
            let raw_id = matched.id.unwrap_or_default();
            let input: crate::inbox_notifications::InboxAnswerBody =
                serde_json::from_value(body).map_err(decode)?;
            let found = env
                .inbox_feed
                .as_ref()
                .and_then(|items| items.iter().find(|item| item.id == raw_id))
                .ok_or_else(|| {
                    audit.reject(
                        store,
                        "inbox_item",
                        &raw_id,
                        ApiProblem::new(
                            StatusCode::CONFLICT,
                            "cos_revision_conflict",
                            format!("inbox item {raw_id} is not open (answered or changed)"),
                        ),
                    )
                })?;
            let (dm, dp, domain_body) =
                crate::inbox_notifications::delegated_request(found, &input)
                    .map_err(|problem| audit.reject(store, "inbox_item", &raw_id, problem))?;
            dispatch_operation(store, env, audit, dm.as_str(), &dp, domain_body)
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in decisions"
        ))),
    }
}
