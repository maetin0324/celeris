//! 第 4 フェーズ P4-A: blocked の task の数と tick・dispatch の時間の関係を測るベースライン（接頭辞 `tick_scale_baseline`）。
//!
//! P1-I の 26 分の計測（`long_run_soak_*`）では、fake harness が question を返して task が blocked で積もり、
//! dispatch の遅延（task の挿入から run の終わりまで）が p50 205 ms → 3,953 ms（blocked 0 → 655 件）に伸びた。
//! 同じ状態を数十秒で作るため、本物の dispatch で blocked になった task を 1 件作り、その task の行
//! （`tasks`・`events`・`runs`、ほか `task_id` を持つ表）を SQL で N 件に複写してから測る。
//!
//! 測るもの（`TICK_SCALE_BLOCKED` の各 N について、N ごとに新しい一時 DB）:
//! - 空 tick（ready の task なし）の総時間と、`TickReport::step_us` の段ごとの時間（p50・最大）
//! - `store.list(None)` の時間
//! - dispatch の遅延（`Env::dispatch`: 挿入 → tick → run の終わり。P1-I と同じ定義）
//!
//! - LLM を呼ばない（fake harness の `sh`）。外部ネットワークに出ない。本番の DB・daemon に触れない（一時 DB）。
//! - CPU を焼く負荷をかけない。負荷は DB の行の数だけで決める。既定（`0,20`）は test-affected 向けの小さい値。
//!   ベースラインは `TICK_SCALE_BLOCKED=0,600,2000 TICK_SCALE_TICKS=5 TICK_SCALE_DISPATCHES=5` などで走らせる。
//! - `TICK_SCALE_COND=shadow` なら `lane_rules` を compare にして本物の V8 で TS も呼ぶ（P4-C。既定は `off`）。
//! - `TICK_SCALE_NATURAL=1` なら複写せず本物の dispatch で N 件を blocked にする（複写の妥当性の対照）。
//! - `TICK_SCALE_RAW` があれば、その file に 1 行 1 標本の JSON（`celeris.tick-scale-baseline/1`）を追記する。
//! - 判定には使わない計測の試験である。assert は「複写した task が全部 blocked」「dispatch が終わり run・リースが
//!   残らない」「blocked の task は dispatch されない（数が変わらない）」「挿入した task は 1 回だけ run になる」だけ。

mod support;

use std::io::Write as _;
use std::time::Instant;

use serde_json::{Value, json};
use support::script_compare::Env;
use task_core::{Status, TaskStore};

const SCHEMA: &str = "celeris.tick-scale-baseline/1";

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_list(key: &str, default: &[usize]) -> Vec<usize> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v
            .split(',')
            .map(|x| x.trim().parse().unwrap_or_else(|_| panic!("{key}: {v}")))
            .collect(),
        _ => default.to_vec(),
    }
}

fn pct(v: &[u64], p: usize) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    s[((s.len() - 1) * p) / 100]
}

fn raw(v: &Value) {
    println!("{v}");
    if let Ok(path) = std::env::var("TICK_SCALE_RAW") {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("open {path}: {e}"));
        writeln!(f, "{v}").expect("write raw");
    }
}

fn count(db: &rusqlite::Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).expect(sql)
}

struct Column {
    name: String,
    ty: String,
    pk: bool,
}

fn columns(db: &rusqlite::Connection, table: &str) -> Vec<Column> {
    let mut st = db
        .prepare(&format!(
            "SELECT name, type, pk FROM pragma_table_info('{table}')"
        ))
        .expect("table_info");
    st.query_map([], |r| {
        Ok(Column {
            name: r.get(0)?,
            ty: r.get::<_, String>(1)?.to_ascii_uppercase(),
            pk: r.get::<_, i64>(2)? > 0,
        })
    })
    .expect("columns")
    .map(|c| c.expect("column"))
    .collect()
}

