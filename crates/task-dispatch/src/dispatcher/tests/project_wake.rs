//! ADR 2026-11-04-project-controller A1: 案件の durable wake の E2E（接頭辞 `project_wake_`）。
//!
//! 一時 DB・注入した時計・fake decider だけで動く（LLM なし、外部ネットワークなし）。CoS chat は有効だが
//! `max_cos_runs = 0` で run は起きない（LLM 起動の数は `chat_runs`・`runs` の行数で数える）。crash は
//! 境界の hook で panic し、`catch_unwind` の後に store を開き直した新しい instance で続ける。
//! `PROJECT_WAKE_RAW_DIR` があれば `celeris.long-horizon-record/1` の JSONL を書く
//! （`scripts/dev/test-project-wake-replay.sh` が検査する）。

use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;

#[path = "project_wake_cursor.rs"]
mod cursor;
#[path = "project_wake_effects.rs"]
mod effects;
#[path = "long_horizon_bench.rs"]
mod long_horizon_bench;
#[path = "long_horizon_pilot.rs"]
mod long_horizon_pilot;
#[path = "project_cos.rs"]
mod project_cos;
#[path = "project_controller_replay.rs"]
mod replay;
#[path = "project_controller_soak.rs"]
mod soak;
use crate::project_controller::{
    Boundary, ProjectControllerSettings, ProjectDecider, ProjectDecision, ProjectEffect,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use task_core::project_wake::{
    ControllerMode, ControllerPhase, EffectState, ProjectDecideInput, ProjectWakeKind,
    root_idempotency_key,
};
use task_core::{Project, ProjectId, ProjectStatus};
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};
use task_ops::stagnation::{StagnationFinding, StagnationKind};
use task_worker::fake::FakeAdapter;
use time::format_description::well_known::Rfc3339;

type Calls = Arc<StdMutex<Vec<ProjectDecideInput>>>;
type DecideFn = Box<dyn FnMut(&ProjectDecideInput) -> Result<ProjectDecision, String> + Send>;
type HumanOp = Box<dyn FnOnce() + Send>;

struct Fake {
    calls: Calls,
    f: DecideFn,
}

impl ProjectDecider for Fake {
    fn decide(&mut self, input: &ProjectDecideInput) -> Result<ProjectDecision, String> {
        self.calls.lock().unwrap().push(input.clone());
        (self.f)(input)
    }
}

/// 人の操作の `human_epoch`（本番は API が書き込みと同じ transaction で呼ぶ。D12）。
fn human_bump(store: &SqliteStore, project: &str, now: OffsetDateTime) -> Option<i64> {
    store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::project_controller_human_bump_tx(tx, project, now, 0)
        })
        .unwrap()
}

fn at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339).unwrap()
}

const T0: &str = "2026-11-01T00:00:00Z";

struct Env {
    dir: crate::test_support::WritableTempDir,
    clock: Arc<StdMutex<OffsetDateTime>>,
    calls: Calls,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: crate::test_support::WritableTempDir::new(),
            clock: Arc::new(StdMutex::new(at(T0))),
            calls: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn db(&self) -> std::path::PathBuf {
        self.dir.path().join("celeris.db")
    }

    /// 開き直した store（再起動・別 instance の接続）。
    fn open(&self) -> Arc<SqliteStore> {
        Arc::new(SqliteStore::open(&self.db()).unwrap())
    }

    fn now(&self) -> OffsetDateTime {
        *self.clock.lock().unwrap()
    }

    fn advance(&self, secs: i64) {
        let mut c = self.clock.lock().unwrap();
        *c += time::Duration::seconds(secs);
    }

    /// daemon 1 個（`owner` が lease の持ち主）。2 回目の呼び出しは再起動。
    fn daemon(&self, store: &Arc<SqliteStore>, owner: &str, f: DecideFn) -> Dispatcher {
        self.daemon_with(store, owner, f, ProjectControllerSettings::default())
    }

