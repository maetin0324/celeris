use std::sync::{Arc, Barrier};

use rusqlite::params;
use time::OffsetDateTime;

use super::*;
use crate::model::Status;
use crate::store::task_store::TaskStore;
use crate::store::tests::sample_task;

const SECRET: &str = "jev_api_key";
const FP1: &str = "fp-aaaa";
const FP2: &str = "fp-bbbb";

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn enter(store: &SqliteStore, wu: &str, task: TaskId) -> EnterWaitOutcome {
    store
        .credential_wait_enter(
            &CredentialWaitEnter {
                work_unit_id: wu,
                task_id: task,
                secret_id: SECRET,
                provider: "jev",
                purpose: "request classification",
            },
            now(),
        )
        .expect("enter")
}

fn task_in(store: &SqliteStore, status: Status) -> TaskId {
    let t = sample_task(status);
    store.insert(&t).expect("insert task");
    t.id
}

fn verified_wait(store: &SqliteStore, wu: &str, task: TaskId) {
    enter(store, wu, task);
    store
        .credential_record_secret_saved(SECRET, FP1, now())
        .expect("saved");
    assert!(
        store
            .credential_wait_mark_verified(wu, SECRET, FP1, now())
            .expect("verified")
    );
}

#[test]
fn migration_creates_tables_and_version_is_81() {
    let store = SqliteStore::open_in_memory().expect("open");
    assert_eq!(store.schema_version().expect("v"), crate::SCHEMA_VERSION);
    assert_eq!(crate::SCHEMA_VERSION, 96);
    let conn = store.lock().expect("lock");
    for table in ["credential_waits", "credential_wait_events"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |r| r.get(0),
            )
            .expect("master");
        assert_eq!(n, 1, "{table}");
    }
}

#[test]
fn existing_db_at_version_80_migrates_to_81() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("t.db");
    {
        let store = SqliteStore::open(&path).expect("open");
        let conn = store.lock().expect("lock");
        conn.execute_batch(
            "DROP TABLE credential_waits; DROP TABLE credential_wait_events; \
             DELETE FROM schema_migrations WHERE version = 81;",
        )
        .expect("rewind");
    }
    let store = SqliteStore::open(&path).expect("reopen migrates");
    assert_eq!(store.schema_version().expect("v"), crate::SCHEMA_VERSION);
    assert!(
        store
            .credential_waits_list_waiting()
            .expect("list")
            .is_empty()
    );
}

#[test]
fn schema_has_no_column_for_a_secret_value() {
    let store = SqliteStore::open_in_memory().expect("open");
    let conn = store.lock().expect("lock");
    for table in ["credential_waits", "credential_wait_events"] {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("cols")
            .map(|c| c.expect("col"))
            .collect();
        for c in cols {
            for bad in ["value", "token", "password", "api_key", "plaintext"] {
                assert!(!c.contains(bad), "{table}.{c}");
            }
        }
    }
}

#[test]
fn enter_wait_is_idempotent_and_aggregates_by_secret() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t1 = task_in(&store, Status::Blocked);
    let t2 = task_in(&store, Status::Blocked);
    let a = enter(&store, "wu-1", t1);
    assert!(a.created && a.first_for_key);
    assert_eq!(a.wait.state, CredentialWaitState::Waiting);
    assert_eq!(a.wait.aggregation_key, "credential:jev_api_key");
    let again = enter(&store, "wu-1", t1);
    assert!(!again.created && !again.first_for_key);
    assert_eq!(again.wait.id, a.wait.id);
    let b = enter(&store, "wu-2", t2);
    assert!(b.created && !b.first_for_key, "same secret → same notice");
    assert_eq!(b.wait.aggregation_key, a.wait.aggregation_key);
    assert_eq!(
        store.credential_waits_list_waiting().expect("list").len(),
        2
    );

    // 通知 item id は同じ集約キーの全 wait に付き、後から入る wait も引き継ぐ。
    store
        .credential_wait_set_notice_item(&a.wait.aggregation_key, "item-1", now())
        .expect("notice");
    let t3 = task_in(&store, Status::Blocked);
    let c = enter(&store, "wu-3", t3);
    assert_eq!(c.wait.notice_item_id.as_deref(), Some("item-1"));
    for w in store.credential_waits_list_waiting().expect("list") {
        assert_eq!(w.notice_item_id.as_deref(), Some("item-1"));
    }
}