/// 複写の計画: 表ごとの INSERT … SELECT。`?1` = 新しい task id、`?2` = 新しい run id、`?3` = 元の task id。
/// 複写できない表（task id 以外の文字列の主キー）は `skipped` に名前を返す（raw に残す）。
fn clone_plan(db: &rusqlite::Connection, template: &str) -> (Vec<String>, Vec<Value>) {
    let mut st = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("tables");
    let tables: Vec<String> = st
        .query_map([], |r| r.get(0))
        .expect("tables")
        .map(|t| t.expect("table"))
        .collect();
    let mut plan = Vec::new();
    let mut skipped = Vec::new();
    for table in tables {
        let cols = columns(db, &table);
        let key = if table == "tasks" { "id" } else { "task_id" };
        if !cols.iter().any(|c| c.name == key) {
            continue;
        }
        let rows: i64 = db
            .query_row(
                &format!("SELECT count(*) FROM \"{table}\" WHERE \"{key}\" = ?1"),
                [template],
                |r| r.get(0),
            )
            .expect("rows");
        if rows == 0 {
            continue;
        }
        let mut names = Vec::new();
        let mut exprs = Vec::new();
        let mut ok = true;
        for c in &cols {
            let expr = match (table.as_str(), c.name.as_str()) {
                ("tasks", "id") => "?1".to_string(),
                (_, "task_id") => "?1".to_string(),
                ("runs", "run_id") => "?2".to_string(),
                (_, "root_id") => "CASE WHEN root_id = ?3 THEN ?1 ELSE root_id END".to_string(),
                // INTEGER の主キー（rowid）は新しく振らせる。
                _ if c.pk
                    && c.ty.contains("INT")
                    && !cols.iter().any(|o| o.pk && o.name == key) =>
                {
                    continue;
                }
                _ if c.pk && !cols.iter().any(|o| o.pk && o.name == key) => {
                    ok = false;
                    break;
                }
                // 本文（json・key）に埋め込まれた元の task id も新しい id に置き換える。置き換えないと、
                // 受信箱の行などが元の task を指したままになり、本物の dispatch で作った状態と違う
                // （`TICK_SCALE_NATURAL=1` の対照で空 tick が 5 ms 対 234 ms に食い違った）。
                _ => format!(
                    "CASE WHEN typeof(\"{0}\") = 'text' THEN replace(\"{0}\", ?3, ?1) ELSE \"{0}\" END",
                    c.name
                ),
            };
            names.push(format!("\"{}\"", c.name));
            exprs.push(expr);
        }
        if ok {
            plan.push(format!(
                "INSERT INTO \"{table}\" ({}) SELECT {} FROM \"{table}\" WHERE \"{key}\" = ?3",
                names.join(", "),
                exprs.join(", ")
            ));
        } else {
            skipped.push(json!({"table": table, "rows": rows}));
        }
    }
    (plan, skipped)
}

/// `template` の行を `n` 件に複写する。
fn clone_blocked(db: &rusqlite::Connection, template: &str, n: usize) -> Vec<Value> {
    let (plan, skipped) = clone_plan(db, template);
    let tx = db.unchecked_transaction().expect("tx");
    {
        let mut stmts: Vec<_> = plan
            .iter()
            .map(|sql| tx.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}")))
            .collect();
        for _ in 0..n {
            let id = task_core::TaskId::new().to_string();
            let run = task_core::TaskId::new().to_string();
            for st in &mut stmts {
                st.execute(rusqlite::params![id, run, template])
                    .expect("clone");
            }
        }
    }
    tx.commit().expect("commit");
    skipped
}

/// restart の後の取り込みを待つ tick の上限（保険。1 tick で event を 8 page × 512 行読む）。
const SETTLE_TICKS_MAX: u64 = 400;

/// CoS の一次対応の event の cursor が最新の event に追いつき、受信箱の行に未処理（pending・running）が無い。
fn settled(db: &rusqlite::Connection) -> bool {
    let head = count(db, "SELECT coalesce(max(id), 0) FROM events");
    let cursor: Option<String> = db
        .query_row(
            "SELECT value FROM feed_cursor WHERE name = 'cos_triage:events'",
            [],
            |r| r.get(0),
        )
        .ok();
    let caught_up = cursor.and_then(|c| c.parse::<i64>().ok()) == Some(head);
    caught_up
        && count(
            db,
            "SELECT count(*) FROM cos_inbox_items WHERE state IN ('pending', 'running')",
        ) == 0
}

fn summarize(steps: &std::collections::BTreeMap<&'static str, Vec<u64>>) -> Value {
    steps
        .iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                json!({"p50": pct(v, 50), "max": v.iter().copied().max().unwrap_or(0)}),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// `TICK_SCALE_COND`: `off`（既定。`[script]` なし、mode rust）か `shadow`（`lane_rules` を compare。本物の V8 で
/// TS も呼んで比較行を書く。P1-I の `long_run_soak_shadow` と同じ）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cond {
    Off,
    Shadow,
}

