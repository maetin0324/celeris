//! ADR 2026-11-09-unscheduled-restart-hold: 再起動（promote）直後の最初の tick では、dispatcher のメモリの待ち
//! （`dispatch_waits`）がまだ空なので、停滞の判定を `dispatch_ready` が 1 回終わるまで保留する。
//!
//! 守る不変条件: 再起動直後でも、子や依存の統合を待つ ready の task は `task_unscheduled` に出ない。待ちの理由が
//! 本当に無い ready の task は、保留が明けた最初の判定で今どおり出る。既存の停滞の試験（`stagnation.rs`・
//! `cos_chat_triage_stagnation.rs`）はメモリの待ちを持つ task を使わず、tick の順序による誤検知を検出できない。
//! 時刻は注入した時計（`test_now`）で決める。最後の出来事（壁時計）から 1 時間後に起動した daemon を作る。

use super::*;
use task_ops::stagnation::{StagnationKind, StagnationPolicy};

fn adapter() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(30),
    })
}

fn policy() -> StagnationPolicy {
    StagnationPolicy {
        task_unscheduled_secs: 1800,
        ..Default::default()
    }
}

/// 新しく起動した daemon（メモリは空）。時計は今から 1 時間後（最後の出来事から閾値 1800 秒を越えている）。
fn restarted(store: Arc<dyn TaskStore>) -> (Dispatcher, Arc<StdMutex<OffsetDateTime>>) {
    let mut d = dispatcher(store, adapter(), 4);
    d.set_stagnation(policy(), 60);
    let clock = Arc::new(StdMutex::new(
        OffsetDateTime::now_utc() + time::Duration::hours(1),
    ));
    d.test_now = Some(clock.clone());
    (d, clock)
}

fn advance(clock: &StdMutex<OffsetDateTime>, secs: i64) {
    let mut now = clock.lock().unwrap();
    *now += time::Duration::seconds(secs);
}

fn unscheduled(d: &Dispatcher) -> Vec<TaskId> {
    let mut ids: Vec<TaskId> = d
        .stagnation
        .findings
        .iter()
        .filter(|f| f.kind == StagnationKind::TaskUnscheduled)
        .filter_map(|f| f.task_id)
        .collect();
    ids.sort();
    ids
}

fn memory_wait(d: &Dispatcher, id: TaskId) -> Option<TaskWait> {
    d.snapshot_task_waits()
        .into_iter()
        .find(|w| w.task_id == id)
        .map(|w| w.wait)
}

/// メモリの待ちを使わない（再起動直後の最初の判定が見ていた）事実で、`task_unscheduled` に出る task。
fn unscheduled_without_memory(store: &dyn TaskStore, now: OffsetDateTime) -> Vec<TaskId> {
    let none = std::collections::HashSet::new();
    let facts = task_ops::stagnation::gather(
        store,
        None,
        &policy(),
        now,
        &none,
        &none,
        &task_ops::wait::MemoryWaits::default(),
    )
    .unwrap();
    let mut ids: Vec<TaskId> = task_ops::stagnation::detect(&facts, &policy(), now)
        .into_iter()
        .filter_map(|f| f.task_id)
        .collect();
    ids.sort();
    ids
}

/// 子を待つ親（直近の遷移が `children_wait`、子は未完）は再起動直後の最初の tick で出ない。待ちの理由が無い
/// ready の task（入力の揃っていない知識整理 task。`dispatch_one` は待ちを書かずに見送る）は、保留が明けた
/// 2 回目の tick の判定で出る。
#[tokio::test]
async fn unscheduled_restart_first_tick_skips_parent_waiting_for_children_but_reports_no_reason_task()
 {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = new_task(dir.path(), Check::Human, 0);
    parent.status = Status::Ready;
    sqlite
        .create_task(
            &parent,
            vec![Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "children_wait".into(),
            }],
        )
        .unwrap();
    let mut child = new_task(dir.path(), Check::Human, 0);
    child.parent_id = Some(parent.id);
    child.status = Status::Blocked;
    sqlite.insert(&child).unwrap();
    let mut stuck = new_task(dir.path(), Check::Human, 0);
    stuck.status = Status::Ready;
    stuck.genre = Some(task_ops::knowledge_curation::CURATION_HARNESS.into());
    stuck.labels = vec![task_ops::cron_jobs::CRON_TASK_LABEL.into()];
    sqlite.insert(&stuck).unwrap();
    let store: Arc<dyn TaskStore> = sqlite;
    let (mut d, clock) = restarted(store.clone());
    let mut both = vec![parent.id, stuck.id];
    both.sort();
    let start = *clock.lock().unwrap();
    assert_eq!(
        unscheduled_without_memory(store.as_ref(), start),
        both,
        "without the memory waits the parent looks unscheduled (what the old first tick saw)"
    );

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 0);
    assert!(unscheduled(&d).is_empty(), "{:?}", d.stagnation.findings);
    assert!(
        !d.stagnation_known(),
        "the first tick holds the stagnation check"
    );
    assert!(
        matches!(
            memory_wait(&d, parent.id),
            Some(TaskWait::DelegatedChildren { ref task_ids }) if task_ids == &vec![child.id]
        ),
        "dispatch_ready recorded the children wait"
    );
    assert_eq!(memory_wait(&d, stuck.id), None);

    advance(&clock, 60);
    d.tick().unwrap();
    assert!(d.stagnation_known());
    assert_eq!(
        unscheduled(&d),
        vec![stuck.id],
        "only the ready task without any reason is reported"
    );
    assert_eq!(store.get(parent.id).unwrap().unwrap().status, Status::Ready);
}

/// root 間の依存先が done でも既定 branch に入っていない（M10 `DependsOnIntegration`。git の祖先で決まり、
/// DB の待ちには無い）ready の task も、再起動直後に出ない。
#[tokio::test]
async fn unscheduled_restart_dependency_integration_wait_is_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _repo, pred, dependent, _branch) = super::dep_integration::setup(dir.path(), None);
    let (mut d, clock) = restarted(store.clone());
    assert_eq!(
        unscheduled_without_memory(store.as_ref(), *clock.lock().unwrap()),
        vec![dependent.id]
    );

    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert!(unscheduled(&d).is_empty(), "{:?}", d.stagnation.findings);
    assert!(!d.stagnation_known());
    assert!(matches!(
        memory_wait(&d, dependent.id),
        Some(TaskWait::DependsOnIntegration { task_id, .. }) if task_id == pred.id
    ));
    for _ in 0..3 {
        advance(&clock, 60);
        d.tick().unwrap();
        assert!(d.stagnation_known());
        assert!(unscheduled(&d).is_empty(), "{:?}", d.stagnation.findings);
    }
}
