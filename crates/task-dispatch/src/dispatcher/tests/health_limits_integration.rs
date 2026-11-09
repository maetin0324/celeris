//! ADR 2026-11-08-health-enforcement-loop D4.3・D4.4: WU の check と段の統合で、health の同じ指摘の往復と、段の統合の
//! health の修復の上限の使い切りが、WU の retry・統合の repair・replan を起こさず、Blocked（質問）と木の root の受信箱
//! 1 件で止まる流れの試験。
//!
//! `health_review::limits` の試験は events の列だけを見て往復を判定し、受信箱の試験（task-ops の
//! `health_limits_inbox_*`）は `HealthFindingsRepeated` を直接書く。経路が判定を呼んで retry・repair・replan を止めるかは
//! どちらも通らないので、ここで dispatcher の流れで確かめる。検査器は試験の repo の base に commit した
//! `health_enforce` の stub（指摘の fingerprint の番号は作業ツリーの `FP` の数、無ければ 1）。外部ネットワークに出ず、LLM
//! を呼ばない。時刻に依らず、dispatcher が静まるまで tick を回す出来事待ち（`run_until_idle`）で決まる。
use super::health_enforce::{commit_base_checker, health_items, unit, with_check};
use super::*;
use task_core::HealthRepeatSite;
use task_core::required_checks::health::HealthVerdict;

/// `id` の task の `HealthFindingsRepeated` の `(源, 位置, fingerprint)`。
fn repeats_on(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
) -> Vec<(TaskId, HealthRepeatSite, Vec<String>)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::HealthFindingsRepeated {
                source_task,
                site,
                fingerprints,
                ..
            } => Some((source_task, site, fingerprints)),
            _ => None,
        })
        .collect()
}

/// `id` の task の `WorkUnitChecksFailed`（WU の retry）のうち WU `work_unit_id` のものの数。
fn work_unit_retries(events: &[Event], work_unit_id: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::WorkUnitChecksFailed { work_unit_id: w, .. } if w == work_unit_id))
        .count()
}

/// 段の統合の `RepairScheduled` の数。
fn repairs_scheduled(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::RepairScheduled { .. }))
        .count()
}

/// `QuestionRaised` の本文（段の統合が止めた質問）。
fn questions(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::QuestionRaised { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn replanned(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::AutoReplanRequested { .. }))
}

/// WU の check の試験の土台: base に stub の検査器（FIXED が無いときの exit は `exit`）を commit した repo の task（親は
/// `parent`）に、checks（cmd は `check`）を持つ WU `a` を 1 件だけ置く。
fn work_unit_task(
    repo: &Path,
    store: &Arc<dyn TaskStore>,
    (exit, check): (i32, &str),
    parent: Option<TaskId>,
) -> Task {
    init_test_repo(repo);
    commit_base_checker(repo, exit);
    let mut task = parallel_task(repo, "true");
    task.parent_id = parent;
    store.insert(&task).unwrap();
    let mut spec = with_check(v2_wu("a", "build", &[]));
    spec.checks[0].cmd = check.to_string();
    adopt_v2_plan(store, task.id, &["build"], vec![spec]);
    task
}

/// 段の統合の試験の土台: base に stub の検査器を commit した repo の task に、葉の WU 2 件（checks 無し。WU の check は
/// 走らない）を 1 段で置く。
fn integration_task(repo: &Path, store: &Arc<dyn TaskStore>, exit: i32) -> Task {
    init_test_repo(repo);
    commit_base_checker(repo, exit);
    let task = parallel_task(repo, "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), v2_wu("b", "build", &[])],
    );
    task
}

fn integration_adapter() -> ParallelWuAdapter {
    ParallelWuAdapter::new(Duration::from_millis(5))
        .with_file("a", "a.txt", "a\n")
        .with_file("b", "b.txt", "b\n")
}

