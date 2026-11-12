//! ADR 2026-11-12-cos-triage-scope D4: the deterministic match of one CoS inbox item before CoS
//! judges it. The daemon attaches the result to the item in the CoS prompt, and the resolve API uses
//! the same function to refuse answering (or routing to the operator) what a person keeps (D1/D2).
//! Reads the store only; no LLM.

use serde::{Deserialize, Serialize};
use task_core::authority::{ActionClass, AuthorityGrant, GLOBAL_LEDGER, grant_backs};
use task_core::store::{SqliteStore, StoreError};
use time::OffsetDateTime;

use crate::human_inbox::InboxKind;

/// The text of the authorization a cluster job wait limit raises
/// (`task_dispatch::dispatcher::cluster_job_wait`: 「クラスタ `…` の job の待ちが上限（N 秒）に達しました」).
pub const JOB_WAIT_LIMIT_MARKER: &str = "の job の待ちが上限（";
/// The plan gate reason of a cost/limit approval (`near_limit:`), which a person keeps.
pub const NEAR_LIMIT_MARKER: &str = "near_limit:";

/// D1: the matters a person keeps. Only the ones the API can tell from the item's kind and text are
/// detected here; the rest are CoS's judgment under the policy (skill cos-inbox-triage §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reserved {
    /// login・TOTP・OAuth など credential の入力。
    CredentialInput,
    /// 承認（grant・予算・node-h の枠）を超える予算と資源。
    BeyondApproval,
    /// credential とセキュリティの境界（秘密・権限・sandbox の緩和）。
    SecurityBoundary,
}

impl Reserved {
    pub fn as_str(self) -> &'static str {
        match self {
            Reserved::CredentialInput => "credential_input",
            Reserved::BeyondApproval => "beyond_approval",
            Reserved::SecurityBoundary => "security_boundary",
        }
    }
}

/// D4: a likely answer and what backs it (a live grant, when one covers the item).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Candidate {
    /// The basis category of `/cos/inbox/{i}/resolve` (`job_wait_extend`, `plan_conforms`, …).
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
}

/// The match of one item: what a person keeps, and the answers worth checking first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TriageMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved: Option<Reserved>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
}

impl TriageMatch {
    /// One line for the CoS prompt, or `None` when nothing matched.
    pub fn prompt_line(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(reserved) = self.reserved {
            parts.push(format!(
                "人に残す件（{}）。to=human で回す",
                reserved.as_str()
            ));
        }
        for c in &self.candidates {
            parts.push(match &c.grant_id {
                Some(grant) => format!("{} の候補、grant {grant}", c.category),
                None => format!("{} の候補", c.category),
            });
        }
        (!parts.is_empty()).then(|| parts.join("。"))
    }
}

/// What the match reads of an item: the same fields the triage row and the derived wait both have.
#[derive(Debug, Clone, Copy)]
pub struct MatchInput<'a> {
    /// `cos_inbox_items.source_kind` (an [`InboxKind`] name or `notice`).
    pub source_kind: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    /// The project of the item's task, when known (a project grant covers only its project).
    pub project_id: Option<&'a str>,
}

/// The pure part of the match: `grants` are the live grants the item's project may use.
pub fn match_with(
    input: MatchInput<'_>,
    grants: &[AuthorityGrant],
    now: OffsetDateTime,
) -> TriageMatch {
    let kind = InboxKind::from_name(input.source_kind);
    let text = |marker: &str| input.title.contains(marker) || input.body.contains(marker);
    let reserved = match kind {
        Some(InboxKind::CredentialWait | InboxKind::ClusterLogin) => {
            Some(Reserved::CredentialInput)
        }
        Some(InboxKind::BrowserWait) => Some(Reserved::SecurityBoundary),
        Some(InboxKind::PlanGate) if text(NEAR_LIMIT_MARKER) => Some(Reserved::BeyondApproval),
        _ => None,
    };
    let mut candidates = Vec::new();
    if reserved.is_none() {
        match kind {
            Some(InboxKind::Authorization) if text(JOB_WAIT_LIMIT_MARKER) => {
                let project = input.project_id.unwrap_or("");
                let grant_id = grants
                    .iter()
                    .filter(|g| g.action_class == ActionClass::JobWaitExtend)
                    .find(|g| grant_backs(g, project, now).is_none())
                    .map(|g| g.id.clone());
                candidates.push(Candidate {
                    category: "job_wait_extend".into(),
                    grant_id,
                });
            }
            Some(InboxKind::PlanGate) => candidates.push(Candidate {
                category: "plan_conforms".into(),
                grant_id: None,
            }),
            Some(InboxKind::Failed) if text("reviewer(") => candidates.push(Candidate {
                category: "review_rework".into(),
                grant_id: None,
            }),
            Some(
                InboxKind::Decision
                | InboxKind::Question
                | InboxKind::IntegrationRequest
                | InboxKind::PhaseGate,
            ) => candidates.push(Candidate {
                category: "task_scope".into(),
                grant_id: None,
            }),
            _ => {}
        }
    }
    TriageMatch {
        reserved,
        candidates,
    }
}

/// D4: read the grants the item may use (the global ledger and the item's project) and match.
pub fn match_item(
    store: &SqliteStore,
    input: MatchInput<'_>,
    now: OffsetDateTime,
) -> Result<TriageMatch, StoreError> {
    let mut grants = store.authority_grants(GLOBAL_LEDGER)?;
    if let Some(project) = input
        .project_id
        .filter(|p| !p.is_empty() && *p != GLOBAL_LEDGER)
    {
        grants.extend(store.authority_grants(project)?);
    }
    Ok(match_with(input, &grants, now))
}

#[cfg(test)]
#[path = "triage_match_tests.rs"]
mod tests;
