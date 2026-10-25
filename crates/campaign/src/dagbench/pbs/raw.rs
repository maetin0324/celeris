//! recovery 比較の実 scheduler raw（`celeris.dagbench-scheduler-raw/1`、dataset=`scheduler_measured`）の writer と
//! validator（ADR 2026-10-25-recovery-scheduler-backend D3・実装付記 I6・I7）。
//!
//! raw は測定 dir（[`super::backend::PbsExt`] と同じ dir。intent 台帳・消費台帳・spec pin が並ぶ）に置く。
//!
//! - `run.json`（[`RunMeta`]）: identity/pins、clock=`wall`、資源、予算（limits）、LLM calls=0、実 job 数。
//! - `scheduler-events.jsonl`（[`SchedulerEvent`]）: PBS への呼び出し 1 回 = 1 行。seq・UTC・observer epoch と
//!   単調時計・identity・根拠参照を持つ追記専用。原票（stdout）は `scheduler/<seq>-<call>.txt` に新規作成し、
//!   取得時刻・投入先・sha256 を行に残す。取得失敗は file/sha256 を null にして理由を残す。
//! - `trials.jsonl`（[`TrialBinding`]）: trial_key へ intent 台帳の job_ref・qstat 原票・attempt 別成果物 hash・grade・
//!   既存 collector（`observe::collect`）の分類と `celeris.observed-jobs/1`（`observed/<seq>.json`）を結合した行。
//!   再収集は新しい行を足し、最新の行がその trial の結合になる。
//! - `submit-audit.jsonl`: 台帳の accepted/reconciled から導いた `celeris.submit-audit/1` の副記録（I7）。
//!
//! 再収集は [`QstatView`]（qstat だけ）を受け取るので、型の上で qsub も `record_consumed` も呼べない。
//! gone・取得失敗・履歴消失・採点入力欠落は null と理由で残し、成功に補完しない。`F`/`Exit_status=0` だけでは
//! `validated_completed` にしない（原票・identity・成果物 hash・期待出力・独立 grade の全てが要る）。
//! 旧 `celeris.dagbench-durable-raw/1` は読まない（[`validate`] は版で拒否し、`metrics::compute` は本 raw を拒否する）。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    backend::{Arm, Limits, Resources, SPEC_PIN, SPEC_PIN_SCHEMA, SpecPin},
    intent::{IntentLedger, IntentState, IntentStatus},
    transport::{CallOutcome, PbsCall, PbsReply, QstatView},
};
use crate::{
    dagbench::{ext::drop_torn_tail, metrics::DURABLE_RAW_SCHEMA},
    grader::{evidence::OBSERVED_SCHEMA, safe_join, sha256_file},
    harness_exec::{SUBMIT_AUDIT_FILE, SUBMIT_AUDIT_SCHEMA, audited_jobs},
    ledger::TrialKey as LedgerKey,
    manifest::ConfigId,
    observe::{self, CollectClass, CollectIssue, Expect, JobRef},
};

pub const SCHEDULER_RAW_SCHEMA: &str = "celeris.dagbench-scheduler-raw/1";
pub const DATASET: &str = "scheduler_measured";
pub const RUN_FILE: &str = "run.json";
pub const EVENTS_FILE: &str = "scheduler-events.jsonl";
pub const EVENT_SCHEMA: &str = "celeris.dagbench-scheduler-event/1";
pub const TRIALS_FILE: &str = "trials.jsonl";
pub const TRIAL_SCHEMA: &str = "celeris.dagbench-scheduler-trial/1";
/// 原票（PBS 呼び出しの stdout）の dir。
pub const SCHEDULER_DIR: &str = "scheduler";
/// collector の副記録（`celeris.observed-jobs/1`）と collector が書く控えの dir。
pub const OBSERVED_DIR: &str = "observed";

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// 研究資産の pin（spec に無いもの）。spec 由来の pin（payload・input・grader・env）は [`RunMeta`] が spec から写す。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyPins {
    pub campaign_id: String,
    /// 正規化 manifest の sha256（collector identity の campaign_sha256）。
    pub manifest_sha256: String,
    pub case_sha256: String,
    /// 採用 source（commit 等）と binary の sha256。
    pub source: String,
    pub binary_sha256: String,
}

/// `run.json`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunMeta {
    pub schema: String,
    pub dataset: String,
    pub measurement_id: String,
    pub spec_sha256: String,
    pub pins: StudyPins,
    pub payload_sha256: String,
    pub input_sha256: String,
    pub expected_output_sha256: String,
    pub grader_sha256: String,
    pub grader_version: String,
    pub env_sha256: String,
    /// 時計は実時間のみ（論理秒を実秒と読み替えない）。
    pub clock: String,
    pub scheduler: String,
    pub cluster: String,
    pub queue: String,
    pub account: String,
    pub resources: Resources,
    pub budgets: Limits,
    pub llm_calls: u64,
    /// intent 台帳の受理・照合済み job の数（物理投入の数）。
    pub real_jobs: u64,
    pub written_utc: String,
}

