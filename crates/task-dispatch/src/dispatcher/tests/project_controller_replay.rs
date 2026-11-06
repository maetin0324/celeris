//! ADR 2026-11-04-project-controller A6: 長時間の運用の fault injection と仮想時刻の replay
//! （接頭辞 `project_controller_replay_`）。
//!
//! 一時 DB・注入した時計（`Env.clock`）・fake decider だけで動く（LLM なし、外部ネットワークなし、本番の DB・
//! daemon に触れない）。CoS chat は有効だが `max_cos_runs = 0` で run は起きない。「LLM の起動」は案件 CoS の
//! run を起こす CoS 入力（`pc-input:` の chat message）と run の行（`chat_runs`）で数える。
//! `PROJECT_CONTROLLER_REPLAY_RAW_DIR` があれば `replay.jsonl`・`resource.jsonl`
//! （`celeris.long-horizon-record/1`）を書く（`scripts/dev/test-project-controller-replay.sh` が検査する）。

use super::*;
use task_ops::project_budget::BudgetLimits;

fn raw(file: &str, value: serde_json::Value) {
    let Some(dir) = std::env::var_os("PROJECT_CONTROLLER_REPLAY_RAW_DIR") else {
        return;
    };
    let mut v = value;
    v["schema"] = serde_json::json!("celeris.long-horizon-record/1");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join(file))
        .unwrap();
    use std::io::Write as _;
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
}

fn settings(f: impl FnOnce(&mut BudgetLimits)) -> ProjectControllerSettings {
    let mut s = ProjectControllerSettings::default();
    f(&mut s.budget);
    s
}

fn escalations(env: &Env, p: &Project) -> i64 {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE ?1",
            [format!("project_controller:escalate:{}:%", p.id)],
            |r| r.get(0),
        )
        .unwrap()
}

fn cos_inputs(env: &Env) -> i64 {
    env.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'")
}

fn chat_runs(env: &Env) -> i64 {
    env.count("SELECT COUNT(*) FROM chat_runs")
}

fn phase(store: &SqliteStore, p: &Project) -> ControllerPhase {
    store
        .project_controller_state(&p.id.to_string())
        .unwrap()
        .unwrap()
        .phase
}

fn guard_decisions(store: &SqliteStore, p: &Project, rule: &str) -> usize {
    store
        .project_controller_decisions(&p.id.to_string(), None, 1000)
        .unwrap()
        .iter()
        .filter(|d| d.validation == rule)
        .count()
}

/// 外からの wake を 1 件（`external:<ref>:<n>`。API の `POST …/controller/wake` と同じ形）。
fn external_wake(store: &SqliteStore, p: &Project, n: usize, now: OffsetDateTime) {
    store
        .project_wake_ingest(
            &[task_core::project_wake::NewProjectWake {
                project_id: p.id.to_string(),
                wake_key: format!("external:replay:{n}"),
                kind: ProjectWakeKind::ExternalReleased,
                source_event_id: None,
                source_ref: Some(format!("replay:{n}")),
            }],
            None,
            30,
            now,
        )
        .unwrap();
}

/// 案件の task に `usd_micros`（micro-USD。claude-sonnet-5 の input 単価 2 USD/100 万 token で換算した input token）を使った run を 1 行。
fn seed_run(env: &Env, t: &Task, run: &str, usd_micros: u64, started_at: &str) {
    rusqlite::Connection::open(env.db())
        .unwrap()
        .execute(
            "INSERT INTO runs (run_id, task_id, role, seq, status, model, usage_json, started_at, finished_at) \
             VALUES (?1, ?2, 'worker', 1, 'completed', 'claude-sonnet-5', ?3, ?4, ?4)",
            rusqlite::params![
                run,
                t.id.to_string(),
                format!("{{\"input_tokens\":{},\"output_tokens\":0}}", usd_micros / 2),
                started_at
            ],
        )
        .unwrap();
}

