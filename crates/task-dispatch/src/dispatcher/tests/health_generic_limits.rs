//! ADR 2026-11-10-health-generic-fixtures (3) の (h)（unit rs-limits）: Celeris でない isolated Git fixture と本物の
//! 検査器で、health の修復の上限・同じ指摘の往復・変更の無い repo の棚卸しの省略が、Celeris の repo と同じに働くことを
//! dispatcher の流れで確かめる。
//!
//! 既存の試験で足りない理由: `health_limits_review_*`・`health_limits_integration_*` は repo の base に commit した stub の
//! 検査器（決まった fingerprint を返す）で上限と往復を通し、`health_inventory_*`（`health_review/inventory_tests.rs`・
//! `inventory_filing_tests.rs`・`dispatcher/tests/health_inventory.rs`）も stub の検査器か、検査器を持つ一時 repo を使う。
//! 本物の検査器の fingerprint（層の逆向きの edge と file から決まる）が修復ごとに変わる・変わらないことで上限と往復の
//! 判定が分かれること、本物の検査器の差分の判定（同じ違反は 2 回目に `existing`）と棚卸しの省略・起票の重複防止が
//! 噛み合うことは、どれも確かめない。
//!
//! fixture は builder の CLI で `$TMPDIR` の一時 repo に作り（[`build_fixture`]）、内容を Rust に写さない（ADR の K2）。
//! 修復の代わりの変更は、候補が変えた file の `git mv` と、関係の無い file の commit だけ。検査器は
//! [`configure_real_bundled_checker`]（作業ツリーの `scripts/dev/repo-health/` を同梱の配置に写したもの）。外部
//! ネットワークに出ず、LLM を呼ばない。時刻に依らず、出来事が尽きるまで tick を回す（`run_until_idle`）か、注入した
//! 時計で cron を 1 回ずつ発火させる。
use super::health_enforce::{RepairActionAdapter, non_celeris_dispatcher};
use super::health_generic::{
    Fixture, apply_candidate, blocking_rules, build_fixture, changed_paths,
    configure_real_bundled_checker, dep_inversion_rules, repair_classes, review_health,
};
use super::target_sync::{commit_file, reviewing_task};
use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::HealthRepeatSite;
use task_core::required_checks::health::{CheckerId, HealthEvidence, HealthVerdict};
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};
use task_ops::health_debt::{DEBT_KEY_PREFIX, DEBT_LABEL};
use task_ops::human_inbox::InboxKind;
use time::format_description::well_known::Rfc3339;

fn view_ctx() -> task_ops::view::ViewContext {
    task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(
            PathBuf::from("/nonexistent"),
            None,
        ),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    }
}

