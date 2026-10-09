//! surface registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::decode_problem;
use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[(
    "POST",
    "/api/v1/chat/attachments/{id}/references",
    "attachment.reference",
)];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/v1/browser/identities",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "DELETE",
        "/api/v1/browser/identities/{id}",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/identities/{id}/restore",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/identities/{id}/revoke",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/trusted-devices",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/trusted-devices/verify",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "DELETE",
        "/api/v1/browser/trusted-devices/{id}",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/console/instruct",
        "console_instruction_chain: 人の決定 daemon-wide=no-console。別の指示経路へ入り監査の鎖が二重になる。",
    ),
    (
        "POST",
        "/api/v1/cos/inbox/{i}/resolve",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/operations",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/operations/{o}/override",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/threads/{t}/checkpoint",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/check",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/grant",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/read",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/credential",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/decision",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/registered",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/revoke",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
];

/// Assigned mutations awaiting audited implementation. Move a row to ALLOWED when implemented.
pub(crate) const PENDING: &[(&str, &str)] = &[
    ("DELETE", "/api/v1/browser/site-policies/{policy_id}"),
    ("PUT", "/api/v1/browser/site-policies/{policy_id}"),
    ("DELETE", "/api/v1/chat/attachments/{a}"),
    ("POST", "/api/v1/chat/threads"),
    ("PATCH", "/api/v1/chat/threads/{t}"),
    ("POST", "/api/v1/chat/threads/{t}/attachments"),
    ("POST", "/api/v1/chat/threads/{t}/messages"),
    ("DELETE", "/api/v1/chat/threads/{t}/messages/{m}"),
    ("POST", "/api/v1/chat/threads/{t}/resume-queue"),
    ("POST", "/api/v1/chat/threads/{t}/stop"),
    ("POST", "/api/v1/console/new-conversation"),
    ("POST", "/api/v1/org/{id}/messages"),
    ("POST", "/api/v1/tasks/{id}/artifacts/promote"),
    ("POST", "/api/v1/tasks/{id}/browser/control/{run}/{session}"),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/agent/begin",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/agent/end",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/auth-section",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/disconnect",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/events",
    ),
    ("PUT", "/api/v1/tasks/{id}/browser/policy"),
    ("POST", "/api/v1/tasks/{id}/browser/requests"),
];

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
        "attachment.reference" => {
            let raw_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            crate::chat::attachments::add_reference_op(
                store,
                env.kb_root.clone(),
                raw_id,
                input,
                audit,
            )
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in surface"
        ))),
    }
}