/// 呼ぶたびに別の key の CoS 入力（Waiting）。
fn cos_each_call(p: &Project) -> DecideFn {
    let project = p.id.to_string();
    let mut n = 0;
    Box::new(move |_| {
        n += 1;
        Ok(ProjectDecision {
            effect: ProjectEffect::QueueCosInput {
                key: format!("pc-input:{project}:replay-{n}"),
                text: "案件の次を決めてください。".into(),
            },
            next_wake_after_secs: None,
            phase: ControllerPhase::Waiting,
            rule_id: "fake/cos".into(),
        })
    })
}

/// 呼ぶたびに別の root（試行ごとに新しい key）。
fn root_each_call(p: &Project) -> DecideFn {
    let project = p.id.to_string();
    let mut n = 0;
    Box::new(move |_| {
        n += 1;
        Ok(ProjectDecision {
            effect: ProjectEffect::CreateRoot {
                idempotency_key: root_idempotency_key(&project, &format!("outcome:o1:{n}")),
                spec: Box::new(spec_json(&format!("試行 {n}"))),
                meta: None,
                authority: None,
            },
            next_wake_after_secs: None,
            phase: ControllerPhase::Active,
            rule_id: "fake/create".into(),
        })
    })
}

fn newest_controller_root(store: &SqliteStore, p: &Project, seen: usize) -> Option<Task> {
    let mut roots: Vec<Task> = store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|t| {
            t.project_id == Some(p.id)
                && t.labels
                    .iter()
                    .any(|l| l == crate::project_controller::CONTROLLER_LABEL)
        })
        .collect();
    roots.sort_by_key(|t| t.created_at);
    (roots.len() > seen).then(|| roots.pop()).flatten()
}

fn spin(env: &Env, d: &mut Dispatcher, n: usize) {
    for _ in 0..n {
        env.advance(31);
        tick(d);
    }
}

// ---- crash ----

#[tokio::test]
async fn project_controller_replay_crash_each_stage() {
    let stages = [
        Boundary::BeforeIngest,
        Boundary::AfterIngest,
        Boundary::AfterLease,
        Boundary::BeforeCommit,
        Boundary::AfterCommit,
        Boundary::AfterEffect,
        Boundary::AfterApplied,
    ];
    for boundary in stages {
        let env = Env::new();
        let budget = || settings(|b| b.max_usd_per_day_micros = 1_000);
        let p;
        {
            let store = env.open();
            p = project(&store, Some(ControllerMode::Enabled));
            let mut d = env.daemon_with(&store, "i1", create_root(&p), budget());
            prime(&mut d);
            let seed = root_task(&env, &store, &p, Status::Done);
            seed_run(&env, &seed, "r-seed", 5_000, T0);
            transition(&store, &seed, Status::Done, "review_pass");
            env.advance(31);
            d.set_project_controller_hook(Box::new(move |b| {
                if b == boundary {
                    panic!("crash at {b:?}");
                }
            }));
            let mut crashed = false;
            for _ in 0..3 {
                if catch_unwind(AssertUnwindSafe(|| tick(&mut d))).is_err() {
                    crashed = true;
                    break;
                }
                env.advance(31);
            }
            assert!(crashed, "{boundary:?}: the hook did not fire");
        }
        let store = env.open();
        let mut d = env.daemon_with(&store, "i2", create_root(&p), budget());
        for _ in 0..4 {
            env.advance(61);
            tick(&mut d);
        }
        assert_eq!(escalations(&env, &p), 1, "{boundary:?}");
        assert_eq!(controller_roots(&store, &p), 0, "{boundary:?}");
        assert_eq!(
            env.calls().len(),
            0,
            "{boundary:?}: the guard must not call the decider"
        );
        assert_eq!(phase(&store, &p), ControllerPhase::Blocked, "{boundary:?}");
        assert_eq!(pending_wakes(&store, &p), 0, "{boundary:?}");
        assert_eq!(store.project_controller_pending_effects().unwrap().len(), 0);
        assert_eq!(cos_inputs(&env), 0);
        assert_eq!(chat_runs(&env), 0);
        raw(
            "replay.jsonl",
            serde_json::json!({
                "test": "project_controller_replay_crash_each_stage", "boundary": format!("{boundary:?}"),
                "escalations": escalations(&env, &p), "roots": controller_roots(&store, &p),
                "decides": env.calls().len(), "cos_inputs": cos_inputs(&env), "chat_runs": chat_runs(&env),
                "expected_cos_inputs": 0, "phase": phase(&store, &p).as_str(),
            }),
        );
    }
}

