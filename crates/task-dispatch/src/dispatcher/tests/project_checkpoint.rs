//! ADR 2026-11-22-parent-checkpoint-evaluator D2.3・D3.4・D9: 親の途中評価の reviewer run の起動と決着の dispatcher 側の
//! 試験（接頭辞 `project_controller_checkpoint_`）。
//!
//! A1 の tick（wake → 判定 → tx1 → 効果）に A3 の `TsProjectDecider` を入れ、TS の代わりに snapshot の checkpoint から
//! proposal を返す fake の script（`project_cos.rs` の `FakeTs`）を置く。評価の run は本物の共通の起動口
//! （`start_side_review`）を通り、adapter は `review.json` に `parent_evaluation` を書いて `done` で終える偽物。
//! LLM を呼ばない。外部ネットワークに出ない。run の終わりは `drain_completions` と短い sleep で待つ。
//!
//! 守る不変条件と、既存の試験で検出できない理由:
//! - `evaluate` の判断の後に評価の run が 1 回だけ起き、試行の記録・最終の記録が root の `ArtifactProduced` に残る。
//!   task-core・task-ops の試験は判断の行と成果物の name を手で置くので、起動の段が無くても通る（run 2 の誤読）。
//! - strategy の無い root・controller が無効・shadow の案件では reviewer run が増えない（D9。低抽象度の task の回帰）。
//! - root の run が手元にある間は起こさない（`register_run` の上書きを防ぐ）。root の final review の run を試行に数えない。
//! - 試行の上限で `invalid` の最終の記録を書く（D3.4）。写していない評価がある間も guard の `escalate` が効く（D5.2）。

use super::*;
use crate::run_supervisor::{RunEntry, RunKey};
use task_core::parent_checkpoint as pc;

/// 評価の run の adapter の返し方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// `review.json` に検査を通る `parent_evaluation` を書く。
    Valid,
    /// `review.json` に `parent_evaluation` を書かない（D3.4 の `invalid`）。
    Missing,
}

struct EvalAdapter {
    reply: Reply,
    requests: StdMutex<Vec<RunRequest>>,
}

#[async_trait]
impl WorkerAdapter for EvalAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.requests.lock().unwrap().push(req.clone());
        let mut review = json!({"verdicts": []});
        if self.reply == Reply::Valid {
            review["parent_evaluation"] = json!({
                "schema": pc::EVALUATION_SCHEMA,
                "verdict": "off_track",
                "next": "pivot",
                "axes": {"execution": "succeeded", "epistemic": "advanced", "goal": "not_met"},
                "measured": [], "learned": [], "refuted": [], "uncertainty": [],
                "reason": "子 2 件は done だが親の目標の指標は変わらない",
            });
        }
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        std::fs::write(
            req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
            review.to_string(),
        )
        .unwrap();
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "evaluated".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// TS の代わりの規則: 写していない評価があれば guard の `escalate`（rule `pc/no-progress`）、strategy root に due の
/// trigger があり実行中の判断が無ければ `evaluate`、それ以外は `wait`。strategy root の無い snapshot でも root が
/// あれば `evaluate` を求める（Rust の検証が落とすことを確かめる）。
fn rules(s: &ProjectSnapshot) -> Value {
    if s.checkpoints.iter().any(|c| !c.unmapped.is_empty()) {
        let key = format!("no_progress:{}", s.controller.progress_epoch);
        return proposal(
            s,
            json!({"kind": "escalate", "category": "explicit_human", "question_key": key,
                   "summary": "評価の後も進まない", "options": [], "evidence_refs": []}),
            "pc/no-progress",
            &[],
        );
    }
    if let Some(c) = s
        .checkpoints
        .iter()
        .find(|c| c.running.is_none() && !c.due.is_empty())
    {
        let triggers: Vec<&str> = c.due.iter().map(|t| t.key.as_str()).collect();
        return proposal(
            s,
            json!({"kind": "evaluate", "root_id": c.root_id, "triggers": triggers}),
            "pc/checkpoint/evaluate",
            &[],
        );
    }
    if s.checkpoints.is_empty()
        && let Some(root) = s.roots.first()
    {
        return proposal(
            s,
            json!({"kind": "evaluate", "root_id": root.id, "triggers": ["children:2"]}),
            "pc/checkpoint/evaluate",
            &[],
        );
    }
    proposal(
        s,
        json!({"kind": "wait", "waiting_on": [], "next_wake_after_secs": 3600, "phase": "waiting"}),
        "pc/waiting",
        &[],
    )
}

