//! ADR 2026-11-04-project-controller A7: controller の起票と人への問いが authority ledger に従うことの試験
//! （接頭辞 `authority_ledger_`）。TS は fake の script、時計は注入、LLM を呼ばない。
//! `AUTHORITY_LEDGER_RAW_DIR` があれば JSONL を書く（`scripts/dev/test-authority-ledger-e2e.sh` が検査する）。

use super::*;
use task_core::authority::{self as a, GrantScope, ScopeKind};

fn raw_row(value: Value) {
    let Some(dir) = std::env::var_os("AUTHORITY_LEDGER_RAW_DIR") else {
        return;
    };
    let mut v = value;
    v["schema"] = json!("celeris.long-horizon-record/1");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("dispatch.jsonl"))
        .unwrap();
    use std::io::Write as _;
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
}

/// enabled だが許可の無い案件（API を通さず mode だけを置く）。
fn enabled_without_grant(f: impl Fn(&ProjectSnapshot) -> Value + Send + Sync + 'static) -> Case {
    let c = case(Some(ControllerMode::Shadow), f);
    c.store
        .project_controller_set_mode(
            &c.p.id.to_string(),
            ControllerMode::Enabled,
            None,
            "human:test",
            c.env.now(),
        )
        .unwrap();
    c
}

fn project_scope(c: &Case) -> GrantScope {
    GrantScope {
        kind: ScopeKind::Project,
        reference: c.p.id.to_string(),
    }
}

/// 人が許可を与える（API の `POST …/grants` と同じ関数・同じ tx の unblock）。根拠は人の操作の行。
fn human_grant(
    c: &Case,
    scope: GrantScope,
    budget: i64,
    days: i64,
    question: Option<&str>,
) -> String {
    let project = c.p.id.to_string();
    let now = c.env.now();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            let g = SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: ulid::Ulid::new().to_string(),
                    project_id: project.clone(),
                    scope,
                    action_class: a::ActionClass::LaunchRoot,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:test".into(),
                    question_key: question.map(str::to_owned),
                    note: "人の許可".into(),
                    expires_at: (now + time::Duration::days(days))
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap(),
                    budget,
                },
                now,
            )?
            .unwrap();
            SqliteStore::authority_unblock_tx(tx, &project, now, 0)?;
            Ok(g.id)
        })
        .unwrap()
}

/// 許可が無ければ `enabled` でも起票せず、人に 1 回だけ聞いて止まる。許可を与えると続き、予算を 1 回消費し、
/// 監査に suggestion・observed outcome・actual actor・human decision が揃う。
#[tokio::test]
async fn authority_ledger_launch_requires_grant_then_runs() {
    let mut c = enabled_without_grant(launch);
    c.wake();
    assert_eq!(c.roots(), 0, "no grant, no root");
    assert_eq!(c.escalations(), 1);
    let project = c.p.id.to_string();
    let state = c.store.project_controller_state(&project).unwrap().unwrap();
    assert_eq!(state.phase, ControllerPhase::Blocked);
    let denied = c.decisions();
    assert_eq!(denied[0].validation, "pc/authority-required");
    // blocked の間は聞き直さない。
    c.wake();
    c.wake();
    assert_eq!(c.escalations(), 1, "the same gap is asked once");
    assert_eq!(c.roots(), 0);
    let g = human_grant(&c, project_scope(&c), 2, 30, None);
    c.wake();
    assert_eq!(c.roots(), 1);
    let grant = c.store.authority_grant(&g).unwrap().unwrap();
    assert_eq!(grant.consumed, 1, "one root consumes one use");
    let audit = c.store.authority_audit(&project, 10).unwrap();
    let used = audit.iter().find(|r| r.allow).expect("allowed row");
    assert_eq!(used.action_class, "launch_root");
    assert_eq!(used.actual_actor, task_core::DAEMON_ACTOR);
    assert_eq!(used.outcome_state, "applied");
    assert_eq!(used.suggestion["action"], "launch_root");
    assert!(used.observed_outcome.as_ref().unwrap()["task_id"].is_string());
    assert_eq!(
        used.human_decision_ref.as_deref(),
        Some(format!("authority_grant:{g}").as_str())
    );
    let refused = audit.iter().find(|r| !r.allow).expect("refused row");
    assert_eq!(refused.reason, a::reason::NO_GRANT);
    assert_eq!(refused.outcome_state, "refused");
    raw_row(json!({
        "test": "authority_ledger_launch_requires_grant_then_runs",
        "escalations": c.escalations(), "roots": c.roots(), "consumed": grant.consumed,
        "audit": {"suggestion": used.suggestion.is_object(), "observed_outcome": used.observed_outcome.is_some(),
                  "actual_actor": used.actual_actor, "human_decision": used.human_decision_ref},
        "refusal": refused.reason,
    }));
}

