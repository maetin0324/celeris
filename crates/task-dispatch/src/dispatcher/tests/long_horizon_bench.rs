//! ADR 2026-11-04-long-horizon-bench（A8 実行）: long-horizon ベンチのイベント再生（接頭辞 `long_horizon_bench_`）。
//!
//! 課題の台本（`tests/fixtures/long-horizon/oracle/<case>/scenario.json`）を本物の store・dispatcher の
//! project controller の tick に流し、腕 `old.passive`・`old.proactive`（案件 mode `off`、台本の CoS が起票）と
//! `new`（mode `enabled`、本物の control-ts の bundle を本物の V8 で評価する [`TsProjectDecider`]）を比べる。
//! CoS・worker・reviewer・人は台本（LLM 0）。時計は注入した仮想時刻。一時 DB だけを開き、本番の DB・daemon に触れない。
//!
//! `LONG_HORIZON_BENCH_RAW_DIR` があれば試行ごとに `facts/<trial>.json`（`celeris.long-horizon-facts/1`。腕の欄を
//! 持たない）・`arm-map/<trial>.json`・`trials/<trial>/repo/`（使い捨ての作業 tree の最終の木）を書く。採点と集計は
//! 別 process の `celeris-campaign` の試験（`crates/campaign/tests/long_horizon_bench.rs`）が行う。
//! `new` の腕は bundle（`CELERIS_SCRIPT_BUNDLE_DIR`）が要る。無ければ `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` では失敗、
//! それ以外は `CELERIS_SCRIPT_SKIP_LOG` に 1 行書いて戻る（`scripts/dev/test-affected.sh` が SKIP を数える）。

use super::*;
use crate::project_decider_ts::{DeciderLimits, TsProjectDecider};
use crate::script_policy::{PolicyFuture, PolicyReply, PolicyScript, PolicyScriptBundle};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use task_core::project_proposal::ProjectSnapshot;
use task_core::project_wake::NewProjectWake;

const STEP_SECS: i64 = 300;
const NUDGE_AFTER_SECS: i64 = 6 * 3600;
const ANSWER_DELAY_SECS: i64 = 1800;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    OldPassive,
    OldProactive,
    New,
    /// 対照（ADR D5）: `new` と同じ設定で、decider は常に待つ（`ProjectDecision::none`）。人の bot は催促しない。
    CtlNull,
}

impl Arm {
    fn name(self) -> &'static str {
        match self {
            Arm::OldPassive => "old.passive",
            Arm::OldProactive => "old.proactive",
            Arm::New => "new",
            Arm::CtlNull => "ctl.null",
        }
    }

    /// 案件 mode（config の許可リストの差。ADR D2）。
    fn mode(self) -> ControllerMode {
        match self {
            Arm::OldPassive | Arm::OldProactive => ControllerMode::Off,
            Arm::New | Arm::CtlNull => ControllerMode::Enabled,
        }
    }

    /// 台本の CoS の種類（腕の名前ではなく台本の 2 通り。`new`・`ctl.null` は passive）。
    fn proactive_script(self) -> bool {
        self == Arm::OldProactive
    }
}

/// 対照 `ctl.null` の decider: 何もしない判断だけを返す。
struct NullDecider;

impl crate::project_controller::ProjectDecider for NullDecider {
    fn decide(&mut self, _input: &ProjectDecideInput) -> Result<ProjectDecision, String> {
        let mut d = ProjectDecision::none("bench/ctl-null");
        d.phase = ControllerPhase::Waiting;
        Ok(d)
    }
}

// ---- 台本の形 ----

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Scenario {
    case: String,
    name: String,
    bench_version: u32,
    outcomes: Vec<OutcomeSpec>,
    #[serde(default)]
    invalid_request: bool,
    #[serde(default = "yes")]
    kick: bool,
    #[serde(default)]
    cron_every_secs: Option<i64>,
    horizon_secs: i64,
    attempts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    work_secs: BTreeMap<String, i64>,
    #[serde(default)]
    answered_result: BTreeMap<String, String>,
    #[serde(default)]
    events: Vec<ScenarioEvent>,
    content: BTreeMap<String, String>,
    #[serde(default)]
    approval_delay_secs: Option<i64>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct OutcomeSpec {
    id: String,
    title: String,
    acceptance: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default = "yes")]
    required: bool,
    #[serde(default)]
    effect: Option<String>,
    #[serde(default)]
    approval: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct ScenarioEvent {
    op: String,
    #[serde(default)]
    at: i64,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    creates: Option<String>,
    #[serde(default)]
    outcomes: Option<Vec<String>>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    times: Option<usize>,
    #[serde(default)]
    boundary: Option<String>,
    #[serde(default)]
    delay: Option<i64>,
    #[serde(default)]
    n: Option<usize>,
    #[serde(default)]
    fault: Option<String>,
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/long-horizon")
}

fn load_scenario(case: &str) -> Scenario {
    let path = fixtures_root()
        .join("oracle")
        .join(case)
        .join("scenario.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn trial_salt() -> String {
    let path = fixtures_root().join("oracle/holdout.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["trial_salt"].as_str().unwrap().to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn request_text(outcomes: &[&OutcomeSpec], invalid: bool) -> String {
    let mut rows: Vec<Value> = outcomes
        .iter()
        .map(|o| {
            json!({"id": o.id, "title": o.title, "acceptance": o.acceptance, "depends_on": o.depends_on})
        })
        .collect();
    if invalid {
        // id の重複（Outcome block の形の誤り）。
        if let Some(first) = rows.first().cloned() {
            rows.push(first);
        }
    }
    format!(
        "pilot ベンチの案件。\n```celeris-outcomes\n{}\n```\n",
        serde_json::to_string(&rows).unwrap()
    )
}

// ---- 本物の bundle を本物の V8 で呼ぶ口 ----

pub(super) struct RealTs {
    handle: celeris_script::ScriptRuntimeHandle,
    sha256: String,
}

impl PolicyScript for RealTs {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: self.sha256.clone(),
            host_api: "1.0.0".into(),
        }
    }
    fn lane_rules(
        &self,
        _input: task_core::script_compare::LaneRulesInput,
        _deadline: std::time::Duration,
    ) -> PolicyFuture<'_> {
        unavailable()
    }
    fn route_priority(
        &self,
        _input: task_core::script_compare::RoutePriorityInput,
        _deadline: std::time::Duration,
    ) -> PolicyFuture<'_> {
        unavailable()
    }
    fn cos_digest_pass(
        &self,
        _call: crate::script_policy::CosDigestPassCall,
        _deadline: std::time::Duration,
    ) -> PolicyFuture<'_> {
        unavailable()
    }
    fn project_controller(
        &self,
        input: ProjectSnapshot,
        deadline: std::time::Duration,
    ) -> PolicyFuture<'_> {
        Box::pin(async move {
            let call = celeris_script::ScriptCall {
                domain: "project_controller".into(),
                input: serde_json::to_value(&input).map_err(|e| {
                    crate::script_policy::PolicyScriptError::new("invalid_output", e.to_string())
                })?,
                scope_task_ids: BTreeSet::new(),
                timeout: deadline,
            };
            let reply = self.handle.call(call).await.map_err(|e| {
                crate::script_policy::PolicyScriptError::new("exception", format!("{e:?}"))
            })?;
            Ok(PolicyReply {
                output: reply.output,
                isolate_epoch: reply.generation.saturating_add(1),
            })
        })
    }
}