struct Cp {
    env: Env,
    store: Arc<SqliteStore>,
    p: Project,
    root: Task,
    seen: Arc<StdMutex<Vec<ProjectSnapshot>>>,
    adapter: Arc<EvalAdapter>,
    d: Dispatcher,
    n: u32,
}

/// 戦略の文書を参照する `task.create` の CoS 操作（strategy root の正本。`task_core::strategy_reads`）。
fn strategy_op(store: &SqliteStore, root: &Task) {
    use task_core::chat::{AuditContext, ChatActor};
    let op = ulid::Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "cos-run".into(),
        operation_id: op.clone(),
        reason: "checkpoint test".into(),
        policy_version: "v1".into(),
    };
    let payload = json!({
        "method": "POST", "path": "/api/v1/tasks", "body": {"title": "t"},
        "strategy": {"task_id": root.id.to_string(), "name": "strategy/s1/v1.json",
                     "sha256": format!("sha256:{}", "a".repeat(64)), "candidate": null}
    });
    store
        .cos_operation_apply_at(
            &ctx,
            &op,
            "hash",
            "task",
            &root.id.to_string(),
            None,
            "task.create",
            &payload,
            at(T0),
            |_, _| Ok(json!({})),
        )
        .unwrap();
}

/// root の直下の子を `done` で 1 件（終端の遷移の event を持つ）。
fn done_child(env: &Env, store: &SqliteStore, p: &Project, root: &Task) {
    let mut t = new_task(env.dir.path(), Check::Reviewer, 0);
    t.project_id = Some(p.id);
    t.parent_id = Some(root.id);
    t.status = Status::Done;
    store.create_task(&t, vec![]).unwrap();
    transition(store, &t, Status::Done, "review_pass");
}

fn cp_daemon(
    env: &Env,
    store: &Arc<SqliteStore>,
    adapter: Arc<EvalAdapter>,
    owner: &str,
    seen: &Arc<StdMutex<Vec<ProjectSnapshot>>>,
) -> Dispatcher {
    let script = Arc::new(FakeTs {
        f: Arc::new(rules),
        seen: seen.clone(),
    });
    let decider = TsProjectDecider::with_fixed_script(DeciderLimits::default(), script);
    let mut d = dispatcher(store.clone(), adapter, 2);
    d.test_hooks.now = Some(env.clock.clone());
    d.config.execution.max_cos_runs = 0;
    d.config.min_free_disk_pct = 0.0;
    d.orchestration
        .set_stagnation_checked_at_for_test(at("2100-01-01T00:00:00Z"));
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
            data_dir: env.dir.path().to_path_buf(),
            db_path: env.db(),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: None,
            triage: Default::default(),
        },
    );
    let settings = ProjectControllerSettings {
        enabled: true,
        coalesce_secs: 0,
        ..ProjectControllerSettings::default()
    };
    assert!(d.set_project_controller(settings, Box::new(decider), owner.into()));
    d
}

/// 案件（`mode`）と root（`strategy` なら strategy root）・終端の子 2 件（`children:2` が due）。
fn cp_case(mode: Option<ControllerMode>, strategy: bool, reply: Reply) -> Cp {
    let env = Env::new();
    let store = env.open();
    let p = pc_project(&store, mode);
    let root = root_task(&env, &store, &p, Status::Running);
    if strategy {
        strategy_op(&store, &root);
    }
    done_child(&env, &store, &p, &root);
    done_child(&env, &store, &p, &root);
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(EvalAdapter {
        reply,
        requests: StdMutex::new(Vec::new()),
    });
    let mut d = cp_daemon(&env, &store, adapter.clone(), "i1", &seen);
    prime(&mut d);
    Cp {
        env,
        store,
        p,
        root,
        seen,
        adapter,
        d,
        n: 0,
    }
}

impl Cp {
    /// wake を 1 件入れて tick する（cron の wake の代わり）。
    fn wake(&mut self) {
        self.n += 1;
        self.store
            .project_wake_ingest(
                &[task_core::project_wake::NewProjectWake {
                    project_id: self.p.id.to_string(),
                    wake_key: format!("cron:checkpoint:{}", self.n),
                    kind: ProjectWakeKind::Cron,
                    source_event_id: None,
                    source_ref: None,
                }],
                None,
                0,
                self.env.now(),
            )
            .unwrap();
        self.env.advance(1);
        tick(&mut self.d);
    }

