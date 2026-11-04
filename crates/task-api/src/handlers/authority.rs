//! ADR 2026-11-04-project-controller A7: 人の許可範囲（authority ledger）の API。規則は `task_core::authority`。
//!
//! - `GET …/authority`: 許可（状態と根拠の抜粋つき）・提案・監査。
//! - `POST …/authority/grants`: 人の token だけ。元の根拠は人の発言・人の決定・人の承認・永続の認可だけで、それ以外は 422。
//! - `POST …/authority/grants/{g}/revoke`: 誰でも（CoS は `authority.grant_revoke`）。
//! - `POST …/authority/check`: 読み取り `{action_class, scope, question_key?}` → `{allow, grant_id, reason, …}`。分類・範囲の
//!   不明も 200 の拒否で返す（CoS は `authority.check`）。
//! - `POST …/authority/proposals`: 人の発言を引いた提案（CoS は `authority.grant_propose`）。同じ問いは 2 回通知しない。
//! - `POST …/authority/proposals/{p}/dismiss`・`POST …/authority/audit/{a}/outcome`: 人の token だけ。

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::authority::{
    ActionClass, AuthorityAuditEntry, AuthorityGrant, AuthorityProposal, AuthorityVerdict,
    GrantScope, GrantSource, NewAuthorityGrant, ScopeKind, SourceKind, grant_denial, reason,
};
use task_core::chat::CosOperation;
use task_core::store::{AuthorityRefusal, SqliteStore};
use time::OffsetDateTime;
use ulid::Ulid;

use super::*;
use crate::cos::operations::OperationAudit;
use crate::middleware::require_admin;
use crate::principal::{Caller, Principal};

const AUTHORITY_HUMAN_ONLY: &str = "authority_human_only";
const AUTHORITY_GRANT_INVALID: &str = "authority_grant_invalid";
const AUTHORITY_ALREADY_GRANTED: &str = "authority_already_granted";
const AUTHORITY_QUESTION_ANSWERED: &str = "authority_question_answered";
const AUTHORITY_PROPOSAL_CLOSED: &str = "authority_proposal_closed";
const TEXT_MAX: usize = 2000;
const KEY_MAX: usize = 200;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/projects/{id}/authority", get(get_authority))
        .route("/api/v1/projects/{id}/authority/grants", post(post_grant))
        .route(
            "/api/v1/projects/{id}/authority/grants/{g}/revoke",
            post(post_revoke),
        )
        .route("/api/v1/projects/{id}/authority/check", post(post_check))
        .route(
            "/api/v1/projects/{id}/authority/proposals",
            post(post_proposal),
        )
        .route(
            "/api/v1/projects/{id}/authority/proposals/{p}/dismiss",
            post(post_dismiss),
        )
        .route(
            "/api/v1/projects/{id}/authority/audit/{a}/outcome",
            post(post_outcome),
        )
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn unprocessable(code: &'static str, detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, code, detail)
}

fn refused(r: AuthorityRefusal) -> ApiProblem {
    unprocessable(r.code, r.detail)
}

fn require_project(store: &SqliteStore, id: task_core::ProjectId) -> Result<(), ApiProblem> {
    match store.project_get(id).map_err(store_problem)? {
        Some(_) => Ok(()),
        None => Err(ApiProblem::project_not_found(&id.to_string())),
    }
}

/// 許可を作るのは人の token だけ（D8）。operator・MCP・CoS の credential は 403。
fn require_human(caller: &Caller) -> Result<(), ApiProblem> {
    if matches!(caller.0, Principal::Human { .. }) {
        return Ok(());
    }
    Err(ApiProblem::new(
        StatusCode::FORBIDDEN,
        AUTHORITY_HUMAN_ONLY,
        "only a person's token may grant, dismiss or settle authority",
    ))
}

/// 許可 1 件の表示（状態と根拠の抜粋）。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AuthorityGrantView {
    #[serde(flatten)]
    pub grant: AuthorityGrant,
    /// `live` か拒否の理由（`grant_revoked`・`grant_expired`・`grant_budget_exhausted` など）。
    pub state: String,
    /// 元の根拠の抜粋（人の発言の先頭 200 字など）。読めなければ無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
}