/// 行の identity（D3）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventIdentity {
    pub measurement_id: String,
    pub arm: String,
    pub trial_key: String,
    pub logical_job_id: String,
    pub submit_intent_id: String,
    pub job_ref: Option<String>,
}

/// 原票の投入先。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub scheduler: String,
    pub cluster: String,
    pub queue: String,
    pub account: String,
}

/// `scheduler-events.jsonl` の 1 行（PBS への呼び出し 1 回）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerEvent {
    pub schema: String,
    pub seq: u64,
    pub at_utc: String,
    pub observer_epoch: String,
    /// observer process 内の単調時計（epoch ごとに非減少）。
    pub mono_ms: u64,
    pub identity: EventIdentity,
    pub target: Target,
    pub call: PbsCall,
    pub argv: Vec<String>,
    pub started_utc: String,
    pub finished_utc: String,
    pub outcome: CallOutcome,
    pub exit: Option<i32>,
    pub stderr: String,
    /// 原票 file（raw dir 相対）。stdout が空なら null。
    pub file: Option<String>,
    pub sha256: Option<String>,
    /// 取得が完了しなかった理由（outcome が Ok 以外）。
    pub missing_reason: Option<String>,
    /// 根拠参照（台帳 file 等）。
    pub evidence: Vec<String>,
}

/// trial の結合が参照する qstat 原票。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QstatRef {
    pub event_seq: u64,
    pub file: Option<String>,
    pub sha256: Option<String>,
    pub fetched_utc: String,
}

/// attempt 別成果物の hash。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactHash {
    pub attempt: u32,
    /// attempt の成果物 dir（validator が読み直す）。
    pub dir: String,
    pub path: String,
    pub sha256: Option<String>,
}

/// 独立 grade の参照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeRef {
    pub file: String,
    pub sha256: Option<String>,
    pub passed: bool,
}

/// 副記録（不変 file）の参照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideRef {
    pub file: String,
    pub sha256: String,
}

/// null にした欄と理由。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Missing {
    pub field: String,
    pub reason: String,
}

/// trial の結果。完遂は `ValidatedCompleted` のみ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialStatus {
    ValidatedCompleted,
    /// 実行終了は観測したが、成果物 hash・期待出力・grade のどれかが揃わない・不合格。
    ExecutedUnvalidated,
    /// 収集できなかった（gone・取得失敗・履歴消失・未終了・不一致）。成功にも未投入にもしない。
    Unobserved,
    /// 投入が照合できない（unknown_submit・unresolved_stop）。未投入にしない。
    Unresolved,
    /// intent が無い、または送信前に確実に拒否された。
    NotSubmitted,
}

/// `trials.jsonl` の 1 行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialBinding {
    pub schema: String,
    pub seq: u64,
    pub at_utc: String,
    pub observer_epoch: String,
    pub mono_ms: u64,
    pub measurement_id: String,
    pub arm: String,
    pub trial_key: String,
    pub logical_job_id: String,
    pub submit_intent_id: String,
    /// `observe::trial_identity`。
    pub collector: Value,
    /// 台帳を再生した intent の状態（無ければ null）。
    pub intent_state: Option<String>,
    pub job_ref: Option<String>,
    pub target: Target,
    pub qstat: Option<QstatRef>,
    pub exit_status: Option<i64>,
    pub artifacts: Vec<ArtifactHash>,
    pub output_matches_expected: bool,
    pub grade: Option<GradeRef>,
    pub collect_class: Option<CollectClass>,
    pub collect_issues: Vec<CollectIssue>,
    pub observed: Option<SideRef>,
    pub missing: Vec<Missing>,
    pub status: TrialStatus,
}

/// collector identity（`observe::trial_identity` の欄）。
#[derive(Clone, Debug)]
pub struct CollectorIdentity {
    pub campaign_sha256: String,
    pub config: ConfigId,
    pub problem_id: String,
    pub rep: u32,
    pub trial_no: u32,
}

impl CollectorIdentity {
    pub fn value(&self) -> Value {
        observe::trial_identity(
            &self.campaign_sha256,
            self.config,
            &self.problem_id,
            self.rep,
            self.trial_no,
        )
    }

    /// 既存 `TrialKey::idempotency_key()` から作る trial_key（D3）。
    pub fn trial_key(&self) -> String {
        LedgerKey {
            manifest_sha256: self.campaign_sha256.clone(),
            problem_id: self.problem_id.clone(),
            arm: self.config,
            trial_no: self.trial_no,
            rep: self.rep,
        }
        .idempotency_key()
    }
}

/// recovery 専用 manifest の arm → Config（baseline→A、celeris→B、D3）。
pub fn config_of(arm: Arm) -> ConfigId {
    match arm {
        Arm::Baseline => ConfigId::A,
        Arm::Celeris => ConfigId::B,
    }
}

fn arm_of(s: &str) -> Option<Arm> {
    match s {
        "baseline" => Some(Arm::Baseline),
        "celeris" => Some(Arm::Celeris),
        _ => None,
    }
}

/// 1 attempt の成果物（`dir` 内の相対 path）。
#[derive(Clone, Debug)]
pub struct AttemptArtifacts {
    pub attempt: u32,
    pub dir: PathBuf,
    pub paths: Vec<String>,
}