impl Cond {
    fn from_env() -> Self {
        match std::env::var("TICK_SCALE_COND").as_deref() {
            Err(_) | Ok("") | Ok("off") => Cond::Off,
            Ok("shadow") => Cond::Shadow,
            Ok(other) => panic!("TICK_SCALE_COND: {other}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Cond::Off => "off",
            Cond::Shadow => "shadow",
        }
    }
}

type Policy = std::sync::Arc<dyn task_dispatch::script_policy::PolicyScript>;

/// shadow の口（isolate は試験の間生かす）。bundle が作れなければ SKIP（`None`）。
fn shadow_policy(env: &Env) -> Option<(celeris_script::ScriptRuntimeHandle, Policy)> {
    let bundle = support::script_compare::dist_bundle("tick_scale_baseline", "lane_rules")?;
    let (handle, policy) = support::script_compare::start_policy(
        bundle.source(),
        bundle.manifest.bundle_sha256.clone(),
    );
    let generation = env.deployed_generation(&bundle);
    env.set_mode(
        task_core::script_compare::ScriptMode::Compare,
        Some(generation.id.clone()),
    );
    Some((handle, policy as Policy))
}

fn dispatcher_for(env: &Env, policy: Option<&Policy>) -> task_dispatch::Dispatcher {
    let mut d = env.dispatcher();
    if let Some(p) = policy {
        d.set_policy_script(Some(p.clone()));
    }
    d
}

async fn measure(blocked: usize, ticks: usize, dispatches: usize) {
    let cond = Cond::from_env();
    let env = Env::new("lane_rules");
    let script = match cond {
        Cond::Off => None,
        Cond::Shadow => match shadow_policy(&env) {
            Some(s) => Some(s),
            None => return,
        },
    };
    let policy = script.as_ref().map(|(_, p)| p.clone());
    let db = rusqlite::Connection::open(&env.config.db.path).expect("db");
    let mut d = dispatcher_for(&env, policy.as_ref());
    let mut skipped = Vec::new();
    // `TICK_SCALE_NATURAL=1`: 複写せず、本物の dispatch で N 件を blocked にする（複写の妥当性の対照。遅い）。
    let natural = std::env::var("TICK_SCALE_NATURAL").as_deref() == Ok("1");
    if natural {
        for _ in 0..blocked {
            env.dispatch(&mut d).await;
        }
    } else if blocked > 0 {
        let (template, _) = env.dispatch(&mut d).await;
        let status = env
            .store
            .get(template.id)
            .expect("get")
            .expect("task")
            .status;
        assert_eq!(status, Status::Blocked, "template is blocked");
        skipped = clone_blocked(&db, &template.id.to_string(), blocked - 1);
    }
    let blocked_rows = count(&db, "SELECT count(*) FROM tasks WHERE status = 'blocked'");
    assert_eq!(blocked_rows, blocked as i64, "every clone is blocked");
    // restart と同じ形: 新しい dispatcher で測る（最初の tick は起動の照合を含むので別に記録する）。
    drop(d);
    let mut d = dispatcher_for(&env, policy.as_ref());
    let mut totals = Vec::new();
    let mut steps: std::collections::BTreeMap<&'static str, Vec<u64>> = Default::default();
    let mut first_tick_us = 0;
    let mut settle_ticks = 0u64;
    let mut settle_us = 0u64;
    for i in 0..=ticks {
        let t = Instant::now();
        let report = d.tick().expect("tick");
        let us = t.elapsed().as_micros() as u64;
        if i == 0 {
            first_tick_us = us;
            // 複写の event は CoS の一次対応の取り込みの cursor より後ろにあるので、restart の後の数 tick は
            // その取り込み（受信箱の項目・退避）に使われる。本物の dispatch で作った状態（取り込み済み）に揃えるため、
            // 取り込みと退避が終わるまで tick し、その回数と時間を `settle_*` に残す（P4-C）。
            let t = Instant::now();
            while !settled(&db) {
                assert!(
                    settle_ticks < SETTLE_TICKS_MAX,
                    "triage intake did not settle"
                );
                d.tick().expect("settle tick");
                settle_ticks += 1;
            }
            settle_us = t.elapsed().as_micros() as u64;
            continue;
        }
        totals.push(us);
        for (name, v) in report.step_us {
            steps.entry(name).or_default().push(v);
        }
    }
    let t = Instant::now();
    let listed = env.store.list(None).expect("list").len();
    let list_us = t.elapsed().as_micros() as u64;
    // dispatch の遅延（`Env::dispatch` と同じ手順: 挿入 → tick → run の終わりまで 20 ms ごとに tick）。
    // その間の tick の段の時間を足し合わせ、どの段が遅延を作るかを記録する。
    let mut lag = Vec::new();
    let mut dispatch_steps: std::collections::BTreeMap<&'static str, Vec<u64>> = Default::default();
    let mut dispatch_ticks = Vec::new();
    let mut inserted = Vec::new();
    for _ in 0..dispatches {
        let t = Instant::now();
        let task = env.insert_task();
        let mut sums: std::collections::BTreeMap<&'static str, u64> = Default::default();
        let mut ticks_this = 0u64;
        let mut absorb = |report: task_dispatch::TickReport| {
            ticks_this += 1;
            for (name, v) in report.step_us {
                *sums.entry(name).or_default() += v;
            }
        };
        absorb(d.tick().expect("tick"));
        d.wait_script_compares().await;
        let deadline = Instant::now() + std::time::Duration::from_secs(600);
        while d.in_flight() > 0 {
            assert!(Instant::now() < deadline, "run still in flight");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            absorb(d.tick().expect("tick"));
        }
        lag.push(t.elapsed().as_millis() as u64);
        dispatch_ticks.push(ticks_this);
        for (name, v) in sums {
            dispatch_steps.entry(name).or_default().push(v);
        }
        assert!(env.store.get(task.id).expect("get").is_some());
        inserted.push(task.id.to_string());
    }
    // 規則の不変（P4-A ADR D2）: 挿入した task はどれも 1 回だけ run になり（取りこぼし 0・二重実行 0）、
    // blocked の task は dispatch されない（blocked の数は挿入した task の分だけ増える。fake harness は question を返す）。
    for id in &inserted {
        let runs: i64 = db
            .query_row("SELECT count(*) FROM runs WHERE task_id = ?1", [id], |r| {
                r.get(0)
            })
            .expect("runs");
        assert_eq!(runs, 1, "task {id} ran exactly once");
    }
    assert_eq!(
        count(&db, "SELECT count(*) FROM tasks WHERE status = 'blocked'"),
        (blocked + dispatches) as i64,
        "blocked tasks stay blocked and every inserted task blocked on its question"
    );
    assert_eq!(
        count(&db, "SELECT count(*) FROM runs WHERE status = 'running'"),
        0,
        "no running run is left"
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM tasks WHERE lease_worker_run_id IS NOT NULL"
        ),
        0,
        "no lease is left"
    );
    let steps_json = summarize(&steps);
    raw(&json!({
        "schema": SCHEMA,
        "cond": cond.as_str(),
        "blocked": blocked,
        "seeding": if natural { "dispatch" } else { "sql_clone" },
        "tasks": listed,
        "events_rows": count(&db, "SELECT count(*) FROM events"),
        "runs_rows": count(&db, "SELECT count(*) FROM runs"),
        "not_cloned": skipped,
        "first_tick_us": first_tick_us,
        "settle_ticks": settle_ticks,
        "settle_us": settle_us,
        "idle_tick_us_p50": pct(&totals, 50),
        "idle_tick_us_max": totals.iter().copied().max().unwrap_or(0),
        "idle_ticks": totals.len(),
        "store_list_us": list_us,
        "dispatch_lag_ms_p50": pct(&lag, 50),
        "dispatch_lag_ms_p95": pct(&lag, 95),
        "dispatch_lag_ms_p99": pct(&lag, 99),
        "dispatch_lag_ms_max": lag.iter().copied().max().unwrap_or(0),
        "dispatches": lag.len(),
        "dispatch_ticks": dispatch_ticks,
        "step_us": steps_json,
        "dispatch_step_us": summarize(&dispatch_steps),
    }));
    drop(d);
    drop(script);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_scale_baseline() {
    let ticks = env_usize("TICK_SCALE_TICKS", 2);
    let dispatches = env_usize("TICK_SCALE_DISPATCHES", 2);
    for n in env_list("TICK_SCALE_BLOCKED", &[0, 20]) {
        measure(n, ticks, dispatches).await;
    }
}
