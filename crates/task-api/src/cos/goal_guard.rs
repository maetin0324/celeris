//! ADR 2026-11-04-project-controller D5・D8（A3）: 人が与えた目的・受け入れ条件と、controller の起票を
//! CoS が独断で変えないための拒否。`/cos/operations` で `instructed_by`（人の発言の引用）が無い要求だけに当てる。
//!
//! - `project.update` で `request`（人の依頼文。正本）を変える → 422 `project_request_human_only`。
//! - `task.update` で draft でない task の `objective`・`acceptance` を変える → 422 `task_goal_human_only`。
//! - controller が `enabled` の案件に root（`parent` なし）を `task.create` で起こす → 422
//!   `project_controller_owns_launch`（続きの root は controller が起票する）。
//!
//! - controller が `off` でない案件（`dry_run`・`shadow`・`enabled`）の案件 CoS（kind `project` の thread の run）が、
//!   本番の変更・外部公開・不可逆な操作・API の認可・結果を確かめられない外部の効果を行う（A3・A7 の [`authority_gate`]）→
//!   人の発言の引用か、authority ledger の有効な許可が無ければ 422（`production_requires_human` など）。許可の予算は 1 回消費し、
//!   監査（suggestion・observed outcome・actual actor・human decision）を `authority_audit` に残す。
//!   状態行が無い・`off` の案件と統括 CoS は従来の gate のまま（I13）。
//!
//! 断った要求は呼び出し側が `audit.reject` で `rejected` の監査を残す。secret の書き込み（`PUT /secrets/…`）は
//! registry の EXCLUDED が、routing の段の変更は human_only が断る（試験 `project_cos_refuses_production`）。

use serde_json::Value;
use task_core::TaskStore;
use task_core::authority::{
    ActionClass, AuthorityVerdict, GrantScope, NewAuthorityAudit, ScopeKind,
};
use task_core::chat::CosOperation;
use task_core::project_wake::ControllerMode;
use task_core::store::SqliteStore;

use super::operations::{Matched, OperationAudit, unprocessable};
use crate::problem::ApiProblem;

pub(crate) const PROJECT_REQUEST_HUMAN_ONLY: &str = "project_request_human_only";
pub(crate) const TASK_GOAL_HUMAN_ONLY: &str = "task_goal_human_only";
pub(crate) const PROJECT_CONTROLLER_OWNS_LAUNCH: &str = "project_controller_owns_launch";
pub(crate) const PRODUCTION_REQUIRES_HUMAN: &str = "production_requires_human";
pub(crate) const EXTERNAL_PUBLICATION_REQUIRES_HUMAN: &str = "external_publication_requires_human";
pub(crate) const IRREVERSIBLE_REQUIRES_HUMAN: &str = "irreversible_requires_human";

pub(crate) const API_AUTHORIZATION_REQUIRES_HUMAN: &str = "api_authorization_requires_human";
pub(crate) const OPAQUE_EFFECT_REQUIRES_HUMAN: &str = "opaque_effect_requires_human";

/// `/cos/operations` の操作の許可の分類。無い操作はこの門を通らない（既存の gate のまま）。
pub(crate) fn action_class_of(action: &str) -> Option<ActionClass> {
    Some(match action {
        "release.promote"
        | "daemon.reload"
        | "daemon.replay"
        | "script_domain.mode_set"
        | "script_generation.register"
        | "script_generation.retire"
        | "local.settings_put"
        | "cluster.settings_put" => ActionClass::Production,
        "task.pr_merge" => ActionClass::ExternalPublish,
        "repo.delete"
        | "account.delete"
        | "org.delete"
        | "skill.delete"
        | "project.cancel"
        | "docs.page_delete"
        | "cron_job.delete"
        | "standing_rule.delete" => ActionClass::Destructive,
        "account.create"
        | "account.limit_set"
        | "browser.auth_section"
        | "browser.site_policy_put"
        | "browser.site_policy_delete"
        | "browser.task_policy_put" => ActionClass::ApiAuthorization,
        "notify.test" | "browser.request_open" | "browser.control" | "browser.agent_begin" => {
            ActionClass::OpaqueExternalEffect
        }
        _ => return None,
    })
}