/// 予算切れ・期限切れの許可では起票しない。問いは新しい世代の key になる（前の問いの通知の使い回しにしない）。
#[tokio::test]
async fn authority_ledger_budget_and_expiry_stop_launch() {
    let mut c = enabled_without_grant(|s: &ProjectSnapshot| {
        let n = s.roots.len() + 1;
        proposal(
            s,
            json!({"kind": "launch_root", "root": {
                "key": format!("outcome:o1:{n}"), "title": "土台", "objective": "途中目標 o1 を達成する",
                "acceptance": [{"text": "計測 script が同じ値を 3 回出す", "check": {"type": "reviewer"}}],
                "stages_hint": [], "continues_root_ids": [], "outcome_ids": ["o1"]}}),
            "pc/outcome-open",
            &[("outcome", "o1")],
        )
    });
    let scope = GrantScope {
        kind: ScopeKind::Outcome,
        reference: "o1".into(),
    };
    human_grant(&c, scope.clone(), 1, 30, None);
    c.wake();
    assert_eq!(c.roots(), 1);
    c.wake();
    assert_eq!(c.roots(), 1, "the budget is spent");
    let last = &c.decisions()[0];
    assert_eq!(last.validation, "pc/authority-required");
    let record = record_of(last);
    assert_eq!(
        record["authority"]["verdict"]["reason"],
        a::reason::BUDGET_EXHAUSTED
    );
    assert_eq!(c.escalations(), 1);
    // 新しい許可（1 日）。期限が過ぎたら拒否し、新しい問いにする。
    human_grant(&c, scope, 5, 1, None);
    c.env.advance(2 * 86_400);
    c.wake();
    assert_eq!(c.roots(), 1, "the grant expired");
    let record = record_of(&c.decisions()[0]);
    assert_eq!(record["authority"]["verdict"]["reason"], a::reason::EXPIRED);
    assert_eq!(c.escalations(), 2, "a new generation is a new question");
    raw_row(
        json!({"test": "authority_ledger_budget_and_expiry_stop_launch", "roots": c.roots(), "escalations": c.escalations()}),
    );
}

/// 他の途中目標の許可では起票しない（範囲は広げない）。
#[tokio::test]
async fn authority_ledger_scope_is_not_widened() {
    let mut c = enabled_without_grant(launch);
    let other = GrantScope {
        kind: ScopeKind::Task,
        reference: "01M6JW1EFW86JH3VKZ274J9GT5".into(),
    };
    let project = c.p.id.to_string();
    let now = c.env.now();
    // 案件に無い task・途中目標の許可は作れない。
    let refused = c
        .store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: "g-other".into(),
                    project_id: project.clone(),
                    scope: other,
                    action_class: a::ActionClass::LaunchRoot,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:test".into(),
                    question_key: None,
                    note: "x".into(),
                    expires_at: "2027-01-01T00:00:00Z".into(),
                    budget: 1,
                },
                now,
            )
        })
        .unwrap();
    assert_eq!(refused.unwrap_err().code, "authority_scope_unknown");
    // launch の outcome_ids が o1 の 1 つでも、root の範囲は outcome o1 で判定する。
    c.wake();
    assert_eq!(c.roots(), 0);
    let record = record_of(&c.decisions()[0]);
    assert_eq!(record["authority"]["scope"]["kind"], "outcome");
    assert_eq!(record["authority"]["scope"]["ref"], "o1");
}

