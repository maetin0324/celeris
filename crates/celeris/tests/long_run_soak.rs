//! P1-I（第 3 フェーズ）: 長時間の運用の資源の計測。daemon と同じ組み立て（`build_dispatcher`）の dispatcher に、本物の
//! control-ts の bundle を本物の V8（deno_core の host）で載せ、同じ負荷を 4 つの条件で流して RSS・V8 heap・isolate・
//! dispatch の遅延（task の挿入から `routing_decided` まで）・比較行の増え方・DB の読みの時間・run の数・restart の
//! 後のリース回収を測る（接頭辞 `long_run_soak_`）。
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
//!   `LONG_RUN_SOAK_SECS` があれば壁時計でその秒数だけ回し（半分で restart）、`LONG_RUN_SOAK_PACE_MS` で dispatch の間を空ける。
//! - `LONG_RUN_SOAK_RAW_DIR` があれば `<dir>/soak.jsonl`（`celeris.long-run-soak/1`）に標本を 1 行ずつ書く。
//!   `scripts/dev/long-run-soak.sh` が 4 条件を走らせ、`scripts/dev/long_run_soak_summary.py` が集計する。
//! - bundle が作れないと V8 の 3 条件は SKIP になる。script は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせる。

mod support;

use std::sync::Arc;
use std::time::Instant;

use celeris_script::ScriptRuntimeHandle;
use serde_json::{Value, json};
use support::script_compare::{Env, dist_bundle, hinted_task, start_policy};
use task_core::script_compare::ScriptMode;
use task_core::{Event, Status, TaskStore};
use task_dispatch::Dispatcher;
use task_dispatch::script_policy::PolicyScript;

const DOMAIN: &str = "lane_rules";
const SCHEMA: &str = "celeris.long-run-soak/1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cond {
    Off,
    V8Load,
    Shadow,
    Enabled,
}

impl Cond {
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
    // 壁時計で回すとき（実時間の計測）。半分で restart する。無ければ `cycles` 回。
    let wall_secs = std::env::var("LONG_RUN_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    // dispatch の間の待ち（daemon の tick の間隔に寄せる。CPU を焼かない）。
    let pace_ms = env_usize("LONG_RUN_SOAK_PACE_MS", 0) as u64;
    // V8 を使わない `off` も、同じ build の bundle が在る環境でだけ走らせる（4 条件の比較を揃える）。
    let Some(bundle) = dist_bundle(test, DOMAIN) else {
        return;
    };
    let env = Env::new(DOMAIN);
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
    let mut d = dispatcher_for(&env, script.as_ref());
    let mut lag_all = Vec::new();
    let restart_at = cycles / 2;
    let mut restart_row = Value::Null;
    let mut restarted = false;
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
        let restart_due = match wall_secs {
            Some(secs) => elapsed >= secs / 2,
            None => cycles >= 2 && cycle == restart_at + 1,
        };
        if restart_due && !restarted {
            restarted = true;
            // daemon の再起動に当たる: dispatcher を捨て、isolate も作り直す（daemon は起動時に新しい isolate で load する）。
            drop(d);
            if let Some(old) = script.take() {
                old.handle.shutdown();
                script = Some(start(&bundle));
            }
            let t = Instant::now();
            d = dispatcher_for(&env, script.as_ref());
            d.tick().expect("tick after restart");
            Env::wait_runs_finished(&mut d).await;
            let recover_ms = t.elapsed().as_millis() as u64;
            restart_row = sampler
                .sample(
                    "restart",
                    cycle,
                    &[],
                    script.as_ref(),
                    json!({"restart_recover_ms": recover_ms}),
                )
                .await;
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
    // 判定の不変条件: 条件ごとの比較行・isolate の数・restart の後に running の run とリースが残らない。
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
    if !restart_row.is_null() {
        assert_eq!(
            restart_row["runs_running"], 0,
            "{test}: restart reclaims runs"
        );
    }
    if let Some(s) = &script {
        let stats = s.handle.stats();
        assert_eq!(stats.restarts, 0, "{test}: no isolate recycle");
        let heap = s.handle.heap_stats().await.expect("heap stats");
        assert_eq!(heap.number_of_native_contexts, 1, "{test}: one context");
        assert!(heap.used_heap_size > 0 && heap.used_heap_size <= heap.total_heap_size);
    }
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
