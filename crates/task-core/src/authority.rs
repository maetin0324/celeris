//! ADR 2026-11-04-project-controller A7: 人の許可範囲（authority ledger）の型と判定の規則。
//!
//! 許可は人だけが与える。1 件ごとに範囲（案件・途中目標・task）、分類、元の根拠（人の発言・決定・承認・永続の認可・
//! 人の mode 操作）、期限、予算を持つ。無条件の許可は表現できない（期限・予算は必須）。
//!
//! 判定（[`crate::store::SqliteStore::authority_check`]）は fail closed。分類・範囲・根拠・期限・前回の結果のどれかが
//! 読めない、または確かめられないときは拒否する。`human_required` は許可では解除できない（常に拒否。CoS の既存の
//! 安全不変条件を弱めない）。
//!
//! 判定は Rust だけが行う（D7）。LLM は関わらない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 許可の分類。文字列の不明な値は [`ActionClass::parse`] が `None` を返し、判定は拒否する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    /// 人が指定した判断（triage の `human_required`）。許可では解除できない。
    HumanRequired,
    Destructive,
    Production,
    ApiAuthorization,
    ExternalPublish,
    OpaqueExternalEffect,
    LaunchRoot,
    /// ADR 2026-11-11-cos-standing-context D4: CoS の定型の代答（受信箱の cluster job の待ちの上限を
    /// 「延長」で答える。`basis.category = job_wait_extend`）を人に回さずに行ってよいという委任。
    /// 定型の代答の分類だけは global の範囲と長い期限（[`MAX_ROUTINE_GRANT_DAYS`]）を持てる。
    JobWaitExtend,
}

impl ActionClass {
    pub const ALL: [ActionClass; 8] = [
        ActionClass::HumanRequired,
        ActionClass::Destructive,
        ActionClass::Production,
        ActionClass::ApiAuthorization,
        ActionClass::ExternalPublish,
        ActionClass::OpaqueExternalEffect,
        ActionClass::LaunchRoot,
        ActionClass::JobWaitExtend,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ActionClass::HumanRequired => "human_required",
            ActionClass::Destructive => "destructive",
            ActionClass::Production => "production",
            ActionClass::ApiAuthorization => "api_authorization",
            ActionClass::ExternalPublish => "external_publish",
            ActionClass::OpaqueExternalEffect => "opaque_external_effect",
            ActionClass::LaunchRoot => "launch_root",
            ActionClass::JobWaitExtend => "job_wait_extend",
        }
    }

    /// 定型の代答の委任か（global の範囲と長い期限を持てる分類）。
    pub fn is_routine_answer(self) -> bool {
        matches!(self, ActionClass::JobWaitExtend)
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Project,
    /// 途中目標。approved の Roadmap があればその版の取り下げていない Outcome、無ければ `Project.request` の Outcome block。
    Outcome,
    /// 作業単位（task）。
    Task,
    /// ADR 2026-11-11-cos-standing-context D4: 全案件。`ref` は [`GLOBAL_LEDGER`]、行の `project_id` も
    /// [`GLOBAL_LEDGER`]。定型の代答の分類（[`ActionClass::is_routine_answer`]）だけが持てる。
    Global,
}

/// global の許可の台帳の key（`authority_grants.project_id`・`scope_ref`・API の path の `global` に対応）。
pub const GLOBAL_LEDGER: &str = "*";

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Project => "project",
            ScopeKind::Outcome => "outcome",
            ScopeKind::Task => "task",
            ScopeKind::Global => "global",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "project" => Some(ScopeKind::Project),
            "outcome" => Some(ScopeKind::Outcome),
            "task" => Some(ScopeKind::Task),
            "global" => Some(ScopeKind::Global),
            _ => None,
        }
    }
}

/// 許可の範囲。`kind = project` の `ref` は案件 id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantScope {
    pub kind: ScopeKind,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// 元の根拠の種類。`human_operation` は人が controller を `enabled` にした操作で、API の中だけが作る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    HumanMessage,
    Decision,
    Approval,
    StandingRule,
    HumanOperation,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::HumanMessage => "human_message",
            SourceKind::Decision => "decision",
            SourceKind::Approval => "approval",
            SourceKind::StandingRule => "standing_rule",
            SourceKind::HumanOperation => "human_operation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human_message" => Some(SourceKind::HumanMessage),
            "decision" => Some(SourceKind::Decision),
            "approval" => Some(SourceKind::Approval),
            "standing_rule" => Some(SourceKind::StandingRule),
            "human_operation" => Some(SourceKind::HumanOperation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantSource {
    pub kind: SourceKind,
    #[serde(rename = "ref")]
    pub reference: String,
}

/// 許可 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorityGrant {
    pub id: String,
    pub project_id: String,
    pub scope: GrantScope,
    pub action_class: ActionClass,
    pub source: GrantSource,
    pub granted_by: String,
    /// この許可が答えた問いの key（同じ問いを人に聞き直さないために引く）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_key: Option<String>,
    pub note: String,
    pub expires_at: String,
    pub budget: i64,
    pub consumed: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
    pub created_at: String,
}

