//! recovery 比較の opt-in 入口（ADR 2026-10-25-recovery-scheduler-backend D2・D5・実装付記 I7・I12）。
//!
//! - [`BackendChoice`] は harness の外部 backend の選択。既定は [`BackendChoice::ExtSim`]（offline・ネットワークを
//!   呼ばない）。PBS は [`BackendChoice::Pbs`] を明示したときだけで、transport と時計は呼び出し側が注入する
//!   （試験は偽 transport。実 transport `SshPbs` は人の承認後の実接続でだけ渡す）。
//! - recovery の投入経路は [`PbsExt`] → intent 台帳 → `PbsTransport::qsub` の 1 本。試行内 wrapper
//!   `scripts/campaign/celeris-qsub` の環境（[`WRAPPER_ENV`]）があれば起動を拒否する（[`refuse_wrapper_env`]）。
//! - [`run_trial`] が 1 trial（arm × condition）を流す。normal は保存済み Waiting の確認後に完了解放 barrier を外し、
//!   A2 は保存済み Waiting 確認 → 所有 controller の停止 → 停止中に同じ job を observer が観測 → 再開 → 同じ job の
//!   回収 → 独立採点の順に進める。controller（baseline / CelerisSide）は [`Controller`] で差し替える。
//! - 結果は同じ測定 dir の `celeris.dagbench-scheduler-raw/1`（[`RawWriter`]）に原票・結合行として残す。
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    backend::{Arm, CONSUMED_LEDGER, ConsumedRow, PbsExt, RecoverySpec},
    intent::{INTENT_LEDGER, IntentClock},
    raw::{
        AttemptArtifacts, CollectorIdentity, GradeEvidence, RawWriter, StudyPins, TrialBinding,
        TrialInput, config_of,
    },
    transport::{PbsReply, PbsTransport, QstatClass, QstatView, classify_qstat},
};
use crate::dagbench::{
    baseline::{Baseline, BaselineConfig, Outcome},
    celeris_side::{RunSpec, load_scenarios},
    ext::{ExtBackend, ExtConfig, ExtSim, Lookup, Notice, SubmitError, SubmitReply},
    run::merged_scenario,
};

/// 試行内 wrapper（`celeris-qsub`）の環境。recovery の入口はこれを payload に渡さず、設定されていれば拒否する。
pub const WRAPPER_ENV: [&str; 2] = ["CELERIS_SUBMIT_CMD", "CELERIS_SUBMIT_AUDIT"];
/// recovery の DAG（外部 job 1 本）。
pub const RECOVERY_SCENARIO: &str = "rcv-1";
pub const RECOVERY_POINT: &str = "camp-1";
/// barrier と A2 の証跡（trial dir の下、追記専用）。
pub const BARRIERS_FILE: &str = "barriers.jsonl";
pub const BARRIER_SCHEMA: &str = "celeris.recovery-barrier/1";
pub const A2_FILE: &str = "a2.json";
pub const A2_SCHEMA: &str = "celeris.recovery-a2/1";
pub const GRADE_FILE: &str = "grade.json";
pub const GRADE_SCHEMA: &str = "celeris.recovery-grade/1";
/// controller を止めたときに backend が返す誤りの文言。
pub const STOPPED: &str = "a2: owned controller stopped after the saved wait";

/// harness の外部 backend。既定は offline の ExtSim。
pub enum BackendChoice {
    ExtSim(ExtConfig),
    Pbs(Box<PbsChoice>),
}

/// PBS backend の注入物（transport・時計は呼び出し側が決める）。
pub struct PbsChoice {
    pub spec: RecoverySpec,
    pub arm: Arm,
    pub trial_key: String,
    pub epoch: String,
    pub transport: Arc<dyn PbsTransport + Send + Sync>,
    pub clock: Arc<dyn IntentClock + Send + Sync>,
}

impl BackendChoice {
    /// 選んだ backend を `dir` に開く。ExtSim は `dir/ext.jsonl`、PBS は `dir` の intent 台帳・消費台帳・spec pin。
    pub fn open(self, dir: &Path) -> io::Result<Box<dyn ExtBackend + Send>> {
        Ok(match self {
            BackendChoice::ExtSim(cfg) => Box::new(ExtSim::open(&dir.join("ext.jsonl"), cfg)?),
            BackendChoice::Pbs(p) => {
                let p = *p;
                refuse_wrapper_env(|k| std::env::var_os(k)).map_err(io::Error::other)?;
                Box::new(PbsExt::open(
                    dir,
                    p.spec,
                    p.arm,
                    &p.trial_key,
                    &p.epoch,
                    p.transport,
                    p.clock,
                )?)
            }
        })
    }
}

