//! Celeris 側の薄い driver（`docs/ops/dag-fault-comparison.md`）。
//!
//! scenarios.json の 1 点（scenario × fixed point × seed）を、既存 Celeris の通常経路でそのまま流す。
//! 新しい scheduler・router・投入 API は作らない。再利用先（ADR D1 の表の番号）:
//! - R6 通常 POST: campaign（DAG instance）ごとの root task は `celeris_exec::Transport` 越しの
//!   `POST /api/v1/tasks`（task-api の実 router）で作り、冪等キーは `celeris_exec::idempotency_label` の label。
//! - R1 ExecutionPlan: planner run の成果物 `execution-plan.json`（v3）に scenario の node・依存辺を
//!   WorkUnit・`depends_on` として写す。ready 判定・並列上限は task-core / task-dispatch の既存実装のまま。
//!   worker 枠は既存の `[execution] max_parallel_work_units` と dispatcher 全体の同時 run 上限に写す。
//! - R2 durable wait: 外部 node の worker 台本は ext-sim に投入して `Terminal::Waiting`（`result.json` の
//!   `{"type":"wait","kind":"cluster_job"}` と同じ終端）を返す。continuation の run が結果を消費する。
//! - R3 poller の差し替え口: `Dispatcher::set_cluster_job_poller` に ext-sim を差し、`qstat -xf` 相当の出力を
//!   返して既存の PBS parser を通す（実クラスタ・ssh には出ない）。
//! - R13 events / runs / SQLite 永続: 状態の真実は一時 DB の SQLite（`TaskStore`）。driver は読むだけ。
//! - stub worker: `tests/celeris_exec.rs`・dispatcher 試験（`dispatcher/tests/cluster_job_wait.rs` の
//!   `ScriptedAdapter`・`TreeWaitAdapter`）と同じ「台本の `WorkerAdapter`」。LLM 呼び出しは 0。
//!
//! 時間: 論理時計（`logical_secs`）は driver が離散事象として進める。全ての in-flight の run と poll が論理時計
//! 待ち（sleeper）になり、tick を回しても何も起きない（静止）ときだけ、最も早い期限まで進める。sleep の長さで
//! 時間を作らない（ADR-0125）。Celeris の poll は wait 開始から `poll_secs` 刻みで行われ、job が終わった最初の
//! 刻みで `F` を返す（途中の `R` の poll は返さず、待ち時間は同じ）。
//!
//! 測定専用の所有資源だけを使う: 呼び出し側が渡す一時 DB・一時 dir と、この process の中の Dispatcher・ext-sim。
//! 本番 daemon・本番 DB・本番設定・実クラスタには触れない（daemon process も起こさない）。
use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use task_core::{Status, TaskId, TaskStore};
use task_dispatch::{
    ClusterSpec, ContainersRuntimeConfig, DispatchConfig, Dispatcher, ExecutionConfig,
    KnowledgeRuntimeConfig,
    dispatcher::{ClusterJobNotice, ClusterJobNoticeOutcome, ClusterJobPollRequest},
};
use task_worker::{
    AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, Terminal, WorkerAdapter,
};

use super::ext::{
    DATASET, ExtBackend, ExtConfig, ExtSim, FaultSpec, JobState, Lookup, NoticeIds, StopHook,
    SubmitError, read_ledger,
};
use super::ipc::{NoticeChannel, Settled};
use crate::celeris_exec::{Transport, idempotency_label};

pub const SYSTEM: &str = "celeris";
const CLUSTER: &str = "dagbench-ext";
const HARNESS: &str = "dagbench";