    /// 走っている評価の run の終わりを受け取る（`drain_completions` → `on_side_review_finished` → `settle`）。
    async fn finish_runs(&mut self) {
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.d.drain_completions().unwrap();
            if !self.d.supervisor.has_task_run(self.root.id) {
                return;
            }
        }
        panic!("the evaluation run did not finish");
    }

    fn decisions(&self, kind: &str) -> Vec<task_core::project_wake::ProjectControllerDecision> {
        self.store
            .project_controller_decisions(&self.p.id.to_string(), None, 100)
            .unwrap()
            .into_iter()
            .filter(|d| {
                crate::project_decider_ts::decision_proposal(d)
                    .is_some_and(|p| p.action.kind() == kind)
            })
            .collect()
    }

    fn artifacts(&self) -> Vec<String> {
        self.store
            .events_for(self.root.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::ArtifactProduced { artifact, .. } => Some(artifact.name),
                _ => None,
            })
            .collect()
    }

    fn attempts(&self) -> Vec<String> {
        self.artifacts()
            .into_iter()
            .filter(|n| pc::attempt_of(n).is_some())
            .collect()
    }

    fn finals(&self) -> Vec<String> {
        self.artifacts()
            .into_iter()
            .filter(|n| pc::evaluation_decision_id(n).is_some())
            .collect()
    }

    /// root の reviewer run（`WorkerStarted` の role reviewer）の id。
    fn reviewer_runs(&self) -> Vec<String> {
        self.store
            .events_for(self.root.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::WorkerStarted {
                    run_id,
                    role: Some(RunRole::Reviewer),
                    ..
                } => Some(run_id),
                _ => None,
            })
            .collect()
    }

    /// 全 task の runs 索引の reviewer の行の数。
    fn reviewer_rows(&self) -> i64 {
        self.env
            .count("SELECT COUNT(*) FROM runs WHERE role = 'reviewer'")
    }

    /// 最終の記録の中身。
    fn final_record(&self) -> pc::ParentEvaluationRecord {
        let path = self
            .store
            .events_for(self.root.id)
            .unwrap()
            .into_iter()
            .find_map(|(_, e)| match e {
                Event::ArtifactProduced { artifact, .. }
                    if pc::evaluation_decision_id(&artifact.name).is_some() =>
                {
                    Some(artifact.path)
                }
                _ => None,
            })
            .expect("final record");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn last_checkpoint(&self) -> Option<task_core::project_proposal::SnapshotCheckpoint> {
        self.seen
            .lock()
            .unwrap()
            .last()
            .and_then(|s| s.checkpoints.first().cloned())
    }
}

/// 発火: 終端の子 2 件で `children:2` が due になり、`evaluate` の判断の後の同じ tick で評価の run が 1 回起き、試行の
/// 記録 `attempt-1` が残る。run の間は `holds_side_review` が真（lease の無い run を止めない）。終わりは `review.json`
/// を検査して最終の記録（checked）を書き、次の判断の snapshot で写していない評価になる。
#[tokio::test]
async fn project_controller_checkpoint_fires_one_run_and_settles() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    c.wake();
    let evaluate = c.decisions("evaluate");
    assert_eq!(evaluate.len(), 1, "one evaluate decision");
    assert_eq!(evaluate[0].effect_state, EffectState::Applied);
    let id = evaluate[0].id.clone();
    assert_eq!(c.attempts(), vec![pc::attempt_artifact_name(&id, 1)]);
    let runs = c.reviewer_runs();
    assert_eq!(runs.len(), 1, "one evaluation run");
    assert!(c.d.holds_side_review(&c.root, &runs[0]).unwrap());
    c.finish_runs().await;
    assert!(!c.d.holds_side_review(&c.root, &runs[0]).unwrap());
    assert_eq!(c.finals(), vec![pc::evaluation_artifact_name(&id)]);
    let record = c.final_record();
    assert_eq!(record.status, pc::RecordStatus::Checked);
    // 入力の節は reviewer の request に届いた（判断の id・root・trigger）。
    let requests = c.adapter.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let section = requests[0]
        .context
        .review
        .as_ref()
        .and_then(|r| r.parent_checkpoint.as_ref())
        .expect("parent_checkpoint section");
    assert_eq!(section.decision_id, id);
    assert_eq!(section.root_id, c.root.id);
    assert_eq!(section.triggers, vec!["children:2".to_string()]);
    assert_eq!(section.metrics.children_terminal, 2);
    c.wake();
    let cp = c.last_checkpoint().expect("checkpoint");
    assert_eq!(cp.running, None);
    assert_eq!(cp.unmapped.len(), 1, "the evaluation waits to be mapped");
    assert!(cp.due.is_empty(), "children:2 is evaluated");
}