/// dep-inversion の review の repair の class（fail 級の rule の bucket `health:<rule_id>` を `,` で並べたもの）。
fn dep_inversion_class(kind: &str) -> String {
    dep_inversion_rules(kind)
        .iter()
        .map(|rule| format!("health:{rule}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// fixture の dep-inversion の候補を作業ツリーに入れた、`Reviewing` の atomic の task（親は `parent`）。返す dir は
/// fixture の repo の親で、試験の終わりまで持つ。
fn reviewing_dep_inversion(
    d: &Dispatcher,
    store: &Arc<dyn TaskStore>,
    kind: &str,
    parent: Option<TaskId>,
) -> (tempfile::TempDir, Fixture, Task) {
    let fixtures = tempfile::tempdir().unwrap();
    let repo = fixtures.path().join(kind);
    let fx = build_fixture(kind, "dep-inversion", &repo);
    let mut task = reviewing_task(&repo, store);
    task.parent_id = parent;
    store.insert(&task).unwrap();
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), fx.base());
    apply_candidate(&wt.dir, fx.head());
    (fixtures, fx, task)
}

/// review の health の項目のうち、本物の同梱の検査器の dep-inversion の Fail（fail 級の rule は
/// [`dep_inversion_rules`] だけ）の evidence を順に返す。どれかが合格・別の検査器・別の rule なら落とす。
fn dep_inversion_fails(kind: &str, events: &[Event], bundled: &CheckerId) -> Vec<HealthEvidence> {
    review_health(events)
        .into_iter()
        .map(|(pass, evidence)| {
            assert!(!pass, "{evidence:?}");
            assert_eq!(evidence.verdict, HealthVerdict::Fail, "{evidence:?}");
            assert_eq!(evidence.checker.as_ref(), Some(bundled), "{evidence:?}");
            assert_eq!(
                blocking_rules(&evidence),
                dep_inversion_rules(kind),
                "{evidence:?}"
            );
            evidence
        })
        .collect()
}

/// 止めた後の不変条件: `Reviewing → Blocked`（`review_blocked`）が 1 回で attempts は 0 のまま、その後に run も repair も
/// 起こさず、`review_fail`（再実行・retry 切れの Failed）と自動 replan が無い。
fn assert_blocked_without_more_repairs(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<Event> {
    let stored = store.get(id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert_eq!(stored.attempts, 0, "{stored:?}");
    let events = events_of(store, id);
    let blocked: Vec<usize> = (events.iter().enumerate())
        .filter(|(_, e)| {
            matches!(e, Event::Transitioned { from: Status::Reviewing, to: Status::Blocked, reason, .. }
                if reason == Trigger::ReviewBlocked.name())
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(blocked.len(), 1, "{events:?}");
    assert!(
        !events[blocked[0]..].iter().any(|e| matches!(
            e,
            Event::WorkerStarted { .. } | Event::RepairScheduled { .. }
        )),
        "no run or repair after the block: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::Transitioned { reason, .. } if reason == Trigger::ReviewFail.name()
        )),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::AutoReplanRequested { .. })),
        "{events:?}"
    );
    events
}

/// `id` の task が上げた質問（受信箱の `questions` の本文）。
fn inbox_questions(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    task_ops::inbox::inbox(
        store.as_ref(),
        None,
        &view_ctx(),
        OffsetDateTime::now_utc(),
        &|_, _| Vec::new(),
    )
    .unwrap()
    .questions
    .into_iter()
    .filter(|q| q.task.id == id)
    .map(|q| q.question)
    .collect()
}

/// 不変条件（G3 の D4.4 を非 Celeris の repo と本物の検査器で）: atomic の task の review で、層の逆向きの依存の
/// health の Fail が class `health:deps.violation` の修復を `max_repairs_per_class`（既定 2）回使い切ったら、修復の
/// 合計の上限（既定 3）に余りがあっても 3 回目の repair も再実行（`review_fail`）も replan もせず、task を Blocked に
/// して受信箱に bucket と回数を書いた質問を 1 件出す。修復は逆向きの import の file を `git mv` で別の名前に移すだけで
/// 依存は直らず、本物の検査器の fingerprint（edge と file で決まる）が毎回変わるので、往復（D4.3）ではなく上限で
/// 止まる。`health_limits_review_repair_cap_blocks_without_rerun` は stub の検査器で `max_repairs_per_class` を 1 に
/// 縮めて確かめ、本物の検査器の fingerprint が修復で変わる場合を通らない。rust の fixture の逆向きの依存は
/// `Cargo.toml` の行で、内容を書かずに file を移せないので ts だけで確かめる。
#[tokio::test]
async fn health_generic_limits_repair_cap_blocks() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // `(候補が足した逆向きの import の file, 今の path)`。修復 `repair-<n>` は今の path を `<stem>-repair-<n>.<ext>` に移す。
    let moved = Arc::new(StdMutex::new((String::new(), String::new())));
    let current = moved.clone();
    let adapter = Arc::new(RepairActionAdapter::new(move |dir, key| {
        let mut guard = current.lock().unwrap();
        let (original, path) = &mut *guard;
        let (stem, ext) = original
            .rsplit_once('.')
            .expect("the inverted file has an extension");
        let next = format!("{stem}-{key}.{ext}");
        git_out(dir, &["mv", path.as_str(), &next]);
        git_out(
            dir,
            &["commit", "-q", "-m", &format!("move {path} to {next}")],
        );
        *path = next;
    }));
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    assert_eq!(d.config.execution.max_repairs_per_class, 2);
    assert!(d.config.execution.max_repairs > 2);
    let (fixtures, fx, task) = reviewing_dep_inversion(&d, &store, "ts", None);
    let changed = changed_paths(&fixtures.path().join("ts"), &fx);
    assert_eq!(changed.len(), 1, "{changed:?}");
    *moved.lock().unwrap() = (changed[0].clone(), changed[0].clone());

    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);

    let events = assert_blocked_without_more_repairs(&store, task.id);
    let fails = dep_inversion_fails("ts", &events, &bundled);
    let sets: Vec<&Vec<String>> = fails.iter().map(|e| &e.blocking).collect();
    assert_eq!(sets.len(), 3, "{sets:?}");
    let mut distinct = sets.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "every repair moved the finding: {sets:?}"
    );
    assert_eq!(fails[0].head_sha, fx.head());
    assert_eq!(
        repair_classes(&events),
        vec!["health:deps.violation".to_string(); 2]
    );
    assert_eq!(
        (store.work_units_for(task.id).unwrap().iter())
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .count(),
        2
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::HealthFindingsRepeated { .. })),
        "a moving finding is not a repeat: {events:?}"
    );
    let questions = inbox_questions(&store, task.id);
    assert_eq!(questions.len(), 1, "{questions:?}");
    let q = &questions[0];
    assert!(q.starts_with("health_repair_limit: review: "), "{q}");
    assert!(q.contains("health:deps.violation の修復 2/2 回"), "{q}");
    assert!(q.contains("修復の合計 2/3 回"), "{q}");
}