/// 拒否の理由・許可の理由の code（`POST …/authority/check` の `reason`）。
pub mod reason {
    pub const ALLOWED: &str = "allowed";
    pub const UNKNOWN_ACTION_CLASS: &str = "unknown_action_class";
    pub const HUMAN_REQUIRED: &str = "human_required_not_grantable";
    pub const UNKNOWN_SCOPE: &str = "unknown_scope";
    pub const NO_GRANT: &str = "no_grant";
    pub const REVOKED: &str = "grant_revoked";
    pub const EXPIRED: &str = "grant_expired";
    pub const EXPIRY_UNREADABLE: &str = "grant_expiry_unreadable";
    pub const BUDGET_EXHAUSTED: &str = "grant_budget_exhausted";
    pub const SOURCE_UNREADABLE: &str = "grant_source_unreadable";
    pub const PRIOR_OUTCOME_UNKNOWN: &str = "prior_outcome_unknown";
    pub const LEDGER_UNREADABLE: &str = "ledger_unreadable";
}

/// 判定の結果。`allow = true` のときだけ `grant_id` と根拠が入る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorityVerdict {
    pub allow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    pub reason: String,
    /// 許可の元の根拠（人に聞き直さず、これを示す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<GrantSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining: Option<i64>,
}

impl AuthorityVerdict {
    pub fn denied(reason: &str) -> Self {
        Self {
            allow: false,
            grant_id: None,
            reason: reason.to_owned(),
            source: None,
            expires_at: None,
            budget_remaining: None,
        }
    }

    pub fn allowed(grant: &AuthorityGrant) -> Self {
        Self {
            allow: true,
            grant_id: Some(grant.id.clone()),
            reason: reason::ALLOWED.to_owned(),
            source: Some(grant.source.clone()),
            expires_at: Some(grant.expires_at.clone()),
            budget_remaining: Some(grant.budget - grant.consumed),
        }
    }
}

/// 根拠を読まずに分かる許可の拒否の理由（取り消し・期限・予算）。読めない期限は拒否する。
pub fn grant_denial(grant: &AuthorityGrant, now: OffsetDateTime) -> Option<&'static str> {
    if grant.revoked_at.is_some() {
        return Some(reason::REVOKED);
    }
    match OffsetDateTime::parse(&grant.expires_at, &Rfc3339) {
        Ok(at) if at <= now => return Some(reason::EXPIRED),
        Ok(_) => {}
        Err(_) => return Some(reason::EXPIRY_UNREADABLE),
    }
    (grant.consumed >= grant.budget).then_some(reason::BUDGET_EXHAUSTED)
}

/// 範囲 `request` を `grant` が含むか。global の許可はすべての案件を、案件の許可は案件の中のすべてを、
/// 途中目標・task の許可は同じ ref だけを含む（案件の許可の候補は同じ案件の行だけから選ぶ）。
pub fn scope_covers(grant: &GrantScope, request: &GrantScope) -> bool {
    matches!(grant.kind, ScopeKind::Global | ScopeKind::Project)
        || (grant.kind == request.kind && grant.reference == request.reference)
}

/// 候補の順: 狭い範囲を先に、期限の近いものを先に、id の順（決定的）。
pub fn scope_rank(kind: ScopeKind) -> u8 {
    match kind {
        ScopeKind::Task => 0,
        ScopeKind::Outcome => 1,
        ScopeKind::Project => 2,
        ScopeKind::Global => 3,
    }
}

/// 許可の提案（CoS が人の発言を引いて出す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorityProposal {
    pub id: String,
    pub project_id: String,
    pub question_key: String,
    pub scope: GrantScope,
    pub action_class: ActionClass,
    pub source: GrantSource,
    pub expires_at: String,
    pub budget: i64,
    pub note: String,
    pub proposed_by: String,
    /// `open`・`accepted`・`dismissed`。
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<String>,
    pub created_at: String,
}

/// 監査の 1 行（suggestion・observed outcome・actual actor・human decision）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AuthorityAuditEntry {
    pub id: String,
    pub project_id: String,
    pub action_class: String,
    pub scope_kind: String,
    pub scope_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    pub allow: bool,
    pub reason: String,
    /// 誰が何を提案したか（CoS の操作の要求、controller の proposal の種類）。
    pub suggestion: serde_json::Value,
    /// 実際に実行した主体（`cos:<thread>`・`daemon`）。
    pub actual_actor: String,
    /// 人の判断（`authority_grant:<id>`・`human_message:<id>`）。無ければ人は判断していない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_decision_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// `unknown`（結果を確かめられていない）・`applied`・`failed`・`refused`。
    pub outcome_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_outcome: Option<serde_json::Value>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