/// `--backend ext-sim|pbs`（無ければ ext-sim）。harness の `DAGBENCH_BACKEND` も同じ値（付記 I14 (1)）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendFlag {
    #[default]
    ExtSim,
    Pbs,
}

impl BackendFlag {
    /// `DAGBENCH_BACKEND` の値（無ければ ext-sim、未知の値は Err）。
    pub fn from_env_value(v: Option<&str>) -> Result<Self, String> {
        match v {
            None | Some("ext-sim") => Ok(BackendFlag::ExtSim),
            Some("pbs") => Ok(BackendFlag::Pbs),
            Some(v) => Err(format!("DAGBENCH_BACKEND ext-sim|pbs (got {v:?})")),
        }
    }

    pub fn from_args(args: &[String]) -> Result<Self, String> {
        match args
            .iter()
            .position(|a| a == "--backend")
            .map(|i| args.get(i + 1).map(String::as_str))
        {
            None => Ok(BackendFlag::ExtSim),
            Some(Some("ext-sim")) => Ok(BackendFlag::ExtSim),
            Some(Some("pbs")) => Ok(BackendFlag::Pbs),
            Some(v) => Err(format!("--backend ext-sim|pbs (got {v:?})")),
        }
    }
}

/// 試行内 wrapper の環境が残っていれば Err（recovery の投入を intent 台帳の 1 本に限る、I7）。
pub fn refuse_wrapper_env(get: impl Fn(&str) -> Option<OsString>) -> Result<(), String> {
    let set: Vec<&str> = WRAPPER_ENV
        .iter()
        .copied()
        .filter(|k| get(k).is_some())
        .collect();
    if set.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "recovery submits only through the intent ledger; unset {} (celeris-qsub is the A arm wrapper)",
            set.join(", ")
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Normal,
    A2,
}

impl Condition {
    /// `DAGBENCH_RECOVERY` の値。
    pub fn parse(v: &str) -> Result<Self, String> {
        match v {
            "normal" => Ok(Condition::Normal),
            "a2" => Ok(Condition::A2),
            _ => Err(format!("DAGBENCH_RECOVERY normal|a2 (got {v:?})")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Condition::Normal => "normal",
            Condition::A2 => "a2",
        }
    }
}

/// spec に無い研究資産と collector の欄。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Study {
    pub pins: StudyPins,
    /// 条件ごとの problem_id は `<problem_id>-<condition>`。
    pub problem_id: String,
    pub trial_no: u32,
}

impl Study {
    pub fn collector(&self, spec: &RecoverySpec, arm: Arm, cond: Condition) -> CollectorIdentity {
        CollectorIdentity {
            campaign_sha256: self.pins.manifest_sha256.clone(),
            config: config_of(arm),
            problem_id: format!("{}-{}", self.problem_id, cond.as_str()),
            rep: spec.rep,
            trial_no: self.trial_no,
        }
    }
}

pub fn recovery_scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/recovery-scenarios.json")
}

/// 両 arm 共通の DAG と制御条件。poll と枠は spec から取り、scenario と食い違えば Err。
pub fn run_spec(spec: &RecoverySpec) -> Result<RunSpec, String> {
    let file = load_scenarios(&recovery_scenarios())?;
    let mut rs = RunSpec::resolve(&file, RECOVERY_SCENARIO, RECOVERY_POINT, spec.seed)?;
    if rs.scenario.resources.worker_slots != spec.limits.worker_slots as usize
        || rs.scenario.resources.external_job_slots != spec.limits.external_slots as usize
    {
        return Err("recovery scenario slots differ from spec.limits".into());
    }
    rs.poll_secs = spec.limits.poll_secs;
    rs.caps.max_logical_secs = spec.limits.trial_deadline_secs;
    rs.caps.max_wall_secs = spec.limits.trial_deadline_secs;
    Ok(rs)
}

