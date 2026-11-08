//! ADR 2026-11-08-health-enforcement-loop D7（P-9・決定 p9-scope）: 依存先の reopen で、`dependency_failed` で
//! cancel された後続を戻す（`SqliteStore::cascade_after_transition_tx`）。
//!
//! 守る不変条件: 依存先が終端から戻ったら、その失敗で cancel された後続だけが同じ transaction で戻る。人・CoS の
//! cancel と、ほかの依存先がまだ failed・cancelled の後続は戻らない。戻すのは新しい `Transitioned` の追記で、
//! 既存の event は変わらない。既存の試験（`dependency_failure_cancels_dependents_transitively`）は cancel の向き
//! だけを見ており、reopen の後に後続が cancelled のまま残る不具合（P-9）を検出できない。

use std::time::Duration as StdDuration;

use super::SqliteStore;
use super::tests::sample_task;
use crate::model::{Event, Status, TaskId};
use crate::store::TaskStore;
use crate::transition::Trigger;

fn status(store: &SqliteStore, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

fn reasons(store: &SqliteStore, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// `id` を running から `failed` にする（やり直しの余地なし）。後続へ `dependency_failed` が連鎖する。
fn fail(store: &SqliteStore, id: TaskId) {
    assert!(
        store
            .acquire_lease(id, &format!("run-{id}"), StdDuration::from_secs(60))
            .unwrap()
    );
    let outcome = store
        .apply_transition(id, Trigger::WorkerError { retryable: false }, None)
        .unwrap();
    assert_eq!(outcome.next, Status::Failed);
}

fn insert_failable(store: &SqliteStore) -> TaskId {
    let mut t = sample_task(Status::Ready);
    t.budget.max_retries = 0;
    store.insert(&t).unwrap();
    t.id
}

fn insert_dependent(store: &SqliteStore, status: Status, depends_on: Vec<TaskId>) -> TaskId {
    let mut t = sample_task(status);
    t.depends_on = depends_on;
    store.insert(&t).unwrap();
    t.id
}

/// 依存先 A の reopen で、A の失敗で cancel された後続 B と、B の cancel で cancel された C が推移的に `ready` に
/// 戻る。cancel までの event はそのまま残り、`dependency_reopened` の遷移が後ろに足される。
#[test]
fn dependency_reopen_restores_cancelled_dependents_transitively() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    let c = insert_dependent(&store, Status::Ready, vec![b]);
    fail(&store, a);
    assert_eq!(status(&store, b), Status::Cancelled);
    assert_eq!(status(&store, c), Status::Cancelled);
    let b_before = store.events_for(b).unwrap();
    let c_before = store.events_for(c).unwrap();

    let outcome = store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(outcome.next, Status::Ready);

    for (id, before) in [(b, b_before), (c, c_before)] {
        let t = store.get(id).unwrap().unwrap();
        assert_eq!(t.status, Status::Ready);
        assert_eq!(t.attempts, 0);
        assert_eq!(
            reasons(&store, id),
            vec!["dependency_failed", "dependency_reopened"]
        );
        let after = store.events_for(id).unwrap();
        assert_eq!(after.len(), before.len() + 1, "1 件だけ追記する");
        assert_eq!(&after[..before.len()], &before[..], "既存の event は変わらない");
        assert_eq!(
            after[before.len()].1,
            Event::Transitioned {
                from: Status::Cancelled,
                to: Status::Ready,
                reason: "dependency_reopened".to_string(),
            }
        );
    }
    // 戻った後続は依存先 A の完了を待つ（A は ready で、done ではない）。
    let ready: Vec<TaskId> = store.ready_tasks(100).unwrap().iter().map(|t| t.id).collect();
    assert!(ready.contains(&a));
    assert!(!ready.contains(&b) && !ready.contains(&c));
}

/// ほかの依存先 E がまだ failed の後続 D は、A を reopen しても戻らない。E を reopen すると、A も E も終端でない
/// ので D が戻る。
#[test]
fn dependency_reopen_keeps_dependent_while_another_dependency_failed() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let e = insert_failable(&store);
    let d = insert_dependent(&store, Status::Ready, vec![a, e]);
    fail(&store, e);
    fail(&store, a);
    assert_eq!(status(&store, d), Status::Cancelled);

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, d), Status::Cancelled);
    assert_eq!(reasons(&store, d), vec!["dependency_failed"]);

    store.apply_transition(e, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, d), Status::Ready);
    assert_eq!(
        reasons(&store, d),
        vec!["dependency_failed", "dependency_reopened"]
    );
}

/// 人・CoS が手で cancel した後続（理由 `cancel`）と、木の親の中止で cancel された後続（`parent_cancelled`）は、
/// 依存先を reopen しても戻らない。
#[test]
fn dependency_reopen_keeps_manually_cancelled_dependent() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let manual = insert_dependent(&store, Status::Ready, vec![a]);
    let by_parent = insert_dependent(&store, Status::Ready, vec![a]);
    store.apply_transition(manual, Trigger::Cancel, None).unwrap();
    store
        .apply_transition(by_parent, Trigger::ParentCancelled, None)
        .unwrap();
    fail(&store, a);

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, manual), Status::Cancelled);
    assert_eq!(reasons(&store, manual), vec!["cancel"]);
    assert_eq!(status(&store, by_parent), Status::Cancelled);
    assert_eq!(reasons(&store, by_parent), vec!["parent_cancelled"]);
}

/// cancel の前が `draft`（人がまだ受け入れていない）の後続は `draft` に戻り、`ready` にはならない。
#[test]
fn dependency_reopen_returns_draft_dependent_to_draft() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Draft, vec![a]);
    fail(&store, a);
    assert_eq!(status(&store, b), Status::Cancelled);

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, b), Status::Draft);
    assert_eq!(
        store.events_for(b).unwrap().last().map(|(_, e)| e.clone()),
        Some(Event::Transitioned {
            from: Status::Cancelled,
            to: Status::Draft,
            reason: "dependency_reopened".to_string(),
        })
    );
}

/// 再判定（`rereview`: failed → reviewing）も終端から戻る遷移なので、後続を戻す。再判定が不合格なら後続は
/// 再び `dependency_failed` で cancel される（event は追記だけ）。
#[test]
fn dependency_reopen_on_rereview_and_cancels_again_on_failure() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    fail(&store, a);

    store.apply_transition(a, Trigger::Rereview, None).unwrap();
    assert_eq!(status(&store, a), Status::Reviewing);
    assert_eq!(status(&store, b), Status::Ready);

    store.apply_transition(a, Trigger::ReviewFail, None).unwrap();
    assert_eq!(status(&store, a), Status::Failed);
    assert_eq!(status(&store, b), Status::Cancelled);
    assert_eq!(
        reasons(&store, b),
        vec!["dependency_failed", "dependency_reopened", "dependency_failed"]
    );
}