#[test]
fn secret_saved_event_is_append_only_and_deduplicated() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    enter(&store, "wu-1", t);
    let need = store
        .credential_record_secret_saved(SECRET, FP1, now())
        .expect("saved");
    assert_eq!(need.len(), 1);
    assert_eq!(need[0].seen_fingerprint.as_deref(), Some(FP1));
    // 重複 event は何も足さず、検証対象も返さない。
    assert!(
        store
            .credential_record_secret_saved(SECRET, FP1, now())
            .expect("dup")
            .is_empty()
    );
    let n: i64 = store
        .lock()
        .expect("lock")
        .query_row("SELECT COUNT(*) FROM credential_wait_events", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(n, 1);
}

#[test]
fn claim_resume_succeeds_once_sequentially() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    verified_wait(&store, "wu-1", t);
    assert!(
        store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim")
    );
    assert!(
        !store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim 2")
    );
    // 重複 event が来ても再び検証対象にならず、再度 verified にもならない。
    assert!(
        store
            .credential_record_secret_saved(SECRET, FP1, now())
            .expect("dup")
            .is_empty()
    );
    assert!(
        !store
            .credential_wait_mark_verified("wu-1", SECRET, FP1, now())
            .expect("mark again")
    );
    let w = store
        .credential_wait_get("wu-1", SECRET)
        .expect("get")
        .expect("row");
    assert_eq!(w.state, CredentialWaitState::Resumed);
    assert!(w.resumed_at.is_some());
}

#[test]
fn claim_resume_succeeds_exactly_once_under_concurrency() {
    let store = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let t = task_in(&store, Status::Blocked);
    verified_wait(&store, "wu-1", t);
    let n = 8;
    let barrier = Arc::new(Barrier::new(n));
    let handles: Vec<_> = (0..n)
        .map(|_| {
            let s = Arc::clone(&store);
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                s.credential_wait_claim_resume("wu-1", SECRET, now())
                    .expect("claim")
            })
        })
        .collect();
    let wins = handles
        .into_iter()
        .map(|h| h.join().expect("join"))
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1);
}

#[test]
fn claim_resume_requires_verified() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    enter(&store, "wu-1", t);
    assert!(
        !store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("waiting")
    );
    store
        .credential_record_secret_saved(SECRET, FP1, now())
        .expect("saved");
    assert!(
        !store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("saved only")
    );
}

#[test]
fn cancelled_task_is_never_resumed() {
    let store = SqliteStore::open_in_memory().expect("open");
    // (a) cancel API で wait を閉じる。
    let t = task_in(&store, Status::Blocked);
    enter(&store, "wu-1", t);
    assert_eq!(store.credential_wait_cancel(t, now()).expect("cancel"), 1);
    store
        .credential_record_secret_saved(SECRET, FP1, now())
        .expect("saved");
    assert!(
        !store
            .credential_wait_mark_verified("wu-1", SECRET, FP1, now())
            .expect("verify cancelled")
    );
    assert!(
        !store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim")
    );
    assert_eq!(
        store
            .credential_wait_get("wu-1", SECRET)
            .expect("get")
            .expect("row")
            .state,
        CredentialWaitState::Cancelled
    );

    // (b) cancel API を通らず task だけ cancelled になっても、claim 時に tasks を見て再開しない。
    let t2 = task_in(&store, Status::Blocked);
    verified_wait(&store, "wu-2", t2);
    store
        .lock()
        .expect("lock")
        .execute(
            "UPDATE tasks SET status = 'cancelled' WHERE id = ?1",
            params![t2.to_string()],
        )
        .expect("cancel task");
    assert!(
        !store
            .credential_wait_claim_resume("wu-2", SECRET, now())
            .expect("claim 2")
    );
    assert_eq!(
        store
            .credential_wait_get("wu-2", SECRET)
            .expect("get")
            .expect("row")
            .state,
        CredentialWaitState::Cancelled
    );
}