/// job の完了解放 barrier と成果物の取得（cluster 側。試験は偽実装）。
pub trait Site: Send + Sync {
    fn release(&self, job_id: &str, reason: &str) -> io::Result<()>;
    /// job の成果物を `dest` に写し、相対 path を返す。
    fn fetch_outputs(&self, job_id: &str, dest: &Path) -> io::Result<Vec<String>>;
}

/// 外部 job を追う controller（baseline / CelerisSide）。
pub trait Controller {
    /// DAG を流す（再開なら保存済み state から）。完走で Ok(true)、gate による停止で Ok(false)。
    fn run(&mut self, ext: GatedExt) -> Result<bool, String>;
    /// 保存済みの状態（memory ではなく state file / SQLite）を読む reader。`physical` を待つ Waiting があれば、その写し。
    fn saved_reader(&self) -> SavedReader;
}

pub type SavedReader = Box<dyn Fn(&str) -> io::Result<Option<Value>> + Send + Sync>;

/// 子 process の controller が保存済み Waiting を見つけたときの合図（`(logical, job_id, snapshot)`）。
/// 親の応答を待つ出来事待ちで、Err なら controller を止める（付記 I14 (3)）。
pub type Announce = Box<dyn Fn(&str, &str, &Value) -> io::Result<()> + Send + Sync>;

/// baseline arm の controller。state は `state.json`（sync/rename）で、再開は同じ file から。
pub struct BaselineController {
    pub spec: RunSpec,
    pub state: PathBuf,
}

impl BaselineController {
    pub fn new(spec: RunSpec, trial_dir: &Path) -> Self {
        Self {
            spec,
            state: trial_dir.join("state.json"),
        }
    }
}

impl Controller for BaselineController {
    fn run(&mut self, mut ext: GatedExt) -> Result<bool, String> {
        let gate = ext.gate();
        let mut b = Baseline::open(
            merged_scenario(&self.spec),
            BaselineConfig {
                poll_secs: self.spec.poll_secs,
                ext_job_secs: self.spec.ext_job_secs,
            },
            &self.state,
        )
        .map_err(|e| format!("{e:?}"))?;
        b.set_logical_cap(self.spec.caps.max_logical_secs);
        match b.run(&mut ext) {
            Ok(Outcome::Complete) => Ok(true),
            Ok(o) => Err(format!("baseline stopped: {o:?}")),
            Err(_) if gate.is_stopped() => Ok(false),
            Err(e) => Err(format!("{e:?}")),
        }
    }

    fn saved_reader(&self) -> SavedReader {
        let path = self.state.clone();
        Box::new(move |physical| {
            let state: Value =
                serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)?;
            Ok(state["nodes"].as_object().and_then(|nodes| {
                nodes.iter().find_map(|(id, n)| {
                    (n["status"] == "waiting" && n["physical_id"] == physical).then(|| {
                        json!({"node": id, "job_id": physical, "state": n, "source": "state.json"})
                    })
                })
            }))
        })
    }
}

/// [`PbsExt`] を incarnation の外から読めるようにした共有 handle。
#[derive(Clone)]
pub struct SharedExt(Arc<Mutex<PbsExt>>);

impl SharedExt {
    fn with<T>(&self, f: impl FnOnce(&mut PbsExt) -> io::Result<T>) -> io::Result<T> {
        let mut g = self
            .0
            .lock()
            .map_err(|_| io::Error::other("pbs backend lock poisoned"))?;
        f(&mut g)
    }
}

struct GateState {
    selected: Option<(String, String)>,
    saved: Option<Value>,
}

/// controller と backend の間の境界。保存済み Waiting を確かめたら normal は barrier を外し、A2 は止める。
pub struct Gate {
    cond: Option<Condition>,
    trial_dir: PathBuf,
    site: Arc<dyn Site>,
    saved_wait: SavedReader,
    on_stop: Box<dyn Fn() + Send + Sync>,
    /// 子 process の境界（`Some` なら cond に代わり、合図して親の応答を待つ。barrier は親が残す）。
    announce: Option<Announce>,
    stopped: AtomicBool,
    state: Mutex<GateState>,
}