/// 分類ごとの拒否の code と文。
fn class_refusal(class: ActionClass) -> (&'static str, &'static str) {
    match class {
        ActionClass::Production => (
            PRODUCTION_REQUIRES_HUMAN,
            "a project CoS does not change production on its own judgment",
        ),
        ActionClass::ExternalPublish => (
            EXTERNAL_PUBLICATION_REQUIRES_HUMAN,
            "this publishes the change outside Celeris",
        ),
        ActionClass::ApiAuthorization => (
            API_AUTHORIZATION_REQUIRES_HUMAN,
            "this changes what Celeris is authorized to do on an external API",
        ),
        ActionClass::OpaqueExternalEffect => (
            OPAQUE_EFFECT_REQUIRES_HUMAN,
            "Celeris cannot observe the outcome of this external effect",
        ),
        ActionClass::Destructive | ActionClass::HumanRequired | ActionClass::LaunchRoot => (
            IRREVERSIBLE_REQUIRES_HUMAN,
            "this operation cannot be undone",
        ),
    }
}

/// controller が `off` でない案件の案件 thread の run なら案件 id（I13: 状態行が無い・`off` の案件と統括 CoS は従来の gate）。
fn autonomous_project(store: &SqliteStore, thread_id: &str) -> Option<String> {
    let thread = store.chat_thread_get(thread_id).ok()??;
    if thread.kind != task_core::chat::ChatThreadKind::Project {
        return None;
    }
    let project = thread.project_id?;
    matches!(store.project_controller_state(&project), Ok(Some(state)) if state.mode != ControllerMode::Off)
        .then_some(project)
}

/// 許可の門を通った操作（実行の後に結果を記録する）。
pub(crate) struct AuthorityPass {
    project: String,
    audit_id: String,
}

/// A7: 分類のある操作を autonomy の中で通してよいか。`instruction` は確かめ済みの人の発言の id。
/// 許可があれば予算を消費し、監査の行を `unknown` で書く（結果は [`settle`] が閉じる。閉じる前に落ちれば `unknown` のままで、
/// 同じ分類の次の判定は `prior_outcome_unknown` で拒否する）。
pub(crate) fn authority_gate(
    store: &SqliteStore,
    audit: &OperationAudit,
    matched: Option<&Matched>,
    path: &str,
    instruction: Option<&str>,
) -> Result<Option<AuthorityPass>, ApiProblem> {
    let Some(m) = matched else {
        return Ok(None);
    };
    let Some(class) = action_class_of(m.action) else {
        return Ok(None);
    };
    let Some(project) = autonomous_project(store, &audit.ctx.thread_id) else {
        return Ok(None);
    };
    // task の操作は task の範囲、それ以外は案件の範囲（他の案件の task は範囲不明で拒否される）。
    let scope = match m.id.as_deref() {
        Some(task) if path.starts_with("/api/v1/tasks/") => GrantScope {
            kind: ScopeKind::Task,
            reference: task.to_owned(),
        },
        _ => GrantScope {
            kind: ScopeKind::Project,
            reference: project.clone(),
        },
    };
    let now = time::OffsetDateTime::now_utc();
    let row = NewAuthorityAudit {
        project_id: project.clone(),
        action_class: class.as_str().to_owned(),
        scope,
        verdict: AuthorityVerdict::denied("pending"),
        suggestion: serde_json::json!({
            "by": audit.by(),
            "action": m.action,
            "path": path,
            "reason": audit.ctx.reason,
        }),
        actual_actor: audit.by(),
        human_decision_ref: None,
        decision_id: None,
        dedupe_key: format!("cos:{}:{}", audit.ctx.thread_id, audit.idempotency_key),
        outcome_state: "unknown",
        observed_outcome: None,
    };
    if let Some(message) = instruction {
        // 人が決めた（既存の `instructed_by` の確かめは済んでいる）。予算は消費しない。
        let mut row = row;
        row.verdict.allow = true;
        row.verdict.reason = "human_instruction".into();
        row.human_decision_ref = Some(format!("human_message:{message}"));
        let audit_id = store
            .authority_audit_record(&row, now)
            .map_err(crate::problem::store_problem)?;
        return Ok(Some(AuthorityPass { project, audit_id }));
    }
    let (verdict, audit_id) = store
        .authority_use(&row, now)
        .map_err(crate::problem::store_problem)?;
    if verdict.allow {
        return Ok(Some(AuthorityPass { project, audit_id }));
    }
    let (code, detail) = class_refusal(class);
    Err(unprocessable(
        code,
        format!(
            "{detail}; authority ledger: {} ({}). Relay the person's message (instructed_by), or \
             propose a grant citing it (authority.grant_propose)",
            verdict.reason,
            class.as_str()
        ),
    ))
}