/// 独立 grader の出力。
#[derive(Clone, Debug)]
pub struct GradeEvidence {
    pub file: PathBuf,
    pub passed: bool,
}

/// 収集する trial。
#[derive(Clone, Debug)]
pub struct TrialInput {
    pub arm: Arm,
    pub logical_job_id: String,
    pub collector: CollectorIdentity,
    pub attempts: Vec<AttemptArtifacts>,
    /// 採点入力が無ければ None（null と理由で残す）。
    pub grade: Option<GradeEvidence>,
}

fn read_pin(dir: &Path) -> io::Result<SpecPin> {
    let path = dir.join(SPEC_PIN);
    let pin: SpecPin = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    if pin.schema != SPEC_PIN_SCHEMA || pin.spec_sha256 != pin.spec.sha256() {
        return Err(invalid(format!(
            "{}: pin schema or hash is broken",
            path.display()
        )));
    }
    Ok(pin)
}

fn intents(dir: &Path) -> io::Result<BTreeMap<String, IntentStatus>> {
    IntentLedger::open(dir, "raw-reader")?
        .replay()
        .map_err(|e| io::Error::other(e.to_string()))
}

fn job_of(st: &IntentStatus) -> Option<&str> {
    match &st.state {
        IntentState::Accepted { job_id } | IntentState::Reconciled { job_id } => Some(job_id),
        _ => None,
    }
}

fn state_name(st: &IntentState) -> String {
    match st {
        IntentState::Recorded => "intent_recorded",
        IntentState::Sent => "sent",
        IntentState::Accepted { .. } => "accepted",
        IntentState::RejectedBeforeSend => "rejected_before_send",
        IntentState::Unknown { .. } => "unknown_submit",
        IntentState::Reconciled { .. } => "reconciled",
        IntentState::Unresolved { .. } => "unresolved_stop",
    }
    .into()
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (i, line) in BufReader::new(File::open(path)?).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(&line)
                .map_err(|e| invalid(format!("{} line {}: {e}", path.display(), i + 1)))?,
        );
    }
    Ok(out)
}

fn append_line(path: &Path, v: &impl Serialize) -> io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(v).map_err(io::Error::other)?;
    line.push(b'\n');
    f.write_all(&line)?;
    f.sync_data()
}

/// 新規作成だけで書く（既存の原票・副記録を上書きしない）。
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn hash(path: &Path) -> Result<String, String> {
    sha256_file(path)
}

fn missing_names(b: &TrialBinding) -> BTreeSet<&str> {
    b.missing.iter().map(|m| m.field.as_str()).collect()
}

/// 結合行の欄から結果を導く（writer と validator が同じ規則を使う）。
pub fn derive_status(b: &TrialBinding) -> TrialStatus {
    match b.intent_state.as_deref() {
        None | Some("rejected_before_send") => return TrialStatus::NotSubmitted,
        Some("accepted" | "reconciled") => {}
        Some(_) => return TrialStatus::Unresolved,
    }
    if b.job_ref.is_none() {
        return TrialStatus::Unresolved;
    }
    if b.collect_class.is_some() || b.observed.is_none() || b.qstat.is_none() {
        return TrialStatus::Unobserved;
    }
    let artifacts_ok = !b.artifacts.is_empty() && b.artifacts.iter().all(|a| a.sha256.is_some());
    let graded = b
        .grade
        .as_ref()
        .is_some_and(|g| g.sha256.is_some() && g.passed);
    if b.exit_status == Some(0) && artifacts_ok && b.output_matches_expected && graded {
        TrialStatus::ValidatedCompleted
    } else {
        TrialStatus::ExecutedUnvalidated
    }
}

/// raw の writer。`create` は新規の出力先だけ、`reopen` は再収集（`run.json` 済み）だけを受ける。
pub struct RawWriter {
    dir: PathBuf,
    epoch: String,
    started: Instant,
    pin: SpecPin,
}