    fn daemon_with(
        &self,
        store: &Arc<SqliteStore>,
        owner: &str,
        f: DecideFn,
        mut settings: ProjectControllerSettings,
    ) -> Dispatcher {
        let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec!["false".into()]));
        let mut d = dispatcher(store.clone(), adapter, 0);
        d.test_now = Some(self.clock.clone());
        d.config.execution.max_cos_runs = 0;
        d.config.min_free_disk_mb = 0;
        d.stagnation.checked_at = Some(at("2100-01-01T00:00:00Z"));
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
                data_dir: self.dir.path().to_path_buf(),
                db_path: self.db(),
                attachment_limits: Default::default(),
                api_base_url: "http://127.0.0.1:7700/api/v1".into(),
                web_base_url: None,
                triage: Default::default(),
            },
        );
        settings.enabled = true;
        let fake = Fake {
            calls: self.calls.clone(),
            f,
        };
        assert!(d.set_project_controller(settings, Box::new(fake), owner.into()));
        d
    }

    fn calls(&self) -> Vec<ProjectDecideInput> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, sql: &str) -> i64 {
        rusqlite::Connection::open(self.db())
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }

    /// LLM の起動の数: CoS run と worker の run の行。
    fn llm_launches(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM chat_runs") + self.count("SELECT COUNT(*) FROM runs")
    }
}

fn project(store: &SqliteStore, mode: Option<ControllerMode>) -> Project {
    let now = at(T0);
    let p = Project {
        id: ProjectId::new(),
        title: "pilot:wake".into(),
        request: "依頼".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        auto_integrate: false,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&p).unwrap();
    if let Some(mode) = mode {
        store
            .project_controller_set_mode(&p.id.to_string(), mode, None, "human:test", now)
            .unwrap();
    }
    p
}

/// 案件の root task（`status` で作る）。
fn root_task(env: &Env, store: &SqliteStore, p: &Project, status: Status) -> Task {
    let mut t = new_task(env.dir.path(), Check::Reviewer, 0);
    t.project_id = Some(p.id);
    t.status = status;
    store.create_task(&t, vec![]).unwrap();
    t
}

fn transition(store: &SqliteStore, t: &Task, to: Status, reason: &str) -> u64 {
    store
        .append_event(
            t.id,
            &Event::Transitioned {
                from: Status::Running,
                to,
                reason: reason.into(),
            },
        )
        .unwrap()
}

fn event_json(store: &SqliteStore, t: &Task, v: serde_json::Value) {
    let e: Event = serde_json::from_value(v).unwrap();
    store.append_event(t.id, &e).unwrap();
}

fn spec_json(title: &str) -> task_ops::add::NewTaskSpec {
    serde_json::from_value(serde_json::json!({
        "title": title,
        "objective": "続きの root",
        "acceptance": [{"type": "command", "cmd": "true"}],
    }))
    .unwrap()
}

/// 毎回同じ root を作る判定（I5 の key は判断 id を含まない）。
fn create_root(project: &Project) -> DecideFn {
    let key = root_idempotency_key(&project.id.to_string(), "outcome:o1:1");
    Box::new(move |_| {
        Ok(ProjectDecision {
            effect: ProjectEffect::CreateRoot {
                idempotency_key: key.clone(),
                spec: Box::new(spec_json("次の root")),
                meta: None,
                authority: None,
            },
            next_wake_after_secs: None,
            phase: ControllerPhase::Active,
            rule_id: "fake/create".into(),
        })
    })
}

fn none() -> DecideFn {
    Box::new(|_| Ok(ProjectDecision::none("fake/none")))
}

fn controller_roots(store: &SqliteStore, p: &Project) -> usize {
    store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|t| {
            t.project_id == Some(p.id)
                && t.labels
                    .iter()
                    .any(|l| l == crate::project_controller::CONTROLLER_LABEL)
        })
        .count()
}

fn tick(d: &mut Dispatcher) {
    d.tick_project_controller();
}

/// 初回の tick で全体の cursor を今の末尾に置く（過去の event を遡らない）。
fn prime(d: &mut Dispatcher) {
    tick(d);
}

fn record(file: &str, value: serde_json::Value) {
    let Some(dir) = std::env::var_os("PROJECT_WAKE_RAW_DIR") else {
        return;
    };
    let path = std::path::Path::new(&dir).join(file);
    let mut v = value;
    v["schema"] = serde_json::json!("celeris.long-horizon-record/1");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    // 試験は別の process で並んで走る。`writeln!` は書式の断片ごとに write するので行が混ざりうる
    // （JSON が壊れて script の検査が落ちた）。1 行を 1 回の O_APPEND の write で書く。
    use std::io::Write as _;
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
}

