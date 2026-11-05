//! P1-I（第 3 フェーズ）: 長時間の運用の資源の計測。daemon と同じ組み立て（`build_dispatcher`）の dispatcher に、本物の
//! control-ts の bundle を本物の V8（deno_core の host）で載せ、同じ負荷を 4 つの条件で流して RSS・V8 heap・isolate・
//! dispatch の遅延（task の挿入から `routing_decided` まで）・比較行の増え方・DB の読みの時間・run の数を測り、
//! 窓の終わりに run を走らせたまま持ち主の process を SIGKILL して、リース回収と restart を測る（接頭辞 `long_run_soak_`）。
//!
//! 条件（過去の本番の観測 RSS 182→589 MB／26 分と比べるため、1 条件 = 1 process。nextest は試験ごとに process を分ける）:
//! - `off`: `[script]` なし。判定の口を渡さない（mode rust）。
//! - `v8_load`: isolate を起こして口を渡すが、mode は rust のまま（TS を呼ばない）。
//! - `shadow`: `lane_rules` を compare（Rust が決め、TS も呼んで比較行を書く）。本番の 2026-11-04 の観測の条件に当たる。
//! - `enabled`: `lane_rules` を ts（canary を経て ts。TS の答えで決める）。
//!
//! - LLM を呼ばない（fake harness の `sh`）。外部ネットワークに出ない。本番の DB・daemon に触れない（一時 DB）。userns を使わない。
//! - CPU を焼く負荷をかけない。負荷は task の数（`LONG_RUN_SOAK_CYCLES` × `LONG_RUN_SOAK_TASKS`）と、過去の event の行の数
//!   （`LONG_RUN_SOAK_SEED_EVENTS`。本番の DB の大きさに寄せる）で決める。既定は test-affected 向けの小さい値。
//!   `LONG_RUN_SOAK_SECS` があれば壁時計でその秒数だけ回し、`LONG_RUN_SOAK_PACE_MS` で dispatch の間を空ける。
//! - 計測の窓の中では restart しない。dispatcher も isolate も 1 個のまま（本番の 26 分の観測と同じ）。
//! - restart の計測は窓の後に、同じ DB で行う。別 process（`long_run_soak_lease_holder`、同じ test binary）が dispatcher を
//!   動かし、harness が止まったままの run を [`RESTART_RUNS`] 本持つ。running の run とリースがあることを DB で確かめてから
//!   SIGKILL し、新しい isolate と dispatcher（孤児の回収あり。daemon と同じ設定）で回収の時間を測る。
//! - `LONG_RUN_SOAK_RAW_DIR` があれば `<dir>/soak.jsonl`（`celeris.long-run-soak/1`）に標本を 1 行ずつ書く。
//!   `scripts/dev/long-run-soak.sh` が 4 条件を走らせ、`scripts/dev/long_run_soak_summary.py` が集計する。
//! - bundle が作れないと V8 の 3 条件は SKIP になる。script は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせる。

mod support;

use std::sync::Arc;
use std::time::Instant;

use celeris_script::ScriptRuntimeHandle;
use serde_json::{Value, json};
use support::script_compare::{
    Env, config_text, dist_bundle, hinted_task, start_policy, write_config,
};
use task_core::script_compare::ScriptMode;
use task_core::{DaemonInstance, Event, InstanceRole, Status, TaskStore};
use task_dispatch::Dispatcher;
use task_dispatch::script_policy::PolicyScript;

const DOMAIN: &str = "lane_rules";
const SCHEMA: &str = "celeris.long-run-soak/1";
/// restart のときに走らせておく run の数（`max_runs_per_account` の既定 2 と同じ）。
const RESTART_RUNS: usize = 2;
/// 持ち主の process を別に起こす変数（config の path）。在るときだけ `long_run_soak_lease_holder` が動く。
const HOLDER_CONFIG_ENV: &str = "LONG_RUN_SOAK_HOLDER_CONFIG";
const HOLDER_COND_ENV: &str = "LONG_RUN_SOAK_HOLDER_COND";
/// 持ち主の process の寿命の上限（親が先に死んでも残らないための保険）。
const HOLDER_MAX_SECS: u64 = 600;
const HOLD_FILE: &str = "hold-runs";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cond {
    Off,
    V8Load,
    Shadow,
    Enabled,
}