// ---- suspend ----

#[tokio::test]
async fn project_controller_replay_suspend_clock_jump_3days() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let mut d = env.daemon(&store, "i1", none());
    d.tick().unwrap();
    for hour in 1..=2 {
        *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(hour);
        d.tick().unwrap();
        env.advance(31);
        d.tick().unwrap();
    }
    let before = env.calls().len();
    assert_eq!(before, 2);
    // suspend の間に root が 5 件終わる（event は積もる）。時計が 3 日飛ぶ。
    for _ in 0..5 {
        let t = root_task(&env, &store, &p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
    }
    *env.clock.lock().unwrap() =
        at(T0) + time::Duration::hours(2 + 72) + time::Duration::minutes(5);
    for _ in 0..4 {
        d.tick().unwrap();
        env.advance(31);
    }
    let after = env.calls().len() - before;
    assert_eq!(after, 1, "the jump is folded into one evaluation");
    let last = env.calls().last().unwrap().clone();
    assert!(last.wakes.iter().any(|w| w.kind == ProjectWakeKind::Cron));
    assert_eq!(
        last.wakes
            .iter()
            .filter(|w| w.kind == ProjectWakeKind::RootDone)
            .count(),
        5
    );
    assert_eq!(pending_wakes(&store, &p), 0);
    assert_eq!(chat_runs(&env), 0);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_suspend_clock_jump_3days", "virtual_hours_jumped": 72,
            "decides_after_jump": after, "wakes_folded": last.wakes.len(), "cos_inputs": cos_inputs(&env),
            "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- 外部の応答の欠落 ----

#[tokio::test]
async fn project_controller_replay_external_never_replies() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let project_id = p.id.to_string();
    let mut first = true;
    let decide: DecideFn = Box::new(move |_| {
        let effect = if std::mem::take(&mut first) {
            ProjectEffect::QueueCosInput {
                key: format!("pc-input:{project_id}:ask-external"),
                text: "外部の応答を待っています。".into(),
            }
        } else {
            ProjectEffect::None
        };
        Ok(ProjectDecision {
            effect,
            next_wake_after_secs: Some(6 * 3600),
            phase: ControllerPhase::Waiting,
            rule_id: "fake/wait-external".into(),
        })
    });
    let mut d = env.daemon_with(
        &store,
        "i1",
        decide,
        settings(|b| b.ttl_secs = 2 * 24 * 3600),
    );
    prime(&mut d);
    let t = root_task(&env, &store, &p, Status::Done);
    transition(&store, &t, Status::Done, "review_pass");
    for _ in 0..(7 * 24) {
        tick(&mut d);
        env.advance(3600);
    }
    let at_day7 = env.calls().len();
    for _ in 0..(3 * 24) {
        tick(&mut d);
        env.advance(3600);
    }
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(guard_decisions(&store, &p, "pc/guard/ttl"), 1);
    assert_eq!(phase(&store, &p), ControllerPhase::Blocked);
    assert_eq!(cos_inputs(&env), 1);
    assert_eq!(
        env.calls().len(),
        at_day7,
        "no evaluation after the TTL stop"
    );
    // 48 時間 / 6 時間 + 最初の 1 回。
    assert!(at_day7 <= 10, "{at_day7}");
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_external_never_replies", "virtual_hours": 240,
            "decides": at_day7, "decides_after_stop": env.calls().len() - at_day7,
            "escalations": escalations(&env, &p), "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 1,
            "chat_runs": chat_runs(&env), "phase": phase(&store, &p).as_str(),
        }),
    );
}

// ---- 同じ失敗 ----

