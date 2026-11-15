//! ADR 2026-11-15-lease-reclaim-compare-and-release: `TaskStore::apply_transition_if_lease`（lease の回収の書き込み口）の試験。
//! 照合が合えば遷移と終了記録を同じ transaction で書き（D1）、合わなければ何も書かない（D2）。

use std::time::Duration as StdDuration;

use crate::execution::{HarnessErrorClass, RunEnd};
use crate::execution_plan::{RunIndexRole, RunIndexStatus, RunRow};
use crate::model::Lease;
use crate::transition::Trigger;

use super::tests::sample_task;
use super::*;

/// 回収側が書く終了記録（`RunSupervisor::release_lease` が lease を持っていた run ごとに作るものと同じ形）。
fn lease_expired_finish(run_id: &str) -> Event {
    Event::WorkerFinished {
        run_id: run_id.to_string(),
        outcome: format!("infra_requeue: lease expired (run_id={run_id})"),
        usage: None,
        role: None,
        metrics: None,
        end: Some(RunEnd::HarnessError {
            class: HarnessErrorClass::LeaseExpired,
        }),
    }
}

/// ready の Task を入れ、`run_id` で lease を取り、その run の `runs` 行（running）を作る。DB の lease を返す。
fn running_with_lease(store: &SqliteStore, run_id: &str) -> (TaskId, Lease) {
    let task = sample_task(Status::Ready);
    store.insert(&task).unwrap();
    assert!(
        store
            .acquire_lease(task.id, run_id, StdDuration::from_secs(60))
            .unwrap()
    );
    store
        .run_index_start(RunRow {
            run_id: run_id.to_string(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-11-15T00:00:00Z".to_string(),
            finished_at: None,
        })
        .unwrap();
    let lease = store.get(task.id).unwrap().unwrap().lease.unwrap();
    (task.id, lease)
}

/// 回収が書きうるものの全部: `tasks` 行の生の列（status・lease の 2 列・json・updated_at）、events、run の `runs` 行。
#[derive(Debug, PartialEq)]
struct Written {
    row: (String, Option<String>, Option<String>, String, String),
    events: Vec<(u64, Event)>,
    run: Option<RunRow>,
}

fn written(store: &SqliteStore, task_id: TaskId, run_id: &str) -> Written {
    let row = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT status, lease_worker_run_id, lease_expires_at, json, updated_at FROM tasks WHERE id = ?1",
            params![task_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    };
    Written {
        row,
        events: store.events_for(task_id).unwrap(),
        run: store.run_index_get(run_id).unwrap(),
    }
}

/// (a) 不変条件: Task が running で DB の lease が渡した lease と等しければ、遷移（running → ready）・`Transitioned`・
/// 終了記録（`WorkerFinished` とその `runs` 行の閉じ）を書き、lease を消して `Some(outcome)` を返す。
///
/// 既存の試験で検出できない理由: 既存の遷移の試験（`apply_transition_with_events` など）は lease を照合しない口の試験で、
/// 照合が一致したときに回収を書くこと（照合が厳しすぎて、切れた lease を回収できなくなる不具合）を確かめない。
#[test]
fn apply_transition_if_lease_writes_the_reclaim_when_the_lease_matches() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (id, lease) = running_with_lease(&store, "run-a");
    let events_before = store.events_for(id).unwrap().len();

    let outcome = store
        .apply_transition_if_lease(
            id,
            &lease,
            Trigger::InfraRequeue,
            vec![lease_expired_finish("run-a")],
        )
        .unwrap()
        .expect("the matching lease must be reclaimed");
    assert_eq!(outcome.next, Status::Ready);
    assert_eq!(outcome.attempts, 0);

    let task = store.get(id).unwrap().unwrap();
    assert_eq!(task.status, Status::Ready);
    assert_eq!(task.lease, None);
    let after = written(&store, id, "run-a");
    assert_eq!(after.row.0, "ready");
    assert_eq!((after.row.1, after.row.2), (None, None));
    let tail: Vec<Event> = after.events[events_before..]
        .iter()
        .map(|(_, e)| e.clone())
        .collect();
    assert_eq!(
        tail,
        vec![
            Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "infra_requeue".to_string(),
            },
            lease_expired_finish("run-a"),
        ]
    );
    assert_eq!(
        after.run.map(|r| r.status),
        Some(RunIndexStatus::HarnessError)
    );
}