impl Cond {
    fn parse(name: &str) -> Self {
        match name {
            "off" => Cond::Off,
            "v8_load" => Cond::V8Load,
            "shadow" => Cond::Shadow,
            "enabled" => Cond::Enabled,
            other => panic!("unknown condition {other}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Cond::Off => "off",
            Cond::V8Load => "v8_load",
            Cond::Shadow => "shadow",
            Cond::Enabled => "enabled",
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn raw(value: &Value) {
    let Some(dir) = std::env::var_os("LONG_RUN_SOAK_RAW_DIR") else {
        return;
    };
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("soak.jsonl"))
        .and_then(|mut f| f.write_all(format!("{value}\n").as_bytes()))
        .expect("raw");
}

fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, |d| d.count())
}

fn pct(v: &[u64], p: usize) -> u64 {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get((v.len().max(1) - 1) * p / 100).copied().unwrap_or(0)
}

fn count(db: &rusqlite::Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).expect(sql)
}

/// 本番の DB の大きさに寄せるため、終わった task に `worker_progress` の event を `n` 行積む。
/// 1 行目は store の API で書き、残りはその json を同じ transaction で複写する（seq だけ変える）。
fn seed_history(env: &Env, n: usize) {
    if n == 0 {
        return;
    }
    const PER_TASK: usize = 500;
    let db = rusqlite::Connection::open(&env.config.db.path).expect("seed db");
    let msg =
        "seed: 過去の run の進捗の行（長時間の計測の土台。本文の長さを本番の event に寄せる）"
            .repeat(3);
    let mut left = n;
    while left > 0 {
        let mut task = hinted_task(env.dir.path());
        task.status = Status::Done;
        env.store.insert(&task).expect("seed task");
        env.store
            .append_event(
                task.id,
                &Event::WorkerProgress {
                    run_id: "seed".into(),
                    msg: msg.clone(),
                    kind: None,
                    tool: None,
                    summary: None,
                    detail: None,
                    truncated: false,
                    error: false,
                },
            )
            .expect("seed event");
        let (seq, json): (i64, String) = db
            .query_row(
                "SELECT seq, json FROM events WHERE task_id = ?1 ORDER BY seq DESC LIMIT 1",
                [task.id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("seed row");
        let batch = left.min(PER_TASK);
        let tx = db.unchecked_transaction().expect("tx");
        {
            let mut ins = tx
                .prepare(
                    "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?3)",
                )
                .expect("prepare");
            for k in 1..batch {
                ins.execute(rusqlite::params![task.id.to_string(), seq + k as i64, json])
                    .expect("insert");
            }
        }
        tx.commit().expect("commit");
        left -= batch;
    }
}

struct Script {
    handle: ScriptRuntimeHandle,
    policy: Arc<dyn PolicyScript>,
}

fn start(bundle: &celeris_script::manifest::VerifiedBundle) -> Script {
    let (handle, policy) = start_policy(bundle.source(), bundle.manifest.bundle_sha256.clone());
    Script {
        handle,
        policy: policy as Arc<dyn PolicyScript>,
    }
}

struct Sampler<'a> {
    test: &'static str,
    cond: Cond,
    db: rusqlite::Connection,
    env: &'a Env,
    started: Instant,
    rss_start_kb: u64,
}

impl Sampler<'_> {
    async fn sample(
        &self,
        phase: &str,
        cycle: usize,
        lag_ms: &[u64],
        script: Option<&Script>,
        extra: Value,
    ) -> Value {
        let heap = match script {
            Some(s) => s.handle.heap_stats().await,
            None => None,
        };
        let stats = script.map(|s| s.handle.stats());
        let t = Instant::now();
        let listed = self.env.store.list(None).expect("list").len();
        let list_us = t.elapsed().as_micros() as u64;
        let t = Instant::now();
        let events = count(&self.db, "SELECT count(*) FROM events");
        let count_events_us = t.elapsed().as_micros() as u64;
        let rss = status_kb("VmRSS:");
        let mut v = json!({
            "schema": SCHEMA, "test": self.test, "condition": self.cond.name(), "phase": phase,
            "cycle": cycle, "elapsed_ms": self.started.elapsed().as_millis() as u64,
            "rss_kb": rss, "rss_anon_kb": status_kb("RssAnon:"), "rss_file_kb": status_kb("RssFile:"),
            "vm_hwm_kb": status_kb("VmHWM:"), "rss_delta_kb": rss as i64 - self.rss_start_kb as i64,
            "threads": threads(),
            "isolates": u64::from(script.is_some()),
            "v8_used_heap_bytes": heap.map(|h| h.used_heap_size),
            "v8_total_heap_bytes": heap.map(|h| h.total_heap_size),
            "v8_physical_bytes": heap.map(|h| h.total_physical_size),
            "v8_external_bytes": heap.map(|h| h.external_memory),
            "v8_malloced_bytes": heap.map(|h| h.malloced_memory),
            "v8_native_contexts": heap.map(|h| h.number_of_native_contexts),
            "v8_generation": stats.map(|s| s.generation),
            "v8_restarts": stats.map(|s| s.restarts),
            "v8_completed": stats.map(|s| s.completed),
            "v8_timeouts": stats.map(|s| s.timeouts),
            "events_rows": events,
            "compare_rows": count(&self.db, "SELECT count(*) FROM script_compare_log"),
            "compare_events": count(&self.db, "SELECT count(*) FROM events WHERE json LIKE '{\"type\":\"script_compare_recorded\"%'"),
            "routing_events": count(&self.db, "SELECT count(*) FROM events WHERE json LIKE '{\"type\":\"routing_decided\"%'"),
            "tasks": listed,
            "tasks_blocked": count(&self.db, "SELECT count(*) FROM tasks WHERE status = 'blocked'"),
            "tasks_waiting": count(&self.db, "SELECT count(*) FROM tasks WHERE status LIKE 'waiting%'"),
            "runs": count(&self.db, "SELECT count(*) FROM runs"),
            "runs_running": count(&self.db, "SELECT count(*) FROM runs WHERE status = 'running'"),
            "leases_held": count(&self.db, "SELECT count(*) FROM tasks WHERE lease_worker_run_id IS NOT NULL"),
            "db_list_us": list_us, "db_count_events_us": count_events_us,
            "lag_ms_p50": pct(lag_ms, 50), "lag_ms_p95": pct(lag_ms, 95),
            "lag_ms_max": lag_ms.iter().copied().max().unwrap_or(0), "lag_samples": lag_ms.len(),
        });
        if let (Some(obj), Value::Object(extra)) = (v.as_object_mut(), extra) {
            obj.extend(extra);
        }
        raw(&v);
        v
    }
}