impl RawWriter {
    /// 新しい raw を始める。測定 dir には spec pin が要り、raw の file（run.json・events・trials）がまだ無いこと。
    pub fn create(dir: &Path, epoch: &str) -> io::Result<Self> {
        let pin = read_pin(dir)?;
        for f in [RUN_FILE, EVENTS_FILE, TRIALS_FILE] {
            if dir.join(f).exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{}: raw output must be new", dir.join(f).display()),
                ));
            }
        }
        Ok(Self::new(dir, epoch, pin))
    }

    /// 再収集のために開く。`run.json` が本 raw の版であること（旧 raw を読み替えない）。
    pub fn reopen(dir: &Path, epoch: &str) -> io::Result<Self> {
        let pin = read_pin(dir)?;
        let run = read_run(dir).map_err(invalid)?;
        if run.spec_sha256 != pin.spec_sha256 {
            return Err(invalid("run.json spec_sha256 differs from the spec pin"));
        }
        drop_torn_tail(&dir.join(EVENTS_FILE))?;
        drop_torn_tail(&dir.join(TRIALS_FILE))?;
        Ok(Self::new(dir, epoch, pin))
    }

    fn new(dir: &Path, epoch: &str, pin: SpecPin) -> Self {
        Self {
            dir: dir.to_path_buf(),
            epoch: epoch.into(),
            started: Instant::now(),
            pin,
        }
    }

    fn mono_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn next_seq(path: &Path) -> io::Result<u64> {
        drop_torn_tail(path)?;
        let rows: Vec<Value> = read_jsonl(path)?;
        Ok(rows.last().and_then(|r| r["seq"].as_u64()).unwrap_or(0) + 1)
    }

    fn target(&self) -> Target {
        let s = &self.pin.spec;
        Target {
            scheduler: s.scheduler.clone(),
            cluster: s.cluster.clone(),
            queue: s.queue.clone(),
            account: s.account.clone(),
        }
    }

    /// trial の identity（job_ref は台帳から埋める）。
    pub fn identity(&self, arm: Arm, trial_key: &str, logical: &str) -> io::Result<EventIdentity> {
        let spec = self.pin.spec.intent_spec(arm, trial_key, logical);
        let id = spec.submit_intent_id();
        let job_ref = intents(&self.dir)?
            .get(&id)
            .and_then(|st| job_of(st).map(|j| spec.job_ref(j)));
        Ok(EventIdentity {
            measurement_id: self.pin.spec.measurement_id.clone(),
            arm: arm.as_str().into(),
            trial_key: trial_key.into(),
            logical_job_id: logical.into(),
            submit_intent_id: id,
            job_ref,
        })
    }

    /// PBS 呼び出しの原票を追記する（backend が `drain_replies` で渡すもの・収集の qstat）。
    pub fn record_replies(
        &mut self,
        identity: &EventIdentity,
        replies: Vec<PbsReply>,
    ) -> io::Result<Vec<SchedulerEvent>> {
        let path = self.dir.join(EVENTS_FILE);
        let mut out = Vec::new();
        for r in replies {
            let seq = Self::next_seq(&path)?;
            let call = match r.call {
                PbsCall::Qsub => "qsub",
                PbsCall::QstatFull => "qstat",
                PbsCall::FindByMarker => "find",
            };
            let (file, sha256) = if r.stdout.is_empty() {
                (None, None)
            } else {
                let rel = format!("{SCHEDULER_DIR}/{seq:06}-{call}.txt");
                let abs = self.dir.join(&rel);
                write_new(&abs, r.stdout.as_bytes())?;
                (Some(rel), Some(hash(&abs).map_err(io::Error::other)?))
            };
            let missing_reason = (r.outcome != CallOutcome::Ok).then(|| {
                format!(
                    "{:?} exit={:?} stderr={:?}",
                    r.outcome,
                    r.exit,
                    r.stderr.trim()
                )
            });
            let ev = SchedulerEvent {
                schema: EVENT_SCHEMA.into(),
                seq,
                at_utc: now_utc(),
                observer_epoch: self.epoch.clone(),
                mono_ms: self.mono_ms(),
                identity: identity.clone(),
                target: self.target(),
                call: r.call,
                argv: r.argv,
                started_utc: r.started_utc,
                finished_utc: r.finished_utc,
                outcome: r.outcome,
                exit: r.exit,
                stderr: r.stderr,
                file,
                sha256,
                missing_reason,
                evidence: vec![super::intent::INTENT_LEDGER.into()],
            };
            append_line(&path, &ev)?;
            out.push(ev);
        }
        Ok(out)
    }

    /// 台帳の accepted/reconciled から `celeris.submit-audit/1` の副記録を導いて足す（既にある job は足さない）。
    pub fn write_submit_audit(&self) -> io::Result<()> {
        let path = self.dir.join(SUBMIT_AUDIT_FILE);
        let text = fs::read_to_string(&path).unwrap_or_default();
        for (id, st) in intents(&self.dir)? {
            let Some(job) = job_of(&st) else { continue };
            let key = &st.spec.identity.trial_key;
            let job_ref = st.spec.job_ref(job);
            if audited_jobs(&text, key).contains(&job_ref) {
                continue;
            }
            append_line(
                &path,
                &json!({
                    "schema": SUBMIT_AUDIT_SCHEMA,
                    "trial_key": key,
                    "scheduler": "pbs",
                    "cluster": st.spec.cluster,
                    "job_id": job,
                    "submit_intent_id": id,
                    "source": super::intent::INTENT_LEDGER,
                }),
            )?;
        }
        Ok(())
    }

    /// trial を収集して結合行を追記する。qstat は `view` だけで行う（qsub・消費を呼べない）。何度呼んでも原票と
    /// 結合行が追記で増えるだけ。
    pub fn collect_trial(
        &mut self,
        view: QstatView<'_>,
        input: &TrialInput,
    ) -> io::Result<TrialBinding> {
        let trial_key = input.collector.trial_key();
        let identity = self.identity(input.arm, &trial_key, &input.logical_job_id)?;
        let spec = self
            .pin
            .spec
            .intent_spec(input.arm, &trial_key, &input.logical_job_id);
        let statuses = intents(&self.dir)?;
        let st = statuses.get(&identity.submit_intent_id);
        let seq = Self::next_seq(&self.dir.join(TRIALS_FILE))?;
        let mut missing = Vec::new();
        let mut miss = |field: &str, reason: String| {
            missing.push(Missing {
                field: field.into(),
                reason,
            })
        };
        let workload_attempt = spec.identity.workload_attempt;

        // attempt 別成果物の hash（欠けは null と理由）。
        let mut artifacts = Vec::new();
        for a in &input.attempts {
            for p in &a.paths {
                let sha = safe_join(&a.dir, p).and_then(|f| hash(&f));
                if let Err(e) = &sha {
                    miss(&format!("artifacts.{}.{p}", a.attempt), e.clone());
                }
                artifacts.push(ArtifactHash {
                    attempt: a.attempt,
                    dir: a.dir.display().to_string(),
                    path: p.clone(),
                    sha256: sha.ok(),
                });
            }
        }
        if artifacts.is_empty() {
            miss("artifacts", "no attempt artifacts were given".into());
        }
        let output_matches_expected = artifacts.iter().any(|a| {
            a.attempt == workload_attempt
                && a.sha256.as_deref()
                    == Some(self.pin.spec.payload.expected_output_sha256.as_str())
        });

        let grade = match &input.grade {
            Some(g) => {
                let sha = hash(&g.file);
                if let Err(e) = &sha {
                    miss("grade.sha256", e.clone());
                }
                Some(GradeRef {
                    file: g.file.display().to_string(),
                    sha256: sha.ok(),
                    passed: g.passed,
                })
            }
            None => {
                miss("grade", "grader input or output is missing".into());
                None
            }
        };

        let (mut qstat, mut exit_status, mut observed) = (None, None, None);
        let (mut collect_class, mut collect_issues) = (None, Vec::new());
        match st.and_then(job_of) {
            None => {
                let why = match st {
                    None => "no submit intent in the ledger (never submitted)".to_string(),
                    Some(st) => format!(
                        "intent is {} (submission not reconciled; not treated as not submitted)",
                        state_name(&st.state)
                    ),
                };
                // 照合できない投入を not_submitted（model_failure）へ写さない（I4）。
                collect_class = Some(match st.map(|s| &s.state) {
                    None | Some(IntentState::RejectedBeforeSend) => CollectClass::NotSubmitted,
                    Some(_) => CollectClass::Unobserved,
                });
                miss("job_ref", why);
                miss("qstat", "no job to query".into());
                miss("observed", "no job to collect".into());
            }
            Some(job_id) => {
                let job = JobRef {
                    scheduler: "pbs".into(),
                    cluster: spec.cluster.clone(),
                    job_id: job_id.into(),
                };
                let expect = Expect {
                    scheduler: "pbs".into(),
                    cluster: spec.cluster.clone(),
                    conditions: [
                        ("queue", &spec.queue),
                        ("select", &spec.select),
                        ("place", &spec.place),
                        ("walltime", &spec.walltime),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), json!(v)))
                    .collect(),
                    trial_key: Some(trial_key.clone()),
                    require_marker: true,
                };
                let attempt = input
                    .attempts
                    .iter()
                    .find(|a| a.attempt == workload_attempt);
                let (art_dir, raw_paths) = match attempt {
                    Some(a) => (a.dir.clone(), a.paths.clone()),
                    None => (self.dir.join("no-attempt-artifacts"), Vec::new()),
                };
                let copies = self.dir.join(OBSERVED_DIR).join(format!("c{seq:06}"));
                let collected =
                    observe::collect(&view, &[job], &expect, &art_dir, &raw_paths, &copies);
                let events = self.record_replies(&identity, view.into_replies())?;
                if let Some(ev) = events.iter().rev().find(|e| e.call == PbsCall::QstatFull) {
                    qstat = Some(QstatRef {
                        event_seq: ev.seq,
                        file: ev.file.clone(),
                        sha256: ev.sha256.clone(),
                        fetched_utc: ev.finished_utc.clone(),
                    });
                    if let Some(r) = &ev.missing_reason {
                        miss("qstat.file", r.clone());
                    }
                } else {
                    miss("qstat", "the collector made no scheduler query".into());
                }
                match collected {
                    Ok(mut doc) => {
                        doc["trial"] = input.collector.value();
                        exit_status = doc["jobs"][0]["exit_status"].as_i64();
                        let rel = format!("{OBSERVED_DIR}/{seq:06}.json");
                        let abs = self.dir.join(&rel);
                        write_new(&abs, doc.to_string().as_bytes())?;
                        observed = Some(SideRef {
                            file: rel,
                            sha256: hash(&abs).map_err(io::Error::other)?,
                        });
                    }
                    Err(issues) => {
                        collect_class = issues.first().map(|i| i.class);
                        let detail = issues
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ");
                        miss("observed", detail);
                        miss("exit_status", "not collected".into());
                        collect_issues = issues;
                    }
                }
            }
        }

        let mut b = TrialBinding {
            schema: TRIAL_SCHEMA.into(),
            seq,
            at_utc: now_utc(),
            observer_epoch: self.epoch.clone(),
            mono_ms: self.mono_ms(),
            measurement_id: identity.measurement_id.clone(),
            arm: identity.arm.clone(),
            trial_key,
            logical_job_id: input.logical_job_id.clone(),
            submit_intent_id: identity.submit_intent_id.clone(),
            collector: input.collector.value(),
            intent_state: st.map(|s| state_name(&s.state)),
            job_ref: identity.job_ref.clone(),
            target: self.target(),
            qstat,
            exit_status,
            artifacts,
            output_matches_expected,
            grade,
            collect_class,
            collect_issues,
            observed,
            missing,
            status: TrialStatus::NotSubmitted,
        };
        b.status = derive_status(&b);
        self.write_submit_audit()?;
        append_line(&self.dir.join(TRIALS_FILE), &b)?;
        Ok(b)
    }

    /// `run.json` を新規に書く（1 回だけ）。実 job 数は台帳から数え、LLM calls は 0。
    pub fn write_run(&self, pins: StudyPins) -> io::Result<RunMeta> {
        let s = &self.pin.spec;
        let jobs: BTreeSet<String> = intents(&self.dir)?
            .values()
            .filter_map(|st| job_of(st).map(|j| st.spec.job_ref(j)))
            .collect();
        let run = RunMeta {
            schema: SCHEDULER_RAW_SCHEMA.into(),
            dataset: DATASET.into(),
            measurement_id: s.measurement_id.clone(),
            spec_sha256: self.pin.spec_sha256.clone(),
            pins,
            payload_sha256: s.payload.script_sha256.clone(),
            input_sha256: s.payload.input_sha256.clone(),
            expected_output_sha256: s.payload.expected_output_sha256.clone(),
            grader_sha256: s.grader.binary_sha256.clone(),
            grader_version: s.grader.version.clone(),
            env_sha256: s.env.env_sha256.clone(),
            clock: "wall".into(),
            scheduler: s.scheduler.clone(),
            cluster: s.cluster.clone(),
            queue: s.queue.clone(),
            account: s.account.clone(),
            resources: s.resources.clone(),
            budgets: s.limits.clone(),
            llm_calls: 0,
            real_jobs: jobs.len() as u64,
            written_utc: now_utc(),
        };
        write_new(
            &self.dir.join(RUN_FILE),
            &serde_json::to_vec_pretty(&run).map_err(io::Error::other)?,
        )?;
        Ok(run)
    }
}