/// 不変条件（G3 の D4.3 を非 Celeris の repo と本物の検査器で）: 1 つの木の 2 つの子 task が、それぞれの review で
/// 修復の後も同じ fingerprint の集合（本物の検査器の dep-inversion の fail 級）を出したら、どちらも 2 回目の repair を
/// 作らず Blocked になり、往復は木の root に 2 件追記され（子には書かない）、root の受信箱の `health_repeat` は
/// `(root, 集合)` で 1 件になって源に 2 つの子が並ぶ。修復は関係の無い file を commit するだけ。ts と rust（Cargo の
/// 循環 `deps.cycle` も fail 級）で確かめる。`health_limits_same_fingerprint_escalates_once` は stub の検査器と WU の
/// check で、`health_limits_review_same_fingerprint_blocks` は stub の検査器と root の task 自身で確かめ、本物の検査器の
/// fingerprint が repo をまたいで一致し 1 件に束ねられることを通らない。
#[tokio::test]
async fn health_generic_limits_repeat_escalates() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    for kind in ["ts", "rust"] {
        repeat_escalates(kind, &bundled).await;
    }
}

async fn repeat_escalates(kind: &str, bundled: &CheckerId) {
    let plain = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
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
    let adapter = Arc::new(RepairActionAdapter::new(|dir, key| {
        commit_file(dir, &format!("{key}.txt"), "tried\n");
    }));
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 2);
    let children: Vec<(tempfile::TempDir, Fixture, Task)> = (0..2)
        .map(|_| reviewing_dep_inversion(&d, &store, kind, Some(top.id)))
        .collect();
    for (_, _, child) in &children {
        assert!(
            d.spawn_review(child.id, "worker".into(), &ReviewSubject::default())
                .unwrap()
        );
    }
    assert!(run_until_idle(&mut d, 1200).await.idle);

    let mut want: Vec<TaskId> = children.iter().map(|(_, _, c)| c.id).collect();
    want.sort();
    let mut sets = Vec::new();
    for (_, fx, child) in &children {
        let events = assert_blocked_without_more_repairs(&store, child.id);
        let fails = dep_inversion_fails(kind, &events, bundled);
        assert_eq!(fails.len(), 2, "{kind}: {fails:?}");
        assert_eq!(fails[0].head_sha, fx.head());
        assert_ne!(fails[1].head_sha, fails[0].head_sha, "the repair committed");
        assert_eq!(fails[0].blocking, fails[1].blocking, "{kind}: {fails:?}");
        sets.push(fails[0].blocking.clone());
        assert_eq!(repair_classes(&events), vec![dep_inversion_class(kind)]);
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::HealthFindingsRepeated { source_task, .. } if *source_task == child.id
            )),
            "{kind}: a child must not keep the repeat on itself"
        );
        let asked = inbox_questions(&store, child.id);
        assert_eq!(asked.len(), 1, "{kind}: {asked:?}");
        assert!(
            asked[0].starts_with("health_findings_repeated: "),
            "{kind}: {asked:?}"
        );
    }
    assert_eq!(sets[0], sets[1], "{kind}: the same finding in both repos");

    let on_root: Vec<(TaskId, HealthRepeatSite, Vec<String>)> = events_of(&store, top.id)
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
        .collect();
    let mut sources: Vec<TaskId> = on_root.iter().map(|(s, ..)| *s).collect();
    sources.sort();
    assert_eq!(sources, want, "{kind}: {on_root:?}");
    for (_, site, fingerprints) in &on_root {
        assert_eq!(*site, HealthRepeatSite::Review, "{kind}: {on_root:?}");
        let mut fps = fingerprints.clone();
        fps.sort();
        let mut set = sets[0].clone();
        set.sort();
        assert_eq!(fps, set, "{kind}: {on_root:?}");
    }

    let inbox = task_ops::human_inbox::human_inbox(
        store.as_ref(),
        None,
        &view_ctx(),
        OffsetDateTime::now_utc(),
        &|_, _| vec![],
    )
    .unwrap();
    let items: Vec<&task_ops::human_inbox::InboxItem> = inbox
        .items
        .iter()
        .filter(|i| i.kind == InboxKind::HealthRepeat)
        .collect();
    assert_eq!(items.len(), 1, "{kind}: {items:?}");
    assert_eq!(items[0].task.as_ref().map(|t| t.id), Some(top.id));
    let mut blocking: Vec<TaskId> = items[0].blocking.tasks.iter().map(|t| t.id).collect();
    blocking.sort();
    assert_eq!(blocking, want, "{kind}: {items:?}");
}

