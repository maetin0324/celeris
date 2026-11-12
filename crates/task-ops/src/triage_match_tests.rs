//! ADR 2026-11-12-cos-triage-scope D4: the match is a pure function of the item and the ledger.
//! The API's resolve refuses on `reserved` and the prompt shows `candidates`; no other test pins
//! which kinds a person keeps or which grant backs a job wait limit.

use task_core::authority::{
    ActionClass, AuthorityGrant, GLOBAL_LEDGER, GrantScope, GrantSource, ScopeKind, SourceKind,
};
use time::OffsetDateTime;

use super::*;

fn grant(id: &str, project: &str, kind: ScopeKind, expires_at: &str) -> AuthorityGrant {
    AuthorityGrant {
        id: id.into(),
        project_id: project.into(),
        scope: GrantScope {
            kind,
            reference: project.into(),
        },
        action_class: ActionClass::JobWaitExtend,
        source: GrantSource {
            kind: SourceKind::HumanMessage,
            reference: "m1".into(),
        },
        granted_by: "human".into(),
        question_key: None,
        note: String::new(),
        expires_at: expires_at.into(),
        budget: 100,
        consumed: 0,
        revoked_at: None,
        revoked_by: None,
        created_at: "2026-11-01T00:00:00Z".into(),
    }
}

fn input<'a>(kind: &'a str, title: &'a str, body: &'a str) -> MatchInput<'a> {
    MatchInput {
        source_kind: kind,
        title,
        body,
        project_id: Some("01PROJ"),
    }
}

fn at() -> OffsetDateTime {
    // 2026-11-12T00:00:00Z
    OffsetDateTime::from_unix_timestamp(1_794_441_600).unwrap()
}

const JOB_WAIT: &str = "クラスタ `pegasus` の job の待ちが上限（3600 秒）に達しました: 123 Q";

#[test]
fn triage_match_job_wait_limit_names_the_live_grant() {
    let now = at();
    let live = grant(
        "g-live",
        GLOBAL_LEDGER,
        ScopeKind::Global,
        "2027-01-01T00:00:00Z",
    );
    let expired = grant(
        "g-old",
        GLOBAL_LEDGER,
        ScopeKind::Global,
        "2026-11-01T00:00:00Z",
    );
    let m = match_with(
        input("authorization", "認可", JOB_WAIT),
        &[expired.clone(), live],
        now,
    );
    assert_eq!(m.reserved, None);
    assert_eq!(
        m.candidates,
        vec![Candidate {
            category: "job_wait_extend".into(),
            grant_id: Some("g-live".into())
        }]
    );
    assert_eq!(
        m.prompt_line().as_deref(),
        Some("job_wait_extend の候補、grant g-live")
    );
    // Without a live grant the candidate stays, with nothing backing it.
    let m = match_with(input("authorization", "認可", JOB_WAIT), &[expired], now);
    assert_eq!(m.candidates[0].grant_id, None);
    // Another project's grant does not back this project.
    let other = grant(
        "g-other",
        "01OTHER",
        ScopeKind::Project,
        "2027-01-01T00:00:00Z",
    );
    let m = match_with(input("authorization", "認可", JOB_WAIT), &[other], now);
    assert_eq!(m.candidates[0].grant_id, None);
    // The same input gives the same match (deterministic).
    assert_eq!(
        match_with(input("authorization", "認可", JOB_WAIT), &[], now),
        match_with(input("authorization", "認可", JOB_WAIT), &[], now)
    );
}

#[test]
fn triage_match_reserves_credential_and_budget_for_a_person() {
    let now = at();
    for kind in ["credential_wait", "cluster_login"] {
        let m = match_with(input(kind, "login", ""), &[], now);
        assert_eq!(m.reserved, Some(Reserved::CredentialInput), "{kind}");
        assert!(m.candidates.is_empty());
    }
    let m = match_with(input("browser_wait", "credential", ""), &[], now);
    assert_eq!(m.reserved, Some(Reserved::SecurityBoundary));
    let m = match_with(
        input("plan_gate", "計画 v3", "理由: near_limit: 予算の 92%"),
        &[],
        now,
    );
    assert_eq!(m.reserved, Some(Reserved::BeyondApproval));
    assert_eq!(
        m.prompt_line().as_deref(),
        Some("人に残す件（beyond_approval）。to=human で回す")
    );
    let m = match_with(
        input("plan_gate", "計画 v3", "理由: decisions: d1"),
        &[],
        now,
    );
    assert_eq!(m.reserved, None);
    assert_eq!(m.candidates[0].category, "plan_conforms");
    let m = match_with(input("decision", "write set を広げるか", ""), &[], now);
    assert_eq!(m.candidates[0].category, "task_scope");
    let m = match_with(
        input("dependents_reopened", "依存先の復帰で後続を戻した", ""),
        &[],
        now,
    );
    assert_eq!(m.reserved, None);
    assert_eq!(m.prompt_line().as_deref(), Some("observe の候補"));
    let m = match_with(input("notice", "知らせ", ""), &[], now);
    assert_eq!(m, TriageMatch::default());
    assert_eq!(m.prompt_line(), None);
}