/// 新しい許可の入力。
#[derive(Debug, Clone)]
pub struct NewAuthorityGrant {
    pub id: String,
    pub project_id: String,
    pub scope: GrantScope,
    pub action_class: ActionClass,
    pub source: GrantSource,
    pub granted_by: String,
    pub question_key: Option<String>,
    pub note: String,
    pub expires_at: String,
    pub budget: i64,
}

/// 新しい監査の行の入力（判定の結果と提案）。
#[derive(Debug, Clone)]
pub struct NewAuthorityAudit {
    pub project_id: String,
    pub action_class: String,
    pub scope: GrantScope,
    pub verdict: AuthorityVerdict,
    pub suggestion: serde_json::Value,
    pub actual_actor: String,
    pub human_decision_ref: Option<String>,
    pub decision_id: Option<String>,
    pub dedupe_key: String,
    pub outcome_state: &'static str,
    pub observed_outcome: Option<serde_json::Value>,
}

/// 許可の上限（無条件の許可を入れない）。
pub const MAX_GRANT_BUDGET: i64 = 1000;
pub const MAX_GRANT_DAYS: i64 = 366;
/// 定型の代答の委任の期限の上限（人が取り消すまで続く常設の委任。D4）。
pub const MAX_ROUTINE_GRANT_DAYS: i64 = 3660;
/// 人が controller を `enabled` にしたときの `launch_root` の許可の既定。
pub const DEFAULT_LAUNCH_GRANT_DAYS: i64 = 30;
pub const DEFAULT_LAUNCH_GRANT_BUDGET: i64 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &Rfc3339).unwrap()
    }

    fn grant() -> AuthorityGrant {
        AuthorityGrant {
            id: "g".into(),
            project_id: "p".into(),
            scope: GrantScope {
                kind: ScopeKind::Project,
                reference: "p".into(),
            },
            action_class: ActionClass::Production,
            source: GrantSource {
                kind: SourceKind::HumanMessage,
                reference: "m".into(),
            },
            granted_by: "human:web".into(),
            question_key: None,
            note: String::new(),
            expires_at: "2026-12-01T00:00:00Z".into(),
            budget: 1,
            consumed: 0,
            revoked_at: None,
            revoked_by: None,
            created_at: "2026-11-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn authority_ledger_grant_denial_order_and_fail_closed() {
        let now = t("2026-11-04T00:00:00Z");
        assert_eq!(grant_denial(&grant(), now), None);
        let mut g = grant();
        g.expires_at = "2026-11-04T00:00:00Z".into();
        assert_eq!(grant_denial(&g, now), Some(reason::EXPIRED));
        g.expires_at = "see the note".into();
        assert_eq!(grant_denial(&g, now), Some(reason::EXPIRY_UNREADABLE));
        let mut g = grant();
        g.consumed = 1;
        assert_eq!(grant_denial(&g, now), Some(reason::BUDGET_EXHAUSTED));
        g.revoked_at = Some("2026-11-02T00:00:00Z".into());
        assert_eq!(grant_denial(&g, now), Some(reason::REVOKED));
    }

    #[test]
    fn authority_ledger_scope_cover_is_narrow() {
        let project = GrantScope {
            kind: ScopeKind::Project,
            reference: "p".into(),
        };
        let o1 = GrantScope {
            kind: ScopeKind::Outcome,
            reference: "o1".into(),
        };
        let o2 = GrantScope {
            kind: ScopeKind::Outcome,
            reference: "o2".into(),
        };
        assert!(scope_covers(&project, &o1));
        assert!(scope_covers(&o1, &o1));
        assert!(!scope_covers(&o1, &o2));
        assert!(!scope_covers(&o1, &project));
        // D4: the global scope covers every project's scopes and ranks after the project.
        let global = GrantScope {
            kind: ScopeKind::Global,
            reference: GLOBAL_LEDGER.into(),
        };
        assert!(scope_covers(&global, &project));
        assert!(scope_covers(&global, &o1));
        assert!(scope_rank(ScopeKind::Global) > scope_rank(ScopeKind::Project));
        assert_eq!(ScopeKind::parse("global"), Some(ScopeKind::Global));
        assert!(ActionClass::JobWaitExtend.is_routine_answer());
        assert!(!ActionClass::Production.is_routine_answer());
        assert_eq!(ActionClass::parse("anything"), None);
        for class in ActionClass::ALL {
            assert_eq!(ActionClass::parse(class.as_str()), Some(class));
        }
    }
}