fn pending_wakes(store: &SqliteStore, p: &Project) -> usize {
    store
        .project_wakes_pending(&p.id.to_string())
        .unwrap()
        .len()
}

fn idempotency_rows(env: &Env, p: &Project) -> i64 {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM task_idempotency_keys WHERE idempotency_key LIKE ?1",
            [format!("pc:{}:%", p.id)],
            |r| r.get(0),
        )
        .unwrap()
}

// ---- 再配送 ----

#[tokio::test]
async fn project_wake_redelivery_10x() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let before = store.latest_event_id().unwrap();
    let root = root_task(&env, &store, &p, Status::Done);
    transition(&store, &root, Status::Done, "review_pass");
    for _ in 0..10 {
        // 同じ event を配送し直す: 全体の cursor を event の前に戻して取り込み直す。
        store
            .feed_cursor_set(
                crate::dispatcher::cron_tick::project_wake::PROJECT_WAKE_CURSOR,
                &before.to_string(),
            )
            .unwrap();
        tick(&mut d);
        env.advance(31);
        tick(&mut d);
    }
    let roots = controller_roots(&store, &p);
    let wakes = store.project_wakes_all(&p.id.to_string()).unwrap();
    assert_eq!(roots, 1);
    assert_eq!(wakes.len(), 1, "{wakes:?}");
    assert_eq!(idempotency_rows(&env, &p), 1);
    assert_eq!(env.calls().len(), 1);
    assert_eq!(pending_wakes(&store, &p), 0);
    record(
        "redelivery.jsonl",
        serde_json::json!({
            "test": "project_wake_redelivery_10x", "deliveries": 10, "roots": roots,
            "wakes": wakes.len(), "idempotency_keys": idempotency_rows(&env, &p),
            "decides": env.calls().len(), "unprocessed_wakes": pending_wakes(&store, &p),
            "pending_effects": store.project_controller_pending_effects().unwrap().len(),
            "llm_launches": env.llm_launches(),
        }),
    );
}

// ---- kind ごと ----

/// 1 回の評価が `kind` の wake 1 件を持って呼ばれたことを確かめる。
fn assert_one_decide_of(env: &Env, kind: ProjectWakeKind) {
    let calls = env.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let kinds: Vec<ProjectWakeKind> = calls[0].wakes.iter().map(|w| w.kind).collect();
    assert_eq!(kinds, vec![kind]);
}

fn kind_case(setup: impl FnOnce(&Env, &SqliteStore, &Project, &mut Dispatcher)) -> Env {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    setup(&env, &store, &p, &mut d);
    tick(&mut d);
    env.advance(31);
    tick(&mut d);
    env.advance(31);
    tick(&mut d);
    assert_eq!(pending_wakes(&store, &p), 0);
    env
}

#[tokio::test]
async fn project_wake_kind_root_done() {
    let env = kind_case(|env, store, p, _| {
        let t = root_task(env, store, p, Status::Done);
        transition(store, &t, Status::Done, "review_pass");
    });
    assert_one_decide_of(&env, ProjectWakeKind::RootDone);
}

#[tokio::test]
async fn project_wake_kind_root_failed() {
    let env = kind_case(|env, store, p, _| {
        let t = root_task(env, store, p, Status::Failed);
        transition(store, &t, Status::Failed, "worker_error");
    });
    assert_one_decide_of(&env, ProjectWakeKind::RootFailed);
}

#[tokio::test]
async fn project_wake_kind_review_failed() {
    let env = kind_case(|env, store, p, _| {
        let t = root_task(env, store, p, Status::Failed);
        transition(store, &t, Status::Failed, "review_fail");
    });
    assert_one_decide_of(&env, ProjectWakeKind::ReviewFailed);
}

#[tokio::test]
async fn project_wake_kind_children_settled() {
    let env = kind_case(|env, store, p, _| {
        let parent = root_task(env, store, p, Status::Running);
        let mut child = new_task(env.dir.path(), Check::Reviewer, 0);
        child.project_id = Some(p.id);
        child.parent_id = Some(parent.id);
        child.status = Status::Done;
        store.create_task(&child, vec![]).unwrap();
        transition(store, &child, Status::Done, "review_pass");
    });
    assert_one_decide_of(&env, ProjectWakeKind::ChildrenSettled);
}