fn unavailable<'a>() -> PolicyFuture<'a> {
    Box::pin(async {
        Err(crate::script_policy::PolicyScriptError::new(
            "unavailable",
            "-",
        ))
    })
}

/// 本物の bundle（`CELERIS_SCRIPT_BUNDLE_DIR`）。無ければ REQUIRE で失敗、それ以外は SKIP を記録して None。
pub(super) fn real_ts(test: &str) -> Option<Arc<RealTs>> {
    let dir = std::env::var_os("CELERIS_SCRIPT_BUNDLE_DIR").filter(|v| !v.is_empty());
    let Some(dir) = dir else {
        assert!(
            std::env::var("CELERIS_SCRIPT_REQUIRE_BUNDLE").as_deref() != Ok("1"),
            "{test}: CELERIS_SCRIPT_BUNDLE_DIR is required (CELERIS_SCRIPT_REQUIRE_BUNDLE=1)"
        );
        if let Some(log) = std::env::var_os("CELERIS_SCRIPT_SKIP_LOG") {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .unwrap();
            f.write_all(format!("{test}: no CELERIS_SCRIPT_BUNDLE_DIR\n").as_bytes())
                .unwrap();
        }
        return None;
    };
    let bundle = celeris_script::manifest::load_bundle_dir(Path::new(&dir))
        .unwrap_or_else(|e| panic!("{test}: bundle {}: {e:?}", Path::new(&dir).display()));
    let sha256 = bundle.manifest.bundle_sha256.clone();
    let handle = celeris_script::ScriptRuntimeHandle::start(bundle.source()).expect("start V8");
    let ts = Arc::new(RealTs { handle, sha256 });
    // 最初の呼び出し（isolate の温め）は長い deadline で 1 回。判定は捨てる。
    let warm = ts.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(warm.handle.call(celeris_script::ScriptCall {
            domain: "project_controller".into(),
            input: json!({}),
            scope_task_ids: BTreeSet::new(),
            timeout: std::time::Duration::from_secs(60),
        }));
    })
    .join()
    .unwrap();
    Some(ts)
}

// ---- 試行 ----

#[derive(Clone)]
struct RootState {
    logical: String,
    outcome: Option<String>,
    attempt: usize,
    created_by: &'static str,
    /// 人の指示（発言・回答・催促）を引いた起票か。
    instructed: bool,
    created_at: i64,
    started: bool,
    due: Option<i64>,
    result: String,
    yielded: bool,
    waiting: Option<&'static str>,
    wait_since: i64,
    statuses: Vec<String>,
}

struct Trial<'a> {
    sc: &'a Scenario,
    arm: Arm,
    env: Env,
    store: Arc<SqliteStore>,
    p: Project,
    d: Option<Dispatcher>,
    ts: Option<Arc<RealTs>>,
    t0: OffsetDateTime,
    repo: PathBuf,
    roots: BTreeMap<TaskId, RootState>,
    order: Vec<TaskId>,
    attempts_by_outcome: BTreeMap<String, usize>,
    current_outcomes: Vec<String>,
    request_valid: bool,
    human_ops: Vec<Value>,
    faults: Vec<Value>,
    effects: Vec<Value>,
    invocations: Vec<Value>,
    actors: Vec<Value>,
    statements: Vec<(i64, usize)>,
    cos_rejected: usize,
    nudges: usize,
    last_change: i64,
    idle_since: Option<i64>,
    seen_cos_inputs: i64,
    cos_inputs_due: i64,
    seen_escalations: BTreeSet<String>,
    escalation_open_since: Option<i64>,
    pending_boundary: Arc<StdMutex<Option<(Boundary, String)>>>,
    boundary_fired: Arc<StdMutex<Vec<(String, String)>>>,
    restarts: usize,
    achieved_at: Option<i64>,
    mid_digest: Option<(usize, String)>,
    fired_ops: BTreeSet<usize>,
    init_digest: String,
}

fn boundary_of(name: &str) -> Boundary {
    match name {
        "AfterIngest" => Boundary::AfterIngest,
        "AfterLease" => Boundary::AfterLease,
        "BeforeCommit" => Boundary::BeforeCommit,
        "AfterCommit" => Boundary::AfterCommit,
        "AfterEffect" => Boundary::AfterEffect,
        other => panic!("unknown boundary {other}"),
    }
}

impl<'a> Trial<'a> {
    fn new(sc: &'a Scenario, arm: Arm, ts: Option<Arc<RealTs>>, repo: PathBuf) -> Self {
        let env = Env::new();
        let store = env.open();
        let t0 = env.now();
        let outs: Vec<&OutcomeSpec> = sc.outcomes.iter().collect();
        let p = Project {
            id: ProjectId::new(),
            title: format!("pilot:{}-1", sc.name),
            request: request_text(&outs, sc.invalid_request),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            auto_integrate: false,
            created_at: t0,
            updated_at: t0,
        };
        store.project_create(&p).unwrap();
        // 使い捨ての作業 tree（public の repo-seed の写し）。
        copy_dir(
            &fixtures_root()
                .join("public")
                .join(&sc.case)
                .join("repo-seed"),
            &repo,
        );
        let mut t = Trial {
            sc,
            arm,
            env,
            store,
            p,
            d: None,
            ts,
            t0,
            repo,
            roots: BTreeMap::new(),
            order: Vec::new(),
            attempts_by_outcome: BTreeMap::new(),
            current_outcomes: sc.outcomes.iter().map(|o| o.id.clone()).collect(),
            request_valid: !sc.invalid_request,
            human_ops: Vec::new(),
            faults: Vec::new(),
            effects: Vec::new(),
            invocations: Vec::new(),
            actors: Vec::new(),
            statements: Vec::new(),
            cos_rejected: 0,
            nudges: 0,
            last_change: 0,
            idle_since: None,
            seen_cos_inputs: 0,
            cos_inputs_due: 0,
            seen_escalations: BTreeSet::new(),
            escalation_open_since: None,
            pending_boundary: Arc::new(StdMutex::new(None)),
            boundary_fired: Arc::new(StdMutex::new(Vec::new())),
            restarts: 0,
            achieved_at: None,
            mid_digest: None,
            fired_ops: BTreeSet::new(),
            init_digest: String::new(),
        };
        let now = t.env.now();
        // launch_root の許可は全腕に同じ行を作る（ADR 未解決の判断: 揃える。`off` の案件では使われない）。
        seed_grant(&t.store, &t.p.id.to_string(), now);
        // 同一初期条件の digest（D2）: mode を置く前に、ULID と時刻を伏せた行と作業 tree の hash を取る。
        t.init_digest = t.init_digest();
        if arm.mode() != ControllerMode::Off {
            t.store
                .project_controller_set_mode(
                    &t.p.id.to_string(),
                    arm.mode(),
                    None,
                    "human:bench",
                    now,
                )
                .unwrap();
        }
        t.start_daemon("i0");
        // 人の依頼（案件の作成）は全腕で stimulus 1 件。
        t.human_op("stimulus", "project_request", 0);
        t.actor("human", 0);
        t
    }