/// mode と判定の口を条件どおりに置いた dispatcher（daemon の起動と同じ `build_dispatcher`）。
fn dispatcher_for(env: &Env, script: Option<&Script>) -> Dispatcher {
    let mut d = env.dispatcher();
    if let Some(s) = script {
        d.set_policy_script(Some(s.policy.clone()));
    }
    d
}

/// 試験の Env。harness は `<home>/hold-runs` がある間は結果を返さない（`sleep` で待つ。CPU を焼かない）ので、restart の間
/// run を走らせたままにできる。file が無ければ既定の fake harness と同じ（すぐ question を返す）。
fn holding_env() -> Env {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("ws")).expect("ws");
    let hold = dir.path().join(HOLD_FILE);
    // 親が残した file で run が永久に残らないよう、待ちは 300 秒で打ち切る。
    let wait = format!(
        "cat >/dev/null; i=0; while [ -e {} ] && [ $i -lt 1500 ]; do sleep 0.2; i=$((i+1)); done; echo",
        hold.display()
    );
    let text = config_text("", "");
    let replaced = text.replace("cat >/dev/null; echo", &wait);
    assert_ne!(text, replaced, "the fake harness command changed");
    let config = write_config(dir.path(), &replaced);
    let store = task_core::SqliteStore::open(&config.db.path).expect("store");
    Env {
        dir,
        store,
        config,
        domain: DOMAIN,
    }
}

