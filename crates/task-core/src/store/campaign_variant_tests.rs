//! campaign 識別の variant: 保存・再読・再送（冪等照会）・祖先鎖の整合・失敗時に何も残さない。
use time::OffsetDateTime;

use super::tests::sample_task;
use super::*;
use crate::decision_log::*;
use crate::model::{Status, Task};

const H1: &str = "b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44";
const H2: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn now() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn plain(arm: &str) -> CampaignIdentity {
    CampaignIdentity {
        campaign_id: "camp".into(),
        case_id: Some("case-1".into()),
        arm: Some(arm.into()),
        repetition: Some(1),
        manifest_hash: Some(H1.into()),
        routing_setting: Some("fixed".into()),
        variant: None,
        variant_org: None,
    }
}

fn with_variant(arm: &str, variant_id: &str, content: &str) -> CampaignIdentity {
    CampaignIdentity {
        variant: Some(CampaignVariantRef {
            comparison_id: "M1".into(),
            id: variant_id.into(),
            content_sha256: content.into(),
            selection_sha256: H1.into(),
        }),
        variant_org: Some(CampaignOrgPin {
            nodes: vec!["cos".into(), "coding".into()],
            coding_node: "coding".into(),
            effective_profile_sha256: H2.into(),
        }),
        ..plain(arm)
    }
}

fn task(store: &SqliteStore, parent: Option<&Task>) -> Task {
    let mut t = sample_task(Status::Draft);
    t.parent_id = parent.map(|p| p.id);
    store.create_task(&t, vec![]).unwrap();
    t
}

