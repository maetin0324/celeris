//! ADR 2026-11-08-health-enforcement-loop D7（P-9・決定 p9-scope）: 依存先の reopen で、`dependency_failed` で
//! cancel された後続を戻す（`SqliteStore::cascade_after_transition_tx`）。
//!
//! 守る不変条件: 依存先が終端から戻ったら、その失敗で cancel された後続だけが同じ transaction で戻る。人・CoS の
//! cancel と、ほかの依存先がまだ failed・cancelled の後続は戻らない。戻すのは新しい `Transitioned` の追記で、
//! 既存の event は変わらない。既存の試験（`dependency_failure_cancels_dependents_transitively`）は cancel の向き
//! だけを見ており、reopen の後に後続が cancelled のまま残る不具合（P-9）を検出できない。
//!
//! D7.2（retry の後継）: 依存先の失敗で cancel された後続 B を人が retry して後継 B' を作っていたら、依存先 A の
//! reopen でも retry でも B を戻さず付け替えない（B と B' の 2 つが同じ仕事を走らせない）。判定は
//! `retry_successors_tx` と `transition::cancelled_by_dependency` を両経路が同じ入力で使う。上の 5 件はどれも
//! 後継のある後続を作らないので、後継のある B が A の reopen・retry で戻る重複を検出できない。
//!
//! D7.3（記録）: 依存先 A の終端からの復帰は、戻した後続（restored）と、依存先の失敗で cancel されたが戻さなかった
//! 後続（kept）を A の events に `Event::DependentsReopened` として同じ transaction で 1 件追記する。上の試験は
//! 後続の状態と後続の events だけを見ており、A の側に記録が無い・余計な後続が入る・同じ記録が重なる不具合を検出
//! できない。

use std::time::Duration as StdDuration;

use super::SqliteStore;
use super::tests::sample_task;
use super::transition::retry_successors_tx;
use crate::model::{Event, KeptDependent, KeptReason, RestoredDependent, Status, TaskId};
use crate::store::TaskStore;
use crate::transition::{DependencyCancel, Trigger, dependency_reopen_trigger};

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
/// 戻る。cancel までの event はそのまま残り、`dependency_reopened` の遷移が後ろに足される。B の復帰は C を戻すので、
/// B の events には遷移の後に C を restored に持つ `DependentsReopened` が続く（D7.3。A の記録は B だけを持つ）。
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

    let reopened = Event::Transitioned {
        from: Status::Cancelled,
        to: Status::Ready,
        reason: "dependency_reopened".to_string(),
    };
    let restored = |task_id, depends_on| Event::DependentsReopened {
        restored: vec![RestoredDependent {
            task_id,
            depends_on,
        }],
        kept: vec![],
    };
    let b_appended = vec![reopened.clone(), restored(c, vec![b])];
    for (id, before, appended) in [(b, b_before, b_appended), (c, c_before, vec![reopened])] {
        let t = store.get(id).unwrap().unwrap();
        assert_eq!(t.status, Status::Ready);
        assert_eq!(t.attempts, 0);
        assert_eq!(
            reasons(&store, id),
            vec!["dependency_failed", "dependency_reopened"]
        );
        let after = store.events_for(id).unwrap();
        assert_eq!(
            &after[..before.len()],
            &before[..],
            "既存の event は変わらない"
        );
        let tail: Vec<Event> = after[before.len()..]
            .iter()
            .map(|(_, e)| e.clone())
            .collect();
        assert_eq!(tail, appended, "遷移（と B の記録）だけを追記する");
    }
    assert_eq!(reopened_records(&store, a), vec![restored(b, vec![a])]);
    // 戻った後続は依存先 A の完了を待つ（A は ready で、done ではない）。
    let ready: Vec<TaskId> = store
        .ready_tasks(100)
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
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
    store
        .apply_transition(manual, Trigger::Cancel, None)
        .unwrap();
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

    store
        .apply_transition(a, Trigger::ReviewFail, None)
        .unwrap();
    assert_eq!(status(&store, a), Status::Failed);
    assert_eq!(status(&store, b), Status::Cancelled);
    assert_eq!(
        reasons(&store, b),
        vec![
            "dependency_failed",
            "dependency_reopened",
            "dependency_failed"
        ]
    );
}