#[tokio::test]
async fn project_wake_kind_decision_answered() {
    let env = kind_case(|env, store, p, _| {
        let t = root_task(env, store, p, Status::Blocked);
        event_json(
            store,
            &t,
            serde_json::json!({"type": "answered", "question": "q", "answer": "a", "by": "human:t"}),
        );
    });
    assert_one_decide_of(&env, ProjectWakeKind::DecisionAnswered);
}

#[tokio::test]
async fn project_wake_kind_external_released() {
    let env = kind_case(|env, store, p, _| {
        let t = root_task(env, store, p, Status::Running);
        event_json(
            store,
            &t,
            serde_json::json!({"type": "cluster_job_wait_finished", "wait_id": "w1", "state": "satisfied"}),
        );
    });
    assert_one_decide_of(&env, ProjectWakeKind::ExternalReleased);
}

#[tokio::test]
async fn project_wake_kind_stagnation() {
    let env = kind_case(|_, _, p, d| {
        d.stagnation.findings = vec![StagnationFinding {
            kind: StagnationKind::ProjectStalled,
            key: format!("project_stalled-{}", p.id),
            project_id: Some(p.id),
            task_id: None,
            subject: "pilot:wake".into(),
            since: "2026-10-31T00:00:00Z".into(),
            detail: "進んでいない".into(),
        }];
    });
    assert_one_decide_of(&env, ProjectWakeKind::Stagnation);
}

fn reconcile_job(store: &SqliteStore, p: &Project) -> task_core::CronJob {
    let mut template = task_core::CronTaskTemplate {
        title: "案件の再評価".into(),
        ..task_core::CronTaskTemplate::default()
    };
    template
        .extra
        .insert("kind".into(), serde_json::json!("project_reconcile"));
    template
        .extra
        .insert("project_id".into(), serde_json::json!(p.id.to_string()));
    create_job(
        store,
        &CronFireContext::default(),
        NewCronJob {
            name: format!("reconcile-{}", p.id),
            schedule: "0 * * * *".into(),
            timezone: "UTC".into(),
            overlap: task_core::CronOverlap::Skip,
            catch_up: task_core::CronCatchUp::Latest,
            enabled: true,
            template,
        },
        at(T0),
    )
    .unwrap()
}

#[tokio::test]
async fn project_wake_kind_cron() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let job = reconcile_job(&store, &p);
    assert!(task_ops::cron_jobs::check_manual_run(&job).is_err());
    let mut d = env.daemon(&store, "i1", none());
    d.tick().unwrap();
    *env.clock.lock().unwrap() = at("2026-11-01T01:00:00Z");
    d.tick().unwrap();
    env.advance(31);
    d.tick().unwrap();
    assert_one_decide_of(&env, ProjectWakeKind::Cron);
    let wakes = store.project_wakes_all(&p.id.to_string()).unwrap();
    assert_eq!(
        wakes[0].wake_key,
        format!(
            "cron:{}:{}",
            job.id,
            at("2026-11-01T01:00:00Z").unix_timestamp()
        )
    );
}

#[tokio::test]
async fn project_wake_kind_human_changed() {
    let env = kind_case(|env, store, p, _| {
        assert_eq!(human_bump(store, &p.id.to_string(), env.now()), Some(1));
    });
    assert_one_decide_of(&env, ProjectWakeKind::HumanChanged);
    assert_eq!(env.calls()[0].human_epoch, 1);
}

// ---- crash の境界 ----

fn crash_case(boundary: Boundary, name: &str) {
    crash_case_with(boundary, name, create_root);
}

