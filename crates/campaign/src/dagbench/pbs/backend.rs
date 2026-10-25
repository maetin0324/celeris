//! recovery 比較の [`ExtBackend`] の PBS 実装と両 arm 共通の spec（ADR 2026-10-25-recovery-scheduler-backend
//! D2・D4・実装付記 I4・I5）。
//!
//! - [`RecoverySpec`] は 1 つの JSON（`deny_unknown_fields`、既定値なし）から読み、資源（select/place/walltime/
//!   queue/account）・終了条件（poll/deadline）・attempt 上限 1 を arm を引数に取らない関数で作る。arm で変わるのは
//!   identity（arm・trial_key・submit intent）だけ。spec の sha256 は測定 dir の pin（[`SPEC_PIN`]）と照合し、
//!   両 arm が同じ spec でなければ開けない。
//! - [`PbsExt`] は baseline / CelerisSide が ExtSim と同じ trait 経路で使う差し替え先。submit は intent 台帳
//!   （[`IntentLedger`]）経由で qsub を 1 回だけ送り、lookup は `qstat -x -f` の原票を intent と照合して状態を返す。
//!   unknown_submit は poll 間隔ごとに試行 deadline まで marker で照合し、照合できなければ
//!   [`SubmitError::Unresolved`] / [`Lookup::Unresolved`] で止める。[`Lookup::Unknown`] は台帳に intent が無い
//!   （一度も投入していない）場合だけ。履歴消失・照会失敗は `io::Error`（未観測。成功・未投入に補完しない）。
//! - 時計は wall clock（[`IntentClock`] で注入）。`now`/`advance` の秒は測定開始（pin の `start_unix`）からの経過で、
//!   `advance(t)` は wall clock が開始 + t に届くまで待ってから poll する。完了を論理時計で推定しない（`due_secs` は None）。
//! - `record_consumed` は [`CONSUMED_LEDGER`]（追記専用、lock 内で読み直し）に `(job_ref, workload_attempt)` を
//!   1 回だけ書き、2 回目は `false`。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    intent::{
        IntentClock, IntentLedger, IntentSpec, IntentState, IntentStatus, SubmitIdentity,
        SubmitOutcome,
    },
    transport::{PbsReply, PbsTransport, QstatClass, QsubRequest, classify_qstat},
};
use crate::dagbench::ext::{
    ExtBackend, JobState, Lookup, Notice, SubmitError, SubmitReply, drop_torn_tail,
};

/// spec の pin（測定 dir に 1 つ。両 arm が同じ hash を求める）。
pub const SPEC_PIN: &str = "recovery-spec.json";
pub const SPEC_PIN_SCHEMA: &str = "celeris.recovery-spec-pin/1";
/// 消費の台帳（追記専用）。
pub const CONSUMED_LEDGER: &str = "consumed.jsonl";
pub const CONSUMED_LOCK: &str = "consumed.lock";
pub const CONSUMED_ROW_SCHEMA: &str = "celeris.recovery-consumed/1";
/// workload attempt の上限（D2）。spec の `limits.workload_attempts` はこの値でなければ読み込みエラー。
pub const WORKLOAD_ATTEMPTS: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    Baseline,
    Celeris,
}

