//! P4-D（第 4 フェーズ）: GC あり・なしで案件を仮想時刻で回し、wake・判断の行数と tick の時間を比べる
//! （接頭辞 `project_retention_soak_`）。
//!
//! 一時 DB・注入した時計・fake decider だけで動く（LLM なし、外部ネットワークなし、本番に触れない、CPU を焼かない）。
//! 既定は 6 案件 × 仮想 72 時間（15 分刻み）で、GC の規則を短くして（wake 6 時間・判断 12 時間・新しい 8 件）
//! 試験の中で GC が何度も走るようにする。`PROJECT_RETENTION_SOAK_DAYS=30` で 30 日（計測用。nextest の 600 秒を
//! 超えうるので test binary を `--exact` で直接呼ぶ）。36 時間目で daemon を作り直す（GC の間隔は process の中で
//! 数えるので、再起動の後の最初の tick で GC が 1 回走る）。`PROJECT_RETENTION_SOAK_RAW_DIR` があれば
//! `retention-soak.jsonl`（`celeris.project-retention-soak/1`）を書く。
//!
//! 不変条件: GC は判断の数・効果のある判断・未消費の wake・pending の効果を変えない（GC なしの対照と一致）。

use super::*;

const PROJECTS: usize = 6;
const STEP_SECS: i64 = 900;

fn hours() -> i64 {
    std::env::var("PROJECT_RETENTION_SOAK_DAYS")
        .ok()
        .and_then(|d| d.parse::<i64>().ok())
        .map_or(72, |d| d * 24)
}

fn raw(value: &serde_json::Value) {
    let Some(dir) = std::env::var_os("PROJECT_RETENTION_SOAK_RAW_DIR") else {
        return;
    };
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("retention-soak.jsonl"))
        .and_then(|mut f| f.write_all(format!("{value}\n").as_bytes()))
        .unwrap();
}

fn pct(v: &[u64], p: usize) -> u64 {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get((v.len().max(1) - 1) * p / 100).copied().unwrap_or(0)
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    decides: usize,
    effect_rows: i64,
    pending_wakes: usize,
    pending_effects: usize,
    roots_done: usize,
}