/// `GET …/authority` の応答。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AuthorityView {
    pub grants: Vec<AuthorityGrantView>,
    pub proposals: Vec<AuthorityProposal>,
    pub audit: Vec<AuthorityAuditEntry>,
}

fn grant_view(
    store: &SqliteStore,
    grant: AuthorityGrant,
    at: OffsetDateTime,
) -> Result<AuthorityGrantView, ApiProblem> {
    let state = grant_denial(&grant, at).unwrap_or("live").to_owned();
    let source_excerpt = store
        .authority_source_excerpt(&grant.source)
        .map_err(store_problem)?;
    Ok(AuthorityGrantView {
        grant,
        state,
        source_excerpt,
    })
}

async fn get_authority(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let project_id = parse_project_id(&id)?;
    let view = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let project = project_id.to_string();
            let at = now();
            let grants = store
                .authority_grants(&project)
                .map_err(store_problem)?
                .into_iter()
                .map(|g| grant_view(store, g, at))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AuthorityView {
                grants,
                proposals: store.authority_proposals(&project).map_err(store_problem)?,
                audit: store
                    .authority_audit(&project, 200)
                    .map_err(store_problem)?,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// `POST …/authority/grants` の本文。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantCreate {
    pub scope: GrantScope,
    pub action_class: String,
    pub source: GrantSource,
    /// 期限（RFC3339）。必須（無期限の許可は作らない）。
    pub expires_at: String,
    /// 使える回数。必須。
    pub budget: i64,
    /// 人が読む説明。
    pub note: String,
    /// この許可が答える問いの key。同じ問いを人に聞き直さないために引く。
    #[serde(default)]
    pub question_key: Option<String>,
    /// CoS の提案を受けて作るときの提案の id（提案は `accepted` になる）。
    #[serde(default)]
    pub proposal_id: Option<String>,
}

fn validate_text(note: &str, question_key: Option<&str>) -> Result<(), ApiProblem> {
    if note.trim().is_empty() || note.chars().count() > TEXT_MAX {
        return Err(unprocessable(
            AUTHORITY_GRANT_INVALID,
            format!("note must be 1 to {TEXT_MAX} characters and not blank"),
        ));
    }
    if question_key.is_some_and(|k| k.trim().is_empty() || k.chars().count() > KEY_MAX) {
        return Err(unprocessable(
            AUTHORITY_GRANT_INVALID,
            format!("question_key must be 1 to {KEY_MAX} characters"),
        ));
    }
    Ok(())
}

async fn post_grant(
    State(state): State<ApiState>,
    caller: Caller,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_human(&caller)?;
    let project_id = parse_project_id(&id)?;
    let value: Value = read_json(body, false).await?;
    let body: GrantCreate = serde_json::from_value(value)
        .map_err(|e| unprocessable(AUTHORITY_GRANT_INVALID, e.to_string()))?;
    validate_text(&body.note, body.question_key.as_deref())?;
    let Some(class) = ActionClass::parse(&body.action_class) else {
        return Err(unprocessable(
            "authority_action_class_unknown",
            format!("unknown action_class {}", body.action_class),
        ));
    };
    if body.source.kind == SourceKind::HumanOperation {
        return Err(unprocessable(
            task_core::store::SOURCE_KIND_RESERVED,
            "human_operation is written only by the controller mode change itself",
        ));
    }
    let by = caller.actor();
    let view = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let project = project_id.to_string();
            let at = now();
            let grant = store.write_tx::<_, ApiProblem, _>(|tx| {
                let mut question_key = body.question_key.clone();
                if let Some(proposal) = &body.proposal_id {
                    let found = SqliteStore::authority_proposal_by_id_tx(tx, &project, proposal)?
                        .filter(|p| p.state == "open")
                        .ok_or_else(|| {
                            ApiProblem::new(
                                StatusCode::CONFLICT,
                                AUTHORITY_PROPOSAL_CLOSED,
                                format!("proposal {proposal} is not open"),
                            )
                        })?;
                    question_key.get_or_insert(found.question_key);
                }
                let new = NewAuthorityGrant {
                    id: Ulid::new().to_string(),
                    project_id: project.clone(),
                    scope: body.scope.clone(),
                    action_class: class,
                    source: body.source.clone(),
                    granted_by: by.clone(),
                    question_key,
                    note: body.note.clone(),
                    expires_at: body.expires_at.clone(),
                    budget: body.budget,
                };
                let grant =
                    SqliteStore::authority_grant_insert_tx(tx, &new, at)?.map_err(refused)?;
                if let Some(proposal) = &body.proposal_id {
                    SqliteStore::authority_proposal_resolve_tx(
                        tx,
                        &project,
                        proposal,
                        "accepted",
                        Some(&grant.id),
                        &by,
                        at,
                    )?;
                }
                SqliteStore::authority_unblock_tx(tx, &project, at, 0)?;
                Ok(grant)
            })?;
            grant_view(store, grant, at)
        })
        .await?;
    Ok(json_response(StatusCode::CREATED, &view))
}

