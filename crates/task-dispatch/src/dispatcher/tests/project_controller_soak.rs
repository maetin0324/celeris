//! P1-I（第 3 フェーズ）: 複数案件を仮想時刻で 72 時間回す長時間の計測（接頭辞 `project_controller_soak_`）。
//!
//! 一時 DB・注入した時計（`Env.clock`）・fake decider だけで動く（LLM なし、外部ネットワークなし、本番の DB・daemon に
//! 触れない、CPU を焼かない）。20 案件に毎時の照合の cron を置き、6 時間ごとに root の完了、2 時間ごとに 5 案件へ外部の
//! 解放の wake を積む。36 時間目に daemon を作り直す（restart）。controller の mode を `off`・`shadow`・`enabled` で
//! 揃えて、仮想の 1 時間ごとに wake の行・未消費の wake・判断の行・評価の数・blocked・CoS 入力・LLM の起動
//! （`chat_runs`＋`runs`）・tick の段の遅延・wake から判断までの遅れ（仮想秒）・RSS を記録する。
//! `PROJECT_CONTROLLER_SOAK_RAW_DIR` があれば `soak-virtual.jsonl`（`celeris.long-run-soak/1`）を書く。

use super::*;

const PROJECTS: usize = 20;
const HOURS: i64 = 72;
const STEP_SECS: i64 = 300;
const RESTART_HOUR: i64 = 36;

fn raw(value: &serde_json::Value) {
    let Some(dir) = std::env::var_os("PROJECT_CONTROLLER_SOAK_RAW_DIR") else {
        return;
    };
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("soak-virtual.jsonl"))
        .and_then(|mut f| f.write_all(format!("{value}\n").as_bytes()))
        .unwrap();
}

fn cos_inputs(env: &Env) -> i64 {
    env.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'")
}

fn external_wake(store: &SqliteStore, p: &Project, n: usize, now: OffsetDateTime) {
    store
        .project_wake_ingest(
            &[task_core::project_wake::NewProjectWake {
                project_id: p.id.to_string(),
                wake_key: format!("external:soak:{n}"),
                kind: ProjectWakeKind::ExternalReleased,
                source_event_id: None,
                source_ref: Some(format!("soak:{n}")),
            }],
            None,
            30,
            now,
        )
        .unwrap();
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn pct(v: &[u64], p: usize) -> u64 {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get((v.len().max(1) - 1) * p / 100).copied().unwrap_or(0)
}

/// 消費された wake の、作られてから判断の行ができるまでの仮想秒（最大と p95）。
fn wake_lag_secs(env: &Env) -> (i64, i64) {
    let db = rusqlite::Connection::open(env.db()).unwrap();
    let mut st = db
        .prepare(
            "SELECT CAST((julianday(d.created_at) - julianday(w.created_at)) * 86400 AS INTEGER) \
             FROM project_wakes w JOIN project_controller_decisions d ON d.id = w.consumed_by",
        )
        .unwrap();
    let mut lags: Vec<u64> = st
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap().max(0) as u64)
        .collect();
    lags.sort_unstable();
    (
        lags.last().copied().unwrap_or(0) as i64,
        pct(&lags, 95) as i64,
    )
}