#[test]
fn rejected_key_keeps_waiting_until_a_new_key_verifies() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    enter(&store, "wu-1", t);
    store
        .credential_record_secret_saved(SECRET, FP1, now())
        .expect("saved");
    assert!(
        store
            .credential_wait_mark_rejected(
                "wu-1",
                SECRET,
                FP1,
                CredentialVerifyKind::InvalidKey,
                now()
            )
            .expect("reject")
    );
    let w = store
        .credential_wait_get("wu-1", SECRET)
        .expect("get")
        .expect("row");
    assert_eq!(w.state, CredentialWaitState::Rejected);
    assert_eq!(w.last_verify_kind, Some(CredentialVerifyKind::InvalidKey));
    assert!(
        !store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim")
    );
    assert_eq!(
        store.credential_waits_list_waiting().expect("list").len(),
        1
    );

    // 同じ誤キーの重複 event は再検証を起こさない。
    assert!(
        store
            .credential_record_secret_saved(SECRET, FP1, now())
            .expect("dup")
            .is_empty()
    );
    // 古い fingerprint の検証結果は新しい登録に当たらない。
    let need = store
        .credential_record_secret_saved(SECRET, FP2, now())
        .expect("update");
    assert_eq!(need.len(), 1);
    assert!(
        !store
            .credential_wait_mark_verified("wu-1", SECRET, FP1, now())
            .expect("stale")
    );
    assert!(
        store
            .credential_wait_mark_verified("wu-1", SECRET, FP2, now())
            .expect("verify new")
    );
    assert!(
        store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim")
    );
}

#[test]
fn mark_rejected_refuses_valid_kind() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    enter(&store, "wu-1", t);
    assert!(
        store
            .credential_wait_mark_rejected("wu-1", SECRET, FP1, CredentialVerifyKind::Valid, now())
            .is_err()
    );
}

#[test]
fn waits_are_restored_after_reopen_and_registered_key_is_seen_on_enter() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("t.db");
    let t;
    {
        let store = SqliteStore::open(&path).expect("open");
        t = task_in(&store, Status::Blocked);
        enter(&store, "wu-1", t);
        store
            .credential_record_secret_saved(SECRET, FP1, now())
            .expect("saved");
    }
    let store = SqliteStore::open(&path).expect("reopen");
    let open = store.credential_waits_list_waiting().expect("list");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].state, CredentialWaitState::Waiting);
    assert_eq!(open[0].seen_fingerprint.as_deref(), Some(FP1));
    // 鍵が既に登録済みの後に入った wait は、その fingerprint を最初から見ている（検証だけが残る）。
    let t2 = task_in(&store, Status::Blocked);
    let late = enter(&store, "wu-2", t2);
    assert_eq!(late.wait.seen_fingerprint.as_deref(), Some(FP1));
    assert!(
        store
            .credential_wait_mark_verified("wu-2", SECRET, FP1, now())
            .expect("verify")
    );
}

#[test]
fn a_finished_wait_does_not_block_a_new_wait_for_the_same_pair() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task_in(&store, Status::Blocked);
    verified_wait(&store, "wu-1", t);
    assert!(
        store
            .credential_wait_claim_resume("wu-1", SECRET, now())
            .expect("claim")
    );
    let again = enter(&store, "wu-1", t);
    assert!(again.created && again.first_for_key);
    assert_eq!(again.wait.state, CredentialWaitState::Waiting);
}
