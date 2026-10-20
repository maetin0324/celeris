//! 統合開始と Continue{advance} の両順序を、待ち時間や CPU 負荷なしで固定する。
use super::orphan_takeover::{SELF_ID, instance};
use super::*;
use crate::orphan::{ORPHAN_TAKEOVER_REASON, OrphanTakeover};

fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<dyn TaskStore>,
    Task,
    Dispatcher,
) {
    fixture_with_lease(Duration::from_secs(600))
}

fn fixture_with_lease(
    ttl: Duration,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<dyn TaskStore>,
    Task,
    Dispatcher,
) {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
    let mut leaf = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    leaf.status = task_core::WorkUnitStatus::Done;
    store
        .work_units_apply(task.id, vec![], vec![leaf], vec![])
        .unwrap();
    assert!(
        store
            .acquire_lease(task.id, "phase:test:build", ttl)
            .unwrap()
    );
    let task = store.get(task.id).unwrap().unwrap();
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::ZERO));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    store
        .instance_register(&instance(SELF_ID, InstanceRole::Active, std::process::id()))
        .unwrap();
    d.set_orphan_takeover(OrphanTakeover {
        instance_id: SELF_ID.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
    (repo, root, store, task, d)
}

fn integration(store: &Arc<dyn TaskStore>, task_id: TaskId) -> task_core::WorkUnitRow {
    store
        .work_units_for(task_id)
        .unwrap()
        .into_iter()
        .find(|u| u.kind == task_core::WorkUnitKind::Integrate)
        .unwrap()
}

#[tokio::test]
async fn ready_wins_before_integration_start_leaves_pending_and_retries() {
    let (_repo, _root, store, task, mut d) = fixture();
    let wu = integration(&store, task.id);
    d.before_integration_start = Some(|d, task_id| {
        // 最後の葉は Done、統合は Pending、まだ spawn は無い、という同じ tick の隙間。
        d.reconcile_parallel_tasks().unwrap();
        assert_eq!(d.store.get(task_id).unwrap().unwrap().status, Status::Ready);
    });
    d.start_integration(&task, &wu.id).unwrap();
    d.abort_stale_runs().unwrap();
    assert!(!d.integrating.contains_key(&task.id));
    // 修正前は ready の task に統合を起こし、abort 後も WU が Running のまま残る。
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Pending
    );
    d.reconcile_parallel_tasks().unwrap();
    assert!(matches!(
        d.wu_dispatch_gate(task.id).unwrap(),
        WuDispatchGate::StartIntegration(_)
    ));
    run_until_idle(&mut d, 400).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn integration_wins_before_advance_keeps_task_running() {
    let (_repo, _root, store, task, mut d) = fixture();
    let wu = integration(&store, task.id);
    d.start_integration(&task, &wu.id).unwrap();
    assert!(d.integrating.contains_key(&task.id));
    let advance = store.apply_transition(
        task.id,
        Trigger::Continue {
            why: task_core::ContinueWhy::Advance,
        },
        None,
    );
    d.reconcile_parallel_tasks().unwrap();
    d.abort_stale_runs().unwrap();
    assert!(matches!(advance, Err(StoreError::InvalidTransition(_))));
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);
    assert!(d.integrating.contains_key(&task.id));
    run_until_idle(&mut d, 400).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn ready_ownerless_integration_is_recovered_once_without_a_task_lease() {
    let (_repo, _root, store, task, mut d) = fixture();
    store
        .apply_transition(
            task.id,
            Trigger::Continue {
                why: task_core::ContinueWhy::Advance,
            },
            None,
        )
        .unwrap();
    let mut wu = integration(&store, task.id);
    wu.status = task_core::WorkUnitStatus::Running;
    store
        .work_units_apply(task.id, vec![], vec![wu], vec![])
        .unwrap();
    assert!(store.get(task.id).unwrap().unwrap().lease.is_none());
    assert!(matches!(
        d.wu_dispatch_gate(task.id).unwrap(),
        WuDispatchGate::Skip
    ));
    d.reconcile_parallel_tasks().unwrap();
    d.reconcile_parallel_tasks().unwrap();
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Pending
    );
    assert_eq!(events_of(&store, task.id).iter().filter(|e| matches!(e, Event::WorkUnitTransitioned { from: task_core::WorkUnitStatus::Running, to: task_core::WorkUnitStatus::Pending, reason, run_id: None, .. } if reason == ORPHAN_TAKEOVER_REASON)).count(), 1);
    run_until_idle(&mut d, 400).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn running_ownerless_integration_returns_to_pending_and_ready() {
    let (_repo, _root, store, task, mut d) = fixture();
    let mut wu = integration(&store, task.id);
    wu.status = task_core::WorkUnitStatus::Running;
    store
        .work_units_apply(task.id, vec![], vec![wu], vec![])
        .unwrap();
    d.reconcile_parallel_tasks().unwrap();
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Pending
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    run_until_idle(&mut d, 400).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn ready_integration_recovery_preserves_live_owners_and_dispatcher_scope() {
    for protection in [
        "active",
        "draining",
        "local",
        "not_accepting",
        "ineligible",
        "disabled",
    ] {
        let (_repo, _root, store, task, mut d) = fixture();
        store
            .apply_transition(task.id, Trigger::Interrupt, None)
            .unwrap();
        let mut wu = integration(&store, task.id);
        wu.status = task_core::WorkUnitStatus::Running;
        store
            .work_units_apply(task.id, vec![], vec![wu.clone()], vec![])
            .unwrap();
        match protection {
            "active" | "draining" => {
                let role = if protection == "active" {
                    InstanceRole::Active
                } else {
                    InstanceRole::Draining
                };
                store
                    .instance_register(&instance("other", role, std::process::id()))
                    .unwrap();
            }
            "local" => {
                d.integrating.insert(
                    task.id,
                    IntegrationEntry {
                        work_unit_id: wu.id,
                        handle: tokio::spawn(std::future::pending()),
                    },
                );
            }
            "not_accepting" => d.set_accepting_new_work(false),
            "ineligible" => d.set_eligible_tasks(Arc::new(|_| false)),
            "disabled" => d.orphan_takeover = None,
            _ => unreachable!(),
        }
        d.reconcile_parallel_tasks().unwrap();
        assert_eq!(
            integration(&store, task.id).status,
            task_core::WorkUnitStatus::Running,
            "{protection}"
        );
        assert!(!events_of(&store, task.id).iter().any(|e| matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == ORPHAN_TAKEOVER_REASON)), "{protection}");
        d.abort_all_runs();
    }
}

#[tokio::test]
async fn interrupted_integration_is_recovered_on_the_next_tick() {
    let (_repo, _root, store, task, mut d) = fixture();
    let wu = integration(&store, task.id);
    d.start_integration(&task, &wu.id).unwrap();
    // 人の割り込みは Continue{advance} のガードには当たらない。
    store
        .apply_transition(task.id, Trigger::Interrupt, None)
        .unwrap();
    d.reconcile_parallel_tasks().unwrap();
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Running
    );
    d.abort_stale_runs().unwrap();
    d.reconcile_parallel_tasks().unwrap();
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Pending
    );
    assert!(matches!(
        d.wu_dispatch_gate(task.id).unwrap(),
        WuDispatchGate::StartIntegration(_)
    ));
}