/// 実行の結果を監査に記録する。`pending` のまま（外部の効果の結果が不明）なら `unknown` を残す（fail closed）。
pub(crate) fn settle(
    store: &SqliteStore,
    audit: &OperationAudit,
    pass: &AuthorityPass,
    result: &Result<CosOperation, ApiProblem>,
) {
    let now = time::OffsetDateTime::now_utc();
    let state = match result {
        Ok(op) => op.state.clone(),
        Err(_) => store
            .cos_operation_find(&audit.ctx.thread_id, &audit.idempotency_key)
            .ok()
            .flatten()
            .map_or_else(|| "rejected".to_owned(), |op| op.state),
    };
    let observed = match result {
        Ok(op) => {
            serde_json::json!({"operation_id": op.id, "state": op.state, "result": op.result})
        }
        Err(problem) => serde_json::json!({"state": state, "error": problem.code()}),
    };
    let settled = match state.as_str() {
        "applied" => Some("applied"),
        "rejected" => Some("failed"),
        _ => None,
    };
    if let Some(settled) = settled {
        // 書けなければ `unknown` のまま（次の判定が拒否する）。
        let _ =
            store.authority_outcome_settle(&pass.project, &pass.audit_id, settled, &observed, now);
    }
}

fn present(body: &Value, field: &str) -> bool {
    body.get(field).is_some_and(|v| !v.is_null())
}

/// 断る理由があれば problem を返す。
pub(crate) fn check(
    store: &SqliteStore,
    matched: Option<&Matched>,
    body: &Value,
) -> Option<ApiProblem> {
    let m = matched?;
    match m.action {
        "project.update" if present(body, "request") => Some(unprocessable(
            PROJECT_REQUEST_HUMAN_ONLY,
            "the project request is the person's own text; CoS may change it only when relaying \
             the person's message (instructed_by)",
        )),
        "task.update" if present(body, "objective") || present(body, "acceptance") => {
            let id: task_core::TaskId = m.id.as_deref()?.parse().ok()?;
            let task = store.get(id).ok()??;
            (task.status != task_core::Status::Draft).then(|| {
                unprocessable(
                    TASK_GOAL_HUMAN_ONLY,
                    "the objective and acceptance of a task that is not a draft are the giver's; \
                     CoS may change them only when relaying the person's message (instructed_by)",
                )
            })
        }
        "task.create" if !present(body, "parent") => {
            let project = body.get("project_id")?.as_str()?;
            let state = store.project_controller_state(project).ok()??;
            (state.mode == ControllerMode::Enabled).then(|| {
                unprocessable(
                    PROJECT_CONTROLLER_OWNS_LAUNCH,
                    "the project controller is enabled for this project and launches its next \
                     root; record the reasoning in the brief with project.status_put or relay the \
                     person's message (instructed_by)",
                )
            })
        }
        _ => None,
    }
}