impl Gate {
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, GateState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("gate lock poisoned"))
    }

    fn selected(&self) -> io::Result<Option<(String, String)>> {
        Ok(self.lock()?.selected.clone())
    }

    fn select(&self, logical: &str, physical: &str) -> io::Result<()> {
        let mut st = self.lock()?;
        if st.selected.is_none() {
            st.selected = Some((logical.into(), physical.into()));
        }
        Ok(())
    }

    /// 次の backend 呼び出しの前（= controller が直前の状態を保存した後）に走る。
    fn boundary(&self) -> io::Result<()> {
        if self.is_stopped() {
            return Err(io::Error::other(STOPPED));
        }
        if self.cond.is_none() && self.announce.is_none() {
            return Ok(());
        }
        let (selected, done) = {
            let st = self.lock()?;
            (st.selected.clone(), st.saved.is_some())
        };
        let Some((logical, physical)) = selected else {
            return Ok(());
        };
        if done {
            return Ok(());
        }
        let Some(snapshot) = (self.saved_wait)(&physical)? else {
            return Ok(());
        };
        self.lock()?.saved = Some(snapshot.clone());
        if let Some(announce) = &self.announce {
            return announce(&logical, &physical, &snapshot);
        }
        let Some(cond) = self.cond else {
            return Ok(());
        };
        barrier(
            &self.trial_dir,
            "wait_saved",
            json!({"logical_job_id": logical, "job_id": physical, "saved": snapshot}),
        )?;
        match cond {
            Condition::Normal => {
                self.site.release(&physical, "normal: wait saved")?;
                barrier(
                    &self.trial_dir,
                    "released",
                    json!({"job_id": physical, "reason": "normal: wait saved"}),
                )?;
                Ok(())
            }
            Condition::A2 => {
                self.stopped.store(true, Ordering::SeqCst);
                (self.on_stop)();
                Err(io::Error::other(STOPPED))
            }
        }
    }
}

/// controller に渡す backend（[`Gate`] を通した [`SharedExt`]）。
pub struct GatedExt {
    ext: SharedExt,
    gate: Arc<Gate>,
}

impl GatedExt {
    pub fn gate(&self) -> Arc<Gate> {
        self.gate.clone()
    }
}

impl ExtBackend for GatedExt {
    fn submit(
        &mut self,
        logical: &str,
        job_secs: u64,
    ) -> io::Result<Result<SubmitReply, SubmitError>> {
        self.gate.boundary()?;
        let r = self.ext.with(|e| e.submit(logical, job_secs))?;
        if let Ok(reply) = &r {
            self.gate.select(logical, &reply.physical_id)?;
        }
        Ok(r)
    }
    fn wait_started(&mut self, logical: &str) -> io::Result<()> {
        self.ext.with(|e| e.wait_started(logical))
    }
    fn lookup(&mut self, logical: &str) -> io::Result<Lookup> {
        self.gate.boundary()?;
        let r = self.ext.with(|e| e.lookup(logical))?;
        if let Lookup::Known { physical_id, .. } = &r {
            self.gate.select(logical, physical_id)?;
        }
        Ok(r)
    }
    fn advance(&mut self, now_secs: u64) -> io::Result<Vec<Notice>> {
        self.gate.boundary()?;
        self.ext.with(|e| e.advance(now_secs))
    }
    fn record_consumed(&mut self, logical: &str) -> io::Result<bool> {
        self.gate.boundary()?;
        self.ext.with(|e| e.record_consumed(logical))
    }
    fn now(&mut self) -> io::Result<u64> {
        self.ext.with(|e| e.now())
    }
    fn due_secs(&mut self, logical: &str) -> io::Result<Option<u64>> {
        self.ext.with(|e| e.due_secs(logical))
    }
}

fn now_utc() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn barrier(trial_dir: &Path, event: &str, detail: Value) -> io::Result<()> {
    let path = trial_dir.join(BARRIERS_FILE);
    let seq = fs::read_to_string(&path).map_or(0, |t| t.lines().count()) + 1;
    let row = json!({"schema": BARRIER_SCHEMA, "seq": seq, "at_utc": now_utc(), "event": event, "detail": detail});
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(format!("{row}\n").as_bytes())?;
    f.sync_data()
}