impl Arm {
    pub fn as_str(self) -> &'static str {
        match self {
            Arm::Baseline => "baseline",
            Arm::Celeris => "celeris",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub select: String,
    pub place: String,
    pub walltime_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    /// cluster 上の job script の絶対 path。
    pub script: String,
    pub script_sha256: String,
    pub input_sha256: String,
    pub expected_output_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grader {
    pub binary_sha256: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Env {
    pub modules: Vec<String>,
    pub env_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub external_slots: u32,
    pub worker_slots: u32,
    pub poll_secs: u64,
    pub trial_deadline_secs: u64,
    pub ssh_timeout_secs: u64,
    pub max_jobs: u32,
    pub workload_attempts: u32,
}

/// 両 arm 共通の資源・終了条件・pins（I5）。欄の欠落・未知の欄は読み込みエラー。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySpec {
    pub measurement_id: String,
    pub scheduler: String,
    pub cluster: String,
    /// PBS 上の投入者（`Job_Owner` の user 部）。marker 検索もこの user の job に限る。
    pub owner: String,
    pub queue: String,
    pub account: String,
    pub resources: Resources,
    pub rerunnable: bool,
    pub payload: Payload,
    pub grader: Grader,
    pub env: Env,
    pub limits: Limits,
    pub conditions: Vec<String>,
    pub seed: u64,
    pub rep: u32,
}

/// 終了条件（arm に依らない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Termination {
    pub poll_secs: u64,
    pub trial_deadline_secs: u64,
    pub walltime_secs: u64,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn is_sha256(v: &str) -> bool {
    v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

impl RecoverySpec {
    /// JSON から読み、値を検査する。
    pub fn parse(json: &str) -> io::Result<Self> {
        let spec: Self = serde_json::from_str(json).map_err(|e| invalid(e.to_string()))?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> io::Result<()> {
        let fail = |m: String| Err(invalid(format!("recovery spec: {m}")));
        if self.scheduler != "pbs" {
            return fail(format!("scheduler {:?} is not \"pbs\"", self.scheduler));
        }
        for (k, v) in [
            ("measurement_id", &self.measurement_id),
            ("cluster", &self.cluster),
            ("owner", &self.owner),
            ("queue", &self.queue),
            ("account", &self.account),
            ("resources.select", &self.resources.select),
            ("resources.place", &self.resources.place),
            ("grader.version", &self.grader.version),
        ] {
            if v.is_empty() {
                return fail(format!("{k} is empty"));
            }
        }
        if self.rerunnable {
            return fail("rerunnable must be false (D5)".into());
        }
        for (k, v) in [
            ("payload.script_sha256", &self.payload.script_sha256),
            ("payload.input_sha256", &self.payload.input_sha256),
            (
                "payload.expected_output_sha256",
                &self.payload.expected_output_sha256,
            ),
            ("grader.binary_sha256", &self.grader.binary_sha256),
            ("env.env_sha256", &self.env.env_sha256),
        ] {
            if !is_sha256(v) {
                return fail(format!("{k} is not a full sha256"));
            }
        }
        let l = &self.limits;
        if l.workload_attempts != WORKLOAD_ATTEMPTS {
            return fail(format!(
                "limits.workload_attempts is {}, must be {WORKLOAD_ATTEMPTS}",
                l.workload_attempts
            ));
        }
        for (k, v) in [
            ("resources.walltime_secs", self.resources.walltime_secs),
            ("limits.external_slots", u64::from(l.external_slots)),
            ("limits.worker_slots", u64::from(l.worker_slots)),
            ("limits.poll_secs", l.poll_secs),
            ("limits.trial_deadline_secs", l.trial_deadline_secs),
            ("limits.ssh_timeout_secs", l.ssh_timeout_secs),
            ("limits.max_jobs", u64::from(l.max_jobs)),
        ] {
            if v == 0 {
                return fail(format!("{k} must be > 0"));
            }
        }
        if self.conditions.is_empty() {
            return fail("conditions is empty".into());
        }
        // 引数の文字種は qsub の要求と同じ検査で確かめる（起動前に落とす）。
        self.qsub_request(Arm::Baseline, "trial", "job")
            .argv()
            .map(|_| ())
            .map_err(|e| invalid(format!("recovery spec: {e}")))
    }

    /// spec の sha256（struct の欄順で直列化した JSON）。両 arm で同じ値になる。
    pub fn sha256(&self) -> String {
        sha256_hex(&serde_json::to_vec(self).unwrap_or_default())
    }

    /// `HH:MM:SS`。
    pub fn walltime(&self) -> String {
        let s = self.resources.walltime_secs;
        format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }

    /// job script と入力の sha256 をまとめた payload hash。
    pub fn payload_sha256(&self) -> String {
        sha256_hex(
            format!(
                "{{\"input_sha256\":\"{}\",\"script_sha256\":\"{}\"}}",
                self.payload.input_sha256, self.payload.script_sha256
            )
            .as_bytes(),
        )
    }

    /// 終了条件。arm を取らない。
    pub fn termination(&self) -> Termination {
        Termination {
            poll_secs: self.limits.poll_secs,
            trial_deadline_secs: self.limits.trial_deadline_secs,
            walltime_secs: self.resources.walltime_secs,
        }
    }

    /// attempt 上限。arm を取らない。
    pub fn workload_attempts(&self) -> u32 {
        self.limits.workload_attempts
    }

    /// 1 外部 job の identity。node は論理 job ID（scenario の外部 node ごとに一意）。
    pub fn identity(&self, arm: Arm, trial_key: &str, logical: &str) -> SubmitIdentity {
        SubmitIdentity {
            measurement_id: self.measurement_id.clone(),
            arm: arm.as_str().into(),
            trial_key: trial_key.into(),
            logical_job_id: logical.into(),
            node: logical.into(),
            workload_attempt: WORKLOAD_ATTEMPTS,
        }
    }

    /// intent 台帳に渡す投入条件。資源・終了条件は spec だけから、identity は引数から作る。
    pub fn intent_spec(&self, arm: Arm, trial_key: &str, logical: &str) -> IntentSpec {
        let identity = self.identity(arm, trial_key, logical);
        let intent = identity.submit_intent_id();
        IntentSpec {
            // `-N` は表示用（照合に使わない）。PBS の短い job 名上限に収める。
            job_name: format!("rcv-{}", intent.get(3..14).unwrap_or("x")),
            identity,
            owner: self.owner.clone(),
            cluster: self.cluster.clone(),
            queue: self.queue.clone(),
            account: self.account.clone(),
            select: self.resources.select.clone(),
            place: self.resources.place.clone(),
            walltime: self.walltime(),
            script: self.payload.script.clone(),
            payload_sha256: self.payload_sha256(),
            poll_secs: self.limits.poll_secs,
            trial_deadline_secs: self.limits.trial_deadline_secs,
        }
    }

    pub fn qsub_request(&self, arm: Arm, trial_key: &str, logical: &str) -> QsubRequest {
        self.intent_spec(arm, trial_key, logical).request()
    }
}

/// 測定 dir の spec pin。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecPin {
    pub schema: String,
    pub spec_sha256: String,
    /// 測定開始の wall clock（UTC unix 秒）。`now`/`advance` の秒の起点。
    pub start_unix: u64,
    pub spec: RecoverySpec,
}

/// pin を作るか、既存の pin と spec hash を照合する。作成は hard link で原子的に 1 回だけ。
fn pin_spec(dir: &Path, spec: &RecoverySpec, now: u64) -> io::Result<SpecPin> {
    let path = dir.join(SPEC_PIN);
    if !path.exists() {
        let pin = SpecPin {
            schema: SPEC_PIN_SCHEMA.into(),
            spec_sha256: spec.sha256(),
            start_unix: now,
            spec: spec.clone(),
        };
        let tmp = dir.join(format!(
            ".{SPEC_PIN}.{}.{}.tmp",
            std::process::id(),
            sha256_hex(format!("{:?}", std::thread::current().id()).as_bytes())
                .get(..8)
                .unwrap_or("0")
        ));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(&pin).map_err(io::Error::other)?)?;
            f.sync_all()?;
        }
        let linked = std::fs::hard_link(&tmp, &path);
        std::fs::remove_file(&tmp)?;
        match linked {
            Ok(()) => return Ok(pin),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let pin: SpecPin = serde_json::from_slice(&std::fs::read(&path)?)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    if pin.schema != SPEC_PIN_SCHEMA || pin.spec_sha256 != pin.spec.sha256() {
        return Err(invalid(format!(
            "{}: pin schema or hash is broken",
            path.display()
        )));
    }
    if pin.spec_sha256 != spec.sha256() {
        return Err(invalid(format!(
            "recovery spec sha256 {} differs from the pinned {} (both arms must use the same spec)",
            spec.sha256(),
            pin.spec_sha256
        )));
    }
    Ok(pin)
}

/// 消費台帳の 1 行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumedRow {
    pub schema: String,
    pub seq: u64,
    pub at_utc: String,
    pub submit_intent_id: String,
    pub logical_job_id: String,
    pub job_ref: String,
    pub workload_attempt: u32,
}

fn utc(t: u64) -> String {
    i64::try_from(t)
        .ok()
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
        .and_then(|d| d.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// `job_id` を持つ intent の job id。
fn job_of(st: &IntentStatus) -> Option<&str> {
    match &st.state {
        IntentState::Accepted { job_id } | IntentState::Reconciled { job_id } => Some(job_id),
        _ => None,
    }
}

/// [`ExtBackend`] の PBS 実装（1 arm・1 trial）。両 arm は同じ測定 dir・同じ spec を共有する。
pub struct PbsExt {
    spec: RecoverySpec,
    spec_sha256: String,
    arm: Arm,
    trial_key: String,
    dir: PathBuf,
    ledger: IntentLedger,
    transport: Arc<dyn PbsTransport + Send + Sync>,
    clock: Arc<dyn IntentClock + Send + Sync>,
    start_unix: u64,
    last_poll: Option<u64>,
    /// 完了通知を出した論理 ID（process 内。再起動後は lookup / 再通知で同じ job を拾う）。
    notified: BTreeSet<String>,
    /// 取得した qstat 原票（raw writer が引き取る）。
    replies: Vec<PbsReply>,
}

impl PbsExt {
    /// `dir` に intent 台帳・消費台帳・spec pin を置いて開く。pin が別 spec なら Err。
    pub fn open(
        dir: &Path,
        spec: RecoverySpec,
        arm: Arm,
        trial_key: &str,
        epoch: &str,
        transport: Arc<dyn PbsTransport + Send + Sync>,
        clock: Arc<dyn IntentClock + Send + Sync>,
    ) -> io::Result<Self> {
        spec.validate()?;
        spec.qsub_request(arm, trial_key, "job")
            .argv()
            .map_err(invalid)?;
        std::fs::create_dir_all(dir)?;
        let pin = pin_spec(dir, &spec, clock.now_unix())?;
        Ok(Self {
            spec_sha256: pin.spec_sha256,
            start_unix: pin.start_unix,
            ledger: IntentLedger::open(dir, epoch)?,
            spec,
            arm,
            trial_key: trial_key.into(),
            dir: dir.to_path_buf(),
            transport,
            clock,
            last_poll: None,
            notified: BTreeSet::new(),
            replies: Vec::new(),
        })
    }

    pub fn spec(&self) -> &RecoverySpec {
        &self.spec
    }

    pub fn spec_sha256(&self) -> &str {
        &self.spec_sha256
    }

    pub fn arm(&self) -> Arm {
        self.arm
    }

    pub fn intent_spec(&self, logical: &str) -> IntentSpec {
        self.spec.intent_spec(self.arm, &self.trial_key, logical)
    }

    pub fn submit_intent_id(&self, logical: &str) -> String {
        self.intent_spec(logical).submit_intent_id()
    }

    /// これまでに取得した qstat 原票を引き取る。
    pub fn drain_replies(&mut self) -> Vec<PbsReply> {
        std::mem::take(&mut self.replies)
    }

    fn ledger_err(e: impl std::fmt::Display) -> io::Error {
        io::Error::other(e.to_string())
    }

    fn statuses(&self) -> io::Result<BTreeMap<String, IntentStatus>> {
        self.ledger.replay().map_err(Self::ledger_err)
    }

    /// この arm・trial の intent。
    fn mine(&self, st: &IntentStatus) -> bool {
        let id = &st.spec.identity;
        id.measurement_id == self.spec.measurement_id
            && id.arm == self.arm.as_str()
            && id.trial_key == self.trial_key
    }

    fn unresolved(id: &str, reason: impl std::fmt::Debug, detail: &str) -> (String, String) {
        (id.to_string(), format!("{reason:?}: {detail}"))
    }

    /// unknown_submit を poll 間隔ごとに試行 deadline まで照合する。
    fn settle(
        &self,
        id: &str,
        outcome: SubmitOutcome,
    ) -> io::Result<Result<String, (String, String)>> {
        let outcome = match outcome {
            SubmitOutcome::Pending { .. } => self
                .ledger
                .resolve(id, self.transport.as_ref(), self.clock.as_ref())
                .map_err(Self::ledger_err)?,
            o => o,
        };
        Ok(match outcome {
            SubmitOutcome::Accepted { job_id, .. } | SubmitOutcome::Reconciled { job_id, .. } => {
                Ok(job_id)
            }
            SubmitOutcome::Unresolved { reason, detail } => {
                Err(Self::unresolved(id, reason, &detail))
            }
            SubmitOutcome::Pending {
                next_check_unix, ..
            } => Err(Self::unresolved(
                id,
                "pending",
                &format!("still unknown_submit, next check {}", utc(next_check_unix)),
            )),
        })
    }

    /// 消費済みの job_ref。
    fn consumed(&self) -> io::Result<Vec<ConsumedRow>> {
        let path = self.dir.join(CONSUMED_LEDGER);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for line in BufReader::new(File::open(&path)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            out.push(
                serde_json::from_str(&line)
                    .map_err(|e| invalid(format!("{}: {e}", path.display())))?,
            );
        }
        Ok(out)
    }

    /// 枠を占める intent の数（消費済み・送信前拒否以外。未解決は資源を戻さない、D4.6）。
    fn occupied(&self, statuses: &BTreeMap<String, IntentStatus>) -> io::Result<usize> {
        let consumed: BTreeSet<String> = self.consumed()?.into_iter().map(|r| r.job_ref).collect();
        Ok(statuses
            .values()
            .filter(|st| match job_of(st) {
                Some(j) => !consumed.contains(&st.spec.job_ref(j)),
                None => st.state != IntentState::RejectedBeforeSend,
            })
            .count())
    }

    /// qstat 原票で job の状態を得る。原票が intent と合わなければ `Err(理由)`。
    fn job_state(
        &mut self,
        spec: &IntentSpec,
        job_id: &str,
    ) -> io::Result<Result<JobState, String>> {
        let r = self.transport.qstat_full(job_id);
        let class = classify_qstat(&r, job_id);
        self.replies.push(r);
        let job_ref = spec.job_ref(job_id);
        match class {
            QstatClass::Record { attrs } => {
                if let Err(why) = spec.check_record(job_id, &attrs) {
                    return Ok(Err(format!(
                        "record of {job_ref} does not match the intent: {why}"
                    )));
                }
                Ok(Ok(match attrs.get("job_state").map(String::as_str) {
                    Some("F") => JobState::Completed,
                    _ => JobState::Running,
                }))
            }
            // 履歴消失は未観測。成功・未投入に補完しない。
            QstatClass::UnknownJob { detail } => Err(io::Error::other(format!(
                "qstat: scheduler does not know {job_ref} (history missing, unobserved): {detail}"
            ))),
            QstatClass::Failed { reason, detail } => Err(io::Error::other(format!(
                "qstat {job_ref} failed ({reason:?}): {detail}"
            ))),
        }
    }
}

impl ExtBackend for PbsExt {
    fn submit(
        &mut self,
        logical: &str,
        _job_secs: u64,
    ) -> io::Result<Result<SubmitReply, SubmitError>> {
        let spec = self.intent_spec(logical);
        let id = spec.submit_intent_id();
        let statuses = self.statuses()?;
        let existed = statuses.contains_key(&id);
        if !existed {
            // max_jobs・枠は測定 dir 全体（両 arm）で数える。単一 submit owner が前提（D4.1）。
            if statuses.len() >= self.spec.limits.max_jobs as usize {
                return Err(io::Error::other(format!(
                    "submit {logical}: max_jobs {} reached; not submitting",
                    self.spec.limits.max_jobs
                )));
            }
            if self.occupied(&statuses)? >= self.spec.limits.external_slots as usize {
                return Ok(Err(SubmitError::NoSlot));
            }
        }
        let outcome = self
            .ledger
            .submit(&spec, self.transport.as_ref(), self.clock.as_ref())
            .map_err(Self::ledger_err)?;
        Ok(match self.settle(&id, outcome)? {
            Ok(job_id) => Ok(SubmitReply {
                physical_id: job_id,
                deduped: existed,
            }),
            Err((submit_intent_id, reason)) => Err(SubmitError::Unresolved {
                submit_intent_id,
                reason,
            }),
        })
    }

    fn wait_started(&mut self, _logical: &str) -> io::Result<()> {
        Ok(())
    }

    fn lookup(&mut self, logical: &str) -> io::Result<Lookup> {
        let id = self.submit_intent_id(logical);
        let Some(st) = self.statuses()?.remove(&id) else {
            return Ok(Lookup::Unknown);
        };
        let job_id = match job_of(&st) {
            Some(j) => j.to_string(),
            None => {
                // 送ったか分からない・照合中・停止済み: 台帳から照合を進め、未投入には戻さない。
                let outcome = self
                    .ledger
                    .reconcile(&id, self.transport.as_ref(), self.clock.as_ref())
                    .map_err(Self::ledger_err)?;
                match self.settle(&id, outcome)? {
                    Ok(j) => j,
                    Err((submit_intent_id, reason)) => {
                        return Ok(Lookup::Unresolved {
                            submit_intent_id,
                            reason,
                        });
                    }
                }
            }
        };
        Ok(match self.job_state(&st.spec, &job_id)? {
            Ok(state) => Lookup::Known {
                physical_id: job_id,
                state,
            },
            Err(reason) => Lookup::Unresolved {
                submit_intent_id: id,
                reason,
            },
        })
    }

    fn advance(&mut self, now_secs: u64) -> io::Result<Vec<Notice>> {
        self.clock
            .sleep_until(self.start_unix.saturating_add(now_secs));
        let now = self.clock.now_unix();
        if self
            .last_poll
            .is_some_and(|t| now < t.saturating_add(self.spec.limits.poll_secs))
        {
            return Ok(Vec::new());
        }
        self.last_poll = Some(now);
        let consumed: BTreeSet<String> = self.consumed()?.into_iter().map(|r| r.job_ref).collect();
        let mut out = Vec::new();
        let mine: Vec<IntentStatus> = self
            .statuses()?
            .into_values()
            .filter(|s| self.mine(s))
            .collect();
        for st in mine {
            let logical = st.spec.identity.logical_job_id.clone();
            let Some(job_id) = job_of(&st).map(str::to_string) else {
                continue;
            };
            if self.notified.contains(&logical) || consumed.contains(&st.spec.job_ref(&job_id)) {
                continue;
            }
            match self.job_state(&st.spec, &job_id)? {
                Ok(JobState::Completed) => {
                    self.notified.insert(logical.clone());
                    out.push(Notice {
                        logical_job_id: logical,
                        physical_id: job_id,
                        copy: 0,
                        due_secs: now.saturating_sub(self.start_unix),
                    });
                }
                Ok(JobState::Running) => {}
                Err(why) => {
                    return Err(io::Error::other(format!(
                        "{}: {why}",
                        st.spec.submit_intent_id()
                    )));
                }
            }
        }
        Ok(out)
    }

    fn record_consumed(&mut self, logical: &str) -> io::Result<bool> {
        let id = self.submit_intent_id(logical);
        let st = self
            .statuses()?
            .remove(&id)
            .ok_or_else(|| io::Error::other(format!("consume {logical}: no submit intent")))?;
        let job_ref = job_of(&st).map(|j| st.spec.job_ref(j)).ok_or_else(|| {
            io::Error::other(format!("consume {logical}: {id} has no accepted job"))
        })?;
        let attempt = st.spec.identity.workload_attempt;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(CONSUMED_LOCK))?;
        lock.lock()?;
        let path = self.dir.join(CONSUMED_LEDGER);
        drop_torn_tail(&path)?;
        let rows = self.consumed()?;
        if rows
            .iter()
            .any(|r| r.job_ref == job_ref && r.workload_attempt == attempt)
        {
            return Ok(false);
        }
        let row = ConsumedRow {
            schema: CONSUMED_ROW_SCHEMA.into(),
            seq: rows.last().map_or(1, |r| r.seq + 1),
            at_utc: utc(self.clock.now_unix()),
            submit_intent_id: id,
            logical_job_id: logical.into(),
            job_ref,
            workload_attempt: attempt,
        };
        let mut line = serde_json::to_vec(&row).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(&line)?;
        f.sync_data()?;
        Ok(true)
    }

    fn now(&mut self) -> io::Result<u64> {
        Ok(self.clock.now_unix().saturating_sub(self.start_unix))
    }

    /// 論理時計で完了を推定しない（D1）。
    fn due_secs(&mut self, _logical: &str) -> io::Result<Option<u64>> {
        Ok(None)
    }
}