/// 効果 1 回の冪等: 評価の run が走っている間・終わった後の wake で、`evaluate` の判断・試行・reviewer run は増えない。
#[tokio::test]
async fn project_controller_checkpoint_effect_once_is_idempotent() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    c.wake();
    c.wake();
    c.wake();
    assert_eq!(c.decisions("evaluate").len(), 1);
    assert_eq!(c.attempts().len(), 1);
    assert_eq!(c.reviewer_runs().len(), 1);
    c.finish_runs().await;
    c.wake();
    c.wake();
    assert_eq!(c.decisions("evaluate").len(), 1);
    assert_eq!(c.attempts().len(), 1);
    assert_eq!(c.finals().len(), 1);
    assert_eq!(c.reviewer_runs().len(), 1);
    assert_eq!(c.reviewer_rows(), 1);
}

/// crash の境界: tx1 の後・効果の前（B4）と `applied` の後・起動の前（B6）で落ちても、再起動した instance が評価の run を
/// 1 回だけ起こす（判断の行と成果物の name から実行中を導くので、起動の状態を持たない）。
#[tokio::test]
async fn project_controller_checkpoint_crash_boundaries_start_one_run() {
    for boundary in [Boundary::AfterCommit, Boundary::AfterApplied] {
        let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
        c.d.set_project_controller_hook(Box::new(move |b| {
            if b == boundary {
                panic!("crash at {b:?}");
            }
        }));
        let crashed = catch_unwind(AssertUnwindSafe(|| c.wake())).is_err();
        assert!(crashed, "{boundary:?}: the hook did not fire");
        assert!(c.reviewer_runs().is_empty(), "{boundary:?}");
        // 再起動: 同じ DB を開き直した別の instance。
        let store = c.env.open();
        c.d = cp_daemon(&c.env, &store, c.adapter.clone(), "i2", &c.seen);
        c.store = store;
        for _ in 0..3 {
            c.env.advance(61);
            c.wake();
        }
        c.finish_runs().await;
        for _ in 0..2 {
            c.env.advance(61);
            c.wake();
        }
        let states: Vec<EffectState> = c
            .decisions("evaluate")
            .iter()
            .map(|d| d.effect_state)
            .collect();
        assert_eq!(states, vec![EffectState::Applied], "{boundary:?}");
        assert_eq!(c.attempts().len(), 1, "{boundary:?}");
        assert_eq!(c.reviewer_runs().len(), 1, "{boundary:?}");
        assert_eq!(c.finals().len(), 1, "{boundary:?}");
    }
}

/// strategy の無い root（従来の低抽象度の task）と、controller が無効（mode 未設定）の案件では reviewer run が増えない。
/// strategy の無い root への `evaluate` は Rust の検証（`checkpoint_root_unknown`）が落とす。
#[tokio::test]
async fn project_controller_checkpoint_no_strategy_root_or_disabled_starts_no_run() {
    let mut plain = cp_case(Some(ControllerMode::Enabled), false, Reply::Valid);
    plain.wake();
    plain.wake();
    assert!(plain.decisions("evaluate").is_empty());
    assert!(plain.last_checkpoint().is_none());
    assert!(plain.reviewer_runs().is_empty());
    assert_eq!(plain.reviewer_rows(), 0);
    assert!(plain.artifacts().is_empty());

    let mut off = cp_case(None, true, Reply::Valid);
    off.wake();
    off.wake();
    assert!(off.seen.lock().unwrap().is_empty(), "no decision while off");
    assert!(off.reviewer_runs().is_empty());
    assert_eq!(off.reviewer_rows(), 0);
    assert!(off.artifacts().is_empty());
}

/// shadow: `evaluate` の判断は記録するが効果は `suppressed` で、reviewer run・試行の記録は 0。
#[tokio::test]
async fn project_controller_checkpoint_shadow_starts_no_reviewer_run() {
    let mut c = cp_case(Some(ControllerMode::Shadow), true, Reply::Valid);
    c.wake();
    c.wake();
    let evaluate = c.decisions("evaluate");
    assert_eq!(evaluate.len(), 1, "shadow records one decision per trigger");
    assert!(
        evaluate
            .iter()
            .all(|d| d.effect_state == EffectState::Suppressed)
    );
    assert!(c.reviewer_runs().is_empty());
    assert_eq!(c.reviewer_rows(), 0);
    assert!(c.artifacts().is_empty());
}