fn sha256_file(p: &Path) -> io::Result<String> {
    Ok(Sha256::digest(fs::read(p)?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 1 trial の入力。`dir` は両 arm 共通の測定 dir（intent 台帳・spec pin・raw）。
pub struct TrialRun<'a> {
    pub dir: &'a Path,
    pub spec: &'a RecoverySpec,
    pub study: &'a Study,
    pub arm: Arm,
    pub condition: Condition,
    pub transport: Arc<dyn PbsTransport + Send + Sync>,
    pub clock: Arc<dyn IntentClock + Send + Sync>,
    pub site: Arc<dyn Site>,
}

impl TrialRun<'_> {
    /// controller の state を置く dir（trial ごと）。
    pub fn trial_dir(&self) -> PathBuf {
        self.dir
            .join("trials")
            .join(format!("{}-{}", self.arm.as_str(), self.condition.as_str()))
    }

    pub fn trial_key(&self) -> String {
        self.study
            .collector(self.spec, self.arm, self.condition)
            .trial_key()
    }
}

/// A2 の証跡（`a2.json`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct A2Evidence {
    pub schema: String,
    pub logical_job_id: String,
    pub job_id: String,
    /// 親が保存済み state から読み直した Waiting。
    pub saved_wait: Value,
    pub controller_stopped: bool,
    /// 停止中に observer が取った同じ job の終了原票の数。
    pub observed_finished_while_stopped: usize,
    /// 停止の前後で intent 台帳の行が増えたか（新しい投入は不合格）。
    pub intent_rows_before_stop: usize,
    pub intent_rows_after_resume: usize,
    /// 再開後に消費した job（停止前と同じでなければ不合格）。
    pub consumed_job_ref: String,
}

#[derive(Clone, Debug)]
pub struct TrialReport {
    pub logical_job_id: String,
    pub job_id: String,
    pub binding: TrialBinding,
    pub a2: Option<A2Evidence>,
}

fn rows(path: &Path) -> usize {
    fs::read_to_string(path).map_or(0, |t| t.lines().filter(|l| !l.trim().is_empty()).count())
}

fn consumed_rows(dir: &Path) -> Result<Vec<ConsumedRow>, String> {
    let path = dir.join(CONSUMED_LEDGER);
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(&path)
        .map_err(|e| e.to_string())?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{CONSUMED_LEDGER}: {e}")))
        .collect()
}

/// 独立 grader: attempt の成果物がちょうど 1 つで、その sha256 が spec の期待出力と一致すれば合格。
pub fn grade(spec: &RecoverySpec, att: &AttemptArtifacts, out: &Path) -> io::Result<GradeEvidence> {
    let (output_sha256, reason) = match att.paths.as_slice() {
        [one] => (Some(sha256_file(&att.dir.join(one))?), None),
        [] => (None, Some("no output")),
        _ => (None, Some("more than one output")),
    };
    let passed = output_sha256.as_deref() == Some(spec.payload.expected_output_sha256.as_str());
    let v = json!({"schema": GRADE_SCHEMA, "grader": spec.grader, "output_sha256": output_sha256,
        "expected_output_sha256": spec.payload.expected_output_sha256, "passed": passed, "reason": reason});
    fs::write(
        out,
        serde_json::to_vec_pretty(&v).map_err(io::Error::other)?,
    )?;
    Ok(GradeEvidence {
        file: out.to_path_buf(),
        passed,
    })
}

fn open_ext(t: &TrialRun, epoch: &str) -> Result<SharedExt, String> {
    let ext = PbsExt::open(
        t.dir,
        t.spec.clone(),
        t.arm,
        &t.trial_key(),
        epoch,
        t.transport.clone(),
        t.clock.clone(),
    )
    .map_err(|e| e.to_string())?;
    Ok(SharedExt(Arc::new(Mutex::new(ext))))
}