/// 人が `failed`・`cancelled` の `original` を retry して後継を作る。`plan_retry` と同じく `depends_on` を写し、
/// 後継は draft で始まる。
fn retry(store: &SqliteStore, original: TaskId) -> TaskId {
    let mut next = sample_task(Status::Draft);
    next.depends_on = store.get(original).unwrap().unwrap().depends_on;
    store.retry_task(original, &next, "human:web").unwrap();
    next.id
}

/// 依存先 `dep` の reopen が後続 `id` に下す判定（`cascade_after_transition_tx` と同じ読み出しと述語）。
fn reopen_verdict(store: &SqliteStore, dep: TaskId, id: TaskId) -> DependencyCancel<Trigger> {
    let conn = store.lock().unwrap();
    let dependents = SqliteStore::dependents_of_tx(&conn, dep, false).unwrap();
    let successors = retry_successors_tx(&conn, &dependents).unwrap();
    let task = SqliteStore::get_locked(&conn, id).unwrap().unwrap();
    let last = SqliteStore::last_transitioned_tx(&conn, id).unwrap();
    let others: Vec<(TaskId, Option<Status>)> = (task.depends_on.iter().filter(|d| **d != dep))
        .map(|d| (*d, SqliteStore::task_status_tx(&conn, *d).unwrap()))
        .collect();
    dependency_reopen_trigger(
        task.status,
        last.as_ref(),
        successors.get(&id).copied(),
        &others,
    )
}

/// D7.2: A の失敗で cancel された B を人が retry して後継 B'（`depends_on` に A を写す）を作った後、A を reopen しても
/// B は cancelled のまま残り、判定は `Kept(Retried { successor: B' })` になる。B' は draft のまま触られない。
/// 後継の無い後続 C は今までどおり戻る。
#[test]
fn dependency_reopen_keeps_dependent_with_retry_successor() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    let c = insert_dependent(&store, Status::Ready, vec![a]);
    fail(&store, a);
    let b2 = retry(&store, b);
    assert_eq!(store.get(b2).unwrap().unwrap().depends_on, vec![a]);
    assert_eq!(
        reopen_verdict(&store, a, b),
        DependencyCancel::Kept(KeptReason::Retried { successor: b2 })
    );
    let b2_before = store.events_for(b2).unwrap();

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, b), Status::Cancelled);
    assert_eq!(reasons(&store, b), vec!["dependency_failed"]);
    assert_eq!(status(&store, b2), Status::Draft);
    assert_eq!(
        store.events_for(b2).unwrap(),
        b2_before,
        "後継の event は増えない"
    );
    assert_eq!(status(&store, c), Status::Ready);
    assert_eq!(
        reasons(&store, c),
        vec!["dependency_failed", "dependency_reopened"]
    );
}

/// D7.2: A の retry（A → A'）で、後継 B' を持つ B は A' に付け替えず cancelled のまま残し、B' は A' に付け替わる
/// （draft のまま）。後継の無い C は今までどおり A' に付け替わって draft に戻る。
#[test]
fn dependency_reopen_retry_skips_dependent_with_successor() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    let c = insert_dependent(&store, Status::Ready, vec![a]);
    fail(&store, a);
    let b2 = retry(&store, b);

    let a2 = sample_task(Status::Draft);
    let mut rewired = store.retry_task(a, &a2, "human:web").unwrap();
    rewired.sort();
    let mut expected = vec![b2, c];
    expected.sort();
    assert_eq!(rewired, expected, "B は付け替えない");

    let b_task = store.get(b).unwrap().unwrap();
    assert_eq!(b_task.status, Status::Cancelled);
    assert_eq!(b_task.depends_on, vec![a]);
    assert_eq!(reasons(&store, b), vec!["dependency_failed"]);
    let b2_task = store.get(b2).unwrap().unwrap();
    assert_eq!(b2_task.status, Status::Draft);
    assert_eq!(b2_task.depends_on, vec![a2.id]);
    let c_task = store.get(c).unwrap().unwrap();
    assert_eq!(c_task.status, Status::Draft);
    assert_eq!(c_task.depends_on, vec![a2.id]);
    assert_eq!(reasons(&store, c), vec!["dependency_failed", "retried"]);
}