/// `POST …/grants/{g}/revoke` の本文。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRevoke {
    #[serde(default)]
    pub reason: String,
}

fn grant_missing(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "authority_grant_not_found",
        format!("no grant {id} in this project"),
    )
}

async fn post_revoke(
    State(state): State<ApiState>,
    caller: Caller,
    Params((id, grant)): Params<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let _: GrantRevoke = read_json(body, true).await?;
    let by = caller.actor();
    let view = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let at = now();
            let revoked = store.write_tx::<_, ApiProblem, _>(|tx| {
                SqliteStore::authority_grant_revoke_tx(
                    tx,
                    &project_id.to_string(),
                    &grant,
                    &by,
                    at,
                )?
                .ok_or_else(|| grant_missing(&grant))
            })?;
            grant_view(store, revoked, at)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// `POST …/authority/check` の本文。`action_class`・`scope.kind` は文字列で受け、不明は拒否の応答にする。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorityCheckBody {
    pub action_class: String,
    pub scope: CheckScope,
    /// 聞こうとしている問いの key。既に答えがあれば `answered` に根拠を返す。
    #[serde(default)]
    pub question_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckScope {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// `POST …/authority/check` の応答。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AuthorityCheckResult {
    #[serde(flatten)]
    pub verdict: AuthorityVerdict,
    /// 許可の根拠の抜粋（人に聞き直さず示す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_excerpt: Option<String>,
    /// `question_key` に既に答えた許可、または人が断った提案。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<Value>,
}

/// 判定の本体（読み取り）。読めない・不明は拒否（fail closed）。
pub(crate) fn check_op(
    store: &SqliteStore,
    project: &str,
    body: &AuthorityCheckBody,
    at: OffsetDateTime,
) -> AuthorityCheckResult {
    let verdict = match ScopeKind::parse(&body.scope.kind) {
        Some(kind) => store.authority_check(
            project,
            &body.action_class,
            &GrantScope {
                kind,
                reference: body.scope.reference.clone(),
            },
            at,
        ),
        None => AuthorityVerdict::denied(reason::UNKNOWN_SCOPE),
    };
    let source_excerpt = verdict
        .source
        .as_ref()
        .and_then(|s| store.authority_source_excerpt(s).ok().flatten());
    AuthorityCheckResult {
        verdict,
        source_excerpt,
        answered: body
            .question_key
            .as_deref()
            .and_then(|key| answered_question(store, project, key, at)),
    }
}

/// 問いに既に人の答えがあれば、その根拠。読めなければ「答え無し」（聞くのは安全側）。
fn answered_question(
    store: &SqliteStore,
    project: &str,
    key: &str,
    at: OffsetDateTime,
) -> Option<Value> {
    if let Ok(Some(grant)) = store.authority_answered(project, key, at) {
        let excerpt = store.authority_source_excerpt(&grant.source).ok().flatten();
        return Some(serde_json::json!({
            "answer": "granted", "grant_id": grant.id, "source": grant.source,
            "source_excerpt": excerpt, "expires_at": grant.expires_at,
        }));
    }
    if let Ok(Some(proposal)) = store.authority_question_dismissed(project, key) {
        return Some(serde_json::json!({
            "answer": "dismissed", "proposal_id": proposal.id, "resolved_by": proposal.resolved_by,
            "resolved_at": proposal.resolved_at, "source": proposal.source,
        }));
    }
    None
}

async fn post_check(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let body: AuthorityCheckBody = read_json(body, false).await?;
    let out = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            Ok(check_op(store, &project_id.to_string(), &body, now()))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &out))
}

