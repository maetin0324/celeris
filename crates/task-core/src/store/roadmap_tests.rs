use rusqlite::Connection;
use time::OffsetDateTime;

use crate::org::{Project, ProjectId, ProjectStatus};
use crate::project_brief::{ProjectBrief, ProjectBriefInput};
use crate::roadmap::{OutcomeStatus, RoadmapOutcomeSpec, RoadmapProposalInput, RoadmapStatus};

use super::*;
use crate::TaskStore;

fn project(store: &SqliteStore) -> Project {
    let now = OffsetDateTime::now_utc();
    let p = Project {
        id: ProjectId::new(),
        title: "案件".into(),
        request: "依頼".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        auto_integrate: false,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&p).unwrap();
    p
}

fn spec(id: &str) -> RoadmapOutcomeSpec {
    RoadmapOutcomeSpec {
        id: id.into(),
        title: format!("{id} の状態"),
        acceptance: vec![format!("{id} を確かめる")],
        priority: 1,
        depends_on: vec![],
        dropped: false,
        checks: Vec::new(),
    }
}

fn propose(
    store: &SqliteStore,
    p: &str,
    base: Option<i64>,
    outcomes: Vec<RoadmapOutcomeSpec>,
) -> Result<RoadmapVersion, RoadmapWriteError> {
    let input = RoadmapProposalInput {
        base_version: base,
        change_reason: "方針".into(),
        outcomes,
    };
    store.write_tx(|tx| {
        SqliteStore::roadmap_propose_tx(tx, p, &input, "sha", "cos:t", OffsetDateTime::now_utc())
    })
}

fn decide(
    store: &SqliteStore,
    p: &str,
    v: i64,
    approve: bool,
) -> Result<RoadmapVersion, RoadmapWriteError> {
    store.write_tx(|tx| {
        SqliteStore::roadmap_decide_tx(
            tx,
            p,
            v,
            approve,
            Some("ok"),
            "human:r",
            OffsetDateTime::now_utc(),
        )
    })
}

#[test]
fn project_roadmap_store_versions_advance_only_by_decision() {
    let store = SqliteStore::open_in_memory().unwrap();
    let p = project(&store).id.to_string();
    let v1 = propose(&store, &p, None, vec![spec("o1")]).unwrap();
    assert_eq!((v1.version, v1.status), (1, RoadmapStatus::Proposed));
    assert!(
        store.roadmap_approved(&p).unwrap().is_none(),
        "a proposal is not approved"
    );
    decide(&store, &p, 1, true).unwrap();
    // 2 つ目の提案も今の版を変えない。土台が違う提案は Stale。
    assert!(matches!(
        propose(&store, &p, None, vec![spec("o1"), spec("o2")]),
        Err(RoadmapWriteError::Stale { current: Some(1) })
    ));
    let v2 = propose(&store, &p, Some(1), vec![spec("o1"), spec("o2")]).unwrap();
    let v3 = propose(&store, &p, Some(1), vec![spec("o1"), spec("o3")]).unwrap();
    assert_eq!(store.roadmap_approved(&p).unwrap().unwrap().version, 1);
    decide(&store, &p, v2.version, true).unwrap();
    let all = store.roadmap_versions(&p).unwrap();
    assert_eq!(all[0].status, RoadmapStatus::Superseded);
    assert!(all[0].superseded_at.is_some());
    assert_eq!(all[1].status, RoadmapStatus::Approved);
    // v3 の土台（1）は今の approved（2）と違うので承認できない。却下はできる。
    assert!(matches!(
        decide(&store, &p, v3.version, true),
        Err(RoadmapWriteError::Stale { current: Some(2) })
    ));
    let rejected = decide(&store, &p, v3.version, false).unwrap();
    assert_eq!(rejected.status, RoadmapStatus::Rejected);
    assert!(matches!(
        decide(&store, &p, v3.version, true),
        Err(RoadmapWriteError::NotProposed("rejected"))
    ));
}

#[test]
fn project_roadmap_store_rows_are_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let store = SqliteStore::open(&path).unwrap();
    let p = project(&store).id.to_string();
    propose(&store, &p, None, vec![spec("o1")]).unwrap();
    decide(&store, &p, 1, true).unwrap();
    store
        .write_tx(|tx| {
            SqliteStore::roadmap_outcome_event_tx(
                tx,
                &NewOutcomeEvent {
                    project_id: &p,
                    outcome_id: "o1",
                    status: Some(OutcomeStatus::InProgress),
                    root_task_id: None,
                    evidence: &[],
                    reason: "",
                    actor: "human:r",
                },
                OffsetDateTime::now_utc(),
            )
        })
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    for sql in [
        "DELETE FROM project_roadmap_versions",
        "UPDATE project_roadmap_versions SET outcomes_json = '[]'",
        "UPDATE project_roadmap_versions SET status = 'proposed'",
        "DELETE FROM project_roadmap_outcome_events",
        "UPDATE project_roadmap_outcome_events SET status = 'reached'",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql} must be refused");
    }
}

