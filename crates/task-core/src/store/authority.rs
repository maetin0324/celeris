//! ADR 2026-11-04-project-controller A7: authority ledger の読み書き。規則は [`crate::authority`]。
//!
//! - 許可の作成は元の根拠を確かめる（[`source_problem`]）。人の発言でも `role=user` で `human:` の author を持つものだけ。
//!   CoS の文・system 入力・proposal・ログ・取り次ぎは根拠にならない（偽の人の指示の防止）。
//! - 判定（[`SqliteStore::authority_check`]）は使うたびに根拠を読み直す。読めなければ拒否する。
//! - 使用（[`SqliteStore::authority_use_tx`]）は判定・予算の消費・監査の行を同じ transaction で書く。結果が分かるまで
//!   監査の行は `unknown` で、同じ案件・同じ分類の次の判定は `prior_outcome_unknown` で拒否する。

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use time::OffsetDateTime;
use ulid::Ulid;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::authority::{
    ActionClass, AuthorityAuditEntry, AuthorityGrant, AuthorityProposal, AuthorityVerdict,
    GLOBAL_LEDGER, GrantScope, GrantSource, MAX_GRANT_BUDGET, MAX_GRANT_DAYS,
    MAX_ROUTINE_GRANT_DAYS, NewAuthorityAudit, NewAuthorityGrant, ScopeKind, SourceKind,
    grant_denial, reason, scope_covers, scope_rank,
};

/// 許可を作れない理由（API は 422 にする）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityRefusal {
    pub code: &'static str,
    pub detail: String,
}

/// 根拠が人のものとして読めない code。
pub const SOURCE_NOT_HUMAN: &str = "authority_source_not_human";
pub const SOURCE_UNREADABLE: &str = "authority_source_unreadable";
pub const SOURCE_KIND_RESERVED: &str = "authority_source_reserved";
pub const GRANT_NOT_GRANTABLE: &str = "authority_human_required_not_grantable";
pub const GRANT_INVALID: &str = "authority_grant_invalid";
pub const SCOPE_UNKNOWN: &str = "authority_scope_unknown";

fn refusal(code: &'static str, detail: impl Into<String>) -> AuthorityRefusal {
    AuthorityRefusal {
        code,
        detail: detail.into(),
    }
}

const GRANT_COLUMNS: &str = "id, project_id, scope_kind, scope_ref, action_class, source_kind, source_ref, \
     granted_by, question_key, note, expires_at, budget, consumed, revoked_at, revoked_by, created_at";

fn invalid(what: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("authority {what} {value}").into(),
    )
}

fn grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuthorityGrant> {
    let scope_kind: String = r.get(2)?;
    let class: String = r.get(4)?;
    let source_kind: String = r.get(5)?;
    Ok(AuthorityGrant {
        id: r.get(0)?,
        project_id: r.get(1)?,
        scope: GrantScope {
            kind: ScopeKind::parse(&scope_kind).ok_or_else(|| invalid("scope", &scope_kind))?,
            reference: r.get(3)?,
        },
        action_class: ActionClass::parse(&class).ok_or_else(|| invalid("class", &class))?,
        source: GrantSource {
            kind: SourceKind::parse(&source_kind).ok_or_else(|| invalid("source", &source_kind))?,
            reference: r.get(6)?,
        },
        granted_by: r.get(7)?,
        question_key: r.get(8)?,
        note: r.get(9)?,
        expires_at: r.get(10)?,
        budget: r.get(11)?,
        consumed: r.get(12)?,
        revoked_at: r.get(13)?,
        revoked_by: r.get(14)?,
        created_at: r.get(15)?,
    })
}