fn invalid(r: Result<impl std::fmt::Debug, StoreError>) -> String {
    match r {
        Err(StoreError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn variant_roundtrips_and_plain_rows_keep_meaning() {
    let s = SqliteStore::open_in_memory().unwrap();
    let a = task(&s, None);
    let b = task(&s, None);
    let v = with_variant("spec-on", "M1-spec-on", H1);
    s.task_campaign_set(&a.id.to_string(), &v, now()).unwrap();
    s.task_campaign_set(&b.id.to_string(), &plain("x"), now())
        .unwrap();
    // 再読（再 open 相当: 別 connection でなく保存列から組み直す）で同じ hash・構成。
    assert_eq!(
        s.task_campaign_own(&a.id.to_string()).unwrap(),
        Some(v.clone())
    );
    assert_eq!(s.task_campaign_resolve(&a.id.to_string()).unwrap(), Some(v));
    // variant 無しの行は従来どおり variant 無し（列は NULL）。
    let pb = s.task_campaign_own(&b.id.to_string()).unwrap().unwrap();
    assert_eq!(pb, plain("x"));
    let nulls: (Option<String>, Option<String>) = s
        .lock()
        .unwrap()
        .query_row(
            "SELECT variant_json, variant_org_json FROM task_campaigns WHERE task_id = ?1",
            [b.id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(nulls, (None, None));
}

#[test]
fn variant_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.sqlite3");
    let v = with_variant("spec-off", "M1-spec-off", H2);
    let id = {
        let s = SqliteStore::open(&path).unwrap();
        let t = task(&s, None);
        s.task_campaign_set(&t.id.to_string(), &v, now()).unwrap();
        t.id.to_string()
    };
    let s = SqliteStore::open(&path).unwrap();
    assert_eq!(s.task_campaign_resolve(&id).unwrap(), Some(v.clone()));
    let conn = s.lock().unwrap();
    assert_eq!(
        SqliteStore::task_campaign_resend_tx(&conn, &v).unwrap(),
        CampaignResend::Same(id)
    );
}

#[test]
fn resend_same_is_same_and_changed_or_dropped_variant_conflicts() {
    let s = SqliteStore::open_in_memory().unwrap();
    let t = task(&s, None);
    let tid = t.id.to_string();
    let v = with_variant("spec-on", "M1-spec-on", H1);
    s.task_campaign_set(&tid, &v, now()).unwrap();
    assert_eq!(
        s.task_campaign_resend(&v).unwrap(),
        CampaignResend::Same(tid.clone())
    );
    // variant の変更（内容 hash）、構成 pin の変更、variant の脱落はどれも同じ試行の別内容 → Conflict。
    let mut changed = v.clone();
    changed.variant.as_mut().unwrap().content_sha256 = H2.into();
    let mut org_changed = v.clone();
    org_changed
        .variant_org
        .as_mut()
        .unwrap()
        .effective_profile_sha256 = H1.into();
    let dropped = plain("spec-on");
    for other in [changed, org_changed, dropped] {
        assert_eq!(
            s.task_campaign_resend(&other).unwrap(),
            CampaignResend::Conflict(tid.clone()),
            "{other:?}"
        );
        // 同じ task への別識別の保存も黙って置き換えない。
        invalid(s.task_campaign_set(&tid, &other, now()));
    }
    assert_eq!(s.task_campaign_own(&tid).unwrap(), Some(v.clone()));
    // 同じ識別の再設定は何も変えない。
    s.task_campaign_set(&tid, &v, now()).unwrap();
}

#[test]
fn half_variant_is_rejected() {
    let s = SqliteStore::open_in_memory().unwrap();
    let t = task(&s, None);
    let mut half = with_variant("spec-on", "M1-spec-on", H1);
    half.variant_org = None;
    let m = invalid(s.task_campaign_set(&t.id.to_string(), &half, now()));
    assert!(m.contains("together"), "{m}");
    let mut bad = with_variant("spec-on", "M1-spec-on", H1);
    bad.variant_org.as_mut().unwrap().coding_node = "nope".into();
    invalid(s.task_campaign_set(&t.id.to_string(), &bad, now()));
    assert_eq!(s.task_campaign_own(&t.id.to_string()).unwrap(), None);
}

#[test]
fn descendants_inherit_and_mismatch_with_root_is_rejected() {
    let s = SqliteStore::open_in_memory().unwrap();
    let root = task(&s, None);
    let child = task(&s, Some(&root));
    let grand = task(&s, Some(&child));
    let v = with_variant("spec-on", "M1-spec-on", H1);
    s.task_campaign_set(&root.id.to_string(), &v, now())
        .unwrap();
    // 子孫は行を持たずに root の識別（variant 込み）を継ぐ。
    assert_eq!(s.task_campaign_own(&grand.id.to_string()).unwrap(), None);
    assert_eq!(
        s.task_campaign_resolve(&grand.id.to_string()).unwrap(),
        Some(v.clone())
    );
    // 子孫が root と違う識別を持つことはできない（variant だけ違っても）。
    let other = with_variant("spec-on", "M1-spec-off", H2);
    let m = invalid(s.task_campaign_set(&grand.id.to_string(), &other, now()));
    assert!(m.contains(&root.id.to_string()), "{m}");
    invalid(s.task_campaign_set(&child.id.to_string(), &plain("spec-on"), now()));
    // 同じ識別の own は許す（矛盾ではない）。
    s.task_campaign_set(&child.id.to_string(), &v, now())
        .unwrap();
    assert_eq!(
        s.task_campaign_resolve(&grand.id.to_string()).unwrap(),
        Some(v)
    );
}

#[test]
fn root_cannot_be_set_against_existing_descendant_identity() {
    let s = SqliteStore::open_in_memory().unwrap();
    let root = task(&s, None);
    let child = task(&s, Some(&root));
    let grand = task(&s, Some(&child));
    s.task_campaign_set(
        &grand.id.to_string(),
        &with_variant("a", "M1-spec-on", H1),
        now(),
    )
    .unwrap();
    let m = invalid(s.task_campaign_set(
        &root.id.to_string(),
        &with_variant("a", "M1-spec-off", H2),
        now(),
    ));
    assert!(m.contains(&grand.id.to_string()), "{m}");
    assert_eq!(s.task_campaign_own(&root.id.to_string()).unwrap(), None);
}

#[test]
fn stored_ancestor_mismatch_is_not_hidden_by_nearest() {
    let s = SqliteStore::open_in_memory().unwrap();
    let root = task(&s, None);
    let child = task(&s, Some(&root));
    let grand = task(&s, Some(&child));
    s.task_campaign_set(
        &root.id.to_string(),
        &with_variant("a", "M1-spec-on", H1),
        now(),
    )
    .unwrap();
    // 検査を通らずに入った矛盾（旧 binary・手書き）を直接作る。
    s.lock()
        .unwrap()
        .execute(
            "INSERT INTO task_campaigns (task_id, campaign_id, created_at) VALUES (?1, 'camp', 'x')",
            [child.id.to_string()],
        )
        .unwrap();
    let m = invalid(s.task_campaign_resolve(&grand.id.to_string()));
    assert!(
        m.contains(&root.id.to_string()) && m.contains(&child.id.to_string()),
        "{m}"
    );
    invalid(s.task_campaign_resolve(&child.id.to_string()));
    // root 自身は鎖の上に矛盾が無いので読める。
    assert!(
        s.task_campaign_resolve(&root.id.to_string())
            .unwrap()
            .is_some()
    );
}

#[test]
fn cycle_and_depth_limit_are_explicit_errors() {
    let s = SqliteStore::open_in_memory().unwrap();
    let a = task(&s, None);
    let b = task(&s, Some(&a));
    s.lock()
        .unwrap()
        .execute(
            "UPDATE tasks SET parent_id = ?1 WHERE id = ?2",
            [b.id.to_string(), a.id.to_string()],
        )
        .unwrap();
    let m = invalid(s.task_campaign_resolve(&b.id.to_string()));
    assert!(m.contains("cycle"), "{m}");
    invalid(s.task_campaign_set(&b.id.to_string(), &plain("x"), now()));

    let mut prev = task(&s, None);
    for _ in 0..super::decision_log::CAMPAIGN_ANCESTOR_LIMIT {
        prev = task(&s, Some(&prev));
    }
    let m = invalid(s.task_campaign_resolve(&prev.id.to_string()));
    assert!(m.contains("exceeds"), "{m}");
}

#[test]
fn invalid_stored_values_are_explicit_errors() {
    let s = SqliteStore::open_in_memory().unwrap();
    for (variant, org) in [
        (Some("{not json"), None),
        (
            Some(
                r#"{"comparison_id":"M1","id":"x","content_sha256":"zz","selection_sha256":"zz"}"#,
            ),
            Some(r#"{"nodes":["a"],"coding_node":"a","effective_profile_sha256":"zz"}"#),
        ),
        (
            None,
            Some(
                r#"{"nodes":["a"],"coding_node":"a","effective_profile_sha256":"b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44"}"#,
            ),
        ),
    ] {
        let t = task(&s, None);
        s.lock()
            .unwrap()
            .execute(
                "INSERT INTO task_campaigns (task_id, campaign_id, variant_json, variant_org_json, created_at) \
                 VALUES (?1, 'camp', ?2, ?3, 'x')",
                rusqlite::params![t.id.to_string(), variant, org],
            )
            .unwrap();
        let m = invalid(s.task_campaign_own(&t.id.to_string()));
        assert!(m.contains("stored campaign identity is invalid"), "{m}");
        invalid(s.task_campaign_resolve(&t.id.to_string()));
    }
}

#[test]
fn failed_set_in_create_transaction_leaves_nothing() {
    let s = SqliteStore::open_in_memory().unwrap();
    let root = task(&s, None);
    let v = with_variant("spec-on", "M1-spec-on", H1);
    s.task_campaign_set(&root.id.to_string(), &v, now())
        .unwrap();
    let before = s.lock().unwrap().query_row(
        "SELECT (SELECT COUNT(*) FROM tasks), (SELECT COUNT(*) FROM task_campaigns), \
         (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM decision_log_runs)",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        },
    );
    // 子を root と違う識別で作ろうとして、識別の保存で失敗する。
    let mut child = sample_task(Status::Draft);
    child.parent_id = Some(root.id);
    let other = with_variant("spec-on", "M1-spec-off", H2);
    let r = s.create_task_with(&child, vec![], |tx| {
        SqliteStore::task_campaign_set_tx(tx, &child.id.to_string(), &other, now())
    });
    invalid(r);
    assert!(s.get(child.id).unwrap().is_none());
    assert!(s.events_for(child.id).unwrap().is_empty());
    assert_eq!(s.task_campaign_own(&child.id.to_string()).unwrap(), None);
    // 冪等照会にも残らない（試行の一致は root の行だけ）。
    let conn = s.lock().unwrap();
    let after = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM tasks), (SELECT COUNT(*) FROM task_campaigns), \
         (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM decision_log_runs)",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        },
    );
    assert_eq!(before.unwrap(), after.unwrap());
}

#[test]
fn decision_log_run_rows_carry_variant() {
    let s = SqliteStore::open_in_memory().unwrap();
    let mut p = DecisionLogProposal {
        run_id: "r1".into(),
        ..DecisionLogProposal::default()
    };
    p.apply_campaign(&with_variant("spec-on", "M1-spec-on", H1));
    s.decision_log_record_proposal(&p, now()).unwrap();
    let mut q = DecisionLogProposal {
        run_id: "r2".into(),
        ..DecisionLogProposal::default()
    };
    q.apply_campaign(&plain("spec-on"));
    s.decision_log_record_proposal(&q, now()).unwrap();
    let runs = s.decision_log_runs_list(Some("camp")).unwrap();
    let get = |id: &str| {
        runs.iter()
            .find(|r| r.proposal.run_id == id)
            .unwrap()
            .proposal
            .clone()
    };
    assert_eq!(get("r1").variant_id.as_deref(), Some("M1-spec-on"));
    assert_eq!(get("r1").variant_content_sha256.as_deref(), Some(H1));
    assert_eq!(get("r2").variant_id, None);
    assert_eq!(get("r2").variant_content_sha256, None);
}