    fn secs(&self) -> i64 {
        (self.env.now() - self.t0).whole_seconds()
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
        let decider: Box<dyn crate::project_controller::ProjectDecider> = match (&self.ts, self.arm)
        {
            (_, Arm::CtlNull) => Box::new(NullDecider),
            (Some(ts), _) => {
                let script: Arc<dyn PolicyScript> = ts.clone();
                Box::new(TsProjectDecider::with_fixed_script(
                    DeciderLimits::default(),
                    script,
                ))
            }
            (None, _) => Box::new(TsProjectDecider::new(DeciderLimits::default())),
        };
        let settings = ProjectControllerSettings {
            enabled: true,
            coalesce_secs: 0,
            ..ProjectControllerSettings::default()
        };
        assert!(d.set_project_controller(settings, decider, owner.into()));
        let pending = self.pending_boundary.clone();
        let fired = self.boundary_fired.clone();
        let store = self.store.clone();
        let project = self.p.id.to_string();
        let clock = self.env.clock.clone();
        let outcomes = self.sc.outcomes.clone();
        d.set_project_controller_hook(Box::new(move |b| {
            let mut slot = pending.lock().unwrap();
            let Some((want, action)) = slot.clone() else {
                return;
            };
            if want != b {
                return;
            }
            *slot = None;
            drop(slot);
            fired
                .lock()
                .unwrap()
                .push((format!("{b:?}"), action.clone()));
            if let Some(ids) = action.strip_prefix("request:") {
                // 判定と適用の間の人の変更（D12 の human_epoch を同じ tx で上げる）。
                let keep: Vec<&str> = ids.split(',').collect();
                let outs: Vec<&OutcomeSpec> = outcomes
                    .iter()
                    .filter(|o| keep.contains(&o.id.as_str()))
                    .collect();
                let req = request_text(&outs, false);
                let now = *clock.lock().unwrap();
                store
                    .write_tx::<_, task_core::StoreError, _>(|tx| {
                        SqliteStore::project_set_text_tx(
                            tx,
                            project.parse::<ProjectId>().unwrap(),
                            None,
                            Some(&req),
                        )?;
                        SqliteStore::project_controller_human_bump_tx(tx, &project, now, 0)
                    })
                    .unwrap();
            } else if action == "crash" {
                panic!("bench crash at {b:?}");
            }
        }));
        // 初回の tick で全体の cursor を末尾に置く。
        d.tick_project_controller();
        self.d = Some(d);
    }

    fn restart(&mut self) {
        self.restarts += 1;
        self.d = None;
        self.store = self.env.open();
        let owner = format!("i{}", self.restarts);
        self.start_daemon(&owner);
    }

    fn human_op(&mut self, class: &str, kind: &str, at: i64) {
        self.human_ops
            .push(json!({"at": at, "class": class, "kind": kind}));
        self.last_change = at;
        self.idle_since = None;
    }

    fn actor(&mut self, who: &str, at: i64) {
        self.actors.push(json!({"at": at, "actor": who}));
    }

    fn invocation(&mut self, role: &str, at: i64) {
        self.invocations
            .push(json!({"role": role, "at": at, "usage": null}));
    }

    fn fault(&mut self, kind: &str, at: i64) {
        self.faults.push(json!({"kind": kind, "at": at}));
    }

    fn outcome(&self, id: &str) -> &OutcomeSpec {
        self.sc.outcomes.iter().find(|o| o.id == id).unwrap()
    }

    /// root を Outcome に対応づける（acceptance を文字どおり含む）。
    fn map_outcome(&self, t: &Task) -> Option<String> {
        let texts: BTreeSet<&str> = t.acceptance.iter().map(|c| c.text.as_str()).collect();
        self.sc
            .outcomes
            .iter()
            .find(|o| o.acceptance.iter().all(|a| texts.contains(a.as_str())))
            .map(|o| o.id.clone())
    }

    fn reached(&self, id: &str) -> bool {
        // 台本の CoS が次の手を決めるための内部の判定で、採点ではない（採点は celeris-campaign の検査 script）。
        let want = self.sc.content.get(id).cloned().unwrap_or_default();
        std::fs::read_to_string(self.repo.join("out").join(format!("{id}.txt")))
            .is_ok_and(|got| got == want)
    }

    fn open_required(&self) -> Vec<String> {
        self.sc
            .outcomes
            .iter()
            .filter(|o| o.required && self.current_outcomes.contains(&o.id) && !self.reached(&o.id))
            .map(|o| o.id.clone())
            .collect()
    }

    fn in_flight(&self) -> bool {
        self.roots.values().any(|r| !is_terminal(r.statuses.last()))
    }

    fn set_status(&mut self, id: TaskId, to: Status, reason: &str) {
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
        if let Some(r) = self.roots.get_mut(&id) {
            r.statuses.push(s.to_owned());
        }
    }

    /// 台本の CoS が root を起票する。controller が `enabled` の案件では人の指示を引かない起票は 422
    /// `project_controller_owns_launch`（ADR Project Controller D8。daemon 側の規則なので DB の mode で決める）。
    fn cos_create(&mut self, outcome: &str, instructed: bool) -> bool {
        self.cos_create_as(outcome, instructed, false)
    }

    fn cos_create_as(&mut self, outcome: &str, instructed: bool, answer: bool) -> bool {
        let now = self.secs();
        self.invocation("cos", now);
        self.actor("cos", now);
        if self.controller_enabled() && !instructed {
            self.cos_rejected += 1;
            return false;
        }
        let o = self.outcome(outcome).clone();
        let mut t = new_task(self.env.dir.path(), Check::Reviewer, 0);
        t.project_id = Some(self.p.id);
        t.title = format!("{}: {}", self.p.title, o.title);
        t.acceptance = o
            .acceptance
            .iter()
            .map(|a| Criterion {
                text: a.clone(),
                check: Check::Reviewer,
            })
            .collect();
        t.status = Status::Ready;
        self.store.create_task(&t, vec![]).unwrap();
        self.track_as(t.id, "cos", answer, instructed);
        true
    }