fn soak(mode: ControllerMode, name: &str) {
    let test = format!("project_controller_soak_72h_{name}");
    let env = Env::new();
    let store = env.open();
    let projects: Vec<Project> = (0..PROJECTS)
        .map(|_| {
            let p = project(&store, Some(mode));
            reconcile_job(&store, &p);
            p
        })
        .collect();
    let mut s = ProjectControllerSettings::default();
    s.max_evaluations_per_tick = 10;
    let mut d = env.daemon_with(&store, "i1", none(), s.clone());
    d.tick().unwrap();
    let rss_start = rss_kb();
    let mut loops = Vec::new();
    let mut external = 0usize;
    let mut roots_done = 0usize;
    let steps_per_hour = 3600 / STEP_SECS;
    for hour in 1..=HOURS {
        if hour == RESTART_HOUR + 1 {
            // daemon の再起動: 別の owner で作り直す（lease の回収と wake の取りこぼしを見る）。
            drop(d);
            d = env.daemon_with(&store, "i2", none(), s.clone());
        }
        if hour % 6 == 0 {
            for p in &projects {
                let t = root_task(&env, &store, p, Status::Done);
                transition(&store, &t, Status::Done, "review_pass");
                roots_done += 1;
            }
        }
        if hour % 2 == 0 {
            for p in projects.iter().take(5) {
                external_wake(&store, p, external, env.now());
                external += 1;
            }
        }
        let mut hour_loops = Vec::new();
        for _ in 0..steps_per_hour {
            env.advance(STEP_SECS);
            d.tick().unwrap();
            if let Some(last) = d.project_controller_last() {
                hour_loops.push(last.pass_micros);
            }
        }
        loops.extend(hour_loops.iter().copied());
        let (lag_max, lag_p95) = wake_lag_secs(&env);
        let pending: usize = projects.iter().map(|p| pending_wakes(&store, p)).sum();
        let row = serde_json::json!({
            "schema": "celeris.long-run-soak/1", "test": test, "condition": name, "phase": "virtual_hour",
            "virtual_hour": hour, "projects": PROJECTS, "rss_kb": rss_kb(),
            "rss_delta_kb": rss_kb() as i64 - rss_start as i64,
            "wake_rows": env.count("SELECT COUNT(*) FROM project_wakes"),
            "wake_rows_consumed": env.count("SELECT COUNT(*) FROM project_wakes WHERE consumed_by IS NOT NULL"),
            "wakes_pending": pending,
            "decision_rows": env.count("SELECT COUNT(*) FROM project_controller_decisions"),
            "decides": env.calls().len(),
            "blocked": env.count("SELECT COUNT(*) FROM project_controller_state WHERE phase = 'blocked'"),
            "pending_effects": store.project_controller_pending_effects().unwrap().len(),
            "cos_inputs": cos_inputs(&env), "llm_launches": env.llm_launches(),
            "events_rows": env.count("SELECT COUNT(*) FROM events"),
            "roots_done": roots_done, "external_wakes": external,
            "pass_micros_p50": pct(&hour_loops, 50), "pass_micros_p95": pct(&hour_loops, 95),
            "wake_lag_secs_max": lag_max, "wake_lag_secs_p95": lag_p95,
            "restarted": hour > RESTART_HOUR,
        });
        raw(&row);
    }
    // 毎時の cron の wake は正時の tick で積まれ、次の tick で消費される。最後の正時の分を 2 tick で畳む。
    for _ in 0..2 {
        env.advance(STEP_SECS);
        d.tick().unwrap();
    }
    // 不変条件: LLM を起こさない、最後の照合の後に未消費の wake・未適用の効果が残らない、restart で判断が止まらない。
    let pending: usize = projects.iter().map(|p| pending_wakes(&store, p)).sum();
    assert_eq!(env.llm_launches(), 0, "{test}: no LLM");
    assert_eq!(cos_inputs(&env), 0, "{test}: no CoS input");
    assert_eq!(
        store.project_controller_pending_effects().unwrap().len(),
        0,
        "{test}"
    );
    let calls = env.calls().len();
    match mode {
        ControllerMode::Off => {
            assert_eq!(calls, 0, "{test}: off never decides");
        }
        _ => {
            assert_eq!(pending, 0, "{test}: every wake is consumed");
            // 評価は案件 × 時間（cron）を上限に畳まれる（root の完了・外部の wake は同じ評価に合流しうる）。
            let ceiling = PROJECTS * HOURS as usize + roots_done + external;
            assert!(calls <= ceiling, "{test}: {calls} > {ceiling}");
            assert!(
                calls >= PROJECTS * (HOURS as usize - 1),
                "{test}: cron must evaluate each project hourly ({calls})"
            );
            let (lag_max, _) = wake_lag_secs(&env);
            assert!(lag_max <= 2 * STEP_SECS, "{test}: a wake waited {lag_max}s");
        }
    }
    raw(&serde_json::json!({
        "schema": "celeris.long-run-soak/1", "test": test, "condition": name, "phase": "end",
        "virtual_hours": HOURS, "projects": PROJECTS, "ticks": loops.len(), "decides": calls,
        "pass_micros_p50": pct(&loops, 50), "pass_micros_p95": pct(&loops, 95),
        "rss_kb": rss_kb(), "rss_delta_kb": rss_kb() as i64 - rss_start as i64,
        "wake_rows": env.count("SELECT COUNT(*) FROM project_wakes"),
        "decision_rows": env.count("SELECT COUNT(*) FROM project_controller_decisions"),
        "llm_launches": env.llm_launches(),
    }));
}

#[tokio::test]
async fn project_controller_soak_72h_off() {
    soak(ControllerMode::Off, "off");
}

#[tokio::test]
async fn project_controller_soak_72h_shadow() {
    soak(ControllerMode::Shadow, "shadow");
}

#[tokio::test]
async fn project_controller_soak_72h_enabled() {
    soak(ControllerMode::Enabled, "enabled");
}