/// `POST …/authority/proposals` の本文。根拠は人の発言だけ。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalCreate {
    pub question_key: String,
    pub scope: GrantScope,
    pub action_class: String,
    pub source: GrantSource,
    pub expires_at: String,
    pub budget: i64,
    pub note: String,
}

/// 提案の応答。`already_asked` は同じ問いが開いていて、通知を増やさなかった印。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProposalResult {
    pub proposal: AuthorityProposal,
    pub already_asked: bool,
}

fn propose_tx(
    tx: &rusqlite::Transaction<'_>,
    project: &str,
    body: &ProposalCreate,
    by: &str,
    at: OffsetDateTime,
) -> Result<ProposalResult, ApiProblem> {
    validate_text(&body.note, Some(&body.question_key))?;
    let class = ActionClass::parse(&body.action_class).ok_or_else(|| {
        unprocessable(
            "authority_action_class_unknown",
            format!("unknown action_class {}", body.action_class),
        )
    })?;
    if let Some(found) =
        SqliteStore::authority_proposal_by_question_tx(tx, project, &body.question_key)?
    {
        return match found.state.as_str() {
            "open" => Ok(ProposalResult {
                proposal: found,
                already_asked: true,
            }),
            state => Err(ApiProblem::new(
                StatusCode::CONFLICT,
                AUTHORITY_QUESTION_ANSWERED,
                format!(
                    "the person already answered {} ({state} by {}, grant {}); show that evidence \
                     instead of asking again",
                    body.question_key,
                    found.resolved_by.as_deref().unwrap_or("unknown"),
                    found.resolved_grant_id.as_deref().unwrap_or("none"),
                ),
            )),
        };
    }
    let verdict = SqliteStore::authority_check_tx(tx, project, class, &body.scope, at)?;
    if verdict.allow {
        let source = verdict.source.as_ref();
        return Err(ApiProblem::new(
            StatusCode::CONFLICT,
            AUTHORITY_ALREADY_GRANTED,
            format!(
                "grant {} already covers this (source {} {}); act on it instead of asking again",
                verdict.grant_id.as_deref().unwrap_or(""),
                source.map_or("", |s| s.kind.as_str()),
                source.map_or("", |s| s.reference.as_str()),
            ),
        ));
    }
    let new = AuthorityProposal {
        id: Ulid::new().to_string(),
        project_id: project.to_owned(),
        question_key: body.question_key.clone(),
        scope: body.scope.clone(),
        action_class: class,
        source: body.source.clone(),
        expires_at: body.expires_at.clone(),
        budget: body.budget,
        note: body.note.clone(),
        proposed_by: by.to_owned(),
        state: "open".into(),
        resolved_grant_id: None,
        resolved_by: None,
        resolved_at: None,
        created_at: String::new(),
    };
    let proposal = SqliteStore::authority_proposal_insert_tx(tx, &new, at)?.map_err(refused)?;
    Ok(ProposalResult {
        proposal,
        already_asked: false,
    })
}

async fn post_proposal(
    State(state): State<ApiState>,
    caller: Caller,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let value: Value = read_json(body, false).await?;
    let body: ProposalCreate = serde_json::from_value(value)
        .map_err(|e| unprocessable(AUTHORITY_GRANT_INVALID, e.to_string()))?;
    let by = caller.actor();
    let out = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            store.write_tx::<_, ApiProblem, _>(|tx| {
                propose_tx(tx, &project_id.to_string(), &body, &by, now())
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &out))
}