/// crash の境界の試験。`decide` が作る効果（root か CoS 入力）が再起動後に 1 回だけ効くことを確かめる。
fn crash_case_with(boundary: Boundary, name: &str, decide: fn(&Project) -> DecideFn) {
    let env = Env::new();
    let p;
    {
        let store = env.open();
        p = project(&store, Some(ControllerMode::Enabled));
        let mut d = env.daemon(&store, "i1", decide(&p));
        prime(&mut d);
        let root = root_task(&env, &store, &p, Status::Done);
        transition(&store, &root, Status::Done, "review_pass");
        env.advance(31);
        let target = boundary;
        d.set_project_controller_hook(Box::new(move |b| {
            if b == target {
                panic!("crash at {b:?}");
            }
        }));
        // B1・B2 は coalesce の前にも通るので、境界に当たるまで tick を重ねる。
        let mut crashed = false;
        for _ in 0..3 {
            if catch_unwind(AssertUnwindSafe(|| tick(&mut d))).is_err() {
                crashed = true;
                break;
            }
            env.advance(31);
        }
        assert!(crashed, "{name}: the hook did not fire");
    }
    // 再起動: store を開き直した別の instance。切れた lease も引き継ぐ。
    let store = env.open();
    let mut d = env.daemon(&store, "i2", decide(&p));
    for _ in 0..4 {
        env.advance(61);
        tick(&mut d);
    }
    let roots = controller_roots(&store, &p);
    let unprocessed = pending_wakes(&store, &p);
    let pending = store.project_controller_pending_effects().unwrap().len();
    let cos_inputs =
        env.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'");
    let cos = name.contains("_cos_");
    assert_eq!(roots, usize::from(!cos), "{name}");
    assert_eq!(unprocessed, 0, "{name}");
    assert_eq!(pending, 0, "{name}");
    assert_eq!(cos_inputs, i64::from(cos), "{name}");
    assert_eq!(idempotency_rows(&env, &p), i64::from(!cos), "{name}");
    let applied = store
        .project_controller_decisions(&p.id.to_string(), None, 10)
        .unwrap()
        .iter()
        .filter(|d| d.effect_state == EffectState::Applied)
        .count();
    assert_eq!(applied, 1, "{name}");
    assert_eq!(env.llm_launches(), 0, "{name}");
    record(
        "crash.jsonl",
        serde_json::json!({
            "test": name, "boundary": format!("{boundary:?}"), "roots": roots,
            "unprocessed_wakes": unprocessed, "pending_effects": pending,
            "cos_inputs": cos_inputs, "idempotency_keys": idempotency_rows(&env, &p),
            "llm_launches": env.llm_launches(),
        }),
    );
}

#[tokio::test]
async fn project_wake_crash_b1() {
    crash_case(Boundary::BeforeIngest, "project_wake_crash_b1");
}

#[tokio::test]
async fn project_wake_crash_b2() {
    crash_case(Boundary::AfterIngest, "project_wake_crash_b2");
}

#[tokio::test]
async fn project_wake_crash_b3() {
    crash_case(Boundary::AfterLease, "project_wake_crash_b3");
}

#[tokio::test]
async fn project_wake_crash_b4() {
    crash_case(Boundary::AfterCommit, "project_wake_crash_b4");
}

#[tokio::test]
async fn project_wake_crash_b5() {
    crash_case(Boundary::AfterEffect, "project_wake_crash_b5");
}

#[tokio::test]
async fn project_wake_crash_b6() {
    crash_case(Boundary::AfterApplied, "project_wake_crash_b6");
}

#[tokio::test]
async fn project_wake_crash_cos_b4() {
    crash_case_with(
        Boundary::AfterCommit,
        "project_wake_crash_cos_b4",
        effects::queue_cos_input,
    );
}

#[tokio::test]
async fn project_wake_crash_cos_b5() {
    crash_case_with(
        Boundary::AfterEffect,
        "project_wake_crash_cos_b5",
        effects::queue_cos_input,
    );
}

// ---- 待機中 ----

#[tokio::test]
async fn project_wake_idle_7days() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    for _ in 0..(7 * 24) {
        d.tick().unwrap();
        env.advance(3600);
    }
    let messages = env.count("SELECT COUNT(*) FROM chat_messages");
    assert_eq!(env.calls().len(), 0);
    assert_eq!(env.llm_launches(), 0);
    assert_eq!(messages, 0);
    assert_eq!(controller_roots(&store, &p), 0);
    record(
        "idle.jsonl",
        serde_json::json!({
            "test": "project_wake_idle_7days", "virtual_hours": 168, "decides": env.calls().len(),
            "llm_launches": env.llm_launches(), "chat_messages": messages,
            "roots": controller_roots(&store, &p), "unprocessed_wakes": pending_wakes(&store, &p),
        }),
    );
}

