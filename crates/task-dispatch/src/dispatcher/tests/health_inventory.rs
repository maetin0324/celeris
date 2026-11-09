//! ADR 2026-11-08-health-enforcement-loop D6.1: 全体棚卸しの Busy の印は Dispatcher の欄 `health_inventory` で、
//! `in_flight` に数える（draining の instance は棚卸しの終わりを待つ）。同じ job の発火は `skipped_overlap` になる。
//! 既存の cron の試験は task を作らない種類の sink を Dispatcher の外で見るだけで、この配線を検出できない。
//! 注入した時計・一時 DB だけで動く（LLM・run・外部ネットワークなし）。
//! `health_inventory_does_not_call_llm` は tick から起票までを通し、LLM の起動（worker の run・CoS chat の run）と
//! chat の入力が増えないことを見る（runner の試験は tick を通らず、Dispatcher の他の段が draft の debt を拾って
//! run を始めないことを検出できない）。

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::{Project, ProjectId, ProjectStatus};
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};
use task_worker::fake::FakeAdapter;
use time::format_description::well_known::Rfc3339;

fn at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

/// 一時 DB・案件・毎時の棚卸しの job・CoS chat の launch（store の持ち主）を持つ Dispatcher。
struct Setup {
    dir: crate::test_support::WritableTempDir,
    store: Arc<SqliteStore>,
    clock: Arc<StdMutex<OffsetDateTime>>,
    project: ProjectId,
    job: task_core::CronJob,
    d: Dispatcher,
}

fn setup() -> Setup {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
    let clock = Arc::new(StdMutex::new(at("2026-11-09T00:00:00Z")));
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: at("2026-11-09T00:00:00Z"),
        updated_at: at("2026-11-09T00:00:00Z"),
    };
    store.project_create(&project).unwrap();
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
        store.as_ref(),
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
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec!["false".into()]));
    let mut d = dispatcher(store.clone(), adapter, 0);
    d.test_now = Some(clock.clone());
    d.config.execution.max_cos_runs = 0;
    d.config.min_free_disk_mb = 0;
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            enabled: true,
            worker_reserve_five_hour: 0.90,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            inline_read_max_commands: 20,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path: dir.path().join("celeris.db"),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    Setup {
        dir,
        store,
        clock,
        project: project.id,
        job,
        d,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_inventory_running_mark_counts_in_flight_and_skips_overlap() {
    let Setup {
        store,
        clock,
        job,
        mut d,
        ..
    } = setup();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    d.health_inventory
        .start(job.id, move || {
            let _ = wait.recv();
        })
        .unwrap();
    assert_eq!(d.in_flight(), 1);

    *clock.lock().unwrap() = at("2026-11-09T01:00:30Z");
    d.tick().unwrap();
    let runs = store.cron_job_runs(job.id, None).unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].outcome, CronRunOutcome::SkippedOverlap, "{runs:?}");

    release.send(()).unwrap();
    d.health_inventory.join_all().await;
    assert_eq!(d.in_flight(), 0);
    *clock.lock().unwrap() = at("2026-11-09T02:00:30Z");
    d.tick().unwrap();
    d.health_inventory.join_all().await;
    let runs = store.cron_job_runs(job.id, None).unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    let created = runs
        .iter()
        .find(|r| r.outcome == CronRunOutcome::Created)
        .unwrap_or_else(|| panic!("{runs:?}"));
    assert!(
        created
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("0 git repo(s)"),
        "{runs:?}"
    );
    assert_eq!(d.in_flight(), 0);
}

/// 不変条件（D6.1・I4）: tick から起票まで通しても、worker の run・CoS chat の run・chat の入力（message）は
/// 増えない。起票した draft の debt は次の tick でも dispatch されない。数は表の行の集計に入る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_inventory_does_not_call_llm() {
    use crate::health_review::gate::tests::{
        base_policy, checker_files, commit, finding, init_repo, report, rule,
    };
    let Setup {
        dir,
        store,
        clock,
        project,
        job,
        mut d,
    } = setup();
    let text = report(
        "fail",
        1,
        vec![finding("size.x", 1, "error", "new", false)],
        base_policy(false),
        vec![rule("size.x", "fail", "")],
    );
    let stub = serde_json::json!({"exit": 1, "report": text}).to_string();
    let repo_dir = init_repo();
    let files = checker_files(&stub, true);
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    commit(repo_dir.path(), &refs, "checker");
    store
        .repo_create(&task_core::ProjectRepo {
            id: task_core::RepoId::new(),
            project_id: project,
            name: "code".into(),
            kind: task_core::RepoKind::Git,
            location: task_core::WorkspaceSpec::local(repo_dir.path().to_string_lossy().as_ref()),
            default_branch: None,
            sync: None,
            run: task_core::RepoRun::Auto,
            is_primary: false,
            created_at: at("2026-11-09T00:00:00Z"),
        })
        .unwrap();
    let count = |table: &str| -> i64 {
        rusqlite::Connection::open(dir.path().join("celeris.db"))
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    // 基準は job の予定時刻の前の tick の後（他の段が tick で作る固定の thread を基準に含める）。
    *clock.lock().unwrap() = at("2026-11-09T00:00:30Z");
    d.tick().unwrap();
    assert!(store.cron_job_runs(job.id, None).unwrap().is_empty());
    // chat_threads は数えない: draft の debt は受信箱の項目になり、CoS triage の取り込み（`chat::triage` の
    // `item_thread`）が案件の CoS thread を用意する。thread は入力でも LLM の起動でもない（message・run は 0）。
    let launches = ["runs", "chat_runs", "chat_messages"];
    let before: Vec<i64> = launches.iter().map(|t| count(t)).collect();

    *clock.lock().unwrap() = at("2026-11-09T01:00:30Z");
    d.tick().unwrap();
    d.health_inventory.join_all().await;
    *clock.lock().unwrap() = at("2026-11-09T01:01:30Z");
    d.tick().unwrap();
    d.health_inventory.join_all().await;

    let fired = store
        .health_inventory_fire_counts(job.id, at("2026-11-09T01:00:00Z"))
        .unwrap();
    assert_eq!(
        (fired.repos, fired.evaluated, fired.tasks_created),
        (1, 1, 1)
    );
    let tasks = store.list(None).unwrap();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].status, Status::Draft);
    let after: Vec<i64> = launches.iter().map(|t| count(t)).collect();
    assert_eq!(after, before, "{launches:?}");
}