#[tokio::test]
async fn project_controller_replay_worker_fails_same_way_5x() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon(&store, "i1", root_each_call(&p));
    prime(&mut d);
    let seed = root_task(&env, &store, &p, Status::Done);
    transition(&store, &seed, Status::Done, "review_pass");
    spin(&env, &mut d, 2);
    let mut failed = 0;
    for attempt in 0..5 {
        let Some(root) = newest_controller_root(&store, &p, attempt) else {
            break;
        };
        transition(
            &store,
            &root,
            Status::Failed,
            &format!("check failed: cargo test exit 101 after {attempt}2s"),
        );
        failed += 1;
        spin(&env, &mut d, 2);
    }
    assert_eq!(failed, 3, "the third failure stops the controller");
    assert_eq!(controller_roots(&store, &p), 3);
    assert_eq!(env.calls().len(), 3);
    assert_eq!(guard_decisions(&store, &p, "pc/guard/same-failure"), 1);
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(phase(&store, &p), ControllerPhase::Blocked);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_worker_fails_same_way_5x", "failures_injected": 5,
            "failures_seen": failed, "roots": controller_roots(&store, &p), "decides": env.calls().len(),
            "escalations": escalations(&env, &p), "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0,
            "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- 人への問いの繰り返し ----

#[tokio::test]
async fn project_controller_replay_human_escalation_repeat() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let ask: DecideFn = Box::new(|_| {
        Ok(ProjectDecision {
            effect: ProjectEffect::OpenEscalation {
                question_key: "outcome:o1:scope".into(),
                category: "explicit_human".into(),
                summary: "範囲を決めてください".into(),
            },
            next_wake_after_secs: Some(3600),
            phase: ControllerPhase::Waiting,
            rule_id: "fake/ask".into(),
        })
    });
    let mut d = env.daemon(&store, "i1", ask);
    prime(&mut d);
    for _ in 0..5 {
        let t = root_task(&env, &store, &p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
        spin(&env, &mut d, 3);
        env.advance(3600);
        tick(&mut d);
    }
    assert_eq!(env.calls().len(), 1, "blocked after the first question");
    // 人が controller を置き直す（答えずに）: 同じ問いは 2 回目を作らない。
    store
        .project_controller_set_mode(
            &p.id.to_string(),
            ControllerMode::Enabled,
            None,
            "human:test",
            env.now(),
        )
        .unwrap();
    human_bump(&store, &p.id.to_string(), env.now());
    spin(&env, &mut d, 3);
    assert_eq!(env.calls().len(), 2);
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(cos_inputs(&env), 0);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_human_escalation_repeat", "wakes": 6,
            "decides": env.calls().len(), "escalations": escalations(&env, &p),
            "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- 予算 ----

#[tokio::test]
async fn project_controller_replay_budget_day_cap() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut s = settings(|b| b.max_cos_inputs_per_day = 2);
    s.max_cos_inputs_open_per_project = 10;
    let mut d = env.daemon_with(&store, "i1", cos_each_call(&p), s);
    prime(&mut d);
    for n in 0..6 {
        external_wake(&store, &p, n, env.now());
        spin(&env, &mut d, 2);
    }
    assert_eq!(cos_inputs(&env), 2);
    assert_eq!(env.calls().len(), 2);
    assert_eq!(guard_decisions(&store, &p, "pc/guard/budget"), 1);
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(phase(&store, &p), ControllerPhase::Blocked);
    // 当日は外の入力があっても止まったまま。翌日の再評価は `project_recovery_k40_*` が見る。
    env.advance(3600);
    external_wake(&store, &p, 100, env.now());
    spin(&env, &mut d, 2);
    assert_eq!(cos_inputs(&env), 2);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_budget_day_cap", "cap": 2, "wakes": 7,
            "decides": env.calls().len(), "escalations": escalations(&env, &p),
            "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 2, "chat_runs": chat_runs(&env),
        }),
    );
}