/// D7.2: 人が後継 B' の `depends_on` から A を外すと、B' は A の後続でなくなり、B は後継を持たない扱いになる。A を
/// reopen すると B は戻り（cancel の前の ready へ）、B' は触られない。
#[test]
fn dependency_reopen_restores_when_successor_drops_dependency() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    fail(&store, a);
    let b2 = retry(&store, b);
    let mut edited = store.get(b2).unwrap().unwrap();
    edited.depends_on.clear();
    let event = Event::Edited {
        fields: vec!["depends_on".to_string()],
        by: "human".to_string(),
    };
    store.update_task(&edited, event).unwrap();
    assert!(matches!(
        reopen_verdict(&store, a, b),
        DependencyCancel::Restore(Trigger::DependencyReopened { draft: false })
    ));

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, b), Status::Ready);
    assert_eq!(
        reasons(&store, b),
        vec!["dependency_failed", "dependency_reopened"]
    );
    let b2_task = store.get(b2).unwrap().unwrap();
    assert_eq!(b2_task.status, Status::Draft);
    assert!(b2_task.depends_on.is_empty());
    // D7.3: 戻った B は A の記録の restored に依存先と一緒に入る。後継 B' は A の後続でないので記録に無い。
    assert_eq!(
        reopened_records(&store, a),
        vec![Event::DependentsReopened {
            restored: vec![RestoredDependent {
                task_id: b,
                depends_on: vec![a],
            }],
            kept: vec![],
        }]
    );
}

/// `id` の events の `Event::DependentsReopened`（古い順）。
fn reopened_records(store: &SqliteStore, id: TaskId) -> Vec<Event> {
    (store.events_for(id).unwrap().into_iter())
        .filter_map(|(_, e)| matches!(e, Event::DependentsReopened { .. }).then_some(e))
        .collect()
}