fn register_instance(env_store: &dyn TaskStore, id: &str, pid: u32) {
    let now = time::OffsetDateTime::now_utc();
    env_store
        .instance_register(&DaemonInstance {
            instance_id: id.into(),
            release: "long-run-soak".into(),
            pid,
            role: InstanceRole::Active,
            started_at: now,
            heartbeat_at: now,
            handoff_requested_at: None,
            drained_at: None,
        })
        .expect("register instance");
}

/// 孤児の回収を daemon と同じ判定（`pid_alive`、heartbeat の窓 60 秒）で有効にした dispatcher。
fn taking_over(mut d: Dispatcher, instance_id: &str) -> Dispatcher {
    d.set_orphan_takeover(task_dispatch::orphan::OrphanTakeover {
        instance_id: instance_id.into(),
        freshness: std::time::Duration::from_secs(60),
        pid_alive: Arc::new(task_dispatch::orphan::proc_pid_alive),
    });
    d
}

fn running_run_ids(db: &rusqlite::Connection) -> Vec<String> {
    let mut q = db
        .prepare("SELECT run_id FROM runs WHERE status = 'running' ORDER BY run_id")
        .expect("prepare");
    q.query_map([], |r| r.get(0))
        .expect("query")
        .map(|r| r.expect("row"))
        .collect()
}

/// 元の持ち主の run のうち、まだ running の行の数。
fn still_running(db: &rusqlite::Connection, old: &[String]) -> usize {
    let running = running_run_ids(db);
    old.iter().filter(|r| running.contains(r)).count()
}