async fn post_dismiss(
    State(state): State<ApiState>,
    caller: Caller,
    Params((id, proposal)): Params<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_human(&caller)?;
    let project_id = parse_project_id(&id)?;
    let _: GrantRevoke = read_json(body, true).await?;
    let by = caller.actor();
    let out = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let project = project_id.to_string();
            store.write_tx::<_, ApiProblem, _>(|tx| {
                if !SqliteStore::authority_proposal_resolve_tx(
                    tx,
                    &project,
                    &proposal,
                    "dismissed",
                    None,
                    &by,
                    now(),
                )? {
                    return Err(ApiProblem::new(
                        StatusCode::CONFLICT,
                        AUTHORITY_PROPOSAL_CLOSED,
                        format!("proposal {proposal} is not open"),
                    ));
                }
                SqliteStore::authority_proposal_by_id_tx(tx, &project, &proposal)?
                    .ok_or_else(|| ApiProblem::internal("proposal vanished"))
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &out))
}

/// `POST …/audit/{a}/outcome` の本文: 人が確かめた結果。`failed` は効果が無かったので予算を戻す。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeSettle {
    /// `applied` か `failed`。
    pub state: String,
    pub note: String,
}

async fn post_outcome(
    State(state): State<ApiState>,
    caller: Caller,
    Params((id, audit)): Params<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    require_human(&caller)?;
    let project_id = parse_project_id(&id)?;
    let body: OutcomeSettle = read_json(body, false).await?;
    if !matches!(body.state.as_str(), "applied" | "failed") {
        return Err(unprocessable(
            AUTHORITY_GRANT_INVALID,
            "state must be applied or failed",
        ));
    }
    let by = caller.actor();
    let out = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let project = project_id.to_string();
            let observed = serde_json::json!({"observed_by": by, "note": body.note});
            if !store
                .authority_outcome_settle(&project, &audit, &body.state, &observed, now())
                .map_err(store_problem)?
            {
                return Err(ApiProblem::new(
                    StatusCode::CONFLICT,
                    "authority_outcome_settled",
                    format!("audit {audit} is not waiting for an outcome"),
                ));
            }
            Ok(serde_json::json!({"audit_id": audit, "state": body.state}))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &out))
}

// ---- CoS（`/cos/operations`）----

/// `authority.grant_revoke`: 誰でも取り消せる。監査と同じ transaction。
pub(crate) fn cos_revoke(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    grant: &str,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    require_project(store, project_id).map_err(|p| audit.reject(store, "project", &target, p))?;
    audit.apply_checked(store, "project", &target, "authority.grant_revoke", |tx| {
        let revoked =
            SqliteStore::authority_grant_revoke_tx(tx, &target, grant, &audit.by(), now())?
                .ok_or_else(|| grant_missing(grant))?;
        serde_json::to_value(revoked).map_err(|e| ApiProblem::internal(e.to_string()))
    })
}

/// `authority.check`: 読み取り。判定を監査つきの操作の結果として返す。
pub(crate) fn cos_check(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    body: AuthorityCheckBody,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    require_project(store, project_id).map_err(|p| audit.reject(store, "project", &target, p))?;
    let result = check_op(store, &target, &body, now());
    let value = serde_json::to_value(result).map_err(|e| ApiProblem::internal(e.to_string()))?;
    audit.apply_checked(store, "project", &target, "authority.check", |_| Ok(value))
}

/// `authority.grant_propose`: 人の発言を引いた提案。許可そのものは人だけが作る（D8）。
pub(crate) fn cos_propose(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    body: ProposalCreate,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    require_project(store, project_id).map_err(|p| audit.reject(store, "project", &target, p))?;
    audit.apply_checked(store, "project", &target, "authority.grant_propose", |tx| {
        let out = propose_tx(tx, &target, &body, &audit.by(), now())?;
        serde_json::to_value(out).map_err(|e| ApiProblem::internal(e.to_string()))
    })
}
