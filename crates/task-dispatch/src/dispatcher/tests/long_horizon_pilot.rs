//! ADR 2026-11-04-project-controller A9: 非本番の長時間 pilot の仮想時刻 E2E（接頭辞 `long_horizon_pilot_`）。
//!
//! 使い捨ての `pilot:` 案件を本物の store・dispatcher の controller の tick に流し、判定は本物の control-ts の
//! bundle を本物の V8 で評価する（`TsProjectDecider`）。worker・reviewer・人は台本で、LLM は 0（CoS の run は
//! `max_cos_runs = 0` で起きない。起動は `chat_runs` と `pc-input:` の message で数える）。時計は注入した仮想時刻で、
//! 5 分刻みで 3 日分を流す。一時 DB だけを開き、本番の DB・daemon に触れない。
//!
//! `LONG_HORIZON_PILOT_RAW_DIR` があれば `pilot.jsonl`（`celeris.long-horizon-record/1`）を書く
//! （`scripts/dev/test-long-horizon-pilot.sh` が試験ごとの期待値を検査する）。
//! bundle（`CELERIS_SCRIPT_BUNDLE_DIR`）が無ければ `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` では失敗、それ以外は
//! `CELERIS_SCRIPT_SKIP_LOG` に 1 行書いて戻る（`real_ts`）。

use super::long_horizon_bench::{RealTs, real_ts, seed_grant};
use super::*;
use crate::project_decider_ts::{DeciderLimits, TsProjectDecider};
use crate::script_policy::PolicyScript;
use serde_json::{Value, json};
use task_core::project_wake::NewProjectWake;
use task_ops::project_budget::BudgetLimits;

const STEP_SECS: i64 = 300;
const CRON_EVERY_SECS: i64 = 6 * 3600;
const HOUR: i64 = 3600;
const THREE_DAYS: i64 = 72 * HOUR;

fn request_of(outcomes: &[(&str, &[&str])]) -> String {
    let rows: Vec<Value> = outcomes
        .iter()
        .map(|(id, deps)| {
            json!({"id": id, "title": format!("{id} を仕上げる"),
                   "acceptance": [format!("{id} の検査が通る")], "depends_on": deps})
        })
        .collect();
    format!(
        "pilot の案件。長期に計測を続ける。\n```celeris-outcomes\n{}\n```\n",
        serde_json::to_string(&rows).unwrap()
    )
}

struct Pilot {
    env: Env,
    store: Arc<SqliteStore>,
    p: Project,
    d: Option<Dispatcher>,
    ts: Arc<RealTs>,
    settings: ProjectControllerSettings,
    start: OffsetDateTime,
    wakes: usize,
}

impl Pilot {
    fn new(test: &str, request: &str, budget: impl FnOnce(&mut BudgetLimits)) -> Option<Self> {
        let ts = real_ts(test)?;
        let env = Env::new();
        let store = env.open();
        let p = project(&store, None);
        store
            .write_tx::<_, task_core::StoreError, _>(|tx| {
                SqliteStore::project_set_text_tx(tx, p.id, None, Some(request))
            })
            .unwrap();
        seed_grant(&store, &p.id.to_string(), env.now());
        store
            .project_controller_set_mode(
                &p.id.to_string(),
                ControllerMode::Enabled,
                None,
                "human:pilot",
                env.now(),
            )
            .unwrap();
        let mut settings = ProjectControllerSettings {
            enabled: true,
            coalesce_secs: 0,
            ..ProjectControllerSettings::default()
        };
        budget(&mut settings.budget);
        let start = env.now();
        let mut pilot = Pilot {
            env,
            store,
            p,
            d: None,
            ts,
            settings,
            start,
            wakes: 0,
        };
        pilot.start_daemon("pilot-1");
        pilot.external("start");
        Some(pilot)
    }

