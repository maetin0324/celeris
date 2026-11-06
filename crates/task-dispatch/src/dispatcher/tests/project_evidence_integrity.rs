//! N5: 人・CoS の root、旧証跡、CAS、再達成。
use super::*;
use task_ops::roadmap::{OutcomeStatusInput, check_status, record_status_tx};

fn input(t: &Task) -> OutcomeStatusInput {
    OutcomeStatusInput {
        status: OutcomeStatus::Reached,
        evidence_refs: vec![format!("task:{}", t.id)],
        reason: "N5".into(),
    }
}

#[test]
fn project_revocation_n5_human_cos_checks_only_revision_fails_closed() {
    for actor in ["human:test", "cos:test"] {
        let c = lying_case(&["o1"]);
        c.approve_specs(vec![spec("o1", &acc("o1"), &["true"])]);
        let r = c.root_by(c.p.id, "o1", false, actor);
        // 調査案件: repo 無しでも成果物と review を要求する。
        assert!(check_status(&c.store, c.p.id, "o1", &input(&r), c.env.now()).is_err());
        c.review_done(&r, true);
        c.deliver(&r);
        let checked = check_status(&c.store, c.p.id, "o1", &input(&r), c.env.now()).unwrap();
        c.store
            .write_tx(|tx| {
                record_status_tx(tx, &c.pid(), "o1", &checked, "human:test", c.env.now())
            })
            .unwrap();
        let reopened = SqliteStore::open(&c.env.db()).unwrap();
        assert!(check_status(&reopened, c.p.id, "o1", &input(&r), c.env.now()).is_ok());
        let mut title_only = spec("o1", &acc("o1"), &["true"]);
        title_only.title = "題名だけを変更".into();
        c.approve_specs(vec![title_only]);
        assert!(check_status(&c.store, c.p.id, "o1", &input(&r), c.env.now()).is_ok());
        let conn = rusqlite::Connection::open(c.env.db()).unwrap();
        assert!(
            conn.execute(
                "UPDATE task_roadmap_specs SET version = 999 WHERE task_id = ?1",
                [r.id.to_string()]
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "DELETE FROM task_roadmap_specs WHERE task_id = ?1",
                [r.id.to_string()]
            )
            .is_err()
        );
        c.approve_specs(vec![spec("o1", &acc("o1"), &["printf fresh"])]);
        assert!(
            check_status(&c.store, c.p.id, "o1", &input(&r), c.env.now()).is_err(),
            "{actor}: old root accepted checks-only revision"
        );
        assert!(!task_core::project_proposal::root_satisfies(
            &c.snapshot().roots[0],
            &c.snapshot().outcomes[0]
        ));
        let mut fresh = r.clone();
        fresh.id = TaskId::new();
        fresh.acceptance[1].check = Check::Command {
            cmd: "printf fresh".into(),
            expect_exit: 0,
        };
        c.store.create_task_by(&fresh, actor, vec![]).unwrap();
        c.review_done(&fresh, true);
        c.deliver(&fresh);
        let checked = check_status(&c.store, c.p.id, "o1", &input(&fresh), c.env.now()).unwrap();
        let record = || {
            c.store
                .write_tx(|tx| {
                    record_status_tx(tx, &c.pid(), "o1", &checked, "human:test", c.env.now())
                })
                .unwrap()
        };
        record();
        assert_eq!(c.roadmap_reached("o1"), 2);
        let mut note = input(&fresh);
        note.reason = "人の補足を追記".into();
        let checked = check_status(&c.store, c.p.id, "o1", &note, c.env.now()).unwrap();
        c.store
            .write_tx(|tx| {
                record_status_tx(tx, &c.pid(), "o1", &checked, "human:test", c.env.now())
            })
            .unwrap();
        assert_eq!(c.roadmap_reached("o1"), 3);
        assert_eq!(
            c.store
                .roadmap_outcome_events(&c.pid())
                .unwrap()
                .last()
                .unwrap()
                .reason,
            note.reason
        );
        let view = task_ops::roadmap::view(&c.store, c.p.id, None)
            .unwrap()
            .unwrap();
        assert_eq!(view.outcomes[0].evidence, input(&fresh).evidence_refs);
    }
}