/// 窓の後の restart の計測。呼ぶ時点で親の dispatcher は捨ててある（手元に run は無い）。
/// 返すのは新しい dispatcher と isolate（この後の dispatch の確認に使う）。
async fn crash_and_reclaim(
    cond: Cond,
    env: &Env,
    sampler: &Sampler<'_>,
    bundle: &celeris_script::manifest::VerifiedBundle,
    cycle: usize,
) -> (Dispatcher, Option<Script>) {
    let test = sampler.test;
    let db = &sampler.db;
    let hold = env.dir.path().join(HOLD_FILE);
    std::fs::write(&hold, "").expect("hold");
    let tasks: Vec<_> = (0..RESTART_RUNS).map(|_| env.insert_task()).collect();
    let config_path = env.dir.path().join("config.toml");
    let mut holder = std::process::Command::new(std::env::current_exe().expect("exe"))
        .args([
            "--exact",
            "long_run_soak_lease_holder",
            "--ignored",
            "--nocapture",
        ])
        .env(HOLDER_CONFIG_ENV, &config_path)
        .env(HOLDER_COND_ENV, cond.name())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn the lease holder");
    let holder_pid = holder.id();

    // 持ち主が run を起こし、リースを取るまで DB で待つ（出来事待ち。上限は保険）。
    let wait_until = Instant::now() + std::time::Duration::from_secs(300);
    let old_runs = loop {
        let runs = running_run_ids(db);
        let leases = count(
            db,
            "SELECT count(*) FROM tasks WHERE lease_worker_run_id IS NOT NULL",
        );
        if runs.len() == RESTART_RUNS && leases == RESTART_RUNS as i64 {
            break runs;
        }
        assert!(
            Instant::now() < wait_until,
            "{test}: holder did not start the runs: runs={runs:?} leases={leases}"
        );
        assert!(
            holder.try_wait().expect("try_wait").is_none(),
            "{test}: the lease holder exited early"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let lease_left_secs_min: f64 = db
        .query_row(
            "SELECT min((julianday(lease_expires_at) - julianday('now')) * 86400) FROM tasks WHERE lease_worker_run_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .expect("lease left");

    // 対照: 持ち主が生きている間は、孤児の回収を有効にした dispatcher も run を奪わない。
    const NEW_INSTANCE: &str = "01LONGRUNSOAKNEWDAEMON0000";
    let mut control = taking_over(env.dispatcher(), NEW_INSTANCE);
    control.set_accepting_new_work(true);
    register_instance(&env.store, NEW_INSTANCE, std::process::id());
    control.tick().expect("control tick");
    let reclaimed_while_alive = RESTART_RUNS - still_running(db, &old_runs);
    assert_eq!(
        reclaimed_while_alive, 0,
        "{test}: a live holder's runs must not be reclaimed"
    );
    drop(control);

    let before = sampler
        .sample(
            "before_crash",
            cycle,
            &[],
            None,
            json!({"holder_pid": holder_pid, "reclaimed_while_alive": reclaimed_while_alive,
                "lease_left_secs_min": lease_left_secs_min}),
        )
        .await;
    assert_eq!(
        before["runs_running"], RESTART_RUNS,
        "{test}: runs in flight"
    );
    assert_eq!(before["leases_held"], RESTART_RUNS, "{test}: leases held");

    // daemon の異常終了に当たる。ここからを回復の時間として測る。
    let crash = Instant::now();
    holder.kill().expect("SIGKILL the holder");
    holder.wait().expect("reap the holder");
    let old_runs_after_kill = still_running(db, &old_runs);
    assert_eq!(
        old_runs_after_kill, RESTART_RUNS,
        "{test}: nobody closed the runs yet"
    );
    // 残った harness を終わらせ、新しい run は待たずに結果を返すようにする（本番の control-group の kill に当たる）。
    std::fs::remove_file(&hold).expect("release hold");

    let t = Instant::now();
    let script = (cond != Cond::Off).then(|| start(bundle));
    let isolate_start_ms = t.elapsed().as_millis() as u64;
    let t = Instant::now();
    let mut d = taking_over(dispatcher_for(env, script.as_ref()), NEW_INSTANCE);
    d.set_accepting_new_work(true);
    let dispatcher_build_ms = t.elapsed().as_millis() as u64;
    let mut ticks = 0u64;
    let mut reclaim_ms = None;
    let mut first_tick_ms = None;
    let settled_ms = loop {
        let t = Instant::now();
        d.tick().expect("tick after the crash");
        ticks += 1;
        first_tick_ms.get_or_insert(t.elapsed().as_millis() as u64);
        if reclaim_ms.is_none() && still_running(db, &old_runs) == 0 {
            reclaim_ms = Some(crash.elapsed().as_millis() as u64);
        }
        let settled = reclaim_ms.is_some()
            && d.in_flight() == 0
            && running_run_ids(db).is_empty()
            && count(
                db,
                "SELECT count(*) FROM tasks WHERE lease_worker_run_id IS NOT NULL",
            ) == 0
            && tasks.iter().all(|task| {
                env.store.get(task.id).expect("task").expect("row").status == Status::Blocked
            });
        if settled {
            break crash.elapsed().as_millis() as u64;
        }
        assert!(
            crash.elapsed() < std::time::Duration::from_secs(300),
            "{test}: restart did not settle: old={} in_flight={}",
            still_running(db, &old_runs),
            d.in_flight()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let reclaim_ms = reclaim_ms.expect("reclaim time");

    // 元の run は 1 回だけ、孤児の回収として閉じ、新しい run が 1 本ずつ起きて終わる（二重の実行が残らない）。
    let mut interrupted = 0;
    let mut orphan_notes = 0;
    let mut new_runs = 0;
    for task in &tasks {
        for e in &env.events(task) {
            match e {
                Event::WorkerFinished {
                    run_id, outcome, ..
                } if old_runs.contains(run_id)
                    && outcome.starts_with("interrupted: ")
                    && outcome.contains("orphan") =>
                {
                    interrupted += 1;
                }
                Event::WorkerProgress { run_id, msg, .. }
                    if old_runs.contains(run_id) && msg.starts_with("orphan_takeover:") =>
                {
                    orphan_notes += 1;
                }
                Event::WorkerStarted { run_id, .. } if !old_runs.contains(run_id) => new_runs += 1,
                _ => {}
            }
        }
    }
    assert_eq!(
        interrupted, RESTART_RUNS,
        "{test}: each old run closes once as an orphan"
    );
    assert_eq!(orphan_notes, RESTART_RUNS, "{test}: takeover is recorded");
    assert_eq!(
        new_runs, RESTART_RUNS,
        "{test}: each task runs again exactly once"
    );
    sampler
        .sample(
            "restart",
            cycle,
            &[],
            script.as_ref(),
            json!({
                "runs_running_before_crash": before["runs_running"],
                "leases_held_before_crash": before["leases_held"],
                "lease_left_secs_min": lease_left_secs_min,
                "reclaimed_while_alive": reclaimed_while_alive,
                "isolate_start_ms": isolate_start_ms,
                "dispatcher_build_ms": dispatcher_build_ms,
                "first_tick_ms": first_tick_ms,
                "reclaim_ms": reclaim_ms,
                "settled_ms": settled_ms,
                "ticks_to_settle": ticks,
                "old_runs_interrupted": interrupted,
                "orphan_takeover_notes": orphan_notes,
                "new_runs_started": new_runs,
            }),
        )
        .await;
    (d, script)
}

async fn soak(cond: Cond) {
    let test: &'static str = match cond {
        Cond::Off => "long_run_soak_off",
        Cond::V8Load => "long_run_soak_v8_load",
        Cond::Shadow => "long_run_soak_shadow",
        Cond::Enabled => "long_run_soak_enabled",
    };
    let cycles = env_usize("LONG_RUN_SOAK_CYCLES", 3);
    let per_cycle = env_usize("LONG_RUN_SOAK_TASKS", 2);
    let seed_events = env_usize("LONG_RUN_SOAK_SEED_EVENTS", 1_000);
    // 壁時計で回すとき（実時間の計測）。窓の中では restart しない。無ければ `cycles` 回。
    let wall_secs = std::env::var("LONG_RUN_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    // dispatch の間の待ち（daemon の tick の間隔に寄せる。CPU を焼かない）。
    let pace_ms = env_usize("LONG_RUN_SOAK_PACE_MS", 0) as u64;
    // V8 を使わない `off` も、同じ build の bundle が在る環境でだけ走らせる（4 条件の比較を揃える）。
    let Some(bundle) = dist_bundle(test, DOMAIN) else {
        return;
    };
    let env = holding_env();
    seed_history(&env, seed_events);
    let rss_start_kb = status_kb("VmRSS:");
    let sampler = Sampler {
        test,
        cond,
        db: rusqlite::Connection::open(&env.config.db.path).expect("sampler db"),
        env: &env,
        started: Instant::now(),
        rss_start_kb,
    };
    let mut script = (cond != Cond::Off).then(|| start(&bundle));
    match cond {
        Cond::Off | Cond::V8Load => {}
        Cond::Shadow | Cond::Enabled => {
            let generation = env.deployed_generation(&bundle);
            env.set_mode(ScriptMode::Compare, Some(generation.id.clone()));
            if cond == Cond::Enabled {
                env.set_mode(ScriptMode::Ts, Some(generation.id));
            }
        }
    }
    let expected_mode = match cond {
        Cond::Off | Cond::V8Load => ScriptMode::Rust,
        Cond::Shadow => ScriptMode::Compare,
        Cond::Enabled => ScriptMode::Ts,
    };
    assert_eq!(env.mode(), expected_mode);
    let start_row = sampler
        .sample(
            "start",
            0,
            &[],
            script.as_ref(),
            json!({"seed_events": seed_events, "cycles": cycles, "tasks_per_cycle": per_cycle,
                "wall_secs": wall_secs, "pace_ms": pace_ms}),
        )
        .await;
    let start_generation = script.as_ref().map_or(0, |s| s.handle.stats().generation);
    let mut d = dispatcher_for(&env, script.as_ref());
    let mut lag_all = Vec::new();
    let mut cycle = 0;
    loop {
        cycle += 1;
        let elapsed = sampler.started.elapsed().as_secs();
        let done = match wall_secs {
            Some(secs) => elapsed >= secs,
            None => cycle > cycles,
        };
        if done {
            cycle -= 1;
            break;
        }
        let mut lag = Vec::new();
        for _ in 0..per_cycle {
            let t = Instant::now();
            let (task, _) = env.dispatch(&mut d).await;
            lag.push(t.elapsed().as_millis() as u64);
            if pace_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pace_ms)).await;
            }
            assert!(
                env.events(&task)
                    .iter()
                    .any(|e| matches!(e, Event::RoutingDecided { .. })),
                "{test}: routing_decided missing"
            );
        }
        lag_all.extend(lag.iter().copied());
        sampler
            .sample("cycle", cycle, &lag, script.as_ref(), json!({}))
            .await;
    }
    let end = sampler
        .sample("end", cycle, &lag_all, script.as_ref(), json!({}))
        .await;
    // 判定の不変条件: 条件ごとの比較行・isolate の数・窓の中で isolate を作り直していない。
    let dispatched = (cycle * per_cycle) as i64;
    let compare_new = end["compare_events"].as_i64().unwrap_or(0)
        - start_row["compare_events"].as_i64().unwrap_or(0);
    match cond {
        Cond::Off | Cond::V8Load => assert_eq!(compare_new, 0, "{test}: no compare rows"),
        Cond::Shadow => assert!(
            compare_new >= dispatched,
            "{test}: one compare per dispatch ({compare_new} < {dispatched})"
        ),
        Cond::Enabled => {}
    }
    assert_eq!(end["runs_running"], 0, "{test}: no running run is left");
    assert_eq!(end["leases_held"], 0, "{test}: no lease is left");
    if let Some(s) = &script {
        let stats = s.handle.stats();
        assert_eq!(stats.restarts, 0, "{test}: no isolate recycle");
        assert_eq!(
            stats.generation, start_generation,
            "{test}: one isolate for the window"
        );
        let heap = s.handle.heap_stats().await.expect("heap stats");
        assert_eq!(heap.number_of_native_contexts, 1, "{test}: one context");
        assert!(heap.used_heap_size > 0 && heap.used_heap_size <= heap.total_heap_size);
    }

    // 窓の後: run を走らせたまま持ち主を SIGKILL し、リース回収と restart を測る。
    drop(d);
    if let Some(s) = script.take() {
        s.handle.shutdown();
    }
    let (mut d, script) = crash_and_reclaim(cond, &env, &sampler, &bundle, cycle).await;
    // restart の後も dispatch が通り、run とリースが残らない（新しい isolate で判定する）。
    let mut lag = Vec::new();
    for _ in 0..per_cycle {
        let t = Instant::now();
        let (task, _) = env.dispatch(&mut d).await;
        lag.push(t.elapsed().as_millis() as u64);
        assert!(
            env.events(&task)
                .iter()
                .any(|e| matches!(e, Event::RoutingDecided { .. })),
            "{test}: routing_decided missing after restart"
        );
    }
    let after = sampler
        .sample("after_restart", cycle, &lag, script.as_ref(), json!({}))
        .await;
    assert_eq!(
        after["runs_running"], 0,
        "{test}: no running run after restart"
    );
    assert_eq!(after["leases_held"], 0, "{test}: no lease after restart");
    if let Some(s) = script {
        s.handle.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_run_soak_off() {
    soak(Cond::Off).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_run_soak_v8_load() {
    soak(Cond::V8Load).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_run_soak_shadow() {
    soak(Cond::Shadow).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_run_soak_enabled() {
    soak(Cond::Enabled).await;
}

/// `crash_and_reclaim` が別 process で起こす、run を持ったまま殺される daemon の役。単独では何もしない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "started by long_run_soak_* as the process that gets SIGKILLed"]
async fn long_run_soak_lease_holder() {
    let Some(path) = std::env::var_os(HOLDER_CONFIG_ENV) else {
        return;
    };
    let cond = Cond::parse(&std::env::var(HOLDER_COND_ENV).expect("condition"));
    let config = celeris::Config::load(std::path::Path::new(&path)).expect("config");
    let store = task_core::SqliteStore::open(&config.db.path).expect("store");
    const OLD_INSTANCE: &str = "01LONGRUNSOAKOLDDAEMON0000";
    register_instance(&store, OLD_INSTANCE, std::process::id());
    let bundle = dist_bundle("long_run_soak_lease_holder", DOMAIN).expect("bundle");
    let script = (cond != Cond::Off).then(|| start(&bundle));
    let mut d =
        celeris::build_dispatcher(&config, celeris::ClusterMasters::default()).expect("dispatcher");
    if let Some(s) = &script {
        d.set_policy_script(Some(s.policy.clone()));
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(HOLDER_MAX_SECS);
    while Instant::now() < deadline {
        d.tick().expect("tick");
        store
            .instance_heartbeat(OLD_INSTANCE, time::OffsetDateTime::now_utc())
            .expect("heartbeat");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the lease holder was not killed within {HOLDER_MAX_SECS}s");
}