#[test]
fn project_roadmap_store_outcome_events_need_an_approved_live_outcome() {
    let store = SqliteStore::open_in_memory().unwrap();
    let p = project(&store).id.to_string();
    let ev = |outcome: &str, status: Option<OutcomeStatus>, root: Option<&str>| {
        store.write_tx(|tx| {
            SqliteStore::roadmap_outcome_event_tx(
                tx,
                &NewOutcomeEvent {
                    project_id: &p,
                    outcome_id: outcome,
                    status,
                    root_task_id: root,
                    evidence: &[],
                    reason: "",
                    actor: "human:r",
                },
                OffsetDateTime::now_utc(),
            )
        })
    };
    let code = |r: Result<Option<i64>, RoadmapWriteError>| match r {
        Err(RoadmapWriteError::Invalid(i)) => i.code,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        code(ev("o1", Some(OutcomeStatus::InProgress), None)),
        "roadmap_not_approved"
    );
    let mut o2 = spec("o2");
    o2.dropped = true;
    propose(&store, &p, None, vec![spec("o1"), o2]).unwrap();
    decide(&store, &p, 1, true).unwrap();
    assert_eq!(
        code(ev("zz", Some(OutcomeStatus::InProgress), None)),
        "roadmap_outcome_unknown"
    );
    assert_eq!(
        code(ev("o2", Some(OutcomeStatus::InProgress), None)),
        "roadmap_outcome_dropped"
    );
    assert_eq!(
        code(ev("o1", Some(OutcomeStatus::Dropped), None)),
        "roadmap_drop_needs_version"
    );
    assert!(ev("o1", None, Some("r1")).unwrap().is_some());
    assert!(
        ev("o1", None, Some("r1")).unwrap().is_none(),
        "the same link is recorded once"
    );
    assert_eq!(store.roadmap_outcome_events(&p).unwrap().len(), 1);
}

/// migration 0113 の後方互換: 0112 までの DB（Roadmap の表なし）から起動し、既存の行（案件・brief）を保つ。
/// Roadmap の行は作らず、案件は今までどおり request の block を読む（approved なし）。
#[test]
fn project_roadmap_migration_0113_starts_from_a_0112_db_and_keeps_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let kept = {
        let store = SqliteStore::open(&path).unwrap();
        let p = project(&store);
        let brief = ProjectBrief::from_input(
            p.id,
            ProjectBriefInput {
                objective: "目的".into(),
                state: "途中".into(),
                open_issues: vec![],
                next_actions: vec![],
                waiting_on: vec![],
                evidence_refs: vec![],
            },
            "cos:t",
            OffsetDateTime::now_utc().replace_nanosecond(0).unwrap(),
        );
        store.project_brief_put(&brief).unwrap();
        p
    };
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TRIGGER task_roadmap_specs_capture; DROP TABLE task_roadmap_specs; \
             DROP TRIGGER script_compare_log_keep_max; DROP TRIGGER project_wakes_keep_max; \
             DROP TRIGGER project_controller_decisions_keep_max; \
             DROP TABLE authority_proposals; DROP TABLE authority_audit; DROP TABLE authority_grants; \
             DROP TABLE project_roadmap_outcome_events; DROP TABLE project_roadmap_versions; \
             DELETE FROM schema_migrations WHERE version IN (113, 114, 115, 117, 118, 119);",
        )
        .unwrap();
        let max: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(max, 112, "the fixture is a 0112 database");
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
    assert_eq!(crate::SCHEMA_VERSION, 119);
    let p = store.project_get(kept.id).unwrap().unwrap();
    assert_eq!(p.request, "依頼");
    assert_eq!(
        store.project_brief_get(kept.id).unwrap().unwrap().state,
        "途中"
    );
    let pid = kept.id.to_string();
    assert!(store.roadmap_approved(&pid).unwrap().is_none());
    assert!(store.roadmap_versions(&pid).unwrap().is_empty());
    // 開き直しても行は 1 回だけ（冪等）。
    propose(&store, &pid, None, vec![spec("o1")]).unwrap();
    drop(store);
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.roadmap_versions(&pid).unwrap().len(), 1);
}