/// `suppressed` の判断の後の due は open（実行中）にならない（D1.3）: shadow の判断は発火済みに数え、同じ trigger の
/// 2 回目の判断は無く、snapshot の `running` も無い。人が `enabled` にした後、次の trigger（`children:4`）の `evaluate` は
/// suppressed の判断に阻まれず（`checkpoint_evaluation_open` にならない）、評価の run が 1 回起きる。
#[tokio::test]
async fn project_controller_checkpoint_suppressed_due_does_not_open() {
    let mut c = cp_case(Some(ControllerMode::Shadow), true, Reply::Valid);
    c.wake();
    c.wake();
    let evaluate = c.decisions("evaluate");
    assert_eq!(
        evaluate.len(),
        1,
        "no second decision for the fired trigger"
    );
    assert_eq!(evaluate[0].effect_state, EffectState::Suppressed);
    let cp = c.last_checkpoint().expect("checkpoint");
    assert_eq!(cp.running, None, "a suppressed decision is not running");
    assert!(cp.due.is_empty(), "children:2 fired in shadow");
    c.store
        .project_controller_set_mode(
            &c.p.id.to_string(),
            ControllerMode::Enabled,
            None,
            "human:test",
            c.env.now(),
        )
        .unwrap();
    done_child(&c.env, &c.store, &c.p, &c.root);
    done_child(&c.env, &c.store, &c.p, &c.root);
    c.wake();
    let applied: Vec<_> = c
        .decisions("evaluate")
        .into_iter()
        .filter(|d| d.effect_state == EffectState::Applied)
        .collect();
    assert_eq!(applied.len(), 1);
    let id = applied[0].id.clone();
    assert_eq!(c.attempts(), vec![pc::attempt_artifact_name(&id, 1)]);
    assert_eq!(c.reviewer_runs().len(), 1);
    c.finish_runs().await;
    let requests = c.adapter.requests.lock().unwrap().clone();
    let section = requests[0]
        .context
        .review
        .as_ref()
        .and_then(|r| r.parent_checkpoint.as_ref())
        .expect("parent_checkpoint section");
    assert_eq!(section.triggers, vec!["children:4".to_string()]);
}

/// 写していない評価がある間: TS の no-progress の `escalate`（act でない guard の行動）は検証を通って効果（人への
/// notice）になり、計画を進める行動（評価の run の追加）は起きない。
#[tokio::test]
async fn project_controller_checkpoint_no_progress_escalates_while_unmapped() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    c.wake();
    c.finish_runs().await;
    c.wake();
    let escalate = c.decisions("escalate");
    assert_eq!(escalate.len(), 1, "the guard escalation passed validation");
    assert_eq!(escalate[0].effect_state, EffectState::Applied);
    assert!(escalate[0].validation.contains("pc/no-progress"));
    assert_eq!(c.last_checkpoint().expect("checkpoint").unmapped.len(), 1);
    assert_eq!(c.reviewer_runs().len(), 1, "no further evaluation run");
}

/// root の final review の run（runs 索引の reviewer の行・`WorkerStarted`）は評価の試行に数えない: 試行は試行の記録の
/// name だけで数え、最初の評価は `attempt-1`。
#[tokio::test]
async fn project_controller_checkpoint_final_review_run_is_not_an_attempt() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    let review_run = "final-review-run".to_string();
    c.store
        .append_event(
            c.root.id,
            &Event::WorkerStarted {
                run_id: review_run.clone(),
                adapter: "instant".into(),
                model: "m".into(),
                provider: Some("p1".into()),
                account: None,
                role: Some(RunRole::Reviewer),
                task_role: None,
            },
        )
        .unwrap();
    // runs 索引の final review の行（終わった run）。行は SQL で置く（起動口の外で runs の行を書く経路を試験に増やさない）。
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .execute(
            "INSERT INTO runs (run_id, task_id, role, seq, status, started_at, finished_at)
             VALUES (?1, ?2, 'reviewer', 1, 'completed', ?3, ?3)",
            rusqlite::params![review_run, c.root.id.to_string(), T0],
        )
        .unwrap();
    assert_eq!(c.reviewer_rows(), 1);
    c.wake();
    let id = c.decisions("evaluate")[0].id.clone();
    assert_eq!(c.attempts(), vec![pc::attempt_artifact_name(&id, 1)]);
    assert_eq!(
        c.reviewer_runs().len(),
        2,
        "the final review run and one evaluation run"
    );
    assert_eq!(c.reviewer_rows(), 2);
}