    fn track(&mut self, id: TaskId, by: &'static str, answer: bool) {
        self.track_as(id, by, answer, false);
    }

    fn track_as(&mut self, id: TaskId, by: &'static str, answer: bool, instructed: bool) {
        let t = self.store.get(id).unwrap().unwrap();
        let outcome = self.map_outcome(&t);
        let attempt = outcome.as_ref().map_or(0, |o| {
            let n = self.attempts_by_outcome.entry(o.clone()).or_insert(0);
            *n += 1;
            *n
        });
        let result = match &outcome {
            Some(o) if answer && self.sc.answered_result.contains_key(o) => {
                self.sc.answered_result[o].clone()
            }
            Some(o) => {
                let list = &self.sc.attempts[o];
                list.get(attempt - 1)
                    .or(list.last())
                    .cloned()
                    .unwrap_or_else(|| "real".into())
            }
            None => "real".into(),
        };
        let logical = format!(
            "{}:{}",
            outcome.clone().unwrap_or_else(|| "unmapped".into()),
            attempt
        );
        let now = self.secs();
        self.actor(by, now);
        self.order.push(id);
        self.roots.insert(
            id,
            RootState {
                logical,
                outcome,
                attempt,
                created_by: by,
                instructed,
                created_at: now,
                started: false,
                due: None,
                result,
                yielded: false,
                waiting: None,
                wait_since: 0,
                statuses: vec!["ready".into()],
            },
        );
    }