#[tokio::test]
async fn integration_claim_rejects_changed_wu_and_duplicate_start() {
    let (_repo, _root, store, task, mut d) = fixture();
    let wu = integration(&store, task.id);
    let mut changed = wu.clone();
    changed.status = task_core::WorkUnitStatus::Superseded;
    store
        .work_units_apply(task.id, vec![], vec![changed], vec![])
        .unwrap();
    assert!(!store.try_start_work_unit_integration(task.id, &wu).unwrap());
    store
        .work_units_apply(task.id, vec![], vec![wu.clone()], vec![])
        .unwrap();
    d.start_integration(&task, &wu.id).unwrap();
    assert!(!store.try_start_work_unit_integration(task.id, &wu).unwrap());
    assert_eq!(
        events_of(&store, task.id)
            .iter()
            .filter(
                |e| matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == "integrate")
            )
            .count(),
        1
    );
    d.abort_all_runs();
}

/// 2026-10-20 ADR: 段の統合の検査が走っている間に lease が切れても、task を回収（failed 等）せず lease を延ばす。
#[tokio::test]
async fn expired_lease_during_stage_integration_is_extended_not_reclaimed() {
    let (_repo, _root, store, task, mut d) = fixture_with_lease(Duration::from_millis(1));
    // start_integration は lease を integration_ttl まで延ばす。それより検査が長引く状況にする。
    d.config.review_timeout = Duration::from_millis(1);
    d.config.lease_grace = Duration::ZERO;
    let wu = integration(&store, task.id);
    d.start_integration(&task, &wu.id).unwrap();
    assert!(d.integrating.contains_key(&task.id));
    std::thread::sleep(Duration::from_millis(20));
    let before = store.get(task.id).unwrap().unwrap();
    assert!(before.lease.as_ref().unwrap().expires_at <= OffsetDateTime::now_utc());
    d.reclaim_expired_leases().unwrap();
    let after = store.get(task.id).unwrap().unwrap();
    assert_eq!(after.status, Status::Running);
    assert!(after.lease.as_ref().unwrap().expires_at > OffsetDateTime::now_utc());
    assert!(d.integrating.contains_key(&task.id));
    run_until_idle(&mut d, 400).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

/// 検査の時間切れが replan 上限後に繰り返されたら、task は failed にならず、`integration_check_timeout` と
/// 止まった cmd を含む理由で blocked の質問になる。
#[tokio::test]
async fn repeated_integration_check_timeout_blocks_with_a_readable_reason() {
    let (_repo, _root, store, task, mut d) = fixture();
    d.config.execution.max_replans = 0;
    d.config.execution.max_repairs = 0;
    let wu = integration(&store, task.id);
    d.start_integration(&task, &wu.id).unwrap();
    let cmd = "cargo test -p x 2>&1 | tail -20";
    let run = IntegrationRun {
        checks: vec![(
            cmd.to_string(),
            false,
            format!("command timed out after 1800s: {cmd}"),
        )],
        ..Default::default()
    };
    d.on_integration_finished(task.id, &wu.id, Ok(run)).unwrap();
    assert_eq!(
        integration(&store, task.id).status,
        task_core::WorkUnitStatus::Blocked
    );
    assert_ne!(store.get(task.id).unwrap().unwrap().status, Status::Failed);
    let text = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::QuestionRaised { text, .. } => Some(text),
            _ => None,
        })
        .expect("a question is raised");
    assert!(text.contains("integration_check_timeout"), "{text}");
    assert!(text.contains(cmd), "{text}");
    d.abort_all_runs();
}