/// D7.3: A の reopen（failed → ready）で、A の events に記録が 1 件だけ追記される。restored は戻した C（依存先 `[A]`）、
/// kept は後継 B' を持つ B（`retried`）と、ほかの依存先 E が failed の D（`other_dependency`）。人が cancel した M と
/// 木の親の中止で cancel された P は A の失敗と無関係なので kept に無く、draft の後継 B' も入らない。後続の cancel・
/// B' の `Retried` の event は変わらない。done の依存先 X の reopen・rereview では、ほかの依存先の失敗で cancel された
/// 後続 Y も kept に数えないので、restored が空なら X に記録を書かない。
#[test]
fn dependency_reopen_records_restored_and_kept() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let e = insert_failable(&store);
    let b = insert_dependent(&store, Status::Ready, vec![a]);
    let c = insert_dependent(&store, Status::Ready, vec![a]);
    let d = insert_dependent(&store, Status::Ready, vec![a, e]);
    let m = insert_dependent(&store, Status::Ready, vec![a]);
    let p = insert_dependent(&store, Status::Ready, vec![a]);
    store.apply_transition(m, Trigger::Cancel, None).unwrap();
    (store.apply_transition(p, Trigger::ParentCancelled, None)).unwrap();
    fail(&store, e);
    fail(&store, a);
    let b2 = retry(&store, b);
    let before: Vec<_> = (store.events_for(b2).unwrap().into_iter())
        .chain(
            [b, d, m, p]
                .into_iter()
                .flat_map(|id| store.events_for(id).unwrap()),
        )
        .collect();

    store.apply_transition(a, Trigger::Reopen, None).unwrap();
    let mut kept = vec![
        KeptDependent {
            task_id: b,
            why: KeptReason::Retried { successor: b2 },
        },
        KeptDependent {
            task_id: d,
            why: KeptReason::OtherDependency {
                unresolved: vec![e],
            },
        },
    ];
    kept.sort_by_key(|k| k.task_id);
    let restored = vec![RestoredDependent {
        task_id: c,
        depends_on: vec![a],
    }];
    assert_eq!(
        reopened_records(&store, a),
        vec![Event::DependentsReopened { restored, kept }]
    );
    assert_eq!(status(&store, c), Status::Ready);
    let after: Vec<_> = (store.events_for(b2).unwrap().into_iter())
        .chain(
            [b, d, m, p]
                .into_iter()
                .flat_map(|id| store.events_for(id).unwrap()),
        )
        .collect();
    assert_eq!(after, before, "cancel と Retried の event は変わらない");

    let x = insert_dependent(&store, Status::Done, vec![]);
    let f = insert_failable(&store);
    let y = insert_dependent(&store, Status::Ready, vec![x, f]);
    let manual = insert_dependent(&store, Status::Ready, vec![x]);
    store
        .apply_transition(manual, Trigger::Cancel, None)
        .unwrap();
    fail(&store, f);
    assert_eq!(status(&store, y), Status::Cancelled);
    store.apply_transition(x, Trigger::Rereview, None).unwrap();
    assert_eq!(status(&store, x), Status::Reviewing);
    store
        .apply_transition(x, Trigger::ReviewPass, None)
        .unwrap();
    store.apply_transition(x, Trigger::Reopen, None).unwrap();
    assert_eq!(status(&store, x), Status::Ready);
    assert_eq!(status(&store, y), Status::Cancelled);
    assert!(
        reopened_records(&store, x).is_empty(),
        "done からの復帰では kept を書かない"
    );
}

/// D7.3: 同じ内容の記録を重ねない。ほかの依存先 E が failed の D だけを kept に持つ A を、rereview → 不合格 → rereview
/// と 2 回戻しても、2 回目は restored が空で kept が直前の記録と同じなので追記しない。戻す後続も kept も無い依存先
/// （人が cancel した後続だけを持つ Z）の reopen は何も書かない。
#[test]
fn dependency_reopen_skips_unchanged_record() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = insert_failable(&store);
    let e = insert_failable(&store);
    let d = insert_dependent(&store, Status::Ready, vec![a, e]);
    fail(&store, e);
    fail(&store, a);
    let record = Event::DependentsReopened {
        restored: vec![],
        kept: vec![KeptDependent {
            task_id: d,
            why: KeptReason::OtherDependency {
                unresolved: vec![e],
            },
        }],
    };

    store.apply_transition(a, Trigger::Rereview, None).unwrap();
    assert_eq!(reopened_records(&store, a), vec![record.clone()]);
    store
        .apply_transition(a, Trigger::ReviewFail, None)
        .unwrap();
    assert_eq!(status(&store, a), Status::Failed);
    let before = store.events_for(a).unwrap().len();
    store.apply_transition(a, Trigger::Rereview, None).unwrap();
    assert_eq!(reopened_records(&store, a), vec![record]);
    assert_eq!(
        store.events_for(a).unwrap().len(),
        before + 1,
        "遷移の 1 件だけ"
    );

    let z = insert_failable(&store);
    let manual = insert_dependent(&store, Status::Ready, vec![z]);
    store
        .apply_transition(manual, Trigger::Cancel, None)
        .unwrap();
    fail(&store, z);
    store.apply_transition(z, Trigger::Reopen, None).unwrap();
    assert!(reopened_records(&store, z).is_empty(), "両方空なら書かない");
    assert_eq!(status(&store, manual), Status::Cancelled);
}
