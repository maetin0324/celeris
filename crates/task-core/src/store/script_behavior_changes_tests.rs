//! ADR 2026-11-03-ts-intended-behavior-change D4: 承認は人だけ、取り消しは誰でも、記録は追記だけ。

use super::*;
use crate::script_behavior_change::BehaviorChangeInvalid;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

fn spec() -> BehaviorChangeSpec {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/behavior-changes/cos_digest/cos-digest-cap-run-reason.v1.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn v2(prev: &BehaviorChangeSpec) -> BehaviorChangeSpec {
    let mut s = prev.clone();
    s.version = 2;
    s.summary.push_str("（v2）");
    s.supersedes = Some(prev.sha256());
    s
}

#[test]
fn behavior_change_approve_needs_a_human_and_the_right_hash() {
    let store = SqliteStore::open_in_memory().unwrap();
    let s = spec();
    let sha = s.sha256();
    for actor in [
        "cos",
        "operator:claude-code",
        "system:script-guard",
        "human:",
        "mcp",
    ] {
        assert!(matches!(
            store.script_behavior_change_approve(&s, &sha, actor, "r", now()),
            Err(BehaviorChangeError::RequiresHuman(_))
        ));
    }
    assert!(matches!(
        store.script_behavior_change_approve(&s, &"0".repeat(64), "human:a", "r", now()),
        Err(BehaviorChangeError::HashMismatch { .. })
    ));
    let mut lane = s.clone();
    lane.domain = ScriptDomain::LaneRules;
    assert!(matches!(
        store.script_behavior_change_approve(&lane, &lane.sha256(), "human:a", "r", now()),
        Err(BehaviorChangeError::DomainUnsupported(_))
    ));
    let mut bad = s.clone();
    bad.edits.clear();
    assert!(matches!(
        store.script_behavior_change_approve(&bad, &bad.sha256(), "human:a", "r", now()),
        Err(BehaviorChangeError::Invalid(BehaviorChangeInvalid(_)))
    ));
    // どの拒否でも表も記録も変えない。
    assert!(store.script_behavior_changes().unwrap().is_empty());
    assert!(store.script_behavior_change_events().unwrap().is_empty());
    assert!(
        store
            .script_behavior_changes_approved(ScriptDomain::CosDigest)
            .unwrap()
            .is_empty()
    );

    let a = store
        .script_behavior_change_approve(&s, &sha, "human:a", "pilot", now())
        .unwrap();
    assert!(a.changed);
    assert_eq!(a.row.state, BehaviorChangeState::Approved);
    assert_eq!(a.row.approved_by, "human:a");
    let again = store
        .script_behavior_change_approve(&s, &sha, "human:b", "x", now())
        .unwrap();
    assert!(!again.changed);
    let approved = store
        .script_behavior_changes_approved(ScriptDomain::CosDigest)
        .unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].spec_sha256, sha);
    assert!(
        store
            .script_behavior_changes_approved(ScriptDomain::LaneRules)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.script_behavior_change_events().unwrap().len(), 1);
}

#[test]
fn behavior_change_versions_chain_and_never_rebind() {
    let store = SqliteStore::open_in_memory().unwrap();
    let s1 = spec();
    store
        .script_behavior_change_approve(&s1, &s1.sha256(), "human:a", "v1", now())
        .unwrap();
    // 同じ版に別の本文は結び付かない。
    let mut other = s1.clone();
    other.summary.push('x');
    assert!(matches!(
        store.script_behavior_change_approve(&other, &other.sha256(), "human:a", "r", now()),
        Err(BehaviorChangeError::VersionTaken { .. })
    ));
    // 次の版は今の承認を supersedes に書かないと通らない（null・別の hash のどちらも）。
    for wrong in [None, Some("a".repeat(64))] {
        let mut loose = v2(&s1);
        loose.supersedes = wrong;
        assert!(matches!(
            store.script_behavior_change_approve(&loose, &loose.sha256(), "human:a", "r", now()),
            Err(BehaviorChangeError::Invalid(_))
        ));
    }
    let s2 = v2(&s1);
    let a = store
        .script_behavior_change_approve(&s2, &s2.sha256(), "human:a", "v2", now())
        .unwrap();
    assert_eq!(a.superseded.as_deref(), Some(s1.sha256().as_str()));
    let rows = store.script_behavior_changes().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.version, r.state))
            .collect::<Vec<_>>(),
        vec![
            (1, BehaviorChangeState::Superseded),
            (2, BehaviorChangeState::Approved)
        ]
    );
    // 古い版を承認し直せない。
    assert!(matches!(
        store.script_behavior_change_approve(&s1, &s1.sha256(), "human:a", "r", now()),
        Err(BehaviorChangeError::Retired { .. })
    ));
    let events = store.script_behavior_change_events().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| (e.version, e.action))
            .collect::<Vec<_>>(),
        vec![
            (1, BehaviorChangeState::Approved),
            (1, BehaviorChangeState::Superseded),
            (2, BehaviorChangeState::Approved)
        ]
    );
}