/// 不変条件（D4.3）: WU の check の health だけの不合格が、WU の retry の後も同じ fingerprint の集合で残ったら、WU を
/// もう一度 retry せず、WU と task を Blocked（質問 `health_findings_repeated:`）にし、木の root（root の task なら自分）
/// に `HealthFindingsRepeated` を 1 件追記する。`health_enforce_work_unit_check_fail_routes_to_work_unit_repair` は最初の
/// Fail が retry になることしか見ず、2 回目の同じ Fail で止まることを検出できない。
#[tokio::test]
async fn health_limits_work_unit_same_fingerprint_blocks() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = work_unit_task(repo.path(), &store, (1, "true"), None);
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;

    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let a = unit(&store, task.id, "a");
    assert_eq!(a.status, task_core::WorkUnitStatus::Blocked, "{a:?}");
    assert_eq!(
        a.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Question)
    );
    let events = events_of(&store, task.id);
    let evaluations: Vec<(bool, Vec<String>)> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == a.id)
        .map(|(_, pass, h)| {
            assert_eq!(h.verdict, HealthVerdict::Fail, "{h:?}");
            (pass, h.blocking)
        })
        .collect();
    let fp = vec!["deps.violation:0000000000000001".to_string()];
    assert_eq!(
        evaluations,
        vec![(false, fp.clone()), (false, fp.clone())],
        "the second evaluation after the retry must be the last one"
    );
    assert_eq!(
        work_unit_retries(&events, &a.id),
        1,
        "only the first Fail retries"
    );
    assert_eq!(
        repeats_on(&store, task.id),
        vec![(task.id, HealthRepeatSite::WorkUnit { key: "a".into() }, fp)]
    );
    let asked = serde_json::to_string(&events).unwrap();
    assert!(
        asked.contains("health_findings_repeated: WU a の check: "),
        "{asked}"
    );
    assert!(
        !replanned(&events),
        "a repeated health Fail must not replan"
    );
}

/// 不変条件（D4.3）: 修復の後に指摘が変わる（前の fingerprint が消え、新しい fingerprint が出る）WU の check は往復では
/// ない。今どおり WU の retry を続け、Blocked にも root への追記にもしない（往復の判定が retry を全部止めていないこと）。
#[tokio::test]
async fn health_limits_work_unit_different_findings_still_retry() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = work_unit_task(repo.path(), &store, (1, "true"), None);
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = runs.clone();
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5)).with_action("a", move |dir| {
            let n = seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            std::fs::write(dir.join("FP"), n.to_string()).unwrap();
        }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;

    let events = events_of(&store, task.id);
    let a = unit(&store, task.id, "a");
    let sets: Vec<Vec<String>> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == a.id)
        .map(|(_, _, h)| h.blocking)
        .collect();
    assert!(sets.len() >= 2, "{sets:?}");
    assert!(
        sets.windows(2).all(|w| w[0] != w[1]),
        "every evaluation must have a new finding: {sets:?}"
    );
    assert!(
        work_unit_retries(&events, &a.id) >= 2,
        "different findings must keep the work unit retry"
    );
    assert!(repeats_on(&store, task.id).is_empty());
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains("health_findings_repeated:"), "{text}");
    assert_ne!(a.status, task_core::WorkUnitStatus::Done, "{a:?}");
}

/// 不変条件（D4.3）: health の Fail に health でない検査の失敗が混ざる WU の check は、health の指摘が同じでも往復で
/// 止めず、今どおり WU の retry にする（health でない失敗の扱いを変えない）。
#[tokio::test]
async fn health_limits_work_unit_mixed_failure_keeps_retry() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = work_unit_task(repo.path(), &store, (1, "false"), None);
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_idle(&mut d, 800).await;

    let events = events_of(&store, task.id);
    let a = unit(&store, task.id, "a");
    let same: Vec<Vec<String>> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == a.id)
        .map(|(_, _, h)| h.blocking)
        .collect();
    assert!(
        same.len() >= 2 && same.windows(2).all(|w| w[0] == w[1]),
        "{same:?}"
    );
    assert!(work_unit_retries(&events, &a.id) >= 2);
    assert!(repeats_on(&store, task.id).is_empty());
}