#[tokio::test]
async fn project_wake_cron_idle() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let mut d = env.daemon(&store, "i1", none());
    d.tick().unwrap();
    for hour in 1..=(7 * 24) {
        *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(hour);
        d.tick().unwrap();
        env.advance(31);
        d.tick().unwrap();
    }
    let messages = env.count("SELECT COUNT(*) FROM chat_messages");
    assert_eq!(env.calls().len(), 168);
    assert_eq!(env.llm_launches(), 0);
    assert_eq!(messages, 0);
    assert_eq!(pending_wakes(&store, &p), 0);
    record(
        "idle.jsonl",
        serde_json::json!({
            "test": "project_wake_cron_idle", "virtual_hours": 168, "decides": env.calls().len(),
            "llm_launches": env.llm_launches(), "chat_messages": messages,
            "roots": controller_roots(&store, &p), "unprocessed_wakes": pending_wakes(&store, &p),
        }),
    );
}

// ---- coalesce・CAS・lease ----

#[tokio::test]
async fn project_wake_coalesce() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    let t = root_task(&env, &store, &p, Status::Blocked);
    for _ in 0..20 {
        event_json(
            &store,
            &t,
            serde_json::json!({"type": "answered", "question": "q", "answer": "a"}),
        );
        tick(&mut d);
        assert_eq!(env.calls().len(), 0, "still coalescing");
        env.advance(1);
    }
    env.advance(11);
    tick(&mut d);
    let calls = env.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].wakes.len(), 20);
    assert_eq!(pending_wakes(&store, &p), 0);
}

/// decide の後・tx1 の前に人の操作 `human` が入る。判断は捨てられ、root は作られない。
fn human_race(
    env: &Env,
    store: &Arc<SqliteStore>,
    p: &Project,
    d: &mut Dispatcher,
    human: HumanOp,
) {
    let mut human = Some(human);
    d.set_project_controller_hook(Box::new(move |b| {
        if b == Boundary::BeforeCommit
            && let Some(human) = human.take()
        {
            human();
        }
    }));
    env.advance(31);
    tick(d);
    let last = d.project_controller_last().unwrap();
    assert_eq!(last.stale, vec!["rejected:stale_human_epoch".to_string()]);
    assert_eq!(controller_roots(store, p), 0, "the person came first");
    let pid = p.id.to_string();
    assert!(
        store
            .project_controller_decisions(&pid, None, 10)
            .unwrap()
            .is_empty()
    );
    let state = store.project_controller_state(&pid).unwrap().unwrap();
    assert_eq!(state.human_epoch, 1);
    assert!(
        store
            .project_wakes_pending(&pid)
            .unwrap()
            .iter()
            .any(|w| w.wake_key == "human:1"),
        "the wake of the person is waiting for the next evaluation"
    );
}

#[tokio::test]
async fn project_wake_human_races_cancel() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let done = root_task(&env, &store, &p, Status::Done);
    transition(&store, &done, Status::Done, "review_pass");
    let open_root = root_task(&env, &store, &p, Status::Ready);
    tick(&mut d);
    let human_store = env.open();
    let id = open_root.id;
    let project_id = p.id.to_string();
    let clock = env.clock.clone();
    let human = Box::new(move || {
        let now = *clock.lock().unwrap();
        task_ops::gate::cancel_with(&human_store, id, None, |tx, _| {
            SqliteStore::project_controller_human_bump_tx(tx, &project_id, now, 0).map(|_| ())
        })
        .unwrap();
    });
    human_race(&env, &store, &p, &mut d, human);
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Cancelled);
}

#[tokio::test]
async fn project_wake_human_races_pause() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let done = root_task(&env, &store, &p, Status::Done);
    transition(&store, &done, Status::Done, "review_pass");
    tick(&mut d);
    let human_store = env.open();
    let id = p.id;
    let clock = env.clock.clone();
    let human = Box::new(move || {
        let now = *clock.lock().unwrap();
        task_ops::lifecycle::apply_project_action(
            &human_store,
            id,
            task_ops::lifecycle::ProjectAction::Pause,
            now,
            |tx| {
                SqliteStore::project_controller_human_bump_tx(tx, &id.to_string(), now, 0)
                    .map(|_| ())
            },
        )
        .unwrap();
    });
    human_race(&env, &store, &p, &mut d, human);
    assert_eq!(
        store.project_get(p.id).unwrap().unwrap().status,
        ProjectStatus::Paused
    );
}