#[test]
fn project_revocation_n5_legacy_root_and_missing_command_fail_closed() {
    let c = lying_case(&["o1"]);
    c.approve_specs(vec![spec("o1", &acc("o1"), &["true"])]);
    let legacy = c.root(c.p.id, "o1", false);
    c.review_done(&legacy, true);
    c.deliver(&legacy);
    let events_before = serde_json::to_value(c.store.events_for(legacy.id).unwrap()).unwrap();
    // 0117 の DB を再現する。既存 task・events・承認版は保存し、0118 の追加物だけを除く。
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .execute_batch(
            "DROP TRIGGER task_roadmap_specs_capture; DROP TABLE task_roadmap_specs;
         DROP TRIGGER script_compare_log_keep_max; DROP TRIGGER project_wakes_keep_max;
         DROP TRIGGER project_controller_decisions_keep_max;
         DELETE FROM schema_migrations WHERE version = 118;",
        )
        .unwrap();
    let reopened = SqliteStore::open(&c.env.db()).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 118);
    assert_eq!(
        serde_json::to_value(reopened.events_for(legacy.id).unwrap()).unwrap(),
        events_before
    );
    assert_eq!(
        reopened
            .roadmap_approved(&c.pid())
            .unwrap()
            .unwrap()
            .version,
        1
    );
    assert!(
        reopened.task_roadmap_specs(&c.pid()).unwrap().is_empty(),
        "old roots must not be backfilled"
    );
    assert!(check_status(&c.store, c.p.id, "o1", &input(&legacy), c.env.now()).is_err());
    c.approve_specs(vec![spec("o1", &acc("o1"), &["test -f new"])]);
    let missing = c.root(c.p.id, "o1", false);
    c.review_done(&missing, true);
    c.deliver(&missing);
    assert!(check_status(&c.store, c.p.id, "o1", &input(&missing), c.env.now()).is_err());
}

#[test]
fn project_revocation_n5_checked_status_cas_rejects_reopen_and_revision() {
    for revise in [false, true] {
        let c = lying_case(&["o1"]);
        c.approve_specs(vec![spec("o1", &acc("o1"), &["true"])]);
        let r = c.root(c.p.id, "o1", false);
        c.review_done(&r, true);
        c.deliver(&r);
        let checked = check_status(&c.store, c.p.id, "o1", &input(&r), c.env.now()).unwrap();
        if revise {
            c.approve_specs(vec![spec("o1", &acc("o1"), &["false"])]);
        } else {
            c.reopen(&r);
        }
        assert!(
            c.store
                .write_tx(|tx| record_status_tx(
                    tx,
                    &c.pid(),
                    "o1",
                    &checked,
                    "human:test",
                    c.env.now()
                ))
                .is_err()
        );
        assert_eq!(c.roadmap_reached("o1"), 0);
    }
}

#[test]
fn project_revocation_n5_reachievement_appends_fresh_evidence() {
    let (c, r1) = controller_achieved_case();
    let new = spec("o1", &acc("o1"), &["test -f out/o1-v2.txt"]);
    assert_human_change_reopens_and_reachieves(c, &r1, new, "test -f out/o1-v2.txt");
}

#[test]
fn project_revocation_n5_human_progress_links_and_reasons_are_append_only() {
    let c = lying_case(&["o1"]);
    c.approve_specs(vec![spec("o1", &acc("o1"), &[])]);
    for reason in ["最初の root", "次の root"] {
        let r = c.root(c.p.id, "o1", false);
        let input = OutcomeStatusInput {
            status: OutcomeStatus::InProgress,
            evidence_refs: vec![format!("task:{}", r.id)],
            reason: reason.into(),
        };
        let checked = check_status(&c.store, c.p.id, "o1", &input, c.env.now()).unwrap();
        c.store
            .write_tx(|tx| {
                record_status_tx(tx, &c.pid(), "o1", &checked, "human:test", c.env.now())
            })
            .unwrap();
    }
    let events = c.store.roadmap_outcome_events(&c.pid()).unwrap();
    assert_eq!(events.iter().filter(|e| e.kind == "root_linked").count(), 2);
    let reasons: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "status")
        .map(|e| e.reason.as_str())
        .collect();
    assert_eq!(reasons, vec!["最初の root", "次の root"]);
}

#[test]
fn project_revocation_n5_many_outcomes_do_not_inflate_unrelated_root_evidence() {
    let c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", false);
    let specs = (1..=64)
        .map(|i| {
            let id = format!("o{i}");
            spec(&id, &acc(&id), &[])
        })
        .collect();
    c.approve_specs(specs);
    let snapshot = c.snapshot();
    let row = snapshot
        .roots
        .iter()
        .find(|row| row.id == r.id.to_string())
        .unwrap();
    assert_eq!(row.stale_outcome_ids, vec!["o1"]);
    let bytes = serde_json::to_vec(row).unwrap().len();
    let mut without = row.clone();
    without.stale_outcome_ids.clear();
    assert_eq!(
        bytes - serde_json::to_vec(&without).unwrap().len(),
        4,
        "64 outcomes add only the relevant ID to this root's JSON"
    );
}