/// 不変条件（D4.3）: 2 つの WU（同じ木の別の子 task）が同じ fingerprint の集合で往復したら、どちらも Blocked になり、
/// 往復は木の root に 2 件追記されるが、root の受信箱の `health_repeat` は `(root, 集合)` ごとに 1 件で、源に 2 つの task が
/// 並ぶ。task-ops の受信箱の試験は event を直接書くので、経路が root を求めて書くこと（子の task 自身に書かない）を
/// 検出できない。
#[tokio::test]
async fn health_limits_same_fingerprint_escalates_once() {
    let plain = tempfile::tempdir().unwrap();
    let (repo_a, repo_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut top = new_task(
        plain.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    top.status = Status::Draft;
    store.insert(&top).unwrap();
    let children: Vec<Task> = [repo_a.path(), repo_b.path()]
        .into_iter()
        .map(|repo| work_unit_task(repo, &store, (1, "true"), Some(top.id)))
        .collect();
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 4, 4, 2);
    run_until_idle(&mut d, 1200).await;

    for child in &children {
        let stored = store.get(child.id).unwrap().unwrap();
        assert_eq!(stored.status, Status::Blocked, "{stored:?}");
        assert!(
            repeats_on(&store, child.id).is_empty(),
            "a child must not keep the repeat on itself"
        );
    }
    let on_root = repeats_on(&store, top.id);
    let mut sources: Vec<TaskId> = on_root.iter().map(|(s, ..)| *s).collect();
    sources.sort();
    let mut want: Vec<TaskId> = children.iter().map(|c| c.id).collect();
    want.sort();
    assert_eq!(sources, want, "{on_root:?}");

    let ctx = task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(
            root.path().to_path_buf(),
            None,
        ),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    };
    let inbox = task_ops::human_inbox::human_inbox(
        store.as_ref(),
        None,
        &ctx,
        OffsetDateTime::now_utc(),
        &|_, _| vec![],
    )
    .unwrap();
    let items: Vec<&task_ops::human_inbox::InboxItem> = inbox
        .items
        .iter()
        .filter(|i| i.kind == task_ops::human_inbox::InboxKind::HealthRepeat)
        .collect();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].task.as_ref().map(|t| t.id), Some(top.id));
    let mut blocking: Vec<TaskId> = items[0].blocking.tasks.iter().map(|t| t.id).collect();
    blocking.sort();
    assert_eq!(blocking, want);
}

/// 不変条件（D4.3）: 段の統合の health だけの不合格が、統合の修復の後も同じ fingerprint の集合で残ったら、2 つ目の
/// repair も replan も作らず、統合 WU と task を Blocked（質問 `health_findings_repeated:`）にし、root に 1 件追記する。
/// `health_enforce_phase_integration_fail_schedules_repair` は修復が直す流れしか見ない。
#[tokio::test]
async fn health_limits_integration_same_fingerprint_blocks() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = integration_task(repo.path(), &store, 1);
    // 修復は commit を残すが指摘は直さない（同じ fingerprint が残る）。
    let adapter =
        Arc::new(integration_adapter().with_file("repair-build-1", "note.txt", "tried\n"));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;

    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let integ = unit(&store, task.id, "integrate-build");
    assert_eq!(
        integ.status,
        task_core::WorkUnitStatus::Blocked,
        "{integ:?}"
    );
    let events = events_of(&store, task.id);
    let evaluations: Vec<(bool, Vec<String>)> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == integ.id)
        .map(|(_, pass, h)| (pass, h.blocking))
        .collect();
    let fp = vec!["deps.violation:0000000000000001".to_string()];
    assert_eq!(evaluations, vec![(false, fp.clone()), (false, fp.clone())]);
    assert_eq!(repairs_scheduled(&events), 1, "no second repair");
    assert!(
        !replanned(&events),
        "a repeated health Fail must not replan"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::PhaseIntegrated { .. }))
    );
    assert_eq!(
        repeats_on(&store, task.id),
        vec![(
            task.id,
            HealthRepeatSite::Phase {
                key: "build".into()
            },
            fp
        )]
    );
    let asked = questions(&events);
    assert!(
        asked
            .iter()
            .any(|q| q.starts_with("health_findings_repeated: 段 build の統合: ")),
        "{asked:?}"
    );
}