/// root の run が手元にある間（`has_task_run`）は評価の run を起こさない（`register_run` が entry を上書きするため）。
/// run が無くなった次の tick で起こす。
#[tokio::test]
async fn project_controller_checkpoint_waits_while_root_has_local_run() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    let key = RunKey {
        task: c.root.id,
        work_unit: None,
    };
    c.d.supervisor.register_run(
        key.clone(),
        RunEntry {
            run_id: "root-worker-run".into(),
            provider: "p1".into(),
            handle: tokio::spawn(async {}),
            since: OffsetDateTime::now_utc(),
            cluster: None,
            account: None,
            account_adapter: None,
            container: None,
            cos: false,
            route: None,
        },
    );
    c.wake();
    c.wake();
    assert_eq!(c.decisions("evaluate").len(), 1);
    assert!(
        c.attempts().is_empty(),
        "no attempt while the root run is local"
    );
    assert!(c.reviewer_runs().is_empty());
    assert!(c.d.supervisor.take_run(&key).is_some());
    c.wake();
    assert_eq!(c.attempts().len(), 1);
    assert_eq!(c.reviewer_runs().len(), 1);
}

/// 試行の上限（既定 2）: 節の無い `review.json` は上限の前は最終の記録を書かず次の tick で試行し直し、上限の試行で
/// `invalid` の最終の記録を書く。`invalid` の評価は写していない評価として `escalate` だけを許す。
#[tokio::test]
async fn project_controller_checkpoint_attempt_limit_records_invalid() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Missing);
    c.wake();
    c.finish_runs().await;
    assert_eq!(c.attempts().len(), 1);
    assert!(c.finals().is_empty(), "invalid below the limit is retried");
    c.wake();
    c.finish_runs().await;
    let id = c.decisions("evaluate")[0].id.clone();
    assert_eq!(
        c.attempts(),
        vec![
            pc::attempt_artifact_name(&id, 1),
            pc::attempt_artifact_name(&id, 2)
        ]
    );
    assert_eq!(c.finals(), vec![pc::evaluation_artifact_name(&id)]);
    assert_eq!(c.final_record().status, pc::RecordStatus::Invalid);
    c.wake();
    assert_eq!(c.attempts().len(), 2, "no attempt beyond the limit");
    let cp = c.last_checkpoint().expect("checkpoint");
    assert_eq!(cp.unmapped.len(), 1);
    assert_eq!(
        cp.unmapped[0]
            .allowed
            .iter()
            .map(|a| a.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["escalate"]
    );
}

/// 試行の上限に達した判断で run が無い（試行の記録だけが残り決着しなかった）とき、起動の段が `invalid` の最終の記録を
/// 書き、run を起こさない（D2.3）。
#[tokio::test]
async fn project_controller_checkpoint_limit_without_run_records_invalid() {
    let mut c = cp_case(Some(ControllerMode::Enabled), true, Reply::Valid);
    c.d.supervisor.register_run(
        RunKey {
            task: c.root.id,
            work_unit: None,
        },
        RunEntry {
            run_id: "root-worker-run".into(),
            provider: "p1".into(),
            handle: tokio::spawn(async {}),
            since: OffsetDateTime::now_utc(),
            cluster: None,
            account: None,
            account_adapter: None,
            container: None,
            cos: false,
            route: None,
        },
    );
    c.wake();
    let id = c.decisions("evaluate")[0].id.clone();
    for n in 1..=2 {
        c.store
            .append_event(
                c.root.id,
                &Event::ArtifactProduced {
                    run_id: format!("lost-run-{n}"),
                    artifact: task_core::ArtifactRef {
                        name: pc::attempt_artifact_name(&id, n),
                        path: "/nonexistent".into(),
                        sha256: "0".repeat(64),
                        kind: pc::ATTEMPT_ARTIFACT_KIND.into(),
                        declared: false,
                    },
                },
            )
            .unwrap();
    }
    c.d.supervisor.take_runs();
    c.wake();
    assert!(c.reviewer_runs().is_empty(), "no run beyond the limit");
    assert_eq!(c.finals(), vec![pc::evaluation_artifact_name(&id)]);
    assert_eq!(c.final_record().status, pc::RecordStatus::Invalid);
}