fn grants_of(conn: &Connection, project: &str) -> Result<Vec<AuthorityGrant>, StoreError> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {GRANT_COLUMNS} FROM authority_grants WHERE project_id = ?1 ORDER BY created_at, id"
    ))?;
    let rows = st.query_map([project], grant_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn grant_of(conn: &Connection, id: &str) -> Result<Option<AuthorityGrant>, StoreError> {
    let mut st = conn.prepare_cached(&format!(
        "SELECT {GRANT_COLUMNS} FROM authority_grants WHERE id = ?1"
    ))?;
    Ok(st.query_row([id], grant_row).optional()?)
}

/// 範囲が実在し案件に属するか。案件は同じ id、task はその案件の task。途中目標は `project_snapshot` と同じ規則で、
/// approved の Roadmap の版があればその版の取り下げていない Outcome の id だけ（request の block は読まない）、
/// 無ければ依頼文の Outcome block の id。書き込みの tx の中からも呼ばれるので `conn` だけを使う。読めなければ `Err`。
fn scope_exists(conn: &Connection, project: &str, scope: &GrantScope) -> Result<bool, StoreError> {
    Ok(match scope.kind {
        ScopeKind::Global => project == GLOBAL_LEDGER && scope.reference == GLOBAL_LEDGER,
        ScopeKind::Project => {
            scope.reference == project
                && conn
                    .query_row(
                        "SELECT 1 FROM projects WHERE id = ?1",
                        [project],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
        }
        ScopeKind::Task => {
            let owner: Option<Option<String>> = conn
                .query_row(
                    "SELECT project_id FROM tasks WHERE id = ?1",
                    [&scope.reference],
                    |r| r.get(0),
                )
                .optional()?;
            owner.flatten().as_deref() == Some(project)
        }
        ScopeKind::Outcome => match super::roadmap::approved_tx(conn, project)? {
            Some(version) => crate::roadmap::snapshot_outcomes(&version)
                .iter()
                .any(|o| o.id == scope.reference),
            None => {
                let request: Option<Option<String>> = conn
                    .query_row(
                        "SELECT request FROM projects WHERE id = ?1",
                        [project],
                        |r| r.get(0),
                    )
                    .optional()?;
                let request = request.flatten().unwrap_or_default();
                crate::project_proposal::parse_outcomes(&request)
                    .is_ok_and(|outcomes| outcomes.iter().any(|o| o.id == scope.reference))
            }
        },
    })
}

/// 元の根拠を確かめる。`None` なら人のものとして読めた。使うたびに読み直す（消された・取り下げられた根拠は無効）。
pub(crate) fn source_problem(
    conn: &Connection,
    project: &str,
    source: &GrantSource,
    granted_by: &str,
) -> Result<Option<AuthorityRefusal>, StoreError> {
    let unreadable = |what: &str| {
        Ok(Some(refusal(
            SOURCE_UNREADABLE,
            format!("{what} {} cannot be read", source.reference),
        )))
    };
    let not_human = |why: &str| {
        Ok(Some(refusal(
            SOURCE_NOT_HUMAN,
            format!(
                "{} {} is not a person's instruction: {why}",
                source.kind.as_str(),
                source.reference
            ),
        )))
    };
    match source.kind {
        SourceKind::HumanMessage => {
            let row: Option<(String, String, String, String, Option<String>)> = conn
                .query_row(
                    "SELECT m.role, m.state, m.metadata_json, t.kind, t.project_id
                     FROM chat_messages m JOIN chat_threads t ON t.id = m.thread_id
                     WHERE m.id = ?1",
                    [&source.reference],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let Some((role, state, meta, thread_kind, thread_project)) = row else {
                return unreadable("message");
            };
            if role != "user" {
                return not_human(&format!("the message role is {role}"));
            }
            if state == "cancelled" {
                return not_human("the message was cancelled");
            }
            let meta: Value = serde_json::from_str(&meta).unwrap_or(Value::Null);
            if !meta
                .get("author")
                .and_then(Value::as_str)
                .is_some_and(|a| a.starts_with("human:"))
            {
                return not_human("the author is not a person's token");
            }
            if meta.get("relay").is_some_and(|r| !r.is_null()) {
                return not_human("a relayed message is another principal's report of a person");
            }
            let in_project = thread_project.as_deref() == Some(project);
            if !(in_project || thread_kind == "general") {
                return not_human("the message belongs to another project's thread");
            }
            Ok(None)
        }
        SourceKind::Decision => {
            let row: Option<(String, String)> = conn
                .query_row(
                    "SELECT status, json FROM decisions WHERE id = ?1",
                    [&source.reference],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((status, json)) = row else {
                return unreadable("decision");
            };
            let json: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
            let by = json
                .pointer("/answer/by")
                .and_then(Value::as_str)
                .unwrap_or("");
            if status != "answered" || !by.starts_with("human:") {
                return not_human("the decision was not answered by a person");
            }
            let task: Option<String> = conn
                .query_row(
                    "SELECT task_id FROM decisions WHERE id = ?1",
                    [&source.reference],
                    |r| r.get(0),
                )
                .optional()?;
            let owner: Option<Option<String>> = match task {
                Some(task) => conn
                    .query_row("SELECT project_id FROM tasks WHERE id = ?1", [task], |r| {
                        r.get(0)
                    })
                    .optional()?,
                None => None,
            };
            if owner.flatten().as_deref() != Some(project) {
                return not_human("the decision belongs to another project");
            }
            Ok(None)
        }
        SourceKind::Approval => {
            let row: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT project_id, decision FROM approvals WHERE id = ?1",
                    [&source.reference],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((owner, decision)) = row else {
                return unreadable("approval");
            };
            if owner.as_deref() != Some(project) {
                return not_human("the approval belongs to another project");
            }
            if !matches!(decision.as_deref(), Some("once" | "standing")) {
                return not_human("the approval was not granted");
            }
            if cos_decided_on_its_own(conn, "approval.decide", &source.reference)? {
                return not_human("the approval was answered by the CoS, not by a person");
            }
            Ok(None)
        }
        SourceKind::StandingRule => {
            let found = conn
                .query_row(
                    "SELECT 1 FROM standing_rules WHERE id = ?1",
                    [&source.reference],
                    |_| Ok(()),
                )
                .optional()?;
            if found.is_none() {
                return unreadable("standing rule");
            }
            if cos_decided_on_its_own(conn, "standing_rule.create", &source.reference)? {
                return not_human("the standing rule was written by the CoS on its own");
            }
            Ok(None)
        }
        SourceKind::HumanOperation => {
            // API の中だけが作る（POST …/grants は断る）。行そのものが人の操作の記録。
            let expected = format!("project.controller_put:{project}:");
            if source.reference.starts_with(&expected) && granted_by.starts_with("human:") {
                Ok(None)
            } else {
                not_human("the operation is not this project's controller_put by a person")
            }
        }
    }
}

/// CoS が `instructed_by` なしで行った操作の記録があるか。
fn cos_decided_on_its_own(
    conn: &Connection,
    action: &str,
    target: &str,
) -> Result<bool, StoreError> {
    let found = conn
        .query_row(
            "SELECT 1 FROM cos_operations
             WHERE action = ?1 AND target_id = ?2 AND state = 'applied'
               AND json_extract(payload_json, '$.instructed_by') IS NULL",
            params![action, target],
            |_| Ok(()),
        )
        .optional()?;
    Ok(found.is_some())
}

/// 元の根拠の短い抜粋（人に示す。案件 CoS の判定の値には使わない）。
fn source_excerpt(conn: &Connection, source: &GrantSource) -> Result<Option<String>, StoreError> {
    let text: Option<String> = match source.kind {
        SourceKind::HumanMessage => conn
            .query_row(
                "SELECT text FROM chat_messages WHERE id = ?1",
                [&source.reference],
                |r| r.get(0),
            )
            .optional()?,
        SourceKind::Approval => conn
            .query_row(
                "SELECT question FROM approvals WHERE id = ?1",
                [&source.reference],
                |r| r.get(0),
            )
            .optional()?,
        SourceKind::StandingRule => conn
            .query_row(
                "SELECT rule FROM standing_rules WHERE id = ?1",
                [&source.reference],
                |r| r.get(0),
            )
            .optional()?,
        SourceKind::Decision => conn
            .query_row(
                "SELECT json_extract(json, '$.question') FROM decisions WHERE id = ?1",
                [&source.reference],
                |r| r.get(0),
            )
            .optional()?
            .flatten(),
        SourceKind::HumanOperation => None,
    };
    Ok(text.map(|t| t.chars().take(200).collect()))
}

fn unknown_outcome(conn: &Connection, project: &str, class: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM authority_audit
             WHERE project_id = ?1 AND action_class = ?2 AND outcome_state = 'unknown' LIMIT 1",
            params![project, class],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// 判定の本体（読み取りと書き込みの tx の両方から呼ぶ）。行が読めないときは呼び出し側が拒否にする。
fn check_tx(
    conn: &Connection,
    project: &str,
    action_class: &str,
    scope: &GrantScope,
    now: OffsetDateTime,
) -> Result<AuthorityVerdict, StoreError> {
    let Some(class) = ActionClass::parse(action_class) else {
        return Ok(AuthorityVerdict::denied(reason::UNKNOWN_ACTION_CLASS));
    };
    if class == ActionClass::HumanRequired {
        return Ok(AuthorityVerdict::denied(reason::HUMAN_REQUIRED));
    }
    if !scope_exists(conn, project, scope)? {
        return Ok(AuthorityVerdict::denied(reason::UNKNOWN_SCOPE));
    }
    if unknown_outcome(conn, project, action_class)? {
        return Ok(AuthorityVerdict::denied(reason::PRIOR_OUTCOME_UNKNOWN));
    }
    // D4: 案件の判定は global の台帳の行も候補にする（global の許可はどの案件の範囲も含む）。
    let mut ledger = grants_of(conn, project)?;
    if project != GLOBAL_LEDGER {
        ledger.extend(grants_of(conn, GLOBAL_LEDGER)?);
    }
    let mut candidates: Vec<AuthorityGrant> = ledger
        .into_iter()
        .filter(|g| g.action_class == class && scope_covers(&g.scope, scope))
        .collect();
    candidates.sort_by(|a, b| {
        (scope_rank(a.scope.kind), &a.expires_at, &a.id).cmp(&(
            scope_rank(b.scope.kind),
            &b.expires_at,
            &b.id,
        ))
    });
    // 許可できる行が無いときは、最も新しく作られた行の拒否の理由を返す（人が直せる理由を示す）。
    let mut newest: Option<(&str, &str, &'static str)> = None;
    for g in &candidates {
        let denial = match grant_denial(g, now) {
            Some(denial) => denial,
            None if source_problem(conn, project, &g.source, &g.granted_by)?.is_some() => {
                reason::SOURCE_UNREADABLE
            }
            None => return Ok(AuthorityVerdict::allowed(g)),
        };
        let key = (g.created_at.as_str(), g.id.as_str(), denial);
        if newest.is_none_or(|n| (n.0, n.1) < (key.0, key.1)) {
            newest = Some(key);
        }
    }
    Ok(AuthorityVerdict::denied(
        newest.map_or(reason::NO_GRANT, |n| n.2),
    ))
}

fn audit_insert(
    conn: &Connection,
    row: &NewAuthorityAudit,
    now: OffsetDateTime,
) -> Result<String, StoreError> {
    let at = format_rfc3339(now)?;
    let id = Ulid::new().to_string();
    let observed_at = (row.outcome_state != "unknown").then(|| at.clone());
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO authority_audit
         (id, project_id, action_class, scope_kind, scope_ref, grant_id, allow, reason, suggestion_json,
          actual_actor, human_decision_ref, decision_id, dedupe_key, outcome_state, observed_outcome_json,
          created_at, observed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            id,
            row.project_id,
            row.action_class,
            row.scope.kind.as_str(),
            row.scope.reference,
            row.verdict.grant_id,
            row.verdict.allow,
            row.verdict.reason,
            row.suggestion.to_string(),
            row.actual_actor,
            row.human_decision_ref,
            row.decision_id,
            row.dedupe_key,
            row.outcome_state,
            row.observed_outcome.as_ref().map(Value::to_string),
            at,
            observed_at,
        ],
    )?;
    if inserted > 0 {
        return Ok(id);
    }
    Ok(conn.query_row(
        "SELECT id FROM authority_audit WHERE dedupe_key = ?1",
        [&row.dedupe_key],
        |r| r.get(0),
    )?)
}

fn audit_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuthorityAuditEntry> {
    let suggestion: String = r.get(8)?;
    let observed: Option<String> = r.get(12)?;
    Ok(AuthorityAuditEntry {
        id: r.get(0)?,
        project_id: r.get(1)?,
        action_class: r.get(2)?,
        scope_kind: r.get(3)?,
        scope_ref: r.get(4)?,
        grant_id: r.get(5)?,
        allow: r.get(6)?,
        reason: r.get(7)?,
        suggestion: serde_json::from_str(&suggestion).unwrap_or(Value::Null),
        actual_actor: r.get(9)?,
        human_decision_ref: r.get(10)?,
        decision_id: r.get(11)?,
        outcome_state: r.get(13)?,
        observed_outcome: observed.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: r.get(14)?,
        observed_at: r.get(15)?,
    })
}

const AUDIT_COLUMNS: &str = "id, project_id, action_class, scope_kind, scope_ref, grant_id, allow, reason, suggestion_json, \
     actual_actor, human_decision_ref, decision_id, observed_outcome_json, outcome_state, created_at, observed_at";

fn proposal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuthorityProposal> {
    let scope_kind: String = r.get(3)?;
    let class: String = r.get(5)?;
    let source_kind: String = r.get(6)?;
    Ok(AuthorityProposal {
        id: r.get(0)?,
        project_id: r.get(1)?,
        question_key: r.get(2)?,
        scope: GrantScope {
            kind: ScopeKind::parse(&scope_kind).ok_or_else(|| invalid("scope", &scope_kind))?,
            reference: r.get(4)?,
        },
        action_class: ActionClass::parse(&class).ok_or_else(|| invalid("class", &class))?,
        source: GrantSource {
            kind: SourceKind::parse(&source_kind).ok_or_else(|| invalid("source", &source_kind))?,
            reference: r.get(7)?,
        },
        expires_at: r.get(8)?,
        budget: r.get(9)?,
        note: r.get(10)?,
        proposed_by: r.get(11)?,
        state: r.get(12)?,
        resolved_grant_id: r.get(13)?,
        resolved_by: r.get(14)?,
        resolved_at: r.get(15)?,
        created_at: r.get(16)?,
    })
}

const PROPOSAL_COLUMNS: &str = "id, project_id, question_key, scope_kind, scope_ref, action_class, source_kind, source_ref, \
     expires_at, budget, note, proposed_by, state, resolved_grant_id, resolved_by, resolved_at, created_at";

fn validate_new(
    conn: &Connection,
    project: &str,
    scope: &GrantScope,
    class: ActionClass,
    expires_at: &str,
    budget: i64,
    now: OffsetDateTime,
) -> Result<Option<AuthorityRefusal>, StoreError> {
    if class == ActionClass::HumanRequired {
        return Ok(Some(refusal(
            GRANT_NOT_GRANTABLE,
            "human_required cannot be lifted by a grant; the person answers each case",
        )));
    }
    if !(1..=MAX_GRANT_BUDGET).contains(&budget) {
        return Ok(Some(refusal(
            GRANT_INVALID,
            format!("budget must be 1 to {MAX_GRANT_BUDGET}"),
        )));
    }
    if scope.kind == ScopeKind::Global && !class.is_routine_answer() {
        return Ok(Some(refusal(
            GRANT_INVALID,
            format!(
                "a global grant delegates routine answers only; {} needs a project, outcome or task scope",
                class.as_str()
            ),
        )));
    }
    let Ok(expiry) = super::parse_rfc3339(expires_at) else {
        return Ok(Some(refusal(GRANT_INVALID, "expires_at must be RFC3339")));
    };
    let max_days = if class.is_routine_answer() {
        MAX_ROUTINE_GRANT_DAYS
    } else {
        MAX_GRANT_DAYS
    };
    if expiry <= now || expiry > now + time::Duration::days(max_days) {
        return Ok(Some(refusal(
            GRANT_INVALID,
            format!("expires_at must be in the future and within {max_days} days"),
        )));
    }
    if !scope_exists(conn, project, scope)? {
        return Ok(Some(refusal(
            SCOPE_UNKNOWN,
            format!(
                "{} {} is not part of this project",
                scope.kind.as_str(),
                scope.reference
            ),
        )));
    }
    Ok(None)
}

impl SqliteStore {
    /// 許可を 1 件作る（人の API の書き込みの tx の中で呼ぶ）。`Err` の外側は DB の失敗、内側は作らない理由。
    pub fn authority_grant_insert_tx(
        conn: &Connection,
        new: &NewAuthorityGrant,
        now: OffsetDateTime,
    ) -> Result<Result<AuthorityGrant, AuthorityRefusal>, StoreError> {
        if !new.granted_by.starts_with("human:") {
            return Ok(Err(refusal(SOURCE_NOT_HUMAN, "only a person grants")));
        }
        if let Some(problem) = validate_new(
            conn,
            &new.project_id,
            &new.scope,
            new.action_class,
            &new.expires_at,
            new.budget,
            now,
        )? {
            return Ok(Err(problem));
        }
        if let Some(problem) = source_problem(conn, &new.project_id, &new.source, &new.granted_by)?
        {
            return Ok(Err(problem));
        }
        let at = format_rfc3339(now)?;
        conn.execute(
            "INSERT INTO authority_grants
             (id, project_id, scope_kind, scope_ref, action_class, source_kind, source_ref, granted_by,
              question_key, note, expires_at, budget, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                new.id,
                new.project_id,
                new.scope.kind.as_str(),
                new.scope.reference,
                new.action_class.as_str(),
                new.source.kind.as_str(),
                new.source.reference,
                new.granted_by,
                new.question_key,
                new.note,
                new.expires_at,
                new.budget,
                at,
            ],
        )?;
        let grant = grant_of(conn, &new.id)?
            .ok_or_else(|| StoreError::Invalid(format!("grant {} vanished", new.id)))?;
        Ok(Ok(grant))
    }

    pub fn authority_grants(&self, project: &str) -> Result<Vec<AuthorityGrant>, StoreError> {
        self.with_read_conn(|c| grants_of(c, project))
    }

    pub fn authority_grant(&self, id: &str) -> Result<Option<AuthorityGrant>, StoreError> {
        self.with_read_conn(|c| grant_of(c, id))
    }

    /// 取り消す。無い id は `None`、既に取り消し済みはそのまま返す。
    pub fn authority_grant_revoke_tx(
        conn: &Connection,
        project: &str,
        id: &str,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AuthorityGrant>, StoreError> {
        let at = format_rfc3339(now)?;
        conn.execute(
            "UPDATE authority_grants SET revoked_at = ?3, revoked_by = ?4
             WHERE id = ?1 AND project_id = ?2 AND revoked_at IS NULL",
            params![id, project, at, by],
        )?;
        Ok(grant_of(conn, id)?.filter(|g| g.project_id == project))
    }

    /// 人が controller を `enabled` にした操作の許可の前の行を取り消す（同じ操作の繰り返しで予算を積み増さない）。
    pub fn authority_supersede_operation_grants_tx(
        conn: &Connection,
        project: &str,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let at = format_rfc3339(now)?;
        conn.execute(
            "UPDATE authority_grants SET revoked_at = ?2, revoked_by = ?3
             WHERE project_id = ?1 AND source_kind = 'human_operation'
               AND action_class = 'launch_root' AND revoked_at IS NULL",
            params![project, at, by],
        )?;
        Ok(())
    }

    /// 判定（読み取り）。DB が読めないときも拒否を返す（fail closed）。
    pub fn authority_check(
        &self,
        project: &str,
        action_class: &str,
        scope: &GrantScope,
        now: OffsetDateTime,
    ) -> AuthorityVerdict {
        match self.with_read_conn(|c| check_tx(c, project, action_class, scope, now)) {
            Ok(verdict) => verdict,
            Err(_) => AuthorityVerdict::denied(reason::LEDGER_UNREADABLE),
        }
    }

    /// 判定（書き込みの tx の中）。
    pub fn authority_check_tx(
        conn: &Connection,
        project: &str,
        class: ActionClass,
        scope: &GrantScope,
        now: OffsetDateTime,
    ) -> Result<AuthorityVerdict, StoreError> {
        check_tx(conn, project, class.as_str(), scope, now)
    }

    /// `question_key` に答えた、今も有効な許可（同じ問いを人に聞き直さない根拠）。
    pub fn authority_answered(
        &self,
        project: &str,
        question_key: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AuthorityGrant>, StoreError> {
        self.with_read_conn(|c| {
            for g in grants_of(c, project)? {
                if g.question_key.as_deref() == Some(question_key)
                    && grant_denial(&g, now).is_none()
                    && scope_exists(c, project, &g.scope)?
                    && source_problem(c, project, &g.source, &g.granted_by)?.is_none()
                {
                    return Ok(Some(g));
                }
            }
            Ok(None)
        })
    }

    /// 分類の許可がこれまでに何件作られたか（期限切れの後の問いを別の key にする）。
    pub fn authority_grant_generation(
        &self,
        project: &str,
        class: ActionClass,
    ) -> Result<i64, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM authority_grants WHERE project_id = ?1 AND action_class = ?2",
                params![project, class.as_str()],
                |r| r.get(0),
            )?)
        })
    }

    /// 根拠の抜粋（人に示す）。
    pub fn authority_source_excerpt(
        &self,
        source: &GrantSource,
    ) -> Result<Option<String>, StoreError> {
        self.with_read_conn(|c| source_excerpt(c, source))
    }

    /// 許可を 1 回使う: 書き込みの tx の中で判定をやり直し、許可なら予算を消費し、監査の行を書く。
    /// 同じ `dedupe_key` の 2 回目は消費せず前の結果を返す。戻り値は判定と監査の行の id。
    pub fn authority_use_tx(
        conn: &Connection,
        new: &NewAuthorityAudit,
        now: OffsetDateTime,
    ) -> Result<(AuthorityVerdict, String), StoreError> {
        let existing: Option<(String, Option<String>, bool, String)> = conn
            .query_row(
                "SELECT id, grant_id, allow, reason FROM authority_audit WHERE dedupe_key = ?1",
                [&new.dedupe_key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((id, grant_id, allow, why)) = existing {
            let mut verdict = AuthorityVerdict::denied(&why);
            verdict.allow = allow;
            verdict.grant_id = grant_id;
            return Ok((verdict, id));
        }
        let verdict = check_tx(conn, &new.project_id, &new.action_class, &new.scope, now)?;
        if let Some(grant) = verdict.grant_id.as_deref().filter(|_| verdict.allow) {
            conn.execute(
                "UPDATE authority_grants SET consumed = consumed + 1 WHERE id = ?1",
                [grant],
            )?;
        }
        let mut row = new.clone();
        if verdict.allow {
            row.human_decision_ref = verdict
                .grant_id
                .as_ref()
                .map(|g| format!("authority_grant:{g}"));
        } else {
            row.outcome_state = "refused";
            row.observed_outcome = Some(serde_json::json!({"refused": verdict.reason}));
        }
        row.verdict = verdict.clone();
        let id = audit_insert(conn, &row, now)?;
        Ok((verdict, id))
    }

    /// D4: 取り次ぎ（`chat.relay_forward`）の根拠の許可 `grant_id` が案件 `project` の常設の指示を裏付けるか。
    /// 裏付けるなら `None`、裏付けないなら理由。global の台帳か同じ案件の台帳の、範囲が global か案件全体の行で、
    /// 取り消し・期限・回数・元の根拠のどれにも引っかからないこと。取り次ぎを根拠に使うたびにも呼ぶ。
    pub fn authority_grant_backs_tx(
        conn: &Connection,
        grant_id: &str,
        project: &str,
        now: OffsetDateTime,
    ) -> Result<Option<String>, StoreError> {
        let Some(grant) = grant_of(conn, grant_id)? else {
            return Ok(Some("no such grant".into()));
        };
        let covers = match grant.scope.kind {
            ScopeKind::Global => true,
            ScopeKind::Project => grant.project_id == project,
            ScopeKind::Outcome | ScopeKind::Task => false,
        };
        if project.is_empty() || !covers {
            return Ok(Some(format!(
                "the grant's scope {} {} does not cover the whole project",
                grant.scope.kind.as_str(),
                grant.scope.reference
            )));
        }
        if let Some(denial) = grant_denial(&grant, now) {
            return Ok(Some(denial.to_owned()));
        }
        if source_problem(conn, project, &grant.source, &grant.granted_by)?.is_some() {
            return Ok(Some(reason::SOURCE_UNREADABLE.to_owned()));
        }
        Ok(None)
    }

    pub fn authority_grant_backs(
        &self,
        grant_id: &str,
        project: &str,
        now: OffsetDateTime,
    ) -> Result<Option<String>, StoreError> {
        self.with_read_conn(|c| Self::authority_grant_backs_tx(c, grant_id, project, now))
    }

    /// 許可を使わない監査の行（人の発言の取り次ぎ、controller の拒否、問いの抑止）。同じ key は 1 行。
    pub fn authority_audit_record(
        &self,
        row: &NewAuthorityAudit,
        now: OffsetDateTime,
    ) -> Result<String, StoreError> {
        self.write_tx(|tx| audit_insert(tx, row, now))
    }

    pub fn authority_use(
        &self,
        new: &NewAuthorityAudit,
        now: OffsetDateTime,
    ) -> Result<(AuthorityVerdict, String), StoreError> {
        self.write_tx(|tx| Self::authority_use_tx(tx, new, now))
    }

    /// 結果を確かめた（`unknown` の行を 1 回だけ閉じる）。`failed` は効果が無かったので予算を戻す。
    pub fn authority_outcome_settle_tx(
        conn: &Connection,
        project: &str,
        audit_id: &str,
        state: &str,
        observed: &Value,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        if !matches!(state, "applied" | "failed") {
            return Err(StoreError::Invalid(format!("authority outcome {state}")));
        }
        let at = format_rfc3339(now)?;
        let changed = conn.execute(
            "UPDATE authority_audit SET outcome_state = ?3, observed_outcome_json = ?4, observed_at = ?5
             WHERE id = ?1 AND project_id = ?2 AND outcome_state = 'unknown'",
            params![audit_id, project, state, observed.to_string(), at],
        )?;
        if changed > 0 && state == "failed" {
            conn.execute(
                "UPDATE authority_grants SET consumed = consumed - 1
                 WHERE consumed > 0 AND id =
                   (SELECT grant_id FROM authority_audit WHERE id = ?1)",
                [audit_id],
            )?;
        }
        Ok(changed > 0)
    }

    pub fn authority_outcome_settle(
        &self,
        project: &str,
        audit_id: &str,
        state: &str,
        observed: &Value,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        self.write_tx(|tx| {
            Self::authority_outcome_settle_tx(tx, project, audit_id, state, observed, now)
        })
    }

    pub fn authority_audit(
        &self,
        project: &str,
        limit: usize,
    ) -> Result<Vec<AuthorityAuditEntry>, StoreError> {
        self.with_read_conn(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {AUDIT_COLUMNS} FROM authority_audit WHERE project_id = ?1
                 ORDER BY created_at DESC, id DESC LIMIT ?2"
            ))?;
            let rows = st.query_map(params![project, limit as i64], audit_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn authority_proposals(&self, project: &str) -> Result<Vec<AuthorityProposal>, StoreError> {
        self.with_read_conn(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {PROPOSAL_COLUMNS} FROM authority_proposals WHERE project_id = ?1
                 ORDER BY created_at DESC, id DESC"
            ))?;
            let rows = st.query_map([project], proposal_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn authority_proposal_by_question_tx(
        conn: &Connection,
        project: &str,
        question_key: &str,
    ) -> Result<Option<AuthorityProposal>, StoreError> {
        let mut st = conn.prepare_cached(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM authority_proposals
             WHERE project_id = ?1 AND question_key = ?2"
        ))?;
        Ok(st
            .query_row(params![project, question_key], proposal_row)
            .optional()?)
    }

    /// 人が断った問いか（同じ問いを聞き直さない根拠）。
    pub fn authority_question_dismissed(
        &self,
        project: &str,
        question_key: &str,
    ) -> Result<Option<AuthorityProposal>, StoreError> {
        self.with_read_conn(|c| {
            Ok(
                Self::authority_proposal_by_question_tx(c, project, question_key)?
                    .filter(|p| p.state == "dismissed"),
            )
        })
    }

    /// 提案を 1 件書く。根拠は人の発言だけ。`Err` の内側は作らない理由。
    pub fn authority_proposal_insert_tx(
        conn: &rusqlite::Transaction<'_>,
        new: &AuthorityProposal,
        now: OffsetDateTime,
    ) -> Result<Result<AuthorityProposal, AuthorityRefusal>, StoreError> {
        if new.source.kind != SourceKind::HumanMessage {
            return Ok(Err(refusal(
                SOURCE_NOT_HUMAN,
                "a proposal cites the person's message (source.kind = human_message)",
            )));
        }
        if let Some(problem) = validate_new(
            conn,
            &new.project_id,
            &new.scope,
            new.action_class,
            &new.expires_at,
            new.budget,
            now,
        )? {
            return Ok(Err(problem));
        }
        // 発言の確かめは `granted_by` を読まない（読むのは `human_operation` だけ）。
        if let Some(problem) = source_problem(conn, &new.project_id, &new.source, "human:proposal")?
        {
            return Ok(Err(problem));
        }
        let at = format_rfc3339(now)?;
        conn.execute(
            "INSERT INTO authority_proposals
             (id, project_id, question_key, scope_kind, scope_ref, action_class, source_kind, source_ref,
              expires_at, budget, note, proposed_by, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                new.id,
                new.project_id,
                new.question_key,
                new.scope.kind.as_str(),
                new.scope.reference,
                new.action_class.as_str(),
                new.source.kind.as_str(),
                new.source.reference,
                new.expires_at,
                new.budget,
                new.note,
                new.proposed_by,
                at,
            ],
        )?;
        crate::feed::record_in_tx(
            conn,
            &crate::feed::NoticeEvent {
                source_key: format!("authority:proposal:{}:{}", new.project_id, new.question_key),
                kind: crate::feed::NoticeKind::BadNews,
                group_key: format!("bad_news:authority:{}", new.project_id),
                title: if new.project_id == GLOBAL_LEDGER {
                    format!(
                        "統括 CoS が全案件に効く許可（{}）を提案しました",
                        new.action_class.as_str()
                    )
                } else {
                    format!(
                        "案件 {} の CoS が許可（{}）を提案しました",
                        new.project_id,
                        new.action_class.as_str()
                    )
                },
                summary: format!(
                    "{}。案件の画面の「許可」で、許可するか断るかを決めてください。問いの key: {}。人の発言: {}",
                    new.note, new.question_key, new.source.reference
                ),
                project_id: (new.project_id != GLOBAL_LEDGER).then(|| new.project_id.clone()),
                task_id: None,
                target: None,
                links: Vec::new(),
                at: now,
            },
        )?;
        let row =
            Self::authority_proposal_by_question_tx(conn, &new.project_id, &new.question_key)?
                .ok_or_else(|| StoreError::Invalid(format!("proposal {} vanished", new.id)))?;
        Ok(Ok(row))
    }

    /// 人が提案を閉じる（`accepted` は作った許可の id つき）。open でなければ `false`。
    pub fn authority_proposal_resolve_tx(
        conn: &Connection,
        project: &str,
        id: &str,
        state: &str,
        grant_id: Option<&str>,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        Ok(conn.execute(
            "UPDATE authority_proposals SET state = ?3, resolved_grant_id = ?4, resolved_by = ?5, resolved_at = ?6
             WHERE id = ?1 AND project_id = ?2 AND state = 'open'",
            params![id, project, state, grant_id, by, at],
        )? > 0)
    }

    pub fn authority_proposal_by_id_tx(
        conn: &Connection,
        project: &str,
        id: &str,
    ) -> Result<Option<AuthorityProposal>, StoreError> {
        let mut st = conn.prepare_cached(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM authority_proposals WHERE project_id = ?1 AND id = ?2"
        ))?;
        Ok(st
            .query_row(params![project, id], proposal_row)
            .optional()?)
    }

    /// 許可の不足で `blocked` になった案件を、人が許可を与えたときに `active` に戻す。他の理由の `blocked` は触らない。
    pub fn authority_unblock_tx(
        conn: &Connection,
        project: &str,
        now: OffsetDateTime,
        coalesce_secs: u64,
    ) -> Result<(), StoreError> {
        Self::project_controller_human_bump_tx(conn, project, now, coalesce_secs)?;
        conn.execute(
            "UPDATE project_controller_state SET phase = 'active'
             WHERE project_id = ?1 AND phase = 'blocked' AND last_decision_id IN
               (SELECT id FROM project_controller_decisions
                 WHERE project_id = ?1 AND validation = ?2)",
            params![project, AUTHORITY_REQUIRED_RULE],
        )?;
        Ok(())
    }
}

/// 許可が足りずに人に聞いた判断の規則の名（`validation`）。
pub const AUTHORITY_REQUIRED_RULE: &str = "pc/authority-required";