/// `run.json` を本 raw の版として読む。旧 durable raw・他の版は拒否する。
pub fn read_run(dir: &Path) -> Result<RunMeta, String> {
    let text = fs::read_to_string(dir.join(RUN_FILE)).map_err(|e| format!("run.json: {e}"))?;
    let schema = crate::dagbench::metrics::raw_schema_of(&text)?;
    if schema == DURABLE_RAW_SCHEMA {
        return Err(format!(
            "{DURABLE_RAW_SCHEMA} is a logical-clock durable raw; it is not read as {SCHEDULER_RAW_SCHEMA}"
        ));
    }
    if schema != SCHEDULER_RAW_SCHEMA {
        return Err(format!(
            "unknown raw schema {schema} (expected {SCHEDULER_RAW_SCHEMA})"
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("run.json: {e}"))
}

/// 検証の結果。`trials` は trial_key ごとの最新の結合行。
#[derive(Clone, Debug)]
pub struct RawReport {
    pub run: RunMeta,
    pub events: usize,
    pub trials: BTreeMap<String, TrialBinding>,
}

fn check_file(dir: &Path, rel: &str, want: &str, what: &str) -> Result<(), String> {
    let got = hash(&dir.join(rel)).map_err(|e| format!("{what} {rel} is missing: {e}"))?;
    if got != want {
        return Err(format!(
            "{what} {rel} sha256 {got} differs from the recorded {want} (altered)"
        ));
    }
    Ok(())
}

/// raw 全体を検証する。pin・台帳・原票 hash・成果物 hash・副記録・結果の導出のどれかが食い違えば Err。
pub fn validate(dir: &Path) -> Result<RawReport, String> {
    let run = read_run(dir)?;
    let pin = read_pin(dir).map_err(|e| e.to_string())?;
    let s = &pin.spec;
    if run.dataset != DATASET || run.clock != "wall" {
        return Err(format!(
            "run.json dataset/clock must be {DATASET}/wall, got {}/{}",
            run.dataset, run.clock
        ));
    }
    if run.llm_calls != 0 {
        return Err(format!(
            "run.json records {} LLM calls (must be 0)",
            run.llm_calls
        ));
    }
    if run.spec_sha256 != pin.spec_sha256
        || run.measurement_id != s.measurement_id
        || run.resources != s.resources
        || run.budgets != s.limits
        || run.payload_sha256 != s.payload.script_sha256
        || run.input_sha256 != s.payload.input_sha256
        || run.expected_output_sha256 != s.payload.expected_output_sha256
        || run.grader_sha256 != s.grader.binary_sha256
        || run.env_sha256 != s.env.env_sha256
        || (
            run.cluster.as_str(),
            run.queue.as_str(),
            run.account.as_str(),
        ) != (s.cluster.as_str(), s.queue.as_str(), s.account.as_str())
    {
        return Err("run.json identity/pins differ from the spec pin".into());
    }
    let statuses = intents(dir).map_err(|e| e.to_string())?;
    let jobs: BTreeSet<String> = statuses
        .values()
        .filter_map(|st| job_of(st).map(|j| st.spec.job_ref(j)))
        .collect();
    if run.real_jobs != jobs.len() as u64 {
        return Err(format!(
            "run.json real_jobs {} differs from the {} jobs in the intent ledger",
            run.real_jobs,
            jobs.len()
        ));
    }

    let events: Vec<SchedulerEvent> =
        read_jsonl(&dir.join(EVENTS_FILE)).map_err(|e| e.to_string())?;
    let mut mono: BTreeMap<&str, u64> = BTreeMap::new();
    for (i, e) in events.iter().enumerate() {
        let at = format!("{EVENTS_FILE} seq {}", e.seq);
        if e.schema != EVENT_SCHEMA || e.seq != i as u64 + 1 {
            return Err(format!("{at}: schema or seq is broken (append-only)"));
        }
        OffsetDateTime::parse(&e.at_utc, &Rfc3339).map_err(|x| format!("{at}: at_utc: {x}"))?;
        let last = mono.entry(e.observer_epoch.as_str()).or_insert(0);
        if e.mono_ms < *last {
            return Err(format!(
                "{at}: monotonic clock went back in epoch {}",
                e.observer_epoch
            ));
        }
        *last = e.mono_ms;
        if e.identity.measurement_id != s.measurement_id {
            return Err(format!("{at}: belongs to another measurement"));
        }
        match (&e.file, &e.sha256) {
            (Some(f), Some(h)) => check_file(dir, f, h, "scheduler record")?,
            (None, None) => {}
            _ => return Err(format!("{at}: file and sha256 must both be set or null")),
        }
        if e.outcome != CallOutcome::Ok && e.missing_reason.is_none() {
            return Err(format!("{at}: failed fetch has no missing_reason"));
        }
    }

    let rows: Vec<TrialBinding> = read_jsonl(&dir.join(TRIALS_FILE)).map_err(|e| e.to_string())?;
    let audit = fs::read_to_string(dir.join(SUBMIT_AUDIT_FILE)).unwrap_or_default();
    let mut trials = BTreeMap::new();
    for (i, b) in rows.iter().enumerate() {
        check_binding(dir, &pin, &statuses, &events, &audit, b, i as u64 + 1)?;
        trials.insert(b.trial_key.clone(), b.clone());
    }
    Ok(RawReport {
        run,
        events: events.len(),
        trials,
    })
}

fn check_binding(
    dir: &Path,
    pin: &SpecPin,
    statuses: &BTreeMap<String, IntentStatus>,
    events: &[SchedulerEvent],
    audit: &str,
    b: &TrialBinding,
    want_seq: u64,
) -> Result<(), String> {
    let at = format!("{TRIALS_FILE} seq {} ({})", b.seq, b.trial_key);
    let s = &pin.spec;
    if b.schema != TRIAL_SCHEMA || b.seq != want_seq {
        return Err(format!("{at}: schema or seq is broken (append-only)"));
    }
    if b.measurement_id != s.measurement_id {
        return Err(format!("{at}: belongs to another measurement"));
    }
    let arm = arm_of(&b.arm).ok_or_else(|| format!("{at}: unknown arm {}", b.arm))?;
    // collector identity → trial_key（既存 TrialKey）と arm → Config の対応。
    let c = &b.collector;
    let config = match c["config"].as_str() {
        Some("A") => ConfigId::A,
        Some("B") => ConfigId::B,
        other => return Err(format!("{at}: collector config {other:?} is not A/B")),
    };
    if config != config_of(arm) {
        return Err(format!(
            "{at}: arm {} is not collector config {config:?}",
            b.arm
        ));
    }
    let ident = CollectorIdentity {
        campaign_sha256: c["campaign_sha256"].as_str().unwrap_or_default().into(),
        config,
        problem_id: c["problem_id"].as_str().unwrap_or_default().into(),
        rep: c["rep"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0),
        trial_no: c["trial_no"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0),
    };
    if ident.value() != *c || ident.trial_key() != b.trial_key {
        return Err(format!(
            "{at}: trial_key does not match the collector identity {c} (job ID/trial mismatch)"
        ));
    }
    let spec = s.intent_spec(arm, &b.trial_key, &b.logical_job_id);
    if spec.submit_intent_id() != b.submit_intent_id {
        return Err(format!(
            "{at}: submit_intent_id is not the one derived from the identity"
        ));
    }
    // 台帳と job_ref の対応。
    let st = statuses.get(&b.submit_intent_id);
    if st.map(|st| state_name(&st.state)) != b.intent_state {
        return Err(format!(
            "{at}: intent_state {:?} differs from the ledger",
            b.intent_state
        ));
    }
    let ledger_ref = st.and_then(|st| job_of(st).map(|j| spec.job_ref(j)));
    if ledger_ref != b.job_ref {
        return Err(format!(
            "{at}: job_ref {:?} differs from the ledger {ledger_ref:?} (job ID/trial mismatch)",
            b.job_ref
        ));
    }
    // 副記録 celeris.submit-audit/1。
    let audited = audited_jobs(audit, &b.trial_key);
    let want: Vec<String> = ledger_ref.iter().cloned().collect();
    if audited != want {
        return Err(format!(
            "{at}: submit audit {audited:?} differs from the ledger {want:?}"
        ));
    }
    // qstat 原票。
    if let Some(q) = &b.qstat {
        let ev = events
            .iter()
            .find(|e| e.seq == q.event_seq)
            .ok_or_else(|| format!("{at}: qstat event {} is missing", q.event_seq))?;
        if ev.call != PbsCall::QstatFull
            || ev.identity.trial_key != b.trial_key
            || ev.identity.submit_intent_id != b.submit_intent_id
            || ev.identity.job_ref != b.job_ref
            || ev.file != q.file
            || ev.sha256 != q.sha256
            || ev.finished_utc != q.fetched_utc
        {
            return Err(format!("{at}: qstat reference differs from its event"));
        }
    }
    // collector 副記録 celeris.observed-jobs/1。
    match (&b.observed, b.collect_class) {
        (Some(_), Some(_)) | (None, None) if b.job_ref.is_some() => {
            return Err(format!(
                "{at}: collect_class and the observed record must be exclusive"
            ));
        }
        _ => {}
    }
    if let Some(o) = &b.observed {
        check_file(dir, &o.file, &o.sha256, "observed record")?;
        let doc: Value =
            serde_json::from_slice(&fs::read(dir.join(&o.file)).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{at}: observed: {e}"))?;
        let job = &doc["jobs"][0];
        let job_id = b.job_ref.as_deref().and_then(|r| r.rsplit(':').next());
        let qsha = b.qstat.as_ref().and_then(|q| q.sha256.as_deref());
        if doc["schema"] != OBSERVED_SCHEMA
            || doc["trial"] != *c
            || doc["jobs"].as_array().map(Vec::len) != Some(1)
            || job["job_id"].as_str() != job_id
            || job["cluster"].as_str() != Some(s.cluster.as_str())
            || job["scheduler_record_sha256"].as_str() != qsha
            || job["exit_status"].as_i64() != b.exit_status
        {
            return Err(format!(
                "{at}: collector record {} does not match this trial's job, qstat record or exit status",
                o.file
            ));
        }
        let wa = spec.identity.workload_attempt;
        for r in job["raw"].as_array().into_iter().flatten() {
            let hit = b
                .artifacts
                .iter()
                .find(|a| a.attempt == wa && Some(a.path.as_str()) == r["path"].as_str());
            if hit.and_then(|a| a.sha256.as_deref()) != r["sha256"].as_str() {
                return Err(format!(
                    "{at}: collector raw hash for {} differs from the attempt artifact",
                    r["path"]
                ));
            }
        }
    }
    // 成果物・grade の hash（原本を読み直す）。
    let names = missing_names(b);
    for a in &b.artifacts {
        let Some(want) = &a.sha256 else {
            if !names.contains(format!("artifacts.{}.{}", a.attempt, a.path).as_str()) {
                return Err(format!(
                    "{at}: artifact {} is null without a reason",
                    a.path
                ));
            }
            continue;
        };
        let f = safe_join(Path::new(&a.dir), &a.path).map_err(|e| format!("{at}: {e}"))?;
        let got = hash(&f).map_err(|e| format!("{at}: artifact {} is missing: {e}", a.path))?;
        if &got != want {
            return Err(format!(
                "{at}: artifact {} sha256 differs (altered)",
                a.path
            ));
        }
    }
    let wa = spec.identity.workload_attempt;
    let matches = b.artifacts.iter().any(|a| {
        a.attempt == wa && a.sha256.as_deref() == Some(s.payload.expected_output_sha256.as_str())
    });
    if matches != b.output_matches_expected {
        return Err(format!(
            "{at}: output_matches_expected is not derived from the artifacts"
        ));
    }
    match &b.grade {
        Some(g) => match &g.sha256 {
            Some(h) => {
                let got = hash(Path::new(&g.file)).map_err(|e| format!("{at}: grade: {e}"))?;
                if &got != h {
                    return Err(format!("{at}: grade {} sha256 differs (altered)", g.file));
                }
            }
            None if !names.contains("grade.sha256") => {
                return Err(format!("{at}: grade hash is null without a reason"));
            }
            None => {}
        },
        None if !names.contains("grade") => {
            return Err(format!("{at}: grade is null without a reason"));
        }
        None => {}
    }
    for (field, null) in [
        ("job_ref", b.job_ref.is_none()),
        ("observed", b.observed.is_none()),
        ("qstat", b.qstat.is_none() && b.job_ref.is_none()),
    ] {
        if null && !names.contains(field) {
            return Err(format!("{at}: {field} is null without a reason"));
        }
    }
    let derived = derive_status(b);
    if derived != b.status {
        return Err(format!(
            "{at}: status {:?} is not supported by the evidence (derived {derived:?})",
            b.status
        ));
    }
    Ok(())
}