#[test]
fn behavior_change_revoke_by_anyone_fails_closed() {
    let store = SqliteStore::open_in_memory().unwrap();
    let s = spec();
    let sha = s.sha256();
    assert!(matches!(
        store.script_behavior_change_revoke(&sha, "cos", "r", now()),
        Err(BehaviorChangeError::NotFound(_))
    ));
    store
        .script_behavior_change_approve(&s, &sha, "human:a", "r", now())
        .unwrap();
    assert!(
        store
            .script_behavior_change_revoke(&sha, "operator:claude-code", "bad", now())
            .unwrap()
    );
    assert!(
        !store
            .script_behavior_change_revoke(&sha, "cos", "again", now())
            .unwrap()
    );
    assert!(
        store
            .script_behavior_changes_approved(ScriptDomain::CosDigest)
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store.script_behavior_change_approve(&s, &sha, "human:a", "r", now()),
        Err(BehaviorChangeError::Retired {
            state: BehaviorChangeState::Revoked,
            ..
        })
    ));
    let events = store.script_behavior_change_events().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].actor, "operator:claude-code");
}

/// D1: 最初の版は `supersedes` が `null`。`id` に行が無いのに前の版を名乗る仕様は承認しない。
#[test]
fn behavior_change_first_version_must_not_name_a_previous_version() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut first = spec();
    first.supersedes = Some("a".repeat(64));
    let err = store
        .script_behavior_change_approve(&first, &first.sha256(), "human:a", "r", now())
        .unwrap_err();
    assert!(matches!(err, BehaviorChangeError::Invalid(_)), "{err}");
    assert!(store.script_behavior_changes().unwrap().is_empty());
    assert!(store.script_behavior_change_events().unwrap().is_empty());
}

/// D1: 取り消した版の次の版も、取り消した版の hash を `supersedes` に書く。`null` では鎖が切れるので通らない。
/// 取り消した版は `revoked` のまま（`superseded` にしない）。
#[test]
fn behavior_change_chain_continues_across_a_revoked_version() {
    let store = SqliteStore::open_in_memory().unwrap();
    let s1 = spec();
    let sha1 = s1.sha256();
    store
        .script_behavior_change_approve(&s1, &sha1, "human:a", "v1", now())
        .unwrap();
    assert!(
        store
            .script_behavior_change_revoke(&sha1, "cos", "やめる", now())
            .unwrap()
    );
    for wrong in [None, Some("a".repeat(64))] {
        let mut loose = v2(&s1);
        loose.supersedes = wrong;
        assert!(matches!(
            store.script_behavior_change_approve(&loose, &loose.sha256(), "human:a", "r", now()),
            Err(BehaviorChangeError::Invalid(_))
        ));
    }
    assert_eq!(store.script_behavior_changes().unwrap().len(), 1);
    let s2 = v2(&s1);
    let a = store
        .script_behavior_change_approve(&s2, &s2.sha256(), "human:a", "v2", now())
        .unwrap();
    assert_eq!(a.superseded, None);
    assert_eq!(
        store
            .script_behavior_changes()
            .unwrap()
            .iter()
            .map(|r| (r.version, r.state))
            .collect::<Vec<_>>(),
        vec![
            (1, BehaviorChangeState::Revoked),
            (2, BehaviorChangeState::Approved)
        ]
    );
    // v3 は v2 の hash を書く。v1 の hash では通らない（前の版は最大の `version`）。
    let mut stale = v2(&s2);
    stale.version = 3;
    stale.supersedes = Some(sha1);
    assert!(matches!(
        store.script_behavior_change_approve(&stale, &stale.sha256(), "human:a", "r", now()),
        Err(BehaviorChangeError::Invalid(_))
    ));
}

/// D4: 承認の記録は trigger が UPDATE・DELETE を拒む（`script_generation_events`・`script_compare_log` と同じ形）。
#[test]
fn behavior_change_events_append_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    let s = spec();
    store
        .script_behavior_change_approve(&s, &s.sha256(), "human:a", "r", now())
        .unwrap();
    {
        let conn = store.lock().unwrap();
        for sql in [
            "UPDATE script_behavior_change_events SET actor = 'human:b'",
            "DELETE FROM script_behavior_change_events",
        ] {
            let err = conn.execute(sql, []).unwrap_err().to_string();
            assert!(err.contains("append-only"), "{sql}: {err}");
        }
    }
    let events = store.script_behavior_change_events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, "human:a");
}