/// 停止中の observer: 同じ job を poll 間隔で照会し、終了原票（`F`）を取るまで待つ（試行 deadline まで）。
fn observe_stopped(t: &TrialRun, job_id: &str, out: &mut Vec<PbsReply>) -> Result<usize, String> {
    let deadline = t
        .clock
        .now_unix()
        .saturating_add(t.spec.limits.trial_deadline_secs);
    loop {
        let r = t.transport.qstat_full(job_id);
        let class = classify_qstat(&r, job_id);
        out.push(r);
        match class {
            QstatClass::Record { attrs }
                if attrs.get("job_state").map(String::as_str) == Some("F") =>
            {
                return Ok(1);
            }
            QstatClass::Record { .. } => {}
            QstatClass::UnknownJob { detail } => {
                return Err(format!(
                    "observer: {job_id} unknown while stopped (unobserved): {detail}"
                ));
            }
            QstatClass::Failed { reason, detail } => {
                return Err(format!(
                    "observer: qstat {job_id} failed ({reason:?}): {detail}"
                ));
            }
        }
        let next = t.clock.now_unix().saturating_add(t.spec.limits.poll_secs);
        if next > deadline {
            return Err(format!(
                "observer: {job_id} did not finish before the trial deadline"
            ));
        }
        t.clock.sleep_until(next);
    }
}

/// 測定 dir を開く: spec を pin（両 arm 共通）してから新規の raw を作る（出力先は新規のみ）。
pub fn create_measurement(
    dir: &Path,
    spec: &RecoverySpec,
    transport: Arc<dyn PbsTransport + Send + Sync>,
    clock: Arc<dyn IntentClock + Send + Sync>,
    epoch: &str,
) -> io::Result<RawWriter> {
    refuse_wrapper_env(|k| std::env::var_os(k)).map_err(io::Error::other)?;
    PbsExt::open(
        dir,
        spec.clone(),
        Arm::Baseline,
        "pin",
        epoch,
        transport,
        clock,
    )?;
    RawWriter::create(dir, epoch)
}