    fn start_daemon(&mut self, owner: &str) {
        let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec!["false".into()]));
        let mut d = dispatcher(self.store.clone(), adapter, 0);
        d.test_now = Some(self.env.clock.clone());
        d.config.execution.max_cos_runs = 0;
        d.config.min_free_disk_mb = 0;
        d.stagnation.checked_at = Some(at("2100-01-01T00:00:00Z"));
        d.set_cos_chat_launch(
            self.store.clone(),
            CosChatLaunchConfig {
                enabled: true,
                worker_reserve_five_hour: 0.90,
                harness: "fake".into(),
                max_turns: 2,
                max_wall_secs: 30,
                inline_read_max_commands: 20,
                unavailable_reason: None,
                data_dir: self.env.dir.path().to_path_buf(),
                db_path: self.env.db(),
                attachment_limits: Default::default(),
                api_base_url: "http://127.0.0.1:7700/api/v1".into(),
                web_base_url: None,
                triage: Default::default(),
            },
        );
        let script: Arc<dyn PolicyScript> = self.ts.clone();
        assert!(d.set_project_controller(
            self.settings.clone(),
            Box::new(TsProjectDecider::with_fixed_script(
                DeciderLimits::default(),
                script,
            )),
            owner.into(),
        ));
        d.tick_project_controller();
        self.d = Some(d);
    }

    fn id(&self) -> String {
        self.p.id.to_string()
    }

    fn wake(&mut self, key: String, kind: ProjectWakeKind) {
        self.store
            .project_wake_ingest(
                &[NewProjectWake {
                    project_id: self.id(),
                    wake_key: key,
                    kind,
                    source_event_id: None,
                    source_ref: None,
                }],
                None,
                0,
                self.env.now(),
            )
            .unwrap();
    }

    /// `POST …/controller/wake` と同じ形の wake。
    fn external(&mut self, name: &str) {
        self.wakes += 1;
        let key = format!("external:pilot:{name}:{}", self.wakes);
        self.wake(key, ProjectWakeKind::ExternalReleased);
    }

    fn secs(&self) -> i64 {
        (self.env.now() - self.start).whole_seconds()
    }

    /// 5 分刻みで `until` 秒（開始からの仮想時刻）まで流す。cron は 6 時間ごと。
    fn run_until(&mut self, until: i64) {
        self.tick_once();
        while self.secs() < until {
            self.env.advance(STEP_SECS);
            if self.secs() % CRON_EVERY_SECS == 0 {
                let key = format!("cron:pilot:{}", self.secs());
                self.wake(key, ProjectWakeKind::Cron);
            }
            self.tick_once();
        }
    }

    fn tick_once(&mut self) {
        self.d.as_mut().unwrap().tick_project_controller();
    }

    fn roots(&self) -> Vec<Task> {
        let mut roots: Vec<Task> = self
            .store
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|t| t.project_id == Some(self.p.id) && t.parent_id.is_none())
            .collect();
        roots.sort_by_key(|t| t.created_at);
        roots
    }

    fn set_status(&self, id: TaskId, to: Status, reason: &str) {
        let s = serde_json::to_value(to).unwrap();
        let s = s.as_str().unwrap();
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1, json = json_set(json, '$.status', ?1) WHERE id = ?2",
            rusqlite::params![s, id.to_string()],
        )
        .unwrap();
        drop(conn);
        self.store
            .append_event(
                id,
                &Event::Transitioned {
                    from: Status::Running,
                    to,
                    reason: reason.into(),
                },
            )
            .unwrap();
    }

    fn phase(&self) -> ControllerPhase {
        self.store
            .project_controller_state(&self.id())
            .unwrap()
            .unwrap()
            .phase
    }

    fn mode(&self, mode: ControllerMode, epoch_bump: bool) {
        let rev = self
            .store
            .project_controller_state(&self.id())
            .unwrap()
            .unwrap()
            .revision;
        self.store
            .project_controller_set_mode(&self.id(), mode, Some(rev), "human:pilot", self.env.now())
            .unwrap();
        if epoch_bump {
            self.bump();
        }
    }

    fn bump(&self) {
        human_bump(&self.store, &self.id(), self.env.now());
    }

    /// 人が依頼文を直す（`human_epoch` を同じ tx で上げる）。
    fn set_request(&self, request: &str) {
        let (project, now, id) = (self.p.id, self.env.now(), self.id());
        self.store
            .write_tx::<_, task_core::StoreError, _>(|tx| {
                SqliteStore::project_set_text_tx(tx, project, None, Some(request))?;
                SqliteStore::project_controller_human_bump_tx(tx, &id, now, 0)
            })
            .unwrap();
    }

    fn seed_tokens(&self, t: &Task, run: &str, tokens: u64) {
        let started = self
            .env
            .now()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        rusqlite::Connection::open(self.env.db())
            .unwrap()
            .execute(
                "INSERT INTO runs (run_id, task_id, role, seq, status, usage_json, started_at) \
                 VALUES (?1, ?2, 'worker', 1, 'completed', ?3, ?4)",
                rusqlite::params![
                    run,
                    t.id.to_string(),
                    format!("{{\"input_tokens\":{tokens},\"output_tokens\":0}}"),
                    started
                ],
            )
            .unwrap();
    }

    fn decisions(&self) -> Vec<task_core::project_wake::ProjectControllerDecision> {
        self.store
            .project_controller_decisions(&self.id(), None, 5000)
            .unwrap()
    }

    fn rules(&self) -> Vec<String> {
        let mut rules: Vec<String> = self
            .decisions()
            .iter()
            .map(|d| {
                serde_json::from_str::<Value>(&d.proposal_json)
                    .ok()
                    .and_then(|v| {
                        v["explanation"]["rule_id"]
                            .as_str()
                            .or(v["rule_id"].as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| d.validation.clone())
            })
            .collect();
        rules.reverse();
        rules
    }

    fn count(&self, sql: &str) -> i64 {
        self.env.count(sql)
    }

    fn escalations(&self) -> i64 {
        rusqlite::Connection::open(self.env.db())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE ?1",
                [format!("project_controller:escalate:{}:%", self.id())],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn wakes_total(&self) -> i64 {
        rusqlite::Connection::open(self.env.db())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM project_wakes WHERE project_id = ?1",
                [self.id()],
                |r| r.get(0),
            )
            .unwrap()
    }

    /// 試験の最後の共通の検査と raw の 1 行。
    fn finish(&self, test: &str, extra: Value) {
        let roots = self.roots();
        // I14: controller の root に run も lease も無いまま running の行が残らない。
        let running = roots.iter().filter(|t| t.status == Status::Running).count();
        assert_eq!(running, 0, "{test}: a root stays running");
        assert_eq!(
            self.count("SELECT COUNT(*) FROM chat_runs"),
            0,
            "{test}: LLM"
        );
        assert_eq!(
            self.count("SELECT COUNT(*) FROM runs WHERE role = 'cos'"),
            0
        );
        let statuses: Vec<String> = roots
            .iter()
            .map(|t| {
                serde_json::to_value(t.status)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let mut row = json!({
            "schema": "celeris.long-horizon-record/1",
            "dataset": "long_horizon_pilot_virtual",
            "test": test,
            "virtual_hours": self.secs() / HOUR,
            "roots": roots.len(),
            "root_statuses": statuses,
            "decisions": self.decisions().len(),
            "rules": self.rules(),
            "cos_inputs": self.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'"),
            "chat_runs": self.count("SELECT COUNT(*) FROM chat_runs"),
            "llm_calls": 0,
            "escalations": self.escalations(),
            "phase": self.phase().as_str(),
            "wakes_total": self.wakes_total(),
            "project_status": self.store.project_get(self.p.id).unwrap().map(|p| format!("{:?}", p.status)),
        });
        if let (Some(obj), Value::Object(more)) = (row.as_object_mut(), extra) {
            obj.extend(more);
        }
        let Some(dir) = std::env::var_os("LONG_HORIZON_PILOT_RAW_DIR") else {
            return;
        };
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::path::Path::new(&dir).join("pilot.jsonl"))
            .unwrap();
        use std::io::Write as _;
        f.write_all(format!("{row}\n").as_bytes()).unwrap();
    }
}

/// 3 日で root が順に終わり、進捗のたびに次の root が 1 件だけ起票され、Task は done で終わり、案件は続く。
#[tokio::test]
async fn long_horizon_pilot_three_days_progress_launches_next_roots() {
    let test = "long_horizon_pilot_three_days_progress_launches_next_roots";
    let req = request_of(&[("o1", &[]), ("o2", &["o1"]), ("o3", &["o2"])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    let mut launched_at = Vec::new();
    for (i, done_at) in [20, 44, 68].iter().enumerate() {
        p.run_until(p.secs() + STEP_SECS);
        let roots = p.roots();
        assert_eq!(roots.len(), i + 1, "exactly one new root after progress");
        launched_at.push(p.secs() / HOUR);
        p.run_until(done_at * HOUR);
        assert_eq!(
            p.roots().len(),
            i + 1,
            "no extra root while one is in flight"
        );
        p.set_status(roots[i].id, Status::Done, "review_pass");
        p.run_until(p.secs() + STEP_SECS);
    }
    p.run_until(THREE_DAYS);
    let roots = p.roots();
    assert_eq!(roots.len(), 3);
    assert!(roots.iter().all(|t| t.status == Status::Done));
    assert_eq!(p.phase(), ControllerPhase::Achieved);
    assert_eq!(p.escalations(), 0);
    // 長期の継続は Project の行にある（Task はすべて done、案件は active のまま）。
    assert_eq!(
        p.store.project_get(p.p.id).unwrap().unwrap().status,
        ProjectStatus::Active
    );
    p.finish(test, json!({"launched_at_hours": launched_at}));
}

/// 同じ失敗を繰り返す root は上限で止まり、3 日のあいだ起票も CoS 入力も増えない。
#[tokio::test]
async fn long_horizon_pilot_same_failure_stops_the_loop() {
    let test = "long_horizon_pilot_same_failure_stops_the_loop";
    let req = request_of(&[("o1", &[])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    let mut failed = 0;
    while p.secs() < THREE_DAYS {
        p.run_until(p.secs() + HOUR);
        let open: Vec<Task> = p
            .roots()
            .into_iter()
            .filter(|t| !matches!(t.status, Status::Failed))
            .collect();
        for t in open {
            p.set_status(t.id, Status::Failed, "review_fail");
            failed += 1;
        }
    }
    let roots = p.roots().len();
    assert!(roots >= 2, "the controller retried at least once: {roots}");
    assert!(roots <= 3, "the loop stopped: {roots}");
    assert_eq!(failed, roots);
    assert_eq!(p.phase(), ControllerPhase::Blocked);
    assert_eq!(
        p.escalations(),
        1,
        "one question to the human, not one per wake"
    );
    let decisions_before = p.decisions().len();
    p.run_until(THREE_DAYS + 24 * HOUR);
    assert_eq!(
        p.decisions().len(),
        decisions_before,
        "no evaluation after the stop"
    );
    p.finish(test, json!({"failed_roots": failed}));
}

/// token の 1 日の上限を大きく超える（上限の 2 倍以上。P1-G の危険な超過）と止まり、日が変わっても人が mode を
/// 置き直すまで止まったまま。置き直せば次へ進む。上限の 2 倍未満の超過の翌日の再評価は `project_recovery_k40_*`。
#[tokio::test]
async fn long_horizon_pilot_cost_cap_stops_until_human_resets() {
    let test = "long_horizon_pilot_cost_cap_stops_until_human_resets";
    let req = request_of(&[("o1", &[]), ("o2", &["o1"])]);
    let Some(mut p) = Pilot::new(test, &req, |b| b.max_tokens_per_day = 1_000) else {
        return;
    };
    p.run_until(p.secs() + STEP_SECS);
    let roots = p.roots();
    assert_eq!(roots.len(), 1);
    p.seed_tokens(&roots[0], "r-over", 2_500);
    p.set_status(roots[0].id, Status::Done, "review_pass");
    p.run_until(p.secs() + 2 * STEP_SECS);
    assert_eq!(p.phase(), ControllerPhase::Blocked);
    assert_eq!(p.escalations(), 1);
    assert_eq!(p.roots().len(), 1, "the cap stops the next launch");
    p.run_until(THREE_DAYS);
    assert_eq!(
        p.roots().len(),
        1,
        "a new day does not restart a dangerous overrun by itself"
    );
    assert_eq!(p.escalations(), 1);
    let stopped = p.rules();
    // 人が mode を置き直す（観察に落として有効に戻す）。予算の窓は新しい日。
    p.mode(ControllerMode::Off, false);
    p.mode(ControllerMode::Enabled, true);
    p.external("human-reset");
    p.run_until(p.secs() + 2 * STEP_SECS);
    assert_eq!(p.roots().len(), 2, "the human reset resumes the work");
    let second = p.roots()[1].clone();
    p.set_status(second.id, Status::Done, "review_pass");
    p.run_until(p.secs() + 2 * STEP_SECS);
    p.finish(
        test,
        json!({"cap_tokens": 1000, "used_tokens": 2500, "rules_at_stop": stopped}),
    );
}

/// 人が root を取り消して依頼文を巻き戻すと、外した途中目標の root は作られない。元に戻せば 1 件だけ作り直す。
#[tokio::test]
async fn long_horizon_pilot_cancel_and_rewind() {
    let test = "long_horizon_pilot_cancel_and_rewind";
    let both = request_of(&[("o1", &[]), ("o2", &["o1"])]);
    let only_o1 = request_of(&[("o1", &[])]);
    let Some(mut p) = Pilot::new(test, &both, |_| {}) else {
        return;
    };
    p.run_until(10 * HOUR);
    let r1 = p.roots()[0].clone();
    p.set_status(r1.id, Status::Done, "review_pass");
    p.run_until(p.secs() + 2 * STEP_SECS);
    let roots = p.roots();
    assert_eq!(roots.len(), 2);
    // 人が o2 の root を取り消し、同時に依頼文を o1 だけへ巻き戻す。
    p.run_until(20 * HOUR);
    p.set_request(&only_o1);
    p.set_status(roots[1].id, Status::Cancelled, "human_cancel");
    p.run_until(p.secs() + 2 * STEP_SECS);
    p.run_until(44 * HOUR);
    assert_eq!(p.roots().len(), 2, "no root for the removed outcome");
    assert_eq!(p.phase(), ControllerPhase::Achieved);
    let rules_rewound = p.rules();
    // 人が依頼文を元へ戻しても、人が取り消した root を controller は作り直さない。人に 1 回聞いて止まる。
    p.set_request(&both);
    p.mode(ControllerMode::Off, false);
    p.mode(ControllerMode::Enabled, true);
    p.external("human-restore");
    p.run_until(p.secs() + 2 * STEP_SECS);
    assert_eq!(
        p.roots().len(),
        2,
        "a human cancel is not undone by the controller"
    );
    assert_eq!(p.phase(), ControllerPhase::Blocked);
    assert_eq!(p.escalations(), 1);
    let cancelled = p.store.get(roots[1].id).unwrap().unwrap();
    assert_eq!(
        cancelled.status,
        Status::Cancelled,
        "a cancelled root stays cancelled"
    );
    // 人が続きの root を自分で起票して終えると、mode の置き直しの後に案件は到達する。
    let mut human_root = new_task(p.env.dir.path(), Check::Reviewer, 0);
    human_root.project_id = Some(p.p.id);
    human_root.acceptance = vec![Criterion {
        text: "o2 の検査が通る".into(),
        check: Check::Reviewer,
    }];
    human_root.status = Status::Ready;
    p.store.create_task(&human_root, vec![]).unwrap();
    p.set_status(human_root.id, Status::Done, "review_pass");
    p.mode(ControllerMode::Off, false);
    p.mode(ControllerMode::Enabled, true);
    p.external("human-root-done");
    p.run_until(p.secs() + 2 * STEP_SECS);
    assert_eq!(p.phase(), ControllerPhase::Achieved, "{:?}", p.rules());
    assert_eq!(p.roots().len(), 3);
    // 最後に新しい途中目標を足してから案件ごと取り消すと、以後は起票しない。
    p.set_request(&request_of(&[
        ("o1", &[]),
        ("o2", &["o1"]),
        ("o3", &["o2"]),
    ]));
    p.store
        .project_set_status(p.p.id, ProjectStatus::Cancelled)
        .unwrap();
    p.mode(ControllerMode::Off, false);
    p.mode(ControllerMode::Enabled, true);
    p.external("project-cancelled");
    p.run_until(THREE_DAYS);
    assert_eq!(p.roots().len(), 3, "a cancelled project launches nothing");
    p.finish(test, json!({"rules_after_rewind": rules_rewound}));
}

/// `off` に戻すと controller は評価を止め、起票済みの root は残り、wake も増えない（従来の CoS の動きに戻る）。
#[tokio::test]
async fn long_horizon_pilot_off_rolls_back_to_legacy_cos() {
    let test = "long_horizon_pilot_off_rolls_back_to_legacy_cos";
    let req = request_of(&[("o1", &[]), ("o2", &["o1"])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    p.run_until(6 * HOUR);
    assert_eq!(p.roots().len(), 1);
    let (decisions, wakes) = (p.decisions().len(), p.wakes_total());
    p.mode(ControllerMode::Off, true);
    let r1 = p.roots()[0].clone();
    p.set_status(r1.id, Status::Done, "review_pass");
    p.external("after-off");
    p.run_until(THREE_DAYS);
    assert_eq!(p.roots().len(), 1, "no controller root after off");
    assert_eq!(p.decisions().len(), decisions, "no evaluation after off");
    let wakes_after = p.wakes_total();
    p.finish(
        test,
        json!({"decisions_at_off": decisions, "wakes_at_off": wakes, "wakes_after_off": wakes_after}),
    );
}

/// 人の確認で待つ root を controller は進めない。3 日のあいだ状態は変わらず、次の root も CoS 入力も作らない。
#[tokio::test]
async fn long_horizon_pilot_human_check_is_held() {
    let test = "long_horizon_pilot_human_check_is_held";
    let req = request_of(&[("o1", &[]), ("o2", &["o1"])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    p.run_until(p.secs() + STEP_SECS);
    let r1 = p.roots()[0].clone();
    p.set_status(r1.id, Status::Reviewing, "human_check_pending");
    p.run_until(THREE_DAYS);
    let held = p.store.get(r1.id).unwrap().unwrap();
    assert_eq!(held.status, Status::Reviewing, "only the human moves it");
    assert_eq!(p.roots().len(), 1, "no next root past the human check");
    assert_eq!(
        p.count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'"),
        0
    );
    p.finish(test, json!({"held_status": "reviewing"}));
}

/// 途中の再起動と 1 日分の時計の飛びの後も、起票は増えず、続きから進む。
#[tokio::test]
async fn long_horizon_pilot_restart_and_clock_jump_keep_one_root() {
    let test = "long_horizon_pilot_restart_and_clock_jump_keep_one_root";
    let req = request_of(&[("o1", &[]), ("o2", &["o1"])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    p.run_until(12 * HOUR);
    assert_eq!(p.roots().len(), 1);
    let r1 = p.roots()[0].clone();
    p.d = None;
    p.set_status(r1.id, Status::Done, "review_pass");
    p.env.advance(24 * HOUR);
    p.start_daemon("pilot-2");
    p.external("after-restart");
    p.run_until(p.secs() + 2 * STEP_SECS);
    let roots = p.roots();
    assert_eq!(roots.len(), 2, "one new root after the restart");
    p.set_status(roots[1].id, Status::Done, "review_pass");
    p.run_until(THREE_DAYS);
    assert_eq!(p.roots().len(), 2);
    assert_eq!(p.phase(), ControllerPhase::Achieved);
    p.finish(test, json!({"restarts": 1, "clock_jump_hours": 24}));
}