    /// controller が作った root を拾う。
    fn adopt_controller_roots(&mut self) {
        let mut found: Vec<Task> = self
            .store
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|t| t.project_id == Some(self.p.id) && t.parent_id.is_none())
            .filter(|t| !self.roots.contains_key(&t.id))
            .collect();
        found.sort_by_key(|t| t.created_at);
        for t in found {
            self.track(t.id, "daemon", false);
        }
    }

    fn work(&mut self) {
        let now = self.secs();
        let ids: Vec<TaskId> = self.order.clone();
        for id in ids {
            let r = self.roots[&id].clone();
            if is_terminal(r.statuses.last()) {
                continue;
            }
            let outcome = r.outcome.clone().unwrap_or_default();
            let secs = self.sc.work_secs.get(&outcome).copied().unwrap_or(3600);
            if !r.started {
                self.set_status(id, Status::Running, "bench_start");
                if secs >= 6 * 3600 {
                    self.fault("slow_worker", now);
                }
                self.invocation("worker", now);
                self.actor("worker", now);
                let mut due = now + secs;
                let mut waiting = None;
                if r.result.starts_with("question") {
                    self.store
                        .append_event(
                            id,
                            &Event::QuestionRaised {
                                run_id: "bench".into(),
                                text: "計画が不正。replan の方針を決めてほしい".into(),
                            },
                        )
                        .unwrap();
                    self.fault("plan_invalid", now);
                    waiting = Some("question");
                    due = i64::MAX;
                }
                let o = r.outcome.as_deref().map(|x| self.outcome(x).clone());
                if let Some(o) = &o
                    && o.approval.is_some()
                {
                    self.store
                        .append_event(id, &Event::ApprovalRequested)
                        .unwrap();
                    self.fault("approval_wait", now);
                    waiting = Some("approval");
                    due = i64::MAX;
                }
                if let Some(o) = &o
                    && o.effect.as_deref() == Some("external_job")
                {
                    self.effect(&o.id, "external_job", &r.logical, now);
                }
                if r.result == "children_fail" {
                    for n in 0..2 {
                        let mut c = new_task(self.env.dir.path(), Check::Reviewer, 0);
                        c.parent_id = Some(id);
                        c.project_id = Some(self.p.id);
                        c.status = if n == 0 { Status::Failed } else { Status::Done };
                        self.store.create_task(&c, vec![]).unwrap();
                    }
                    self.fault("child_failed", now);
                }
                let st = self.roots.get_mut(&id).unwrap();
                st.started = true;
                st.due = Some(due);
                st.waiting = waiting;
                st.wait_since = now;
                continue;
            }
            if let Some(kind) = r.waiting {
                if now - r.wait_since < self.sc.approval_delay_secs.unwrap_or(ANSWER_DELAY_SECS)
                    && kind == "approval"
                    || now - r.wait_since < ANSWER_DELAY_SECS && kind == "question"
                {
                    continue;
                }
                // 人の bot が答える（必須の回答）。
                if kind == "question" {
                    self.store
                        .append_event(
                            id,
                            &Event::Answered {
                                question: "計画が不正".into(),
                                answer: "replan して続けて".into(),
                                by: Some("human:bench".into()),
                            },
                        )
                        .unwrap();
                    self.human_op("required", "answer", now);
                    self.actor("human", now);
                    let st = self.roots.get_mut(&id).unwrap();
                    st.waiting = None;
                    st.due = Some(now + secs);
                    continue;
                }
                let o = self.outcome(&outcome).clone();
                let approve = o.approval.as_deref() == Some("approve");
                self.store
                    .append_event(
                        id,
                        &Event::ApprovalDecided {
                            by: "human:bench".into(),
                            approved: approve,
                            note: None,
                        },
                    )
                    .unwrap();
                self.human_op("required", "approval", now);
                self.actor("human", now);
                if approve {
                    let st = self.roots.get_mut(&id).unwrap();
                    st.waiting = None;
                    st.due = Some(now + secs);
                } else {
                    self.set_status(id, Status::Cancelled, "approval_rejected");
                }
                continue;
            }
            if r.due.is_some_and(|d| now >= d) {
                if r.result == "yield_then_fail" && !r.yielded {
                    // yield（acceptance の半分）。続きの run で同じ root が続く。
                    self.fault("partial_yield", now);
                    self.invocation("worker", now);
                    let st = self.roots.get_mut(&id).unwrap();
                    st.yielded = true;
                    st.due = Some(now + secs);
                    continue;
                }
                self.invocation("reviewer", now);
                self.actor("reviewer", now);
                match r.result.as_str() {
                    "real" | "question_then_real" | "children_ok" => {
                        let o = self.outcome(&outcome).clone();
                        if o.effect.as_deref() == Some("publish") {
                            self.effect(&o.id, "publish", &r.logical, now);
                        }
                        std::fs::create_dir_all(self.repo.join("out")).unwrap();
                        std::fs::write(
                            self.repo.join("out").join(format!("{outcome}.txt")),
                            &self.sc.content[&outcome],
                        )
                        .unwrap();
                        // 第 3 フェーズ I4: repo の無い root は成果物（`ArtifactProduced`）があって初めて取り込み済み。
                        // 台本の worker が `out/<id>.txt` を書いたので、本物の worker の run と同じく成果物の event を残す。
                        self.store
                            .append_event(
                                id,
                                &Event::ArtifactProduced {
                                    run_id: "bench".into(),
                                    artifact: task_core::ArtifactRef {
                                        name: format!("{outcome}.txt"),
                                        path: format!("out/{outcome}.txt"),
                                        sha256: sha(&self.sc.content[&outcome]),
                                        kind: "file".into(),
                                        declared: true,
                                    },
                                },
                            )
                            .unwrap();
                        self.set_status(id, Status::Done, "review_pass");
                    }
                    _ => {
                        // 虚偽の完了報告・同じ失敗: reviewer が落とす。成果物は正解でない。
                        if r.result == "yield_then_fail" {
                            self.fault("false_completion", now);
                        }
                        if r.result == "fail_same" {
                            self.fault("repeat_failure", now);
                        }
                        std::fs::create_dir_all(self.repo.join("out")).unwrap();
                        std::fs::write(
                            self.repo.join("out").join(format!("{outcome}.txt")),
                            "incomplete\n",
                        )
                        .unwrap();
                        self.set_status(id, Status::Failed, "review_fail");
                    }
                }
                self.cos_on_root_end(id);
            }
        }
    }

    fn effect(&mut self, outcome: &str, kind: &str, root: &str, at: i64) {
        self.effects.push(json!({
            "logical_id": format!("{}:{outcome}:{kind}", self.sc.case),
            "root": root, "at": at,
        }));
    }

    /// root の終了で CoS が 1 回起きる（全腕。done は root_done の要約、failed は受信箱の triage に当たる）。
    /// proactive の台本は次を起票する。controller が `enabled` の案件では CoS の入力に「controller が起票する」
    /// の文（付記 A3 の 9）が入るので、台本の CoS は起票しない。
    fn cos_on_root_end(&mut self, _id: TaskId) {
        let now = self.secs();
        self.invocation("cos", now);
        self.actor("cos", now);
        if self.arm.proactive_script() && !self.controller_enabled() {
            self.proactive_next(false);
        }
    }

    /// 案件の controller が `enabled` か（DB の行で決める。腕の名前は見ない）。
    fn controller_enabled(&self) -> bool {
        self.store
            .project_controller_state(&self.p.id.to_string())
            .unwrap()
            .is_some_and(|s| s.mode == ControllerMode::Enabled)
    }

    /// 同一初期条件の digest（ADR D2）。案件・request・`project_repos`・thread・brief・許可の行を、ULID・時刻・
    /// 案件 id を伏せて並べ、作業 tree の hash と一緒に sha256 にする。controller の状態（mode）は許可リストの差なので
    /// 入れない。
    fn init_digest(&self) -> String {
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        let pid = self.p.id.to_string();
        let mut h = Sha256::new();
        for table in [
            "projects",
            "project_repos",
            "chat_threads",
            "project_briefs",
            "authority_grants",
        ] {
            let filter = if table == "projects" {
                "id"
            } else {
                "project_id"
            };
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} WHERE {filter} = ?1"))
                .unwrap();
            let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
            let mut rows: Vec<String> = stmt
                .query_map([&pid], |r| {
                    let mut line = Vec::new();
                    for (i, c) in cols.iter().enumerate() {
                        if c == "id" || c.ends_with("_at") || c.ends_with("_id") {
                            continue;
                        }
                        let v: rusqlite::types::Value = r.get(i)?;
                        line.push(format!("{c}={v:?}"));
                    }
                    Ok(line.join("|").replace(&pid, "<project>"))
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            rows.sort();
            h.update(format!("{table}:{}\n", rows.join("\n")).as_bytes());
        }
        h.update(tree_digest(&self.repo).as_bytes());
        hex(&h.finalize())
    }

    fn next_open(&self) -> Option<String> {
        let open = self.open_required();
        self.sc
            .outcomes
            .iter()
            .filter(|o| open.contains(&o.id))
            .find(|o| o.depends_on.iter().all(|d| self.reached(d)))
            .map(|o| o.id.clone())
    }

    /// `invoke`: この呼び出しで CoS が新たに起きる（root の終了の起動の中からは false）。
    fn proactive_next(&mut self, invoke: bool) {
        if self.in_flight() || !self.request_valid {
            return;
        }
        if let Some(o) = self.next_open() {
            let tries = self.attempts_by_outcome.get(&o).copied().unwrap_or(0);
            if tries < 2 {
                if invoke {
                    let at = self.secs();
                    self.invocation("cos", at);
                }
                self.cos_create(&o, false);
            }
        }
    }

    /// new の腕: controller の CoS 入力（replan）に台本の CoS が brief の書き直しで答える。
    fn cos_inputs(&mut self) {
        // CoS の run は 1 step（5 分）後に答える（判断と同じ時刻の brief は「書き直し」に見えない）。
        let n = self.cos_inputs_due;
        self.cos_inputs_due = self
            .env
            .count("SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%'");
        while self.seen_cos_inputs < n {
            self.seen_cos_inputs += 1;
            let now = self.secs();
            self.invocation("cos", now);
            self.actor("cos", now);
            self.store
                .project_brief_put(&task_core::project_brief::ProjectBrief {
                    project_id: self.p.id,
                    objective: "pilot".into(),
                    state: "review の指摘を読んで次の試行で直す".into(),
                    open_issues: Vec::new(),
                    next_actions: vec!["次の試行".into()],
                    waiting_on: Vec::new(),
                    evidence_refs: Vec::new(),
                    updated_at: self.env.now(),
                    updated_by: "cos:bench".into(),
                })
                .unwrap();
            // 仮定: CoS は brief を書き直した後に `POST …/controller/wake` で知らせる（brief の更新は wake の
            // 種類に無い。本番の CoS がこれを送るかは未確認。a8b.md の未解決）。
            self.store
                .project_wake_ingest(
                    &[NewProjectWake {
                        project_id: self.p.id.to_string(),
                        wake_key: format!("external:cos-brief:{}", self.seen_cos_inputs),
                        kind: ProjectWakeKind::ExternalReleased,
                        source_event_id: None,
                        source_ref: Some(format!("cos-brief:{}", self.seen_cos_inputs)),
                    }],
                    None,
                    0,
                    self.env.now(),
                )
                .unwrap();
        }
    }

    fn escalations(&mut self) {
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        let mut stmt = conn
            .prepare("SELECT source_key FROM feed_sources WHERE source_key LIKE ?1")
            .unwrap();
        let keys: Vec<String> = stmt
            .query_map(
                [format!("project_controller:escalate:{}:%", self.p.id)],
                |r| r.get(0),
            )
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for k in keys {
            if self.seen_escalations.insert(k.clone()) {
                let now = self.secs();
                self.escalation_open_since.get_or_insert(now);
                self.actor("daemon", now);
            }
        }
    }

    /// 人の bot の催促（D7）: 開いた必須の Outcome があり、未終端の root も開いた問いも無い状態が 6 時間続いたら
    /// 「次のタスクを起票して」。回数は Outcome の数まで。すべて avoidable。
    fn nudge(&mut self) {
        let now = self.secs();
        if self.in_flight() || self.escalation_open_since.is_some() || !self.request_valid {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert(now);
        // 対照 `ctl.null` の人の bot は催促しない（到達が controller によることを確かめる対照）。
        if self.arm == Arm::CtlNull
            || self.nudges >= self.sc.outcomes.len()
            || now - since < NUDGE_AFTER_SECS
        {
            return;
        }
        let Some(o) = self.next_open() else {
            return;
        };
        self.nudges += 1;
        self.human_op("avoidable", "nudge", now);
        self.actor("human", now);
        self.cos_create(&o, true);
    }

    fn scenario_events(&mut self) {
        let now = self.secs();
        let events = self.sc.events.clone();
        for (i, e) in events.iter().enumerate() {
            if self.fired_ops.contains(&i) {
                continue;
            }
            let due = match e.op.as_str() {
                "unrelated_wakes_after_achieved" => self
                    .achieved_at
                    .is_some_and(|a| now >= a + e.delay.unwrap_or(3600)),
                _ => now >= e.at,
            };
            if !due {
                continue;
            }
            self.fired_ops.insert(i);
            let class = e.class.clone().unwrap_or_else(|| "stimulus".into());
            match e.op.as_str() {
                "human_message" => {
                    // 発言の本文は台本の CoS に渡らない（腕によらず種類だけで応答する）。
                    let _ = e.text.as_deref();
                    self.human_op(&class, "message", now);
                    self.actor("human", now);
                    self.statements.push((now, self.order.len()));
                    self.invocation("cos", now);
                    if let Some(o) = &e.creates
                        && !self.reached(o)
                        && !self.roots.values().any(|r| {
                            r.outcome.as_deref() == Some(o) && !is_terminal(r.statuses.last())
                        })
                    {
                        self.cos_create(o, true);
                    }
                }
                "human_request" => {
                    let ids = e.outcomes.clone().unwrap_or_default();
                    self.human_request(&ids, &class, now);
                    if let Some(f) = &e.fault {
                        self.fault(f, now);
                    }
                }
                "human_request_at_boundary" => {
                    let ids = e.outcomes.clone().unwrap_or_default();
                    // 境界は controller の tick の中にある。`off` の案件では tick が判断しないので、すぐ適用する。
                    if self.controller_enabled() {
                        *self.pending_boundary.lock().unwrap() = Some((
                            boundary_of(e.boundary.as_deref().unwrap()),
                            format!("request:{}", ids.join(",")),
                        ));
                    } else {
                        self.human_request(&ids, &class, now);
                        self.fault("human_change_before_apply", now);
                    }
                }
                "redeliver_done" => {
                    let o = e.outcome.clone().unwrap();
                    let id = self
                        .order
                        .iter()
                        .find(|id| {
                            let r = &self.roots[*id];
                            r.outcome.as_deref() == Some(&o)
                                && r.statuses.last().map(String::as_str) == Some("done")
                        })
                        .copied();
                    if let Some(id) = id {
                        for _ in 0..e.times.unwrap_or(10) {
                            self.store
                                .append_event(
                                    id,
                                    &Event::Transitioned {
                                        from: Status::Running,
                                        to: Status::Done,
                                        reason: "review_pass".into(),
                                    },
                                )
                                .unwrap();
                        }
                        self.fault("redelivery", now);
                    } else {
                        // まだ done の root が無い: 次の step でやり直す。
                        self.fired_ops.remove(&i);
                    }
                }
                "restart" => {
                    self.restart();
                    self.fault("restart", now);
                }
                "restart_at_boundary" => {
                    if self.controller_enabled() {
                        *self.pending_boundary.lock().unwrap() =
                            Some((boundary_of(e.boundary.as_deref().unwrap()), "crash".into()));
                    } else {
                        self.restart();
                        self.fault("restart", now);
                    }
                }
                "unrelated_wakes_after_achieved" => {
                    for n in 0..e.n.unwrap_or(3) {
                        self.store
                            .project_wake_ingest(
                                &[NewProjectWake {
                                    project_id: self.p.id.to_string(),
                                    wake_key: format!("external:bench:after:{n}"),
                                    kind: ProjectWakeKind::ExternalReleased,
                                    source_event_id: None,
                                    source_ref: Some(format!("bench:{n}")),
                                }],
                                None,
                                0,
                                self.env.now(),
                            )
                            .unwrap();
                    }
                    self.fault("unrelated_wakes", now);
                }
                "human_answer_instruct" => {
                    self.human_op(&class, "answer", now);
                    self.actor("human", now);
                    self.escalation_open_since = None;
                    self.invocation("cos", now);
                    let o = e.outcome.clone().unwrap();
                    self.cos_create_as(&o, true, true);
                }
                other => panic!("unknown op {other}"),
            }
        }
    }

    fn human_request(&mut self, ids: &[String], class: &str, now: i64) {
        let outs: Vec<&OutcomeSpec> = self
            .sc
            .outcomes
            .iter()
            .filter(|o| ids.contains(&o.id))
            .collect();
        let req = request_text(&outs, false);
        let project = self.p.id.to_string();
        let at = self.env.now();
        self.store
            .write_tx::<_, task_core::StoreError, _>(|tx| {
                SqliteStore::project_set_text_tx(tx, self.p.id, None, Some(&req))?;
                SqliteStore::project_controller_human_bump_tx(tx, &project, at, 0)
            })
            .unwrap();
        if self.controller_enabled() && self.escalation_open_since.is_some() {
            // escalate の後の controller は blocked。人は依頼を直した同じ操作で mode を置き直す（PUT …/controller）。
            self.store
                .project_controller_set_mode(
                    &project,
                    ControllerMode::Enabled,
                    None,
                    "human:bench",
                    at,
                )
                .unwrap();
            seed_grant(&self.store, &project, at);
        }
        self.current_outcomes = ids.to_vec();
        self.request_valid = true;
        self.escalation_open_since = None;
        self.human_op(class, "request_change", now);
        self.actor("human", now);
        if self.arm.proactive_script() && !self.controller_enabled() {
            self.proactive_next(true);
        }
    }

    /// 人の依頼の開始。daemon には全腕で wake（`POST …/controller/wake` 相当。`off` の案件では評価されない）。
    /// 台本の CoS は依頼を読んで最初の root を起票する。ただし controller が `enabled` の案件では CoS の入力に
    /// 「controller が起票する」の文が入るので起票しない。
    fn kick(&mut self) {
        if self.sc.kick {
            self.store
                .project_wake_ingest(
                    &[NewProjectWake {
                        project_id: self.p.id.to_string(),
                        wake_key: "external:bench:start".into(),
                        kind: ProjectWakeKind::ExternalReleased,
                        source_event_id: None,
                        source_ref: Some("bench:start".into()),
                    }],
                    None,
                    0,
                    self.env.now(),
                )
                .unwrap();
        }
        if self.sc.kick
            && self.request_valid
            && !self.controller_enabled()
            && let Some(o) = self.next_open()
        {
            self.cos_create(&o, true);
        }
    }

    fn tick(&mut self) {
        let Some(d) = self.d.as_mut() else {
            return;
        };
        let r =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.tick_project_controller()));
        let fired: Vec<(String, String)> = self.boundary_fired.lock().unwrap().drain(..).collect();
        let now = self.secs();
        for (_, action) in fired {
            if let Some(rest) = action.strip_prefix("request:") {
                let ids: Vec<String> = rest.split(',').map(str::to_owned).collect();
                self.current_outcomes = ids;
                self.human_op("stimulus", "request_change", now);
                self.actor("human", now);
                self.fault("human_change_before_apply", now);
            } else {
                self.fault("restart", now);
            }
        }
        if r.is_err() {
            self.restart();
        }
    }

    fn cron(&mut self) {
        let Some(every) = self.sc.cron_every_secs else {
            return;
        };
        let now = self.secs();
        // cron の wake は全腕に入る（`off` の案件では controller が評価しない）。
        if now % every == 0 {
            self.store
                .project_wake_ingest(
                    &[NewProjectWake {
                        project_id: self.p.id.to_string(),
                        wake_key: format!("cron:bench:{now}"),
                        kind: ProjectWakeKind::Cron,
                        source_event_id: None,
                        source_ref: None,
                    }],
                    None,
                    0,
                    self.env.now(),
                )
                .unwrap();
        }
    }

    fn run(mut self) -> (Value, PathBuf) {
        self.kick();
        let steps = self.sc.horizon_secs / STEP_SECS;
        for _ in 0..steps {
            self.env.advance(STEP_SECS);
            self.cron();
            self.scenario_events();
            self.tick();
            self.adopt_controller_roots();
            self.cos_inputs();
            self.escalations();
            self.work();
            self.nudge();
            if self.achieved_at.is_none() && self.open_required().is_empty() {
                self.achieved_at = Some(self.secs());
            }
            if self.mid_digest.is_none() && self.secs() >= self.sc.horizon_secs / 2 {
                self.mid_digest = Some(self.events_digest(None));
            }
        }
        let facts = self.facts();
        (facts, self.repo.clone())
    }

    fn events_digest(&self, limit: Option<usize>) -> (usize, String) {
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        let mut stmt = conn
            .prepare("SELECT task_id, seq, json FROM events ORDER BY rowid")
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}|{}|{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let n = limit.unwrap_or(rows.len()).min(rows.len());
        let mut h = Sha256::new();
        for r in &rows[..n] {
            h.update(r.as_bytes());
            h.update(b"\n");
        }
        (n, hex(&h.finalize()))
    }

    fn facts(&self) -> Value {
        let (mid_n, mid) = self.mid_digest.clone().unwrap_or((0, String::new()));
        let (_, prefix) = self.events_digest(Some(mid_n));
        let roots: Vec<Value> = self
            .order
            .iter()
            .map(|id| {
                let r = &self.roots[id];
                let t = self.store.get(*id).unwrap().unwrap();
                let acc: Vec<&str> = t.acceptance.iter().map(|c| c.text.as_str()).collect();
                json!({
                    "logical_id": r.logical, "outcome": r.outcome, "attempt": r.attempt,
                    "created_by": r.created_by, "instructed": r.instructed,
                    "created_at": r.created_at,
                    "statuses": r.statuses, "acceptance_sha256": sha(&acc.join("\n")),
                })
            })
            .collect();
        let running_untracked = self
            .store
            .list(Some(Status::Running))
            .unwrap()
            .into_iter()
            .filter(|t| t.project_id == Some(self.p.id) && !self.roots.contains_key(&t.id))
            .count();
        let statements: Vec<Value> = self
            .statements
            .iter()
            .enumerate()
            .map(|(i, (at, before))| {
                let end = self.statements.get(i + 1).map_or(self.order.len(), |s| s.1);
                let by_cos = self.order[*before..end]
                    .iter()
                    .filter(|id| self.roots[*id].created_by == "cos")
                    .count();
                json!({"index": i + 1, "at": at, "tasks_created_by_cos": by_cos})
            })
            .collect();
        let questions: Vec<String> = self
            .seen_escalations
            .iter()
            .map(|k| {
                // `project_controller:escalate:<project>:<key>` の key だけ（project の id は試行ごとに違う）。
                let prefix = format!("project_controller:escalate:{}:", self.p.id);
                let mut key = k.strip_prefix(&prefix).unwrap_or(k).to_owned();
                for (id, r) in &self.roots {
                    key = key.replace(&id.to_string(), &r.logical);
                }
                key
            })
            .collect();
        // `achieved_at` は台本の木が揃った時刻で、腕によらない。controller の主張は案件の phase だけから読む（I12）。
        let controller_achieved = self
            .store
            .project_controller_state(&self.p.id.to_string())
            .unwrap()
            .is_some_and(|c| c.phase == ControllerPhase::Achieved);
        let mut v = json!({
            "schema": "celeris.long-horizon-facts/1",
            "bench_version": self.sc.bench_version,
            "dataset": "long_horizon_replay",
            "case": self.sc.case,
            "seed": 1,
            "init_digest": self.init_digest,
            "horizon_secs": self.sc.horizon_secs,
            "roots": roots,
            "effects": self.effects,
            "human_ops": self.human_ops,
            "faults": self.faults,
            "invocations": self.invocations,
            "actors": self.actors,
            "statements": statements,
            "questions": questions,
            "cos_launch_rejected": self.cos_rejected,
            "running_untracked": running_untracked,
            "achieved_at": self.achieved_at,
            "controller_achieved": controller_achieved,
            "events_append_only": self.mid_digest.is_some() && mid == prefix,
            "usage": {"tokens": null, "quota": null, "wall_secs": null, "missing": "tier=replay"},
            "virtual_secs_to_reach": self.achieved_at,
        });
        // 腕を示す欄は持たない（D6）。
        if let Some(o) = v.as_object_mut() {
            o.retain(|k, _| k != "arm");
        }
        v
    }
}