/// (b) 不変条件（`lease-single-holder`）: 回収側が snapshot を取った後に、保持者が別の接続で `renew_lease` して lease を
/// 延ばしたら、回収側が古い snapshot の lease で書いても何も書かない（`tasks` 行・lease・events・`runs` 行が不変）で `None`。
/// run id は同じで期限だけが違うので、期限も照合していることを確かめる。
///
/// 既存の試験で検出できない理由: `renew_lease_extends_only_the_matching_running_lease` は 1 つの接続で renew の結果だけを見る。
/// 照合の無い書き込み口（`apply_transition_with_events`）の試験は、snapshot と書き込みの間に別の接続の書き込みを挟まない。
#[test]
fn apply_transition_if_lease_writes_nothing_after_a_renew_on_another_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let holder = SqliteStore::open(&path).unwrap();
    let reclaimer = SqliteStore::open(&path).unwrap();
    let (id, _) = running_with_lease(&holder, "run-a");

    // 回収側の snapshot（`reclaim_expired_leases` と同じく running の一覧から読む）。
    let snapshot = reclaimer
        .list(Some(Status::Running))
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .and_then(|t| t.lease)
        .unwrap();
    // 保持者が別の接続で lease を延ばす。
    assert!(
        holder
            .renew_lease(id, "run-a", StdDuration::from_secs(3600))
            .unwrap()
    );
    let extended = holder.get(id).unwrap().unwrap().lease.unwrap();
    assert_eq!(extended.worker_run_id, snapshot.worker_run_id);
    assert!(extended.expires_at > snapshot.expires_at);
    let before = written(&holder, id, "run-a");

    let got = reclaimer
        .apply_transition_if_lease(
            id,
            &snapshot,
            Trigger::InfraRequeue,
            vec![lease_expired_finish("run-a")],
        )
        .unwrap();
    assert_eq!(got, None);
    assert_eq!(written(&holder, id, "run-a"), before);
    let task = holder.get(id).unwrap().unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.lease, Some(extended));
}

/// (c) 不変条件: DB の lease の run id が渡した lease と違えば（期限は同じでも）何も書かずに `None`。lease が別の run に
/// 渡った後の古い回収が、新しい保持者の lease を消さない。
///
/// 既存の試験で検出できない理由: run id の照合を持つ既存の口は `renew_lease`（延ばすだけで遷移を書かない）だけで、
/// 遷移と終了記録を書く口の照合を試す試験は無い。
#[test]
fn apply_transition_if_lease_writes_nothing_for_another_run_id() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (id, lease) = running_with_lease(&store, "run-a");
    let other = Lease {
        worker_run_id: "run-b".to_string(),
        expires_at: lease.expires_at,
    };
    let before = written(&store, id, "run-a");

    let got = store
        .apply_transition_if_lease(
            id,
            &other,
            Trigger::InfraRequeue,
            vec![lease_expired_finish("run-b")],
        )
        .unwrap();
    assert_eq!(got, None);
    assert_eq!(written(&store, id, "run-a"), before);
    assert_eq!(store.get(id).unwrap().unwrap().lease, Some(lease));
}

/// (d) 不変条件: Task が running でなければ、lease の列が渡した lease と等しくても何も書かずに `None`。trigger は
/// `Cancel`（blocked からも通る遷移）にして、止めているのが `transition()` ではなく status の照合であることを確かめる。
/// 無い Task も `None`（照合する lease が無い）。
///
/// 既存の試験で検出できない理由: 照合の無い書き込み口では、running でない Task への回収は `transition()` が通る trigger
/// なら書かれてしまい、その場合を止める口が無かった。
#[test]
fn apply_transition_if_lease_writes_nothing_when_the_task_is_not_running() {
    let store = SqliteStore::open_in_memory().unwrap();
    let lease = Lease {
        worker_run_id: "run-a".to_string(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::minutes(1),
    };
    let mut task = sample_task(Status::Blocked);
    task.lease = Some(lease.clone());
    store.insert(&task).unwrap();
    assert_eq!(
        store.get(task.id).unwrap().unwrap().lease,
        Some(lease.clone())
    );
    let before = written(&store, task.id, "run-a");

    let got = store
        .apply_transition_if_lease(
            task.id,
            &lease,
            Trigger::Cancel,
            vec![lease_expired_finish("run-a")],
        )
        .unwrap();
    assert_eq!(got, None);
    assert_eq!(written(&store, task.id, "run-a"), before);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);

    assert_eq!(
        store
            .apply_transition_if_lease(TaskId::new(), &lease, Trigger::Cancel, vec![])
            .unwrap(),
        None
    );
}

/// (e) 不変条件: 回収で lease を手放した Task は ready に戻り、別の run が `acquire_lease` で取り直せる。取り直した後に
/// 古い snapshot の lease でもう一度回収しても、新しい保持者の lease は消えない。消した試験
/// `release_lease_then_reacquire_succeeds`（`release_lease` の後の取り直し）の置き換え。
///
/// 既存の試験で検出できない理由: `acquire_lease` の既存の試験は `insert` した ready の Task から始まり、回収の後の Task
/// （lease の列を NULL に戻した行）からの取り直しを試さない。
#[test]
fn acquire_lease_succeeds_after_a_compare_and_release_reclaim() {
    let store = SqliteStore::open_in_memory().unwrap();
    let (id, lease) = running_with_lease(&store, "run-a");
    assert!(
        store
            .apply_transition_if_lease(
                id,
                &lease,
                Trigger::InfraRequeue,
                vec![lease_expired_finish("run-a")]
            )
            .unwrap()
            .is_some()
    );

    assert!(
        store
            .acquire_lease(id, "run-b", StdDuration::from_secs(60))
            .unwrap()
    );
    let task = store.get(id).unwrap().unwrap();
    assert_eq!(task.status, Status::Running);
    let reacquired = task.lease.unwrap();
    assert_eq!(reacquired.worker_run_id, "run-b");

    let before = written(&store, id, "run-a");
    assert_eq!(
        store
            .apply_transition_if_lease(id, &lease, Trigger::InfraRequeue, vec![])
            .unwrap(),
        None
    );
    assert_eq!(written(&store, id, "run-a"), before);
    assert_eq!(store.get(id).unwrap().unwrap().lease, Some(reacquired));
}