// ---- cron の棚卸し ----

fn at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

/// 注入した時計を `hour` 時 0 分 30 秒に進めて tick を 1 回回し、始まった棚卸しの終わりを待つ（毎時の job の発火は
/// 1 時から数えて `hour` 回目）。
async fn fire(
    d: &mut Dispatcher,
    clock: &StdMutex<OffsetDateTime>,
    store: &SqliteStore,
    job: CronJobId,
    hour: u8,
) {
    *clock.lock().unwrap() = at(&format!("2026-11-09T{hour:02}:00:30Z"));
    d.tick().unwrap();
    d.health_inventory.join_all().await;
    let runs = store.cron_job_runs(job, None).unwrap();
    assert_eq!(runs.len(), usize::from(hour), "{runs:?}");
}

/// 1 回の発火の表 `health_inventory_runs` の集計 `(repos, evaluated, unchanged, tasks_created, existing_skipped)`。
fn fire_counts(store: &SqliteStore, job: CronJobId, hour: u8) -> (u64, u64, u64, u64, u64) {
    let c = store
        .health_inventory_fire_counts(job, at(&format!("2026-11-09T{hour:02}:00:00Z")))
        .unwrap();
    (
        c.repos,
        c.evaluated,
        c.unchanged,
        c.tasks_created,
        c.existing_skipped,
    )
}