fn is_terminal(s: Option<&String>) -> bool {
    matches!(
        s.map(String::as_str),
        Some("done") | Some("failed") | Some("cancelled")
    )
}

/// 作業 tree の木 hash（相対 path と内容の sha256 を並べたもの）。
fn tree_digest(root: &Path) -> String {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().display().to_string();
                out.push(format!(
                    "{rel}:{}",
                    hex(&Sha256::digest(std::fs::read(&p).unwrap()))
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    sha(&out.join("\n"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

pub(super) fn seed_grant(store: &SqliteStore, project: &str, now: OffsetDateTime) {
    use task_core::authority as a;
    store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_supersede_operation_grants_tx(tx, project, "human:bench", now)?;
            SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: ulid::Ulid::new().to_string(),
                    project_id: project.to_owned(),
                    scope: a::GrantScope {
                        kind: a::ScopeKind::Project,
                        reference: project.to_owned(),
                    },
                    action_class: a::ActionClass::LaunchRoot,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:bench".into(),
                    question_key: None,
                    note: "ベンチの案件を enabled にした".into(),
                    expires_at: (now + time::Duration::days(300))
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap(),
                    budget: 100,
                },
                now,
            )?
            .unwrap();
            Ok(())
        })
        .unwrap();
}

/// 課題 1 件 × 腕 1 つ。同じ seed で 2 回流し、facts の bytes が一致することを確かめてから書く。
fn bench(case: &str, arm: Arm, test: &str) {
    let ts = if arm == Arm::New {
        match real_ts(test) {
            Some(ts) => Some(ts),
            None => return,
        }
    } else {
        None
    };
    let sc = load_scenario(case);
    let work = crate::test_support::WritableTempDir::new();
    let mut outs = Vec::new();
    for run in 0..2 {
        let repo = work.path().join(format!("run{run}/repo"));
        let (facts, repo) = Trial::new(&sc, arm, ts.clone(), repo).run();
        outs.push((serde_json::to_string_pretty(&facts).unwrap(), repo));
    }
    assert_eq!(
        outs[0].0, outs[1].0,
        "{test}: two runs of the same seed differ"
    );
    let facts: Value = serde_json::from_str(&outs[0].0).unwrap();
    // 機構の不変条件（腕によらない）: 追記専用・持ち主の無い running 0。
    assert_eq!(
        facts["events_append_only"], true,
        "{test}: events are not append-only"
    );
    assert_eq!(facts["running_untracked"], 0, "{test}");
    if arm.mode() == ControllerMode::Enabled {
        assert_eq!(
            facts["cos_launch_rejected"], 0,
            "{test}: the scripted CoS was asked to launch"
        );
    }
    let Some(dir) = std::env::var_os("LONG_HORIZON_BENCH_RAW_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let trial = sha(&format!("{}|{}|{}|1", trial_salt(), case, arm.name()))[..16].to_owned();
    for sub in ["facts", "arm-map", "trials"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(
        dir.join("facts").join(format!("{trial}.json")),
        format!("{}\n", outs[0].0),
    )
    .unwrap();
    std::fs::write(
        dir.join("arm-map").join(format!("{trial}.json")),
        format!(
            "{}\n",
            json!({"trial": trial, "case": case, "arm": arm.name(), "seed": 1})
        ),
    )
    .unwrap();
    let dst = dir.join("trials").join(&trial).join("repo");
    let _ = std::fs::remove_dir_all(&dst);
    copy_dir(&outs[0].1, &dst);
}

macro_rules! bench_case {
    ($case:literal, $p:ident, $a:ident, $n:ident, $z:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $z() {
            bench($case, Arm::CtlNull, stringify!($z));
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $p() {
            bench($case, Arm::OldPassive, stringify!($p));
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $a() {
            bench($case, Arm::OldProactive, stringify!($a));
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $n() {
            bench($case, Arm::New, stringify!($n));
        }
    };
}

bench_case!(
    "C01",
    long_horizon_bench_c01_stale_event_old_passive,
    long_horizon_bench_c01_stale_event_old_proactive,
    long_horizon_bench_c01_stale_event_new,
    long_horizon_bench_c01_stale_event_ctl_null
);
bench_case!(
    "C02",
    long_horizon_bench_c02_partial_result_old_passive,
    long_horizon_bench_c02_partial_result_old_proactive,
    long_horizon_bench_c02_partial_result_new,
    long_horizon_bench_c02_partial_result_ctl_null
);
bench_case!(
    "C03",
    long_horizon_bench_c03_slow_worker_old_passive,
    long_horizon_bench_c03_slow_worker_old_proactive,
    long_horizon_bench_c03_slow_worker_new,
    long_horizon_bench_c03_slow_worker_ctl_null
);
bench_case!(
    "C04",
    long_horizon_bench_c04_child_failed_old_passive,
    long_horizon_bench_c04_child_failed_old_proactive,
    long_horizon_bench_c04_child_failed_new,
    long_horizon_bench_c04_child_failed_ctl_null
);
bench_case!(
    "C05",
    long_horizon_bench_c05_plan_invalid_old_passive,
    long_horizon_bench_c05_plan_invalid_old_proactive,
    long_horizon_bench_c05_plan_invalid_new,
    long_horizon_bench_c05_plan_invalid_ctl_null
);
bench_case!(
    "C06",
    long_horizon_bench_c06_auth_wait_old_passive,
    long_horizon_bench_c06_auth_wait_old_proactive,
    long_horizon_bench_c06_auth_wait_new,
    long_horizon_bench_c06_auth_wait_ctl_null
);
bench_case!(
    "C07",
    long_horizon_bench_c07_daemon_restart_old_passive,
    long_horizon_bench_c07_daemon_restart_old_proactive,
    long_horizon_bench_c07_daemon_restart_new,
    long_horizon_bench_c07_daemon_restart_ctl_null
);
bench_case!(
    "C08",
    long_horizon_bench_c08_multi_root_old_passive,
    long_horizon_bench_c08_multi_root_old_proactive,
    long_horizon_bench_c08_multi_root_new,
    long_horizon_bench_c08_multi_root_ctl_null
);
bench_case!(
    "C09",
    long_horizon_bench_c09_consult_short_continue_old_passive,
    long_horizon_bench_c09_consult_short_continue_old_proactive,
    long_horizon_bench_c09_consult_short_continue_new,
    long_horizon_bench_c09_consult_short_continue_ctl_null
);
bench_case!(
    "C10",
    long_horizon_bench_c10_repeat_failure_old_passive,
    long_horizon_bench_c10_repeat_failure_old_proactive,
    long_horizon_bench_c10_repeat_failure_new,
    long_horizon_bench_c10_repeat_failure_ctl_null
);