#[tokio::test]
async fn project_controller_replay_usd_cap() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let mut d = env.daemon_with(
        &store,
        "i1",
        none(),
        settings(|b| b.max_usd_per_day_micros = 1_000),
    );
    prime(&mut d);
    let t = root_task(&env, &store, &p, Status::Done);
    // 前の日の費用は今日の予算に数えない。
    seed_run(&env, &t, "r-yesterday", 50_000, "2026-10-31T12:00:00Z");
    external_wake(&store, &p, 0, env.now());
    spin(&env, &mut d, 2);
    assert_eq!(env.calls().len(), 1);
    seed_run(&env, &t, "r-today", 1_500, "2026-11-01T00:00:10Z");
    external_wake(&store, &p, 1, env.now());
    spin(&env, &mut d, 2);
    assert_eq!(env.calls().len(), 1, "the USD cap stops before the decider");
    assert_eq!(guard_decisions(&store, &p, "pc/guard/budget"), 1);
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(phase(&store, &p), ControllerPhase::Blocked);
    let usage = task_ops::project_budget::usage(
        &store,
        &p.id.to_string(),
        env.now(),
        &task_core::pricing::BudgetRates::default(),
    )
    .unwrap();
    assert_eq!(usage.usd_micros, 1_500);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_usd_cap", "cap_usd_micros": 1000, "usd_micros_today": usage.usd_micros,
            "decides": env.calls().len(), "escalations": escalations(&env, &p),
            "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- 案件の状態 ----

#[tokio::test]
async fn project_controller_replay_paused_project() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let mut d = env.daemon(&store, "i1", create_root(&p));
    d.tick().unwrap();
    assert!(
        store
            .project_set_status(p.id, ProjectStatus::Paused)
            .unwrap()
    );
    for hour in 1..=24 {
        let t = root_task(&env, &store, &p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
        *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(hour);
        d.tick().unwrap();
        env.advance(31);
        d.tick().unwrap();
    }
    assert_eq!(env.calls().len(), 0);
    assert_eq!(phase(&store, &p), ControllerPhase::Paused);
    assert_eq!(pending_wakes(&store, &p), 0);
    assert_eq!(controller_roots(&store, &p), 0);
    let paused_decides = env.calls().len();
    // 再開: 1 回だけ評価する。
    assert!(
        store
            .project_set_status(p.id, ProjectStatus::Active)
            .unwrap()
    );
    spin(&env, &mut d, 3);
    assert_eq!(phase(&store, &p), ControllerPhase::Active);
    assert_eq!(env.calls().len(), 1);
    assert_eq!(controller_roots(&store, &p), 1);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_paused_project", "virtual_hours": 24,
            "decides_while_paused": paused_decides, "decides_after_resume": env.calls().len(),
            "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

#[tokio::test]
async fn project_controller_replay_cancelled_project() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let mut d = env.daemon(&store, "i1", create_root(&p));
    d.tick().unwrap();
    assert!(
        store
            .project_set_status(p.id, ProjectStatus::Cancelled)
            .unwrap()
    );
    for hour in 1..=24 {
        let t = root_task(&env, &store, &p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
        human_bump(&store, &p.id.to_string(), env.now());
        *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(hour);
        d.tick().unwrap();
        env.advance(31);
        d.tick().unwrap();
    }
    assert_eq!(env.calls().len(), 0);
    assert_eq!(phase(&store, &p), ControllerPhase::Cancelled);
    assert_eq!(pending_wakes(&store, &p), 0);
    assert_eq!(controller_roots(&store, &p), 0);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_cancelled_project", "virtual_hours": 24,
            "decides": env.calls().len(), "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0,
            "chat_runs": chat_runs(&env),
        }),
    );
}