/// 不変条件（G3 の D6.3〜D6.6 を非 Celeris の repo と本物の検査器で）: 毎時の cron `health_inventory` が、1 つの案件の
/// ts と rust の fixture の repo（既定の branch `main`）を棚卸しする。
/// 1. 初回は base = head（fixture の base）で評価して Pass、起票しない。
/// 2. `main` が dep-inversion の候補へ進むと、前回の head を base に評価し、本物の検査器の fail 級の指摘を debt の
///    draft にする（fingerprint ごとに 1 件）。
/// 3. head・tree・policy（tree の中の profile）・検査器が同じ 2 回目は検査器を走らせず（`unchanged`）、起票しない。
///    tree の同じ空の commit で `main` が進んでも同じ。
/// 4. `main` を base に戻して評価し（Pass）、もう一度候補へ進めて同じ指摘が出ても、開いた debt があるので 2 件目を
///    作らない（`existing_skipped`）。冪等キーは fingerprint ごとに 1 つのまま。
///
/// `health_inventory_skips_unchanged_repo`・`health_inventory_does_not_file_same_debt_twice_across_fires` は stub の
/// 検査器（指摘が毎回 new）で runner を直接呼び、`health_inventory_does_not_call_llm` は stub の 1 回の起票だけを見る。
/// 本物の検査器の差分の判定と省略・起票の重複防止の組み合わせを cron の tick から通しては確かめない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_generic_limits_inventory_skips_unchanged() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let dir = crate::test_support::WritableTempDir::new();
    let db = dir.path().join("celeris.db");
    let sqlite = Arc::new(SqliteStore::open(&db).unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let clock = Arc::new(StdMutex::new(at("2026-11-09T00:00:00Z")));
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: task_core::ProjectId::new(),
        title: "orchard".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        workspace: None,
        created_at: at("2026-11-09T00:00:00Z"),
        updated_at: at("2026-11-09T00:00:00Z"),
    };
    sqlite.project_create(&project).unwrap();
    let fixtures = tempfile::tempdir().unwrap();
    let repos: Vec<(PathBuf, Fixture, task_core::ProjectRepo)> = ["ts", "rust"]
        .into_iter()
        .map(|kind| {
            let path = fixtures.path().join(kind);
            let fx = build_fixture(kind, "dep-inversion", &path);
            assert_eq!(git_out(&path, &["rev-parse", "main"]), fx.base());
            let repo = task_core::ProjectRepo {
                auto_integrate: false,
                id: task_core::RepoId::new(),
                project_id: project.id,
                name: kind.into(),
                kind: task_core::RepoKind::Git,
                location: task_core::WorkspaceSpec::local(path.to_string_lossy().as_ref()),
                default_branch: Some("main".into()),
                sync: None,
                run: task_core::RepoRun::Auto,
                is_primary: false,
                created_at: at("2026-11-09T00:00:00Z"),
            };
            sqlite.repo_create(&repo).unwrap();
            (path, fx, repo)
        })
        .collect();
    let mut template = CronTaskTemplate {
        title: "棚卸し".into(),
        ..CronTaskTemplate::default()
    };
    template
        .extra
        .insert("kind".into(), serde_json::json!("health_inventory"));
    template.extra.insert(
        "project_id".into(),
        serde_json::json!(project.id.to_string()),
    );
    let job = create_job(
        sqlite.as_ref(),
        &CronFireContext::default(),
        NewCronJob {
            name: "inventory".into(),
            schedule: "0 * * * *".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up: CronCatchUp::Latest,
            enabled: true,
            template,
        },
        at("2026-11-09T00:00:00Z"),
    )
    .unwrap();
    let adapter: Arc<dyn WorkerAdapter> =
        Arc::new(task_worker::fake::FakeAdapter::new(vec!["false".into()]));
    let ws = tempfile::tempdir().unwrap();
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    d.test_now = Some(clock.clone());
    d.config.execution.max_cos_runs = 0;
    d.set_cos_chat_launch(
        sqlite.clone(),
        CosChatLaunchConfig {
            enabled: true,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            inline_read_max_commands: 20,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: db.clone(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    let set_main = |path: &Path, sha: &str| {
        git_out(path, &["update-ref", "refs/heads/main", sha]);
    };
    let debts = || -> Vec<Task> {
        store
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|t| t.labels.iter().any(|l| l == DEBT_LABEL))
            .collect()
    };
    let latest = |repo: &task_core::ProjectRepo| -> HealthEvidence {
        sqlite
            .health_inventory_latest_evaluated(job.id, repo.id)
            .unwrap()
            .and_then(|row| row.evidence)
            .unwrap_or_else(|| panic!("no evaluated row for {}", repo.name))
    };

    // 1. 初回: base = head = fixture の base で Pass。起票しない。
    fire(&mut d, &clock, &sqlite, job.id, 1).await;
    assert_eq!(fire_counts(&sqlite, job.id, 1), (2, 2, 0, 0, 0));
    for (_, fx, repo) in &repos {
        let ev = latest(repo);
        assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
        assert_eq!(ev.checker.as_ref(), Some(&bundled), "{ev:?}");
        assert_eq!(
            (ev.base_sha.as_str(), ev.head_sha.as_str()),
            (fx.base(), fx.base())
        );
    }
    assert!(debts().is_empty());

    // 2. main が dep-inversion の候補へ進む: 前回の head を base に Fail、指摘ごとに debt の draft を 1 件。
    for (path, fx, _) in &repos {
        set_main(path, fx.head());
    }
    fire(&mut d, &clock, &sqlite, job.id, 2).await;
    let mut fingerprints = Vec::new();
    for (_, fx, repo) in &repos {
        let ev = latest(repo);
        assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
        assert_eq!(ev.checker.as_ref(), Some(&bundled), "{ev:?}");
        assert_eq!(
            (ev.base_sha.as_str(), ev.head_sha.as_str()),
            (fx.base(), fx.head())
        );
        assert_eq!(
            blocking_rules(&ev),
            dep_inversion_rules(&repo.name),
            "{ev:?}"
        );
        fingerprints.extend(ev.blocking.iter().map(|fp| (repo.id, fp.clone())));
    }
    let filed = debts();
    assert_eq!(filed.len(), fingerprints.len(), "{filed:?}");
    assert!(filed.iter().all(|t| t.status == Status::Draft), "{filed:?}");
    // ts は `deps.violation` の 1 件、rust は `deps.cycle` と `deps.violation` の 2 件（1 回の発火の枠 3 件の中）。
    assert_eq!(fingerprints.len(), 3, "{fingerprints:?}");
    let n = fingerprints.len() as u64;
    assert_eq!(fire_counts(&sqlite, job.id, 2), (2, 2, 0, n, 0));
    let first_ids: Vec<TaskId> = filed.iter().map(|t| t.id).collect();

    // 3. 変わらない 2 回目は検査器を走らせない。tree の同じ空の commit でも同じ。
    fire(&mut d, &clock, &sqlite, job.id, 3).await;
    assert_eq!(fire_counts(&sqlite, job.id, 3), (2, 0, 2, 0, 0));
    for (path, fx, _) in &repos {
        let tree = git_out(path, &["rev-parse", &format!("{}^{{tree}}", fx.head())]);
        let same_tree = git_out(
            path,
            &["commit-tree", &tree, "-p", fx.head(), "-m", "same tree"],
        );
        set_main(path, &same_tree);
    }
    fire(&mut d, &clock, &sqlite, job.id, 4).await;
    assert_eq!(fire_counts(&sqlite, job.id, 4), (2, 0, 2, 0, 0));
    for (_, fx, repo) in &repos {
        assert_eq!(latest(repo).head_sha, fx.head(), "the skip keeps the base");
    }

    // 4. main を base に戻して Pass、もう一度候補へ進めて同じ指摘: 開いた debt があるので 2 件目を作らない。
    for (path, fx, _) in &repos {
        set_main(path, fx.base());
    }
    fire(&mut d, &clock, &sqlite, job.id, 5).await;
    assert_eq!(fire_counts(&sqlite, job.id, 5), (2, 2, 0, 0, 0));
    for (_, _, repo) in &repos {
        assert_eq!(latest(repo).verdict, HealthVerdict::Pass);
    }
    for (path, fx, _) in &repos {
        set_main(path, fx.head());
    }
    fire(&mut d, &clock, &sqlite, job.id, 6).await;
    assert_eq!(fire_counts(&sqlite, job.id, 6), (2, 2, 0, 0, n));
    for (_, _, repo) in &repos {
        let ev = latest(repo);
        assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
        let again: Vec<(task_core::RepoId, String)> =
            ev.blocking.iter().map(|fp| (repo.id, fp.clone())).collect();
        assert!(
            again.iter().all(|fp| fingerprints.contains(fp)),
            "the same findings: {again:?} vs {fingerprints:?}"
        );
    }
    assert_eq!(debts().iter().map(|t| t.id).collect::<Vec<_>>(), first_ids);
    let keys = sqlite
        .idempotency_keys_with_prefix(DEBT_KEY_PREFIX)
        .unwrap();
    assert_eq!(keys.len(), fingerprints.len(), "{keys:?}");
}