/// 不変条件（D4.4）: 段の統合の health の修復が `max_repairs_per_class`（bucket `health:deps.violation`、既定 2）を使い
/// 切ったら、指摘が毎回変わる（往復でない）ときも replan せず、統合 WU と task を Blocked（質問 `health_repair_limit:`）
/// で止める。以前は `integration_gives_up_with` が `max_replans` の余りで replan していた。
#[tokio::test]
async fn health_limits_integration_repair_cap_blocks() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = integration_task(repo.path(), &store, 1);
    // 修復ごとに指摘の fingerprint が変わる（1 → 2 → 3）。
    let adapter = Arc::new(
        integration_adapter()
            .with_file("repair-build-1", "FP", "2")
            .with_file("repair-build-2", "FP", "3"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    assert_eq!(d.config.execution.max_repairs_per_class, 2);
    assert!(d.config.execution.max_repairs > 2);
    run_until_idle(&mut d, 800).await;

    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let integ = unit(&store, task.id, "integrate-build");
    assert_eq!(
        integ.status,
        task_core::WorkUnitStatus::Blocked,
        "{integ:?}"
    );
    let events = events_of(&store, task.id);
    let sets: Vec<Vec<String>> = health_items(&events)
        .into_iter()
        .filter(|(id, ..)| *id == integ.id)
        .map(|(_, _, h)| h.blocking)
        .collect();
    assert_eq!(sets.len(), 3, "{sets:?}");
    assert_eq!(repairs_scheduled(&events), 2);
    assert!(
        !replanned(&events),
        "the exhausted health repair must not replan"
    );
    assert!(repeats_on(&store, task.id).is_empty(), "not a repeat");
    let asked = questions(&events);
    assert!(
        asked
            .iter()
            .any(|q| q.starts_with("health_repair_limit: 段 build の統合: ")
                && q.contains("health:deps.violation の修復 2/2 回")),
        "{asked:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::PhaseIntegrated { .. }))
    );
}

/// 不変条件（D3.4・D4.4）: 段の統合の health の Error（検査器の exit 2）は、上限の判定・往復の判定に入らず、合格にも
/// repair・replan にもならず、統合 WU と task を Blocked（`health_check_error:`）で止める。replan を止める判定を
/// `stops_without_replan` に移したので、Error が replan に戻らないことを固定する（Unavailable は
/// `health_enforce_phase_integration_unavailable_blocks_task` が見る）。
#[tokio::test]
async fn health_limits_integration_error_blocks_without_repair() {
    let repo = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = integration_task(repo.path(), &store, 2);
    let adapter = Arc::new(integration_adapter());
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;

    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let integ = unit(&store, task.id, "integrate-build");
    assert_eq!(
        integ.status,
        task_core::WorkUnitStatus::Blocked,
        "{integ:?}"
    );
    let events = events_of(&store, task.id);
    let items = health_items(&events);
    assert!(
        items
            .iter()
            .any(|(id, pass, h)| *id == integ.id && !pass && h.verdict == HealthVerdict::Error),
        "{items:?}"
    );
    assert_eq!(repairs_scheduled(&events), 0);
    assert!(!replanned(&events));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::PhaseIntegrated { .. })),
        "an Error must not pass"
    );
    assert!(repeats_on(&store, task.id).is_empty());
    let asked = questions(&events);
    assert!(
        asked.iter().any(|q| q.starts_with("health_check_error: ")),
        "{asked:?}"
    );
}