#[tokio::test]
async fn project_controller_replay_achieved_stays_quiet() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let key = root_idempotency_key(&p.id.to_string(), "outcome:o9:1");
    let mut first = true;
    let decide: DecideFn = Box::new(move |_| {
        if std::mem::take(&mut first) {
            let mut d = ProjectDecision::none("fake/achieved");
            d.phase = ControllerPhase::Achieved;
            return Ok(d);
        }
        Ok(ProjectDecision {
            effect: ProjectEffect::CreateRoot {
                idempotency_key: key.clone(),
                spec: Box::new(spec_json("人の変更の後の root")),
                meta: None,
                authority: None,
            },
            next_wake_after_secs: None,
            phase: ControllerPhase::Active,
            rule_id: "fake/create".into(),
        })
    });
    let mut d = env.daemon(&store, "i1", decide);
    d.tick().unwrap();
    let t = root_task(&env, &store, &p, Status::Done);
    transition(&store, &t, Status::Done, "review_pass");
    spin(&env, &mut d, 2);
    assert_eq!(phase(&store, &p), ControllerPhase::Achieved);
    for hour in 1..=48 {
        let t = root_task(&env, &store, &p, Status::Done);
        transition(&store, &t, Status::Done, "review_pass");
        *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(hour);
        d.tick().unwrap();
        env.advance(31);
        d.tick().unwrap();
    }
    assert_eq!(
        env.calls().len(),
        1,
        "achieved: no evaluation for new wakes"
    );
    assert_eq!(controller_roots(&store, &p), 0);
    assert!(guard_decisions(&store, &p, "pc/guard/achieved-quiet") >= 1);
    let quiet = env.calls().len();
    human_bump(&store, &p.id.to_string(), env.now());
    spin(&env, &mut d, 2);
    assert_eq!(env.calls().len(), 2, "a human change evaluates again");
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_achieved_stays_quiet", "virtual_hours": 48,
            "decides_while_achieved": quiet, "roots_while_achieved": 0,
            "decides_after_human": env.calls().len(), "cos_inputs": cos_inputs(&env),
            "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- event が主、cron の照合が保険 ----