fn run(env: &Env, retention: Option<&std::path::Path>, name: &str) -> (Outcome, i64, i64) {
    let test = format!("project_retention_soak_{name}");
    let store = env.open();
    let projects: Vec<Project> = (0..PROJECTS)
        .map(|_| {
            let p = project(&store, Some(ControllerMode::Enabled));
            reconcile_job(&store, &p);
            p
        })
        .collect();
    let mut s = ProjectControllerSettings {
        max_evaluations_per_tick: 10,
        ..Default::default()
    };
    s.retention = retention.map(|dir| crate::project_controller::ProjectRetentionSettings {
        policy: task_core::project_retention::RetentionPolicy {
            wake_keep_secs: 6 * 3600,
            decision_keep_secs: 12 * 3600,
            decision_keep_latest: 8,
            ..Default::default()
        },
        archive_dir: dir.to_path_buf(),
        interval_secs: 3600,
    });
    let mut d = env.daemon_with(&store, "i1", none(), s.clone());
    d.tick().unwrap();
    let mut ticks = Vec::new();
    let mut roots_done = 0usize;
    let mut max_wakes = 0i64;
    let mut max_decisions = 0i64;
    let hours = hours();
    for hour in 1..=hours {
        if hour == 37 {
            drop(d);
            d = env.daemon_with(&store, "i2", none(), s.clone());
        }
        if hour % 6 == 0 {
            for p in &projects {
                let t = root_task(env, &store, p, Status::Done);
                transition(&store, &t, Status::Done, "review_pass");
                roots_done += 1;
            }
        }
        let mut hour_ticks = Vec::new();
        for _ in 0..(3600 / STEP_SECS) {
            env.advance(STEP_SECS);
            let started = std::time::Instant::now();
            d.tick().unwrap();
            hour_ticks.push(started.elapsed().as_micros() as u64);
        }
        ticks.extend(hour_ticks.iter().copied());
        let wakes = env.count("SELECT COUNT(*) FROM project_wakes");
        let decisions = env.count("SELECT COUNT(*) FROM project_controller_decisions");
        max_wakes = max_wakes.max(wakes);
        max_decisions = max_decisions.max(decisions);
        if hour % 24 == 0 || hour == hours {
            let stats = store.retention_stats().unwrap();
            raw(&serde_json::json!({
                "schema": "celeris.project-retention-soak/1", "test": test, "condition": name,
                "virtual_hour": hour, "projects": PROJECTS,
                "wake_rows": wakes, "decision_rows": decisions,
                "wakes_pending": stats.wakes_pending,
                "retention_runs": env.count("SELECT COUNT(*) FROM project_retention_runs"),
                "retention_deleted": env.count("SELECT COALESCE(SUM(deleted), 0) FROM project_retention_runs"),
                "page_count": stats.page_count, "freelist_count": stats.freelist_count,
                "tick_micros_p50": pct(&hour_ticks, 50), "tick_micros_p95": pct(&hour_ticks, 95),
            }));
        }
    }
    for _ in 0..2 {
        env.advance(STEP_SECS);
        d.tick().unwrap();
    }
    let out = Outcome {
        decides: env.calls().len(),
        effect_rows: env.count(
            "SELECT COUNT(*) FROM project_controller_decisions WHERE effect_state <> 'none'",
        ),
        pending_wakes: projects.iter().map(|p| pending_wakes(&store, p)).sum(),
        pending_effects: store.project_controller_pending_effects().unwrap().len(),
        roots_done,
    };
    raw(&serde_json::json!({
        "schema": "celeris.project-retention-soak/1", "test": test, "condition": name, "phase": "end",
        "virtual_hours": hours, "ticks": ticks.len(), "decides": out.decides,
        "tick_micros_p50": pct(&ticks, 50), "tick_micros_p95": pct(&ticks, 95),
        "max_wake_rows": max_wakes, "max_decision_rows": max_decisions,
        "wake_rows": env.count("SELECT COUNT(*) FROM project_wakes"),
        "decision_rows": env.count("SELECT COUNT(*) FROM project_controller_decisions"),
    }));
    assert_eq!(env.llm_launches(), 0, "{test}: no LLM");
    (
        out,
        env.count("SELECT COUNT(*) FROM project_wakes"),
        env.count("SELECT COUNT(*) FROM project_controller_decisions"),
    )
}

#[tokio::test]
async fn project_retention_soak_gc_bounds_rows_without_changing_decisions() {
    let off_env = Env::new();
    let (off, off_wakes, off_decisions) = run(&off_env, None, "off");
    let on_env = Env::new();
    let archive = on_env.dir.path().join("archive");
    let (on, on_wakes, on_decisions) = run(&on_env, Some(&archive), "on");
    // GC は controller の判断を変えない。
    assert_eq!(
        on, off,
        "GC must not change decisions, effects or pending rows"
    );
    assert_eq!(on.pending_wakes, 0);
    assert_eq!(on.pending_effects, 0);
    // 行は減り、上限で頭打ちになる: wake は消費済み cron が 6 時間 + 間隔 1 時間 + root_done、判断は 12 時間分 +
    // 間隔 1 時間 + 最初の 1 件（効果のある判断・最後の判断を含む）。
    let hours = hours() as usize;
    assert!(on_wakes < off_wakes, "{on_wakes} < {off_wakes}");
    assert!(
        on_decisions < off_decisions,
        "{on_decisions} < {off_decisions}"
    );
    let wake_ceiling = (PROJECTS * (6 + 2 + 1) + on.roots_done) as i64;
    assert!(
        on_wakes <= wake_ceiling,
        "wakes {on_wakes} > {wake_ceiling}"
    );
    let decision_ceiling = (PROJECTS * (12 + 2 + 2)) as i64 + on.effect_rows + on.roots_done as i64;
    assert!(
        on_decisions <= decision_ceiling,
        "decisions {on_decisions} > {decision_ceiling}"
    );
    assert!(off_decisions as usize >= PROJECTS * (hours - 1));
    // archive の行数は消した行と同じ。
    let archived: usize = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| {
            std::fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum();
    assert_eq!(
        archived as i64,
        on_env.count("SELECT SUM(deleted) FROM project_retention_runs")
    );
}