// ---- scenarios.json（必要な欄だけ読む） ----

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioFile {
    pub schema_version: u32,
    pub time: TimeSpec,
    pub seed: u64,
    pub observation_caps: Caps,
    pub scenarios: Vec<Scenario>,
    pub fixed_points: FixedPoints,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimeSpec {
    pub poll_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Caps {
    pub max_logical_secs: u64,
    pub max_wall_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub resources: Resources,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resources {
    pub worker_slots: usize,
    pub external_job_slots: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: String,
    pub deps: Vec<String>,
    pub work_secs: u64,
    #[serde(default)]
    pub external: Option<External>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct External {
    pub logical_job_id: String,
    pub post_wait_work_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixedPoints {
    pub axes: Axes,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Axes {
    pub load: DerivedLoad,
    pub external_wait_ratio: DerivedExt,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DerivedLoad {
    pub derived_arrival_interval_secs: BTreeMap<String, BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DerivedExt {
    pub derived_ext_job_secs: BTreeMap<String, BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Point {
    pub id: String,
    pub concurrent_campaigns: usize,
    pub load: f64,
    pub external_wait_ratio: f64,
}

/// 1 回の測定の解決済みの入力（scenario・点・seed から派生値を引いたもの）。
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub schema_version: u32,
    pub scenario: Scenario,
    pub point: Point,
    pub seed: u64,
    pub poll_secs: u64,
    pub ext_job_secs: u64,
    pub arrival_interval_secs: u64,
    pub caps: Caps,
    /// 独立 grader で判定する node（無ければ従来どおり台本だけ）。
    pub grading: Option<std::sync::Arc<super::graded::Grading>>,
}

impl RunSpec {
    pub fn resolve(
        file: &ScenarioFile,
        scenario: &str,
        point: &str,
        seed: u64,
    ) -> Result<Self, String> {
        let sc = file
            .scenarios
            .iter()
            .find(|s| s.id == scenario)
            .ok_or_else(|| format!("unknown scenario {scenario}"))?;
        let pt = file
            .fixed_points
            .points
            .iter()
            .find(|p| p.id == point)
            .ok_or_else(|| format!("unknown point {point}"))?;
        let derived = |m: &BTreeMap<String, BTreeMap<String, u64>>, v: f64, what: &str| {
            m.get(scenario)
                .and_then(|row| row.get(&format!("{v}")))
                .copied()
                .ok_or_else(|| format!("no derived {what} for {scenario} at {v}"))
        };
        Ok(RunSpec {
            schema_version: file.schema_version,
            scenario: sc.clone(),
            point: pt.clone(),
            seed,
            poll_secs: file.time.poll_secs,
            ext_job_secs: derived(
                &file
                    .fixed_points
                    .axes
                    .external_wait_ratio
                    .derived_ext_job_secs,
                pt.external_wait_ratio,
                "ext_job_secs",
            )?,
            arrival_interval_secs: derived(
                &file.fixed_points.axes.load.derived_arrival_interval_secs,
                pt.load,
                "arrival_interval_secs",
            )?,
            caps: file.observation_caps.clone(),
            grading: None,
        })
    }

    /// ADR D6 の測定 ID。
    pub fn measurement_id(&self) -> String {
        format!(
            "dagbench-{}-{}-{}-{}-{SYSTEM}",
            self.schema_version, self.scenario.id, self.point.id, self.seed
        )
    }

    pub fn campaign_ids(&self) -> Vec<String> {
        (0..self.point.concurrent_campaigns)
            .map(|i| format!("c{i}"))
            .collect()
    }

    /// campaign i の投入時刻（論理秒）。
    pub fn arrival(&self, i: usize) -> u64 {
        self.arrival_interval_secs * i as u64
    }

    /// ext-sim 上の論理 job ID（campaign をまたいで一意）。
    pub fn logical_job(campaign: &str, ext: &External) -> String {
        format!("{campaign}/{}", ext.logical_job_id)
    }

    /// node の位相レベル（依存の最長経路の長さ）。
    pub fn levels(&self) -> BTreeMap<String, usize> {
        let mut lv: BTreeMap<String, usize> = BTreeMap::new();
        // scenarios.json の node は依存より後に並ぶ（定義順 = 位相順）。
        for n in &self.scenario.nodes {
            let l = n
                .deps
                .iter()
                .filter_map(|d| lv.get(d).map(|x| x + 1))
                .max()
                .unwrap_or(0);
            lv.insert(n.id.clone(), l);
        }
        lv
    }

    /// stage 変換が足してしまう順序制約: node `b` の stage より前の stage にある node `a` は、stage の順序で `b` より
    /// 先に終わる必要があるが、DAG 上で `a` が `b` の（推移的な）祖先でなければ、それは scenario に無い余計な依存
    /// （`(a, b)` を返す）。空なら stage 変換は依存を強めていない。
    pub fn stage_extra_precedence(&self) -> Vec<(String, String)> {
        let lv = self.levels();
        let mut anc: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
        // 定義順 = 位相順。
        for n in &self.scenario.nodes {
            let mut set = std::collections::BTreeSet::new();
            for d in &n.deps {
                set.insert(d.as_str());
                if let Some(a) = anc.get(d.as_str()) {
                    set.extend(a.iter().copied());
                }
            }
            anc.insert(n.id.as_str(), set);
        }
        let mut extra = vec![];
        for b in &self.scenario.nodes {
            for a in &self.scenario.nodes {
                if lv[&a.id] < lv[&b.id] && !anc[b.id.as_str()].contains(a.id.as_str()) {
                    extra.push((a.id.clone(), b.id.clone()));
                }
            }
        }
        extra
    }

    /// WorkUnit の check: 判定付き node は独立 grader、それ以外は従来の `true`（科学的成功の根拠にしない node）。
    fn unit_check(&self, campaign: &str, node: &str) -> String {
        match &self.grading {
            Some(g) if g.node == node => g.check_cmd(campaign),
            _ => "true".into(),
        }
    }

    /// planner run が書く v3 計画。node → leaf WorkUnit。既存の計画検証（1 stage の unit 上限・stage 内の依存は
    /// 1 本まで）に合わせ、位相レベルごとに 1 stage とし、依存辺は stage の順序で表す（stage 内の depends_on は
    /// 作らない）。k2-mini / k2-wide は各レベルの node が前のレベルの node 全部に依存するので、DAG と同じ制約になる。
    pub fn plan_json(&self, campaign: &str) -> String {
        let lv = self.levels();
        let depth = lv.values().max().copied().unwrap_or(0);
        let stages: Vec<Value> = (0..=depth)
            .map(|l| json!({"key": format!("l{l}"), "kind": "implement", "title": format!("DAG level {l}")}))
            .collect();
        let units: Vec<Value> = self
            .scenario
            .nodes
            .iter()
            .map(|n| {
                json!({
                    "key": n.id,
                    "stage": format!("l{}", lv.get(&n.id).copied().unwrap_or(0)),
                    "kind": "implement",
                    "title": format!("{campaign} {}", n.id),
                    "objective": format!("Run dagbench node {} of campaign {campaign} ({} work)", n.id, n.kind),
                    "depends_on": [],
                    "done_when": [format!("{} is done", n.id)],
                    "checks": [{"cmd": self.unit_check(campaign, &n.id), "expect_exit": 0}],
                })
            })
            .collect();
        json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": "dagbench scenario DAG mapped one node to one work unit, one stage per topological level",
            "stages": stages,
            "units": units,
        })
        .to_string()
    }
}

pub fn load_scenarios(path: &Path) -> Result<ScenarioFile, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

// ---- 論理時計（離散事象） ----

#[derive(Default)]
struct ClockState {
    now: u64,
    stopped: bool,
    /// sleeper id → 期限。
    sleepers: BTreeMap<u64, u64>,
    next_id: u64,
    /// 何かが起きるたびに増える（静止の判定に使う）。
    generation: u64,
    worker_sleepers: usize,
    poll_sleepers: usize,
    polls_active: usize,
}

#[derive(Default)]
pub struct LogicalClock {
    st: Mutex<ClockState>,
    cv: Condvar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SleeperKind {
    Worker,
    Poll,
}

impl LogicalClock {
    fn lock(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn now(&self) -> u64 {
        self.lock().now
    }

    fn bump(&self) {
        self.lock().generation += 1;
    }

    /// `deadline` まで（論理時計で）待つ。呼び出し元の thread を塞ぐ。
    fn sleep_until(&self, deadline: u64, kind: SleeperKind) {
        let mut st = self.lock();
        if st.stopped || st.now >= deadline {
            return;
        }
        st.next_id += 1;
        let id = st.next_id;
        st.sleepers.insert(id, deadline);
        st.generation += 1;
        match kind {
            SleeperKind::Worker => st.worker_sleepers += 1,
            SleeperKind::Poll => st.poll_sleepers += 1,
        }
        while !st.stopped && st.now < deadline {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.sleepers.remove(&id);
        st.generation += 1;
        match kind {
            SleeperKind::Worker => st.worker_sleepers -= 1,
            SleeperKind::Poll => st.poll_sleepers -= 1,
        }
    }

    fn advance_to(&self, t: u64) {
        let mut st = self.lock();
        st.now = st.now.max(t);
        st.generation += 1;
        self.cv.notify_all();
    }
}

// ---- worker stub ledger（制御側から独立した、台本 worker の記録） ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerEntry {
    pub seq: u64,
    pub logical_secs: u64,
    pub event: String,
    pub dataset: String,
    pub campaign: String,
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

struct WorkerLedger {
    file: File,
    seq: u64,
}

impl WorkerLedger {
    fn append(&mut self, now: u64, event: &str, campaign: &str, node: &str, detail: Option<Value>) {
        self.seq += 1;
        let e = WorkerEntry {
            seq: self.seq,
            logical_secs: now,
            event: event.into(),
            dataset: DATASET.into(),
            campaign: campaign.into(),
            node: node.into(),
            detail,
        };
        if let Ok(mut line) = serde_json::to_string(&e) {
            line.push('\n');
            let _ = self.file.write_all(line.as_bytes());
            let _ = self.file.sync_data();
        }
    }
}

pub fn read_worker_ledger(path: &Path) -> std::io::Result<Vec<WorkerEntry>> {
    let raw = std::fs::read_to_string(path)?;
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(std::io::Error::other))
        .collect()
}

// ---- 共有の状態（台本 worker・poller・driver） ----

struct Shared {
    spec: RunSpec,
    clock: LogicalClock,
    ext: Mutex<Box<dyn ExtBackend + Send>>,
    ledger: Mutex<WorkerLedger>,
    /// root task id → campaign id。
    campaigns: Mutex<HashMap<TaskId, String>>,
    /// qstat の job id → (論理 job ID, 期限の論理秒)。
    jobs: Mutex<HashMap<String, (String, u64)>>,
    /// 通知主導の受入: true の間、poller は完了状態を見ずに失敗を返す（試行回数だけ数える）。
    poll_disabled: std::sync::atomic::AtomicBool,
    poll_attempts: std::sync::atomic::AtomicU64,
    /// 判定付き node の試行回数（campaign/node → 書いた回数）。
    attempts: Mutex<HashMap<String, u32>>,
    exhausted: std::sync::atomic::AtomicBool,
    budget_exhausted: bool,
    verify_failed: std::sync::atomic::AtomicBool,
    validated: Mutex<std::collections::HashSet<String>>,
}

impl Shared {
    fn record(&self, event: &str, campaign: &str, node: &str, detail: Option<Value>) {
        let now = self.clock.now();
        self.ledger
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(now, event, campaign, node, detail);
    }

    fn ext(&self) -> std::sync::MutexGuard<'_, Box<dyn ExtBackend + Send>> {
        self.ext.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn work(self: &Arc<Self>, secs: u64) -> impl std::future::Future<Output = ()> + use<> {
        let me = Arc::clone(self);
        async move {
            let deadline = me.clock.now() + secs;
            let _ = tokio::task::spawn_blocking(move || {
                me.clock.sleep_until(deadline, SleeperKind::Worker)
            })
            .await;
        }
    }
}

/// 台本の stub worker（LLM 0）。planner run は scenario の計画を書き、WorkUnit run は論理時計で `work_secs`
/// 働き、外部 node は ext-sim に投入して wait で終わる。continuation run（前置きに cluster job の結果）は
/// `post_wait_work_secs` 働いて結果を消費する。review 等その他の run はすぐ done。
struct StubWorker {
    shared: Arc<Shared>,
}

fn done(summary: String) -> RunOutcome {
    RunOutcome {
        terminal: Terminal::Done {
            summary,
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    }
}

#[async_trait]
impl WorkerAdapter for StubWorker {
    fn id(&self) -> &str {
        "dagbench-stub"
    }

    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let sh = &self.shared;
        sh.clock.bump();
        let campaign = sh
            .campaigns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req.task.id)
            .cloned();
        let Some(campaign) = campaign else {
            return Ok(done("not a dagbench campaign".into()));
        };
        if req.context.execution_planner.is_some() {
            std::fs::create_dir_all(&req.artifacts_dir).map_err(AdapterError::Io)?;
            std::fs::write(
                req.artifacts_dir.join("execution-plan.json"),
                sh.spec.plan_json(&campaign),
            )
            .map_err(AdapterError::Io)?;
            sh.record("plan_written", &campaign, "", None);
            sh.clock.bump();
            return Ok(done("plan".into()));
        }
        let Some(key) = req.context.work_unit.as_ref().map(|w| w.key.clone()) else {
            sh.clock.bump();
            return Ok(done("ok".into()));
        };
        let Some(node) = sh.spec.scenario.nodes.iter().find(|n| n.id == key).cloned() else {
            return Ok(done(format!("unknown node {key}")));
        };
        let out = match (
            &node.external,
            req.context
                .continuation
                .as_ref()
                .and_then(|c| c.cluster_jobs.as_ref()),
        ) {
            // continuation: 外部 job の結果を消費して後処理。
            (Some(ext), Some(cj)) => {
                let logical = RunSpec::logical_job(&campaign, ext);
                sh.record(
                    "post_wait_started",
                    &campaign,
                    &key,
                    Some(json!({"state": cj.state, "jobs": cj.jobs})),
                );
                let first = sh
                    .ext()
                    .record_consumed(&logical)
                    .map_err(AdapterError::Io)?;
                sh.record("consumed", &campaign, &key, Some(json!({"first": first})));
                sh.work(ext.post_wait_work_secs).await;
                if sh.clock.lock().stopped {
                    return Err(AdapterError::Io(std::io::Error::other(
                        "controller stopped",
                    )));
                }
                sh.record("execution_finished", &campaign, &key, None);

                sh.record("node_finished", &campaign, &key, None);
                done(format!("{key} collected"))
            }
            (Some(ext), None) => {
                sh.record("node_started", &campaign, &key, None);
                sh.work(node.work_secs).await;
                if sh.clock.lock().stopped {
                    return Err(AdapterError::Io(std::io::Error::other(
                        "controller stopped",
                    )));
                }
                sh.record("execution_finished", &campaign, &key, None);
                let logical = RunSpec::logical_job(&campaign, ext);
                let job_secs = sh.spec.ext_job_secs;
                // 枠が空くまで poll_secs ごとに投入し直す（何も起こしていない拒否だけ）。
                let reply = loop {
                    let r = sh
                        .ext()
                        .submit(&logical, job_secs)
                        .map_err(AdapterError::Io)?;
                    match r {
                        Ok(r) => break Ok(r),
                        Err(SubmitError::NoSlot) => sh.work(sh.spec.poll_secs).await,
                        Err(e) => break Err(e),
                    }
                };
                let physical = match reply {
                    Ok(r) => r.physical_id,
                    // 応答喪失: 受理されたかは冪等キー（論理 job ID）の照会で確かめる（baseline と同じ再照合）。
                    Err(SubmitError::ResponseLost) => {
                        match sh.ext().lookup(&logical).map_err(AdapterError::Io)? {
                            Lookup::Known { physical_id, .. } => {
                                sh.record(
                                "submit_reconciled",
                                &campaign,
                                &key,
                                Some(json!({"logical_job_id": logical, "physical_id": physical_id})),
                            );
                                physical_id
                            }
                            Lookup::Unknown => {
                                return Err(AdapterError::Io(std::io::Error::other(format!(
                                    "submit {logical}: response lost and unknown to the lookup"
                                ))));
                            }
                        }
                    }
                    Err(e) => {
                        return Err(AdapterError::Io(std::io::Error::other(format!(
                            "submit {logical}: {e:?}"
                        ))));
                    }
                };
                let job = physical.trim_start_matches("phys-").to_string();
                // 再起動後の再投入は既存の物理起動に寄るので、期限は ext-sim の物理起動のものを使う。
                let due = sh
                    .ext()
                    .due_secs(&logical)
                    .map_err(AdapterError::Io)?
                    .unwrap_or(sh.clock.now() + job_secs);
                sh.jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(job.clone(), (logical.clone(), due));
                // worker ledger に待機開始を先に確定させる（再起動時に poller の表を復元する元）。
                sh.record(
                    "external_wait_started",
                    &campaign,
                    &key,
                    Some(json!({"logical_job_id": logical, "physical_id": physical, "job": job})),
                );
                sh.ext().wait_started(&logical).map_err(AdapterError::Io)?;
                RunOutcome {
                    terminal: Terminal::Waiting {
                        request: task_core::cluster_job::ClusterJobWaitRequest {
                            cluster: Some(CLUSTER.into()),
                            scheduler: task_core::cluster_job::ClusterScheduler::Pbs,
                            jobs: vec![job],
                            poll_secs: Some(sh.spec.poll_secs),
                            timeout_secs: None,
                            summary: format!("submitted {logical}"),
                        },
                        checkpoint: Some(json!({"remaining": ["collect the job result"]})),
                        usage: None,
                    },
                    exit_code: Some(0),
                }
            }
            (None, _) => {
                let grading = sh.spec.grading.as_ref().filter(|g| g.node == key);
                let attempt = if let Some(g) = grading {
                    let mut m = sh.attempts.lock().unwrap_or_else(|e| e.into_inner());
                    let a = m.entry(format!("{campaign}/{key}")).or_insert(0);
                    if *a >= g.max_attempts {
                        sh.exhausted
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        return Err(AdapterError::Io(std::io::Error::other(
                            "graded attempt limit exhausted",
                        )));
                    }
                    let attempt = *a;
                    *a += 1;
                    sh.record(
                        "attempt_started",
                        &campaign,
                        &key,
                        Some(json!({"attempt": attempt})),
                    );
                    attempt
                } else {
                    0
                };
                sh.record("node_started", &campaign, &key, None);
                sh.work(node.work_secs).await;
                if sh.clock.lock().stopped {
                    return Err(AdapterError::Io(std::io::Error::other(
                        "controller stopped",
                    )));
                }
                if let Some(g) = grading {
                    let dir = g.artifact_dir(&campaign);
                    g.materialize(attempt, &dir).map_err(AdapterError::Io)?;
                    sh.record(
                        "artifact_written",
                        &campaign,
                        &key,
                        Some(json!({"attempt": attempt, "dir": dir})),
                    );
                }
                sh.record("execution_finished", &campaign, &key, None);
                let verdict = grading.map_or(Ok(()), |g| g.grade(&g.artifact_dir(&campaign)));
                match verdict {
                    Ok(()) => {
                        sh.record("node_finished", &campaign, &key, None);
                    }
                    Err(why) => {
                        sh.record(
                            "verify_failed",
                            &campaign,
                            &key,
                            Some(json!({"attempt": attempt, "why": why})),
                        );
                        sh.verify_failed
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        if grading.is_some_and(|g| attempt + 1 >= g.max_attempts) {
                            sh.record("attempt_limit_exhausted", &campaign, &key, None);
                            sh.exhausted
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                }
                done(format!("{key} done"))
            }
        };
        sh.clock.bump();
        Ok(out)
    }
}

/// ext-sim を `qstat -xf` の出力として返す poller（R3）。job が終わる最初の poll 刻みまで論理時計で待つ。
fn ext_poller(shared: Arc<Shared>) -> task_dispatch::dispatcher::ClusterJobPoller {
    Arc::new(move |req: &ClusterJobPollRequest| {
        let sh = &shared;
        if sh.poll_disabled.load(std::sync::atomic::Ordering::SeqCst) {
            sh.poll_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return Err("polling disabled: completion arrives by push notice only".to_string());
        }
        sh.clock.lock().polls_active += 1;
        sh.clock.bump();
        let ids: Vec<String> = req
            .script
            .rsplit("qstat -xf")
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut out = String::new();
        for id in &ids {
            let entry = sh
                .jobs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(id)
                .cloned();
            let Some((logical, due)) = entry else {
                continue;
            };
            sh.clock.sleep_until(due, SleeperKind::Poll);
            // IPC 切断・期限超過は poll の対象を落とす（次の poll で再試行される）。
            let Ok(found) = sh.ext().lookup(&logical) else {
                continue;
            };
            let state = match found {
                Lookup::Known {
                    state: JobState::Completed,
                    ..
                } => "F",
                Lookup::Known { .. } => "R",
                Lookup::Unknown => continue,
            };
            out.push_str(&format!(
                "Job Id: {id}.{CLUSTER}\n    Job_Name = {logical}\n    job_state = {state}\n    queue = dagbench\n"
            ));
            if state == "F" {
                out.push_str("    Exit_status = 0\n");
            }
            out.push('\n');
        }
        let mut st = sh.clock.lock();
        st.polls_active -= 1;
        st.generation += 1;
        drop(st);
        Ok(task_worker::RemoteCommandOutput {
            exit: Some(0),
            stdout: out,
            stderr: String::new(),
        })
    })
}

// ---- driver ----

/// 測定の出力 dir の配置。
pub struct Paths {
    pub db: PathBuf,
    pub workspaces: PathBuf,
    pub ext_ledger: PathBuf,
    pub worker_ledger: PathBuf,
}

impl Paths {
    pub fn under(db: &Path, out: &Path) -> Self {
        Paths {
            db: db.to_path_buf(),
            workspaces: out.join("workspaces"),
            ext_ledger: out.join("ext.jsonl"),
            worker_ledger: out.join("worker.jsonl"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CampaignResult {
    pub campaign: String,
    pub task_id: String,
    pub status: String,
    pub submitted_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriveReport {
    pub measurement_id: String,
    pub dataset: String,
    pub llm_calls: u64,
    pub paid_jobs: u64,
    pub logical_end_secs: u64,
    pub ticks: u64,
    pub campaigns: Vec<CampaignResult>,
}

pub struct CelerisSide {
    shared: Arc<Shared>,
    paths: Paths,
    store: Arc<dyn TaskStore>,
    dispatcher: Dispatcher,
    submitted: Vec<CampaignResult>,
    /// 外部 push の通知窓口（測定用の最小フック。無ければ従来どおり poll）。
    notices: Option<NoticeChannel>,
    stop_after_verify_failure: bool,
}

impl Drop for CelerisSide {
    fn drop(&mut self) {
        self.shared.clock.lock().stopped = true;
        self.shared.clock.cv.notify_all();
    }
}

impl CelerisSide {
    /// Deterministic controller-stop boundary after a failed independent grade is durable.
    pub fn stop_after_verify_failure(&mut self) {
        self.stop_after_verify_failure = true;
    }

    /// `store` は `paths.db` を開いた SQLite（task-api の router と同じ file）。
    pub fn new(spec: RunSpec, paths: Paths, store: Arc<dyn TaskStore>) -> Result<Self, String> {
        Self::with_faults(spec, paths, store, vec![], None)
    }

    /// ext-sim に障害を仕込んで組む（`dagbench::fault` の制御 process が使う）。`paths` の ledger が既にあれば
    /// 制御 process の再起動とみなし、ledger から続きを復元する: 論理時計・worker ledger の seq・投入済みの
    /// campaign（root task id）・待機中の外部 job（poller の表）。DB 側（run・WorkUnit・durable wait）は
    /// SQLite が真実で、前の制御 process が残した `running` の run は既存の孤児回収（R4、
    /// `Dispatcher::set_orphan_takeover`）が lease の失効を待たずに拾う。
    pub fn with_faults(
        spec: RunSpec,
        paths: Paths,
        store: Arc<dyn TaskStore>,
        faults: Vec<FaultSpec>,
        stop_hook: Option<StopHook>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.workspaces).map_err(|e| e.to_string())?;
        let mut ext = ExtSim::open(
            &paths.ext_ledger,
            ExtConfig {
                poll_secs: spec.poll_secs,
                job_slots: spec.scenario.resources.external_job_slots,
                faults,
            },
        )
        .map_err(|e| e.to_string())?;
        if let Some(h) = stop_hook {
            ext.set_stop_hook(h);
        }
        Self::with_backend(spec, paths, store, Box::new(ext))
    }

    /// 外部 stub の操作面を差し込んで組む。`backend` は in-process の [`ExtSim`] でも、別 process の ExtSim を
    /// 持つ IPC 実装（`dagbench::ipc::IpcExt`）でもよい。`paths.ext_ledger` は backend が書く ledger と同じ path
    /// （同一 host の file。再開時に読み直す）。
    pub fn with_backend(
        spec: RunSpec,
        paths: Paths,
        store: Arc<dyn TaskStore>,
        mut ext: Box<dyn ExtBackend + Send>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&paths.workspaces).map_err(|e| e.to_string())?;
        super::ext::drop_torn_tail(&paths.worker_ledger).map_err(|e| e.to_string())?;
        let prior = if paths.worker_ledger.exists() {
            read_worker_ledger(&paths.worker_ledger).map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };
        let ext_entries = read_ledger(&paths.ext_ledger).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.worker_ledger)
            .map_err(|e| e.to_string())?;
        let resume =
            Resume::from_ledgers(&prior, &ext_entries, ext.now().map_err(|e| e.to_string())?)?;
        let clock = LogicalClock::default();
        clock.advance_to(resume.now);
        let shared = Arc::new(Shared {
            spec,
            clock,
            ext: Mutex::new(ext),
            ledger: Mutex::new(WorkerLedger {
                file,
                seq: resume.seq,
            }),
            campaigns: Mutex::new(resume.campaigns),
            jobs: Mutex::new(resume.jobs),
            poll_disabled: Default::default(),
            poll_attempts: Default::default(),
            attempts: Mutex::new(resume.attempts),
            exhausted: std::sync::atomic::AtomicBool::new(resume.exhausted),
            budget_exhausted: resume.budget_exhausted,
            verify_failed: Default::default(),
            validated: Mutex::new(resume.validated),
        });
        let mut dispatcher = build_dispatcher(&shared, &paths, store.clone());
        register_instance(&mut dispatcher, &store)?;
        Ok(CelerisSide {
            shared,
            paths,
            store,
            dispatcher,
            submitted: resume.submitted,
            notices: None,
            stop_after_verify_failure: false,
        })
    }

    /// 通知主導にする: 完了は外部 push の実受信 → [`Dispatcher::apply_cluster_job_notice`]（poll と同じ完了判定・
    /// `finish_cluster_job_wait`・continuation）で届く。`disable_poll` なら poller は常に失敗する
    /// （通知なしでは再開しないことの検証用）。
    pub fn enable_notices(&mut self, channel: NoticeChannel, disable_poll: bool) {
        self.shared
            .poll_disabled
            .store(disable_poll, std::sync::atomic::Ordering::SeqCst);
        self.notices = Some(channel);
    }

    /// 無効化された poller が呼ばれた回数。
    pub fn poll_attempts(&self) -> u64 {
        self.shared
            .poll_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 有界 blocking receive を 1 回行い、届いた通知を hook に渡す。処理した件数（保留を除く）を返す。
    /// 適用と ledger 記録が済んでから ack する。
    pub fn pump_notices(&mut self) -> Result<usize, String> {
        let Some(ch) = self.notices.as_mut() else {
            return Ok(0);
        };
        let io = |e: std::io::Error| e.to_string();
        let mut handled = 0;
        for sn in ch.receive().map_err(io)? {
            let n = sn.notice.clone();
            let ids = |w: &task_core::cluster_job::ClusterJobWait| NoticeIds {
                wait_id: Some(w.wait_id.clone()),
                task_id: Some(w.task_id.to_string()),
                work_unit_id: w.work_unit_id.clone(),
                run_id: Some(w.run_id.clone()),
            };
            let dup = |outcome: &str, ids: NoticeIds| Settled {
                outcome: outcome.into(),
                suppressed: true,
                continuation: false,
                ids,
            };
            // 1. 再配送（処理済みの seq）・同じ論理 job の重複 copy: 適用せず ack だけ返す。
            if ch.ledger().processed(sn.seq) {
                ch.settle(&sn, &dup("redelivered", NoticeIds::default()))
                    .map_err(io)?;
                continue;
            }
            if ch.ledger().logical_consumed(&n.logical_job_id) {
                ch.settle(&sn, &dup("duplicate", NoticeIds::default()))
                    .map_err(io)?;
                continue;
            }
            // 2. 通知の行き先 wait（受信時に解決済みならそれ。閉じた wait も引ける）。
            let job = n.physical_id.trim_start_matches("phys-").to_string();
            let wait = match ch.ledger().resolved_wait(sn.seq) {
                Some(w) => self
                    .store
                    .cluster_job_wait_get(&w)
                    .map_err(|e| e.to_string())?,
                None => self
                    .store
                    .cluster_job_waits_waiting()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|w| w.jobs.contains(&job)),
            };
            let Some(wait) = wait else {
                ch.hold(sn).map_err(io)?;
                continue;
            };
            let wait_ids = ids(&wait);
            ch.ledger_mut()
                .append(
                    "notice_resolved",
                    Some(sn.seq),
                    Some(n.copy),
                    Some(&n.logical_job_id),
                    Some(&n.physical_id),
                    &wait_ids,
                    None,
                )
                .map_err(io)?;
            let out = self
                .dispatcher
                .apply_cluster_job_notice(&ClusterJobNotice {
                    wait_id: wait.wait_id.clone(),
                    job_id: job,
                    notice_id: format!("{}#{}", n.logical_job_id, sn.seq),
                    state: task_core::cluster_job::ClusterJobState::Finished,
                    exit_status: Some(0),
                })
                .map_err(|e| e.to_string())?;
            let settled = match out {
                ClusterJobNoticeOutcome::Held => {
                    ch.hold(sn).map_err(io)?;
                    continue;
                }
                ClusterJobNoticeOutcome::Finished => Settled {
                    outcome: "finished".into(),
                    suppressed: false,
                    continuation: true,
                    ids: wait_ids,
                },
                ClusterJobNoticeOutcome::Applied => Settled {
                    outcome: "applied".into(),
                    suppressed: false,
                    continuation: false,
                    ids: wait_ids,
                },
                ClusterJobNoticeOutcome::Duplicate => dup("duplicate", wait_ids),
                ClusterJobNoticeOutcome::UnknownJob => dup("unknown_job", wait_ids),
            };
            ch.settle(&sn, &settled).map_err(io)?;
            handled += 1;
        }
        Ok(handled)
    }

    /// campaign の root task を通常 POST で作る（R6）。冪等キーは測定 ID と campaign。
    fn submit(&mut self, t: &dyn Transport, i: usize) -> Result<(), String> {
        let spec = &self.shared.spec;
        let campaign = spec.campaign_ids()[i].clone();
        let key = format!("{}-{campaign}", spec.measurement_id());
        let label = idempotency_label(&key);
        let ws = self.paths.workspaces.join("repos").join(&campaign);
        init_git_repo(&ws)?;
        let body = json!({
            "title": format!("dagbench {} {campaign}", spec.scenario.id),
            "objective": format!("Run the dagbench DAG {} as campaign {campaign}", spec.scenario.id),
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}],
            "workspace": ws,
            "execution": "compound",
            "harness": HARNESS,
            "labels": [label],
            "max_retries": 0,
        });
        let r = t.request("POST", "/api/v1/tasks", Some(&body))?;
        if r.status != 201 && r.status != 200 {
            return Err(format!("POST /tasks {}: {}", r.status, r.body));
        }
        let id = r.body["id"]
            .as_str()
            .ok_or("POST /tasks: no id")?
            .to_string();
        let tid: TaskId = id.parse().map_err(|e| format!("task id {id}: {e:?}"))?;
        self.shared
            .campaigns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tid, campaign.clone());
        let now = self.shared.clock.now();
        self.shared.record(
            "campaign_submitted",
            &campaign,
            "",
            Some(json!({"task_id": id})),
        );
        self.submitted.push(CampaignResult {
            campaign,
            task_id: id,
            status: "submitted".into(),
            submitted_at: now,
        });
        Ok(())
    }

    fn statuses(&self) -> Result<Vec<Status>, String> {
        self.submitted
            .iter()
            .map(|c| {
                let id: TaskId = c.task_id.parse().map_err(|e| format!("{e:?}"))?;
                self.store
                    .get(id)
                    .map_err(|e| e.to_string())?
                    .map(|t| t.status)
                    .ok_or_else(|| format!("task {id} vanished"))
            })
            .collect()
    }

    /// 静止（全ての in-flight が論理時計待ちで、tick を回しても何も起きない）か。
    fn quiescent(&self, last_gen: &mut u64, stable: &mut u32) -> bool {
        let st = self.shared.clock.lock();
        let all_sleeping = self.dispatcher.in_flight() == st.worker_sleepers
            && st.polls_active == st.poll_sleepers;
        if all_sleeping && st.generation == *last_gen {
            *stable += 1;
        } else {
            *stable = 0;
        }
        *last_gen = st.generation;
        *stable >= QUIESCENT_TICKS
    }

    /// 全 campaign を投入し、全 root task が終端になるまで tick を回す。投入は scenario の到着間隔に従い
    /// 論理時計の上で行う。観測上限（論理秒・壁時計）を超えたら Err。
    pub async fn drive(&mut self, t: &dyn Transport) -> Result<DriveReport, String> {
        let n = self.shared.spec.point.concurrent_campaigns;
        let started = Instant::now();
        let cap_wall = Duration::from_secs(self.shared.spec.caps.max_wall_secs);
        let cap_logical = self.shared.spec.caps.max_logical_secs;
        let (mut last_gen, mut stable, mut ticks) = (u64::MAX, 0u32, 0u64);
        loop {
            if self.shared.budget_exhausted {
                return Err("logical budget exhausted".into());
            }
            if self
                .shared
                .exhausted
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("graded attempt limit exhausted".into());
            }
            if self.stop_after_verify_failure
                && self
                    .shared
                    .verify_failed
                    .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("stopped after durable verification failure".into());
            }
            // 到着時刻に達した campaign を投入する。
            while self.submitted.len() < n
                && self.shared.spec.arrival(self.submitted.len()) <= self.shared.clock.now()
            {
                let i = self.submitted.len();
                self.submit(t, i)?;
                self.shared.clock.bump();
            }
            self.dispatcher.tick().map_err(|e| e.to_string())?;
            ticks += 1;
            // 通知主導: 有界 blocking receive（この待ちが tick の間隔を兼ねる）。完了状態の polling ではない。
            let pushed = self.pump_notices()?;
            if pushed > 0 {
                self.dispatcher.tick().map_err(|e| e.to_string())?;
            }
            // Completion is published only after the WorkUnit's independent check passes.
            for c in &self.submitted {
                let id = c.task_id.parse().map_err(|e| format!("{e:?}"))?;
                for unit in self.store.work_units_for(id).map_err(|e| e.to_string())? {
                    if unit.status == task_core::execution_plan::WorkUnitStatus::Done
                        && self
                            .shared
                            .validated
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(format!("{}/{}", c.campaign, unit.key))
                    {
                        self.shared
                            .record("validated_completed", &c.campaign, &unit.key, None);
                    }
                }
            }
            let statuses = self.statuses()?;
            if self.submitted.len() == n && statuses.iter().all(|s| s.is_terminal()) {
                for (c, s) in self.submitted.iter_mut().zip(&statuses) {
                    c.status = format!("{s:?}").to_lowercase();
                }
                break;
            }
            if started.elapsed() > cap_wall {
                return Err(format!(
                    "wall cap {cap_wall:?} exceeded at logical {} ({ticks} ticks, statuses {statuses:?})",
                    self.shared.clock.now()
                ));
            }
            if self.quiescent(&mut last_gen, &mut stable) {
                let next_sleeper = self.shared.clock.lock().sleepers.values().min().copied();
                let next_arrival = (self.submitted.len() < n)
                    .then(|| self.shared.spec.arrival(self.submitted.len()));
                // 通知主導では poll の sleeper が無いので、ext-sim の次の物理起動の期限が次の出来事になる。
                let next_due = self.notices.is_some().then(|| {
                    let now = self.shared.clock.now();
                    self.shared
                        .jobs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .values()
                        .map(|(_, due)| *due)
                        .filter(|d| *d > now)
                        .min()
                });
                match [next_sleeper, next_arrival, next_due.flatten()]
                    .into_iter()
                    .flatten()
                    .min()
                {
                    Some(next) => {
                        if next > cap_logical {
                            self.shared.record("logical_budget_exhausted", "", "", None);
                            return Err(format!("logical cap {cap_logical} exceeded"));
                        }
                        // ext-sim の時計も同じ時刻へ（期限の来た job を完了させる。通知は Celeris 側では使わず poll で知る）。
                        self.shared.ext().advance(next).map_err(|e| e.to_string())?;
                        self.shared.clock.advance_to(next);
                    }
                    // `waiting` の cluster job wait があるのに poll が走っていない: 止まっているのではなく、dispatcher の
                    // 次の poll を待っている。前の制御 process が起こして結果を返さないまま SIGKILL された poll は
                    // `last_polled_at` を残すので、再起動した dispatcher は壁時計で `poll_secs` 経つまで同じ wait を
                    // poll し直さない（`Dispatcher::poll_cluster_job_waits`）。論理時計は進めずに tick を続け、poll
                    // （poller の呼び出し）が来るのを待つ。上限は壁時計の観測上限（`cap_wall`）。
                    None if !self
                        .store
                        .cluster_job_waits_waiting()
                        .map_err(|e| e.to_string())?
                        .is_empty() => {}
                    None => {
                        return Err(format!(
                            "stuck: nothing sleeping and campaigns not terminal at logical {} ({statuses:?}); recent events: {}",
                            self.shared.clock.now(),
                            self.recent_events()
                        ));
                    }
                }
                stable = 0;
            }
            if self.notices.is_none() {
                tokio::time::sleep(TICK_GAP).await;
            }
        }
        Ok(DriveReport {
            measurement_id: self.shared.spec.measurement_id(),
            dataset: DATASET.into(),
            llm_calls: 0,
            paid_jobs: 0,
            logical_end_secs: self.shared.clock.now(),
            ticks,
            campaigns: self.submitted.clone(),
        })
    }

    /// 診断用: 各 campaign の root task の直近の event（store から読むだけ）。
    fn recent_events(&self) -> String {
        let mut out = Vec::new();
        for c in &self.submitted {
            let Ok(id) = c.task_id.parse::<TaskId>() else {
                continue;
            };
            let events = self.store.events_for(id).unwrap_or_default();
            let tail: Vec<String> = events
                .iter()
                .rev()
                .take(6)
                .map(|(_, e)| format!("{e:?}").chars().take(300).collect())
                .collect();
            out.push(format!("{}: {tail:?}", c.campaign));
        }
        out.join("; ")
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}

/// 制御 process の再起動で ledger から復元する driver 側の状態（DB 側の状態は SQLite が持つ）。
struct Resume {
    now: u64,
    seq: u64,
    campaigns: HashMap<TaskId, String>,
    submitted: Vec<CampaignResult>,
    jobs: HashMap<String, (String, u64)>,
    attempts: HashMap<String, u32>,
    exhausted: bool,
    budget_exhausted: bool,
    validated: std::collections::HashSet<String>,
}

impl Resume {
    fn from_ledgers(
        prior: &[WorkerEntry],
        ext: &[super::ext::Entry],
        ext_now: u64,
    ) -> Result<Self, String> {
        let due: HashMap<&str, u64> = ext
            .iter()
            .filter(|e| e.event == "physical_launch")
            .filter_map(|e| {
                Some((
                    e.logical_job_id.as_deref()?,
                    e.detail.as_ref()?["due_secs"].as_u64()?,
                ))
            })
            .collect();
        let mut r = Resume {
            now: ext_now,
            seq: 0,
            campaigns: HashMap::new(),
            submitted: Vec::new(),
            jobs: HashMap::new(),
            attempts: HashMap::new(),
            exhausted: false,
            budget_exhausted: false,
            validated: Default::default(),
        };
        for e in prior {
            r.seq = r.seq.max(e.seq);
            r.now = r.now.max(e.logical_secs);
            let d = e.detail.as_ref();
            match e.event.as_str() {
                "attempt_started" => {
                    let attempt = d
                        .and_then(|d| d["attempt"].as_u64())
                        .ok_or("attempt_started without attempt")?;
                    r.attempts
                        .insert(format!("{}/{}", e.campaign, e.node), attempt as u32 + 1);
                }
                "attempt_limit_exhausted" => r.exhausted = true,
                "logical_budget_exhausted" => r.budget_exhausted = true,
                "validated_completed" => {
                    r.validated.insert(format!("{}/{}", e.campaign, e.node));
                }
                "campaign_submitted" => {
                    let id = d
                        .and_then(|d| d["task_id"].as_str())
                        .ok_or("campaign_submitted without task_id")?;
                    let tid: TaskId = id.parse().map_err(|e| format!("task id {id}: {e:?}"))?;
                    r.campaigns.insert(tid, e.campaign.clone());
                    r.submitted.push(CampaignResult {
                        campaign: e.campaign.clone(),
                        task_id: id.to_string(),
                        status: "submitted".into(),
                        submitted_at: e.logical_secs,
                    });
                }
                "external_wait_started" => {
                    let (Some(job), Some(logical)) = (
                        d.and_then(|d| d["job"].as_str()),
                        d.and_then(|d| d["logical_job_id"].as_str()),
                    ) else {
                        continue;
                    };
                    let due = due.get(logical).copied().unwrap_or(e.logical_secs);
                    r.jobs.insert(job.to_string(), (logical.to_string(), due));
                }
                _ => {}
            }
        }
        Ok(r)
    }
}

/// この制御 process を `daemon_instances` に active として載せ、既存の孤児回収（R4）を有効にする。
/// SIGKILL された前の制御 process の行は pid が死んでいるので「持ち主の居ない run」と判定される。
fn register_instance(d: &mut Dispatcher, store: &Arc<dyn TaskStore>) -> Result<(), String> {
    let pid = std::process::id();
    let now = time::OffsetDateTime::now_utc();
    let instance_id = format!("dagbench-{pid}-{}", now.unix_timestamp_nanos());
    store
        .instance_register(&task_core::DaemonInstance {
            instance_id: instance_id.clone(),
            release: "dagbench".into(),
            pid,
            role: task_core::InstanceRole::Active,
            started_at: now,
            heartbeat_at: now,
            handoff_requested_at: None,
            drained_at: None,
        })
        .map_err(|e| e.to_string())?;
    d.set_orphan_takeover(task_dispatch::orphan::OrphanTakeover {
        instance_id,
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(task_dispatch::orphan::proc_pid_alive),
    });
    Ok(())
}

/// campaign の作業場所を一時 dir の git リポジトリにする。Celeris は git の作業場所でだけ WorkUnit を並列に
/// 走らせる（ADR-0074 D1.2: 非 git は並列 1 に倒す）ので、worker 枠を scenario どおりに使うには git が要る。
/// 利用者の git 設定は読まない（`-c` で名前を渡し、global/system 設定を無効にする）。
fn init_git_repo(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("README.md"), "dagbench campaign workspace\n")
        .map_err(|e| e.to_string())?;
    let git = |args: &[&str]| -> Result<(), String> {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=dagbench",
                "-c",
                "user.email=dagbench@localhost",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .map_err(|e| format!("git {args:?}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            ))
        }
    };
    git(&["init", "-q", "-b", "main"])?;
    git(&["add", "README.md"])?;
    git(&["commit", "-q", "-m", "dagbench: initial"])
}

/// 静止とみなすまでに、何も起きない tick が続く回数（tick の間に拾い残した完了を拾わせる）。
const QUIESCENT_TICKS: u32 = 3;
/// tick の間隔（実時間。時間の進みは論理時計だけが決めるので、短いほど速いだけ）。
const TICK_GAP: Duration = Duration::from_millis(10);

/// 既存の公開口だけで Dispatcher を組む（本体側の口は増やさない）。
fn build_dispatcher(shared: &Arc<Shared>, paths: &Paths, store: Arc<dyn TaskStore>) -> Dispatcher {
    use task_core::llm_route::{CandidateKey, HarnessRule, HarnessSpec, Registry, SourceSpec};
    let slots = shared.spec.scenario.resources.worker_slots;
    let mut execution = ExecutionConfig {
        gate: task_core::GateMode::On,
        parallel: true,
        max_parallel_work_units: slots,
        ..Default::default()
    };
    if let Some(g) = &shared.spec.grading {
        execution.max_replans = g.max_attempts;
        execution.no_progress_limit = g.max_attempts.saturating_add(1);
    }
    execution.limits.tree.enabled = true;
    // 既存の `[tree] max_stages`（既定 5）: 位相レベルごとの stage（k2-* は 7 段）が上限・上限近くの人の承認に
    // 掛からない値にする（測定 config の設定。人の承認を台本で挟まない）。
    execution.limits.tree.max_stages = 16;
    let max_wait = task_core::cluster_job::MAX_WAIT_SECS_CEILING;
    let cluster = ClusterSpec {
        id: CLUSTER.into(),
        host: CLUSTER.into(),
        concurrency: 1,
        sync: task_worker::SyncMode::None,
        delete_on_push: false,
        setup: vec![],
        env: vec![],
        rsync_excludes: vec![],
        worktree: Default::default(),
        auth: "manual".into(),
        forwards: vec![],
        work_dir: None,
        keepalive_secs: 0,
        liveness_probe_secs: 0,
        job_wait: task_core::cluster_job::ClusterJobWaitLimits {
            poll_secs: shared.spec.poll_secs,
            max_wait_secs: max_wait,
        },
    };
    let mut d = Dispatcher::new(
        store,
        DispatchConfig {
            source_error_cooldown: Duration::from_secs(1),
            delivery: Default::default(),
            config_max_concurrency: slots,
            lease_grace: Duration::from_secs(600),
            idle_timeout: Duration::from_secs(600),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(60),
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                paths.workspaces.clone(),
                None,
            ),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: task_dispatch::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::from([(CLUSTER.to_string(), cluster)]),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 0,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: Default::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: paths.workspaces.join(".no-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution,
            eval: Default::default(),
        },
    );
    // 試験の組織と同じ 1 harness・1 source（非 OAuth）。台本 worker だけが走る。
    let mut r = Registry::default();
    r.sources.insert(
        "dagbench".into(),
        SourceSpec {
            kind: "dagbench".into(),
            api_format: HARNESS.into(),
            accounts: vec!["default".into()],
            models: vec!["stub".into()],
            max_runs: None,
            account_max_runs: Default::default(),
            quota: None,
            endpoint: None,
            credential_ref: None,
        },
    );
    r.route_defaults
        .entry(HARNESS.into())
        .or_default()
        .push(CandidateKey {
            source: "dagbench".into(),
            model: "stub".into(),
        });
    r.harnesses.insert(
        HARNESS.into(),
        HarnessSpec {
            kind: "cli".into(),
            api_formats: [HARNESS.into()].into(),
            source_kinds: ["dagbench".into()].into(),
            capabilities: ["tools".to_string(), "repo".to_string()].into(),
            command: None,
            args: vec![],
        },
    );
    r.harness_selection.rules = vec![HarnessRule {
        harness: HARNESS.into(),
        ..Default::default()
    }];
    d.set_llm_route_registry(r);
    let worker: Arc<dyn WorkerAdapter> = Arc::new(StubWorker {
        shared: Arc::clone(shared),
    });
    d.set_harness_adapters(HashMap::from([(HARNESS.into(), worker)]));
    d.set_cluster_liveness_probe(Arc::new(|_: &[String], _: &str| true));
    d.set_cluster_job_poller(ext_poller(Arc::clone(shared)));
    d
}