#[tokio::test]
async fn project_controller_replay_event_primary_cron_fallback() {
    let env = Env::new();
    let store = env.open();
    // 案件 A: event の経路（coalesce の 30 秒で評価）。
    let a = project(&store, Some(ControllerMode::Enabled));
    // 案件 B: event を落とす（取り込みの前に cursor が先へ進んだ）。cron の照合で回復する。
    let b = project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &b);
    let mut d = env.daemon(&store, "i1", none());
    d.tick().unwrap();
    let start = env.now();
    let ta = root_task(&env, &store, &a, Status::Done);
    transition(&store, &ta, Status::Done, "review_pass");
    let tb = root_task(&env, &store, &b, Status::Done);
    transition(&store, &tb, Status::Done, "review_pass");
    let head = store.latest_event_id().unwrap();
    // B の event だけを落とす: A の wake を手で入れてから cursor を末尾へ（取り込みを飛ばす）。
    d.tick().unwrap();
    let a_wakes = store.project_wakes_all(&a.id.to_string()).unwrap().len();
    assert_eq!(a_wakes, 1);
    store
        .feed_cursor_set(
            crate::dispatcher::cron_tick::project_wake::PROJECT_WAKE_CURSOR,
            &head.to_string(),
        )
        .unwrap();
    store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            // 0118: 最大 ID は別案件の消費済み wake として残し、B の event の欠落だけを再現する。
            tx.execute("INSERT INTO project_wakes (project_id, wake_key, kind, created_at, consumed_by) VALUES ('retention-sentinel', 'keep-max', 'cron', '2026-11-01T00:00:00Z', 'fixture')", [])?;
            tx.execute(
                "DELETE FROM project_wakes WHERE project_id = ?1",
                [b.id.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    env.advance(31);
    d.tick().unwrap();
    let by_event: Vec<String> = env.calls().iter().map(|c| c.project_id.clone()).collect();
    assert_eq!(by_event, vec![a.id.to_string()]);
    let event_latency = (env.now() - start).whole_seconds();
    // 次の正時の cron で B を照合する。
    *env.clock.lock().unwrap() = at(T0) + time::Duration::hours(1);
    d.tick().unwrap();
    env.advance(31);
    d.tick().unwrap();
    let calls = env.calls();
    let b_calls: Vec<_> = calls
        .iter()
        .filter(|c| c.project_id == b.id.to_string())
        .collect();
    assert_eq!(
        b_calls.len(),
        1,
        "cron reconciliation recovers the lost event"
    );
    assert!(
        b_calls[0]
            .wakes
            .iter()
            .all(|w| w.kind == ProjectWakeKind::Cron)
    );
    let cron_latency = (env.now() - start).whole_seconds();
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_event_primary_cron_fallback",
            "event_path_latency_secs": event_latency, "cron_recovery_latency_secs": cron_latency,
            "decides": calls.len(), "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0,
            "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- TS の品質 ----

#[tokio::test]
async fn project_controller_replay_invalid_proposals_stop() {
    let env = Env::new();
    let store = env.open();
    let p = project(&store, Some(ControllerMode::Enabled));
    let invalid: DecideFn = Box::new(|_| {
        let mut d = ProjectDecision::none("pc/invalid:schema");
        d.phase = ControllerPhase::Waiting;
        Ok(d)
    });
    let mut d = env.daemon(&store, "i1", invalid);
    prime(&mut d);
    for n in 0..8 {
        external_wake(&store, &p, n, env.now());
        spin(&env, &mut d, 2);
    }
    assert_eq!(env.calls().len(), 3);
    assert_eq!(guard_decisions(&store, &p, "pc/guard/invalid-proposals"), 1);
    assert_eq!(escalations(&env, &p), 1);
    assert_eq!(phase(&store, &p), ControllerPhase::Blocked);
    raw(
        "replay.jsonl",
        serde_json::json!({
            "test": "project_controller_replay_invalid_proposals_stop", "wakes": 8,
            "decides": env.calls().len(), "escalations": escalations(&env, &p),
            "cos_inputs": cos_inputs(&env), "expected_cos_inputs": 0, "chat_runs": chat_runs(&env),
        }),
    );
}

// ---- 資源（dispatcher の段の loop cost） ----

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

fn pct(v: &mut [u64], p: usize) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[(v.len() - 1) * p / 100]
}

#[tokio::test]
async fn project_controller_replay_resource_loop() {
    let env = Env::new();
    let store = env.open();
    let projects: Vec<Project> = (0..100)
        .map(|_| project(&store, Some(ControllerMode::Enabled)))
        .collect();
    let s = ProjectControllerSettings {
        max_evaluations_per_tick: 10,
        ..Default::default()
    };
    let mut d = env.daemon_with(&store, "i1", none(), s);
    prime(&mut d);
    let mut idle = Vec::new();
    for _ in 0..30 {
        env.advance(60);
        tick(&mut d);
        idle.push(d.project_controller_last().unwrap().pass_micros);
    }
    let rss_idle = rss_kb();
    for (n, p) in projects.iter().enumerate() {
        external_wake(&store, p, n, env.now());
    }
    let mut busy = Vec::new();
    let mut decide = Vec::new();
    for _ in 0..20 {
        env.advance(31);
        tick(&mut d);
        let last = d.project_controller_last().unwrap();
        if last.decided > 0 {
            busy.push(last.pass_micros);
        }
        decide.extend(last.decide_micros);
    }
    let rss_busy = rss_kb();
    assert_eq!(env.calls().len(), 100);
    assert_eq!(chat_runs(&env), 0);
    for (scenario, mut loops, mut ops, rss) in [
        ("idle", idle, Vec::new(), rss_idle),
        ("projects_100", busy, decide, rss_busy),
    ] {
        raw(
            "resource.jsonl",
            serde_json::json!({
                "test": "project_controller_replay_resource_loop", "kind": "dispatcher_loop",
                "scenario": scenario, "projects": 100, "max_evaluations_per_tick": 10,
                "rss_kb": rss, "isolates": 0, "llm_launches": chat_runs(&env),
                "loop_micros_p50": pct(&mut loops, 50), "loop_micros_p95": pct(&mut loops, 95),
                "decide_micros_p50": pct(&mut ops, 50), "decide_micros_p95": pct(&mut ops, 95),
                "samples": loops.len(),
            }),
        );
    }
}