/// 1 trial を流して raw に結合する。gate・barrier・A2 の順序は module 文書のとおり。
pub fn run_trial(
    t: &TrialRun,
    ctl: &mut dyn Controller,
    raw: &mut RawWriter,
    on_stop: Box<dyn Fn() + Send + Sync>,
) -> Result<TrialReport, String> {
    refuse_wrapper_env(|k| std::env::var_os(k))?;
    let trial_dir = t.trial_dir();
    fs::create_dir_all(&trial_dir).map_err(|e| e.to_string())?;
    let collector = t.study.collector(t.spec, t.arm, t.condition);
    let ext = open_ext(t, &format!("{}-{}-1", t.arm.as_str(), t.condition.as_str()))?;
    let first = ctl_gate(t, ctl, &trial_dir, Some(t.condition), on_stop)?;
    let gated = GatedExt {
        ext: ext.clone(),
        gate: first.clone(),
    };
    let finished = ctl.run(gated)?;
    let (logical, job_id) = first
        .selected()
        .map_err(|e| e.to_string())?
        .ok_or("controller never received a job id")?;
    let mut replies: Vec<PbsReply> = ext
        .with(|e| Ok(e.drain_replies()))
        .map_err(|e| e.to_string())?;
    let saved = first.lock().map_err(|e| e.to_string())?.saved.clone();
    let a2 = match t.condition {
        Condition::Normal => {
            if !finished {
                return Err("normal: controller stopped".into());
            }
            saved.ok_or("normal: the wait was never confirmed saved before completion")?;
            None
        }
        Condition::A2 => {
            if finished || !first.is_stopped() {
                return Err("a2: controller completed before the saved wait stop".into());
            }
            let saved = saved.ok_or("a2: stopped without a saved wait")?;
            let intent_rows_before_stop = rows(&t.dir.join(INTENT_LEDGER));
            barrier(&trial_dir, "controller_stopped", json!({"job_id": job_id}))
                .map_err(|e| e.to_string())?;
            t.site
                .release(&job_id, "a2: controller stopped")
                .map_err(|e| e.to_string())?;
            barrier(
                &trial_dir,
                "released",
                json!({"job_id": job_id, "reason": "a2: controller stopped"}),
            )
            .map_err(|e| e.to_string())?;
            let observed = observe_stopped(t, &job_id, &mut replies)?;
            barrier(
                &trial_dir,
                "observed_finished_while_stopped",
                json!({"job_id": job_id}),
            )
            .map_err(|e| e.to_string())?;
            let ext2 = open_ext(t, &format!("{}-a2-2", t.arm.as_str()))?;
            let resumed = ctl_gate(t, ctl, &trial_dir, None, Box::new(|| {}))?;
            barrier(&trial_dir, "controller_resumed", json!({"job_id": job_id}))
                .map_err(|e| e.to_string())?;
            if !ctl.run(GatedExt {
                ext: ext2.clone(),
                gate: resumed,
            })? {
                return Err("a2: resumed controller did not complete".into());
            }
            replies.extend(
                ext2.with(|e| Ok(e.drain_replies()))
                    .map_err(|e| e.to_string())?,
            );
            let intent_rows_after_resume = rows(&t.dir.join(INTENT_LEDGER));
            let spec = t.spec.intent_spec(t.arm, &t.trial_key(), &logical);
            let want = spec.job_ref(&job_id);
            let consumed = consumed_rows(t.dir)?;
            let consumed_job_ref = consumed
                .iter()
                .find(|r| r.submit_intent_id == spec.submit_intent_id())
                .map(|r| r.job_ref.clone())
                .ok_or("a2: the resumed controller did not consume the job")?;
            if consumed_job_ref != want {
                return Err(format!(
                    "a2: consumed {consumed_job_ref}, not the saved {want}"
                ));
            }
            if intent_rows_after_resume != intent_rows_before_stop {
                return Err("a2: the intent ledger grew across the restart (new submit)".into());
            }
            let ev = A2Evidence {
                schema: A2_SCHEMA.into(),
                logical_job_id: logical.clone(),
                job_id: job_id.clone(),
                saved_wait: saved,
                controller_stopped: true,
                observed_finished_while_stopped: observed,
                intent_rows_before_stop,
                intent_rows_after_resume,
                consumed_job_ref,
            };
            fs::write(
                trial_dir.join(A2_FILE),
                serde_json::to_vec_pretty(&ev).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            Some(ev)
        }
    };
    let identity = raw
        .identity(t.arm, &collector.trial_key(), &logical)
        .map_err(|e| e.to_string())?;
    raw.record_replies(&identity, replies)
        .map_err(|e| e.to_string())?;
    let att_dir = trial_dir.join("attempt-1");
    fs::create_dir_all(&att_dir).map_err(|e| e.to_string())?;
    let paths = t
        .site
        .fetch_outputs(&job_id, &att_dir)
        .map_err(|e| e.to_string())?;
    let att = AttemptArtifacts {
        attempt: 1,
        dir: att_dir,
        paths,
    };
    let g = grade(t.spec, &att, &trial_dir.join(GRADE_FILE)).map_err(|e| e.to_string())?;
    let binding = raw
        .collect_trial(
            QstatView::new(t.transport.as_ref(), &t.spec.cluster),
            &TrialInput {
                arm: t.arm,
                logical_job_id: logical.clone(),
                collector,
                attempts: vec![att],
                grade: Some(g),
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(TrialReport {
        logical_job_id: logical,
        job_id,
        binding,
        a2,
    })
}

fn ctl_gate(
    t: &TrialRun,
    ctl: &dyn Controller,
    trial_dir: &Path,
    cond: Option<Condition>,
    on_stop: Box<dyn Fn() + Send + Sync>,
) -> Result<Arc<Gate>, String> {
    let reader = ctl.saved_reader();
    Ok(Arc::new(Gate {
        cond,
        trial_dir: trial_dir.to_path_buf(),
        site: t.site.clone(),
        saved_wait: reader,
        on_stop,
        announce: None,
        stopped: AtomicBool::new(false),
        state: Mutex::new(GateState {
            selected: None,
            saved: None,
        }),
    }))
}

/// 子 process の controller に渡す backend。`announce` が `Some` なら、job ID を受けた後の次の呼び出しで保存済み
/// Waiting を見つけたときに 1 回だけ合図する（`None` は再開した incarnation：境界なし）。
pub fn child_gated(
    ext: PbsExt,
    trial_dir: &Path,
    site: Arc<dyn Site>,
    reader: SavedReader,
    announce: Option<Announce>,
) -> GatedExt {
    GatedExt {
        ext: SharedExt(Arc::new(Mutex::new(ext))),
        gate: Arc::new(Gate {
            cond: None,
            trial_dir: trial_dir.to_path_buf(),
            site,
            saved_wait: reader,
            on_stop: Box::new(|| {}),
            announce,
            stopped: AtomicBool::new(false),
            state: Mutex::new(GateState {
                selected: None,
                saved: None,
            }),
        }),
    }
}