/// TS が同じ問いをもう一度出しても、人の答え（有効な許可）があれば人に聞かず、根拠を記録する。
#[tokio::test]
async fn authority_ledger_answered_question_is_not_asked_again() {
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        escalate(s, "deploy-window")
    });
    let g = human_grant(&c, project_scope(&c), 3, 30, Some("deploy-window"));
    c.wake();
    assert_eq!(c.escalations(), 0, "no notice: the person already answered");
    let last = &c.decisions()[0];
    assert_eq!(last.validation, "pc/authority-answered");
    let record = record_of(last);
    assert_eq!(record["authority"]["answered"]["grant_id"], g.as_str());
    assert_eq!(
        record["authority"]["answered"]["source"]["kind"],
        "human_operation"
    );
    let project = c.p.id.to_string();
    assert_eq!(
        c.store
            .project_controller_state(&project)
            .unwrap()
            .unwrap()
            .phase,
        ControllerPhase::Waiting
    );
    let audit = c.store.authority_audit(&project, 10).unwrap();
    let row = audit
        .iter()
        .find(|r| r.reason == "question_answered")
        .unwrap();
    assert_eq!(
        row.human_decision_ref.as_deref(),
        Some(format!("authority_grant:{g}").as_str())
    );
    // 許可を取り消すと、答えは無くなり人に聞く。
    let now = c.env.now();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_grant_revoke_tx(tx, &project, &g, "human:test", now)
        })
        .unwrap();
    c.wake();
    assert_eq!(c.escalations(), 1);
    raw_row(
        json!({"test": "authority_ledger_answered_question_is_not_asked_again", "asked_while_answered": 0, "asked_after_revoke": c.escalations()}),
    );
}

/// shadow は許可が無くても判定を記録するだけで、人に聞かず、予算も消費しない。
#[tokio::test]
async fn authority_ledger_shadow_only_records_the_verdict() {
    let mut c = case(Some(ControllerMode::Shadow), launch);
    c.wake();
    assert_eq!(c.escalations(), 0);
    let d = &c.decisions()[0];
    assert_eq!(d.effect_state, EffectState::Suppressed);
    let record = record_of(d);
    assert_eq!(record["authority"]["verdict"]["allow"], false);
    assert_eq!(
        record["authority"]["verdict"]["reason"],
        a::reason::NO_GRANT
    );
    assert!(
        c.store
            .authority_audit(&c.p.id.to_string(), 10)
            .unwrap()
            .is_empty()
    );
    // TS は snapshot で判定の結果を読める。根拠の本文は入らない。
    let snap = c.seen.lock().unwrap().last().cloned().unwrap();
    let section = snap.authority.expect("authority section");
    assert_eq!(section["launch_root"]["allow"], false);
    assert_eq!(section["grants"], json!([]));
}

/// 判断の後に許可が取り消されたら、効果の tx が拒否する（判断の時の判定を信用しない）。
#[tokio::test]
async fn authority_ledger_revoked_after_decision_creates_no_root() {
    let mut c = enabled_without_grant(launch);
    let g = human_grant(&c, project_scope(&c), 3, 30, None);
    let project = c.p.id.to_string();
    let store = c.store.clone();
    c.d.set_project_controller_hook(Box::new(move |b| {
        if b == Boundary::AfterCommit {
            let now = OffsetDateTime::now_utc();
            store
                .write_tx::<_, task_core::StoreError, _>(|tx| {
                    SqliteStore::authority_grant_revoke_tx(tx, &project, &g, "human:test", now)
                })
                .unwrap();
        }
    }));
    c.wake();
    // 効果は次の tick の pending の段で実行される（境界の後）。
    for _ in 0..3 {
        c.env.advance(1);
        tick(&mut c.d);
    }
    assert_eq!(c.roots(), 0, "the effect's transaction re-checks the grant");
    let all = c.decisions();
    let d = all
        .iter()
        .find(|d| d.validation == "pc/outcome-open")
        .expect("the launch decision");
    assert_eq!(d.effect_state, EffectState::Failed);
    assert_eq!(
        d.effect_ref.as_deref(),
        Some(format!("authority:{}", a::reason::REVOKED).as_str())
    );
    raw_row(
        json!({"test": "authority_ledger_revoked_after_decision_creates_no_root", "roots": 0, "effect_ref": d.effect_ref}),
    );
}