#[tokio::test]
async fn project_wake_human_races_apply() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let root = root_task(&env, &store, &p, Status::Done);
    transition(&store, &root, Status::Done, "review_pass");
    tick(&mut d);
    let bump_store = env.open();
    let pid = p.id.to_string();
    let hook_pid = pid.clone();
    let clock = env.clock.clone();
    let mut fired = false;
    d.set_project_controller_hook(Box::new(move |b| {
        if b == Boundary::BeforeCommit && !fired {
            fired = true;
            let now = *clock.lock().unwrap();
            human_bump(&bump_store, &hook_pid, now);
        }
    }));
    env.advance(31);
    tick(&mut d);
    let last = d.project_controller_last().unwrap();
    assert_eq!(last.stale, vec!["rejected:stale_human_epoch".to_string()]);
    assert_eq!(
        controller_roots(&store, &p),
        0,
        "the discarded decision created nothing"
    );
    assert!(
        store
            .project_controller_decisions(&pid, None, 10)
            .unwrap()
            .is_empty()
    );
    tick(&mut d);
    let calls = env.calls();
    assert_eq!(calls.len(), 2, "evaluated again after the person");
    assert_eq!(calls[1].human_epoch, 1);
    assert!(
        calls[1]
            .wakes
            .iter()
            .any(|w| w.kind == ProjectWakeKind::HumanChanged)
    );
    assert_eq!(controller_roots(&store, &p), 1);
}

#[tokio::test]
async fn project_wake_cas_stale() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let root = root_task(&env, &store, &p, Status::Done);
    transition(&store, &root, Status::Done, "review_pass");
    tick(&mut d);
    let other = env.open();
    let pid = p.id.to_string();
    let clock = env.clock.clone();
    let mut fired = false;
    d.set_project_controller_hook(Box::new(move |b| {
        if b == Boundary::BeforeCommit && !fired {
            fired = true;
            let now = *clock.lock().unwrap();
            let rev = other
                .project_controller_state(&pid)
                .unwrap()
                .unwrap()
                .revision;
            other
                .project_controller_set_mode(
                    &pid,
                    ControllerMode::Enabled,
                    Some(rev),
                    "human:t",
                    now,
                )
                .unwrap();
        }
    }));
    env.advance(31);
    tick(&mut d);
    assert_eq!(
        d.project_controller_last().unwrap().stale,
        vec!["rejected:stale_revision".to_string()]
    );
    assert_eq!(controller_roots(&store, &p), 0);
    tick(&mut d);
    assert_eq!(controller_roots(&store, &p), 1);
    assert_eq!(
        store
            .project_controller_set_mode(
                &p.id.to_string(),
                ControllerMode::Off,
                Some(0),
                "x",
                env.now()
            )
            .unwrap(),
        task_core::store::ModeChange::Stale {
            current: store
                .project_controller_state(&p.id.to_string())
                .unwrap()
                .unwrap()
                .revision
        }
    );
}

#[tokio::test]
async fn project_wake_lease_takeover() {
    let env = Env::new();
    let p;
    {
        let store = env.open();
        p = project(&store, Some(ControllerMode::Enabled));
        let mut d = env.daemon(&store, "i1", create_root(&p));
        prime(&mut d);
        let root = root_task(&env, &store, &p, Status::Done);
        transition(&store, &root, Status::Done, "review_pass");
        tick(&mut d);
        tick(&mut d);
        env.advance(31);
        d.set_project_controller_hook(Box::new(|b| {
            if b == Boundary::AfterLease {
                panic!("crash holding the lease");
            }
        }));
        assert!(catch_unwind(AssertUnwindSafe(|| tick(&mut d))).is_err());
    }
    let store = env.open();
    let mut b = env.daemon(&store, "i2", create_root(&p));
    env.advance(30);
    tick(&mut b);
    assert_eq!(env.calls().len(), 0, "the lease of i1 is still valid");
    env.advance(31);
    tick(&mut b);
    assert_eq!(env.calls().len(), 1, "i2 took over the expired lease");
    assert_eq!(controller_roots(&store, &p), 1);
}

#[tokio::test]
async fn project_wake_two_instances() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut a = env.daemon(&store, "i1", create_root(&p));
    prime(&mut a);
    let root = root_task(&env, &store, &p, Status::Done);
    transition(&store, &root, Status::Done, "review_pass");
    tick(&mut a);
    // a の tx1 と効果の間に b が tick する（b は pending の効果を拾って実行する）。
    let b_store = env.open();
    let b = Arc::new(StdMutex::new(env.daemon(&b_store, "i2", create_root(&p))));
    let b_in_hook = b.clone();
    a.set_project_controller_hook(Box::new(move |boundary| {
        if boundary == Boundary::AfterCommit {
            b_in_hook.lock().unwrap().tick_project_controller();
        }
    }));
    env.advance(31);
    tick(&mut a);
    b.lock().unwrap().tick_project_controller();
    tick(&mut a);
    assert_eq!(controller_roots(&store, &p), 1);
    assert_eq!(idempotency_rows(&env, &p), 1);
    let decisions = store
        .project_controller_decisions(&p.id.to_string(), None, 10)
        .unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].effect_state, EffectState::Applied);
    assert_eq!(env.calls().len(), 1);
}

// ---- 再試行・backpressure・mode ----

#[tokio::test]
async fn project_wake_backoff() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", Box::new(|_| Err("decider is down".into())));
    prime(&mut d);
    let t = root_task(&env, &store, &p, Status::Blocked);
    event_json(
        &store,
        &t,
        serde_json::json!({"type": "answered", "question": "q", "answer": "a"}),
    );
    let mut decide_at: Vec<i64> = Vec::new();
    let start = env.now();
    for _ in 0..1000 {
        let before = env.calls().len();
        tick(&mut d);
        if env.calls().len() > before {
            decide_at.push((env.now() - start).whole_seconds());
        }
        env.advance(10);
    }
    let gaps: Vec<i64> = decide_at.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(gaps, vec![60, 120, 240, 480, 960], "{decide_at:?}");
    let state = store
        .project_controller_state(&p.id.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(state.phase, ControllerPhase::Blocked);
    assert_eq!(state.retry_count, 6);
    assert_eq!(
        pending_wakes(&store, &p),
        1,
        "the wake is kept for a person"
    );
}

#[tokio::test]
async fn project_wake_backpressure() {
    let env = Env::new();
    let store = env.open();
    let projects: Vec<Project> = (0..50)
        .map(|_| project(&store, Some(ControllerMode::Enabled)))
        .collect();
    let mut d = env.daemon(&store, "i1", none());
    prime(&mut d);
    for p in &projects {
        human_bump(&store, &p.id.to_string(), env.now());
    }
    let mut per_tick = Vec::new();
    for _ in 0..25 {
        let before = env.calls().len();
        tick(&mut d);
        per_tick.push(env.calls().len() - before);
    }
    assert!(per_tick.iter().all(|n| *n <= 2), "{per_tick:?}");
    assert_eq!(env.calls().len(), 50);
    assert!(projects.iter().all(|p| pending_wakes(&store, p) == 0));
}

#[tokio::test]
async fn project_wake_off_is_inert() {
    let env = Env::new();
    let store = env.open();
    let no_row = project(&store, None);
    let off = project(&store, Some(ControllerMode::Off));
    let mut d = env.daemon(&store, "i1", create_root(&no_row));
    prime(&mut d);
    for p in [&no_row, &off] {
        let t = root_task(&env, &store, p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
        assert_eq!(human_bump(&store, &p.id.to_string(), env.now()), None);
    }
    let events_before = store.latest_event_id().unwrap();
    for _ in 0..5 {
        tick(&mut d);
        env.advance(61);
    }
    assert_eq!(
        store.latest_event_id().unwrap(),
        events_before,
        "no event written"
    );
    for p in [&no_row, &off] {
        assert!(
            store
                .project_wakes_all(&p.id.to_string())
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .project_controller_decisions(&p.id.to_string(), None, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(controller_roots(&store, p), 0);
    }
    assert!(
        store
            .project_controller_state(&no_row.id.to_string())
            .unwrap()
            .is_none()
    );
    assert_eq!(env.calls().len(), 0);
}

#[tokio::test]
async fn project_wake_mode_shadow() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Shadow));
    let mut d = env.daemon(&store, "i1", create_root(&p));
    prime(&mut d);
    let root = root_task(&env, &store, &p, Status::Done);
    transition(&store, &root, Status::Done, "review_pass");
    tick(&mut d);
    env.advance(31);
    tick(&mut d);
    tick(&mut d);
    let decisions = store
        .project_controller_decisions(&p.id.to_string(), None, 10)
        .unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].effect_state, EffectState::Suppressed);
    assert_eq!(decisions[0].mode, ControllerMode::Shadow);
    assert_eq!(controller_roots(&store, &p), 0);
    assert_eq!(env.llm_launches(), 0);
}
