//! 別 process の測定専用 controller で 1 trial を流す（ADR 2026-10-25-recovery-scheduler-backend 付記 I14 (3)）。
//!
//! - 子（[`child_trial`]）: [`FilePbs`] を [`RecordingTransport`] 越しに使う [`PbsExt`] で controller を走らせる。
//!   job ID を受けた後の次の backend 呼び出しで保存済み Waiting を見つけたら `PBS_WAIT_SAVED {…}` を stdout に出し、
//!   stdin の 1 行を待つ（出来事待ち）。`CONTINUE` で先へ進む。子の申告は合図だけで証拠にしない。
//! - 親（[`ProcTrial::run`]）: 子を [`Supervisor`] で起こし、合図の後に子と独立の接続で保存を読み直し、intent 台帳と
//!   子の原票（`replies-1.jsonl`）の job ID と突き合わせてから、normal は完了解放 → `CONTINUE`、A2 は starttime を
//!   確かめて所有 `Child` だけを SIGKILL → 停止中に解放と observer の照会（`F` まで）→ 同じ state で再起動 → 回収。
//!   最後に重複投入・重複消費・job 不一致を確かめ、独立 grader で採点して raw に結合する。
//! - 実 ssh・qsub・network・LLM は使わない。待ちは子の行・exit の出来事待ちで、sleep の順序づけや CPU 負荷はない。
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    approval::{self, ApprovalGate},
    backend::{Arm, ConsumedRow, PbsExt, RecoverySpec},
    entry::{
        A2_FILE, A2_SCHEMA, Condition, Controller, GRADE_FILE, Site, Study, child_gated, grade,
    },
    file_pbs::{FileClock, FilePbs},
    intent::{INTENT_LEDGER, IntentClock, IntentLedger, IntentState, SystemClock},
    raw::{AttemptArtifacts, RawWriter, TrialBinding, TrialInput},
    recovery::{ClockKind, RecoveryInput, RecoveryRecord, SavedWaitRef, read_consumed},
    transport::{
        CallOutcome, MarkerQuery, PbsCall, PbsReply, PbsTransport, QstatClass, QstatView,
        QsubRequest, SITE_CALLS_FILE, SshPbs, SshSite, classify_qstat,
    },
};
use crate::{
    dagbench::{
        ext::drop_torn_tail,
        process::{EventLog, Limits, Role, Supervisor},
    },
    observe::Origin,
};

/// 子の入口が読む env（[`ChildConfig`] の JSON）。
pub const CHILD_ENV: &str = "DAGBENCH_PBS_CHILD";
/// 子が保存済み Waiting を見つけたときの合図の行頭。
pub const WAIT_SAVED_LINE: &str = "PBS_WAIT_SAVED";
/// 親が normal で子に送る行。
pub const CONTINUE_LINE: &str = "CONTINUE";
/// 親が読み直した保存済み Waiting（trial dir、新規作成）。
pub const DURABLE_WAIT_FILE: &str = "durable-wait.json";
/// 拒否した trial の理由（trial dir、新規作成。`celeris.recovery-reject/1`）。
pub const REJECT_FILE: &str = "reject.json";
pub const REJECT_SCHEMA: &str = "celeris.recovery-reject/1";
/// 子の試験専用の故障（付記 I14 (5)）。runner が子の env にだけ入れる。
pub const CHILD_FAULT_ENV: &str = "DAGBENCH_PBS_CHILD_FAULT";

/// 測定専用 controller の試験専用の故障。どれも出来事（合図・exit）の順で決まり、sleep・負荷を使わない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildFault {
    /// job ID を受けた直後、Waiting を保存する前に `PBS_WAIT_SAVED` を出す。
    AnnounceBeforeSave,
    /// `CONTINUE` の後、intent を忘れたかのように別の intent（`workload_attempt` + 1）で同じ job を投入し直す。
    DuplicateSubmit,
    /// 完走の後、同じ `(job_ref, workload_attempt)` の消費行を台帳の重複防止を迂回して 2 行目として足す。
    DuplicateConsume,
}

impl ChildFault {
    pub fn parse(v: &str) -> Result<Self, String> {
        match v {
            "announce-before-save" => Ok(Self::AnnounceBeforeSave),
            "duplicate-submit" => Ok(Self::DuplicateSubmit),
            "duplicate-consume" => Ok(Self::DuplicateConsume),
            other => Err(format!(
                "{CHILD_FAULT_ENV}={other} (announce-before-save|duplicate-submit|duplicate-consume)"
            )),
        }
    }
}

/// 子 process の設定（`DAGBENCH_PBS_CHILD`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildConfig {
    /// harness の `Params`（子の入口が controller を組むため。proc は読まない）。
    pub params: Value,
    pub spec_path: PathBuf,
    pub study_path: PathBuf,
    /// PBS への経路（file の偽 cluster / 承認つき ssh）。
    pub access: PbsAccess,
    /// 両 arm 共通の測定 dir（intent 台帳・消費台帳・spec pin）。
    pub dir: PathBuf,
    pub trial_dir: PathBuf,
    pub arm: Arm,
    pub condition: Condition,
    /// 1 = 最初の incarnation（境界で合図する）、2 = 再起動（境界なし）。
    pub incarnation: u64,
}

/// PBS への経路。`File` は I14 の偽 cluster（state dir と偽の時計）、`Ssh` は付記 I15 (1) の承認つき実 cluster
/// （実時計）。どちらも parent と子が同じ値から自分で開き直す（相手の検査結果を信じない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PbsAccess {
    File {
        state: PathBuf,
    },
    Ssh {
        approval: PathBuf,
        /// 試験専用: 起動する ssh（stub）。env 入口からは設定できず、None なら PATH の `ssh`。
        #[serde(default)]
        program: Option<PathBuf>,
        /// 試験専用: 論理時計を state dir の [`FileClock`] にする（stub ssh の試験で実時間を待たない）。
        /// env 入口からは設定できず、None（本番）は実時計。設定すると原票の時計種別は `file_fake`。
        #[serde(default)]
        test_clock: Option<PathBuf>,
    },
}

type SharedTransport = Arc<dyn PbsTransport + Send + Sync>;

impl PbsAccess {
    /// qsub/qstat の transport。`Ssh` は承認を読み直して spec・期限・測定 dir の pin と照合し、[`ApprovalGate`] で包む。
    pub fn transport(&self, spec: &RecoverySpec, dir: &Path) -> Result<SharedTransport, String> {
        match self {
            PbsAccess::File { state } => Ok(Arc::new(FilePbs::open(state).map_err(e2s)?)),
            PbsAccess::Ssh {
                approval, program, ..
            } => {
                let a = approval::load(approval, spec, time::OffsetDateTime::now_utc())?;
                approval::check_pinned(dir, &a)?;
                let mut ssh = SshPbs::new(
                    &spec.cluster,
                    Duration::from_secs(spec.limits.ssh_timeout_secs),
                )?;
                if let Some(p) = program {
                    ssh = ssh.with_program(p);
                }
                Ok(Arc::new(ApprovalGate::new(ssh, dir, &a)))
            }
        }
    }

    /// intent 台帳・controller の時計（`File` は偽の時計、`Ssh` は実時計）。
    pub fn clock(&self) -> Result<Arc<dyn IntentClock + Send + Sync>, String> {
        match self {
            PbsAccess::File { state }
            | PbsAccess::Ssh {
                test_clock: Some(state),
                ..
            } => Ok(Arc::new(FileClock::open(state).map_err(e2s)?)),
            PbsAccess::Ssh { .. } => Ok(Arc::new(SystemClock)),
        }
    }

    pub fn clock_kind(&self) -> ClockKind {
        match self {
            PbsAccess::File { .. }
            | PbsAccess::Ssh {
                test_clock: Some(_),
                ..
            } => ClockKind::FileFake,
            PbsAccess::Ssh { .. } => ClockKind::System,
        }
    }

    /// 解放・成果物の取得。`Ssh` は [`SshSite`]（原票は `trial_dir` の [`SITE_CALLS_FILE`]）。
    pub fn site(&self, spec: &RecoverySpec, trial_dir: &Path) -> Result<Arc<dyn Site>, String> {
        match self {
            PbsAccess::File { state } => Ok(Arc::new(FilePbs::open(state).map_err(e2s)?)),
            PbsAccess::Ssh { program, .. } => {
                let mut site = SshSite::new(
                    &spec.cluster,
                    Duration::from_secs(spec.limits.ssh_timeout_secs),
                    &spec.payload.script,
                    &spec.measurement_id,
                    trial_dir.join(SITE_CALLS_FILE),
                )?;
                if let Some(p) = program {
                    site = site.with_program(p);
                }
                Ok(Arc::new(site))
            }
        }
    }

    /// 論理欄の現在値（`File` は偽の時計の値、`Ssh` は実時計の unix 秒）。
    fn logical_now(&self) -> Option<u64> {
        match self {
            PbsAccess::File { state }
            | PbsAccess::Ssh {
                test_clock: Some(state),
                ..
            } => FileClock::open(state).ok().and_then(|c| c.now().ok()),
            PbsAccess::Ssh { .. } => Some(SystemClock.now_unix()),
        }
    }
}

impl ChildConfig {
    pub fn replies_file(&self) -> PathBuf {
        replies_file(&self.trial_dir, self.incarnation)
    }
}

pub fn replies_file(trial_dir: &Path, incarnation: u64) -> PathBuf {
    trial_dir.join(format!("replies-{incarnation}.jsonl"))
}

/// 返った原票を 1 行ずつ sync で追記する transport（SIGKILL の後も親が読める。controller の memory に置かない）。
pub struct RecordingTransport<T> {
    inner: T,
    out: PathBuf,
}

impl<T: PbsTransport> RecordingTransport<T> {
    pub fn new(inner: T, out: PathBuf) -> Self {
        Self { inner, out }
    }

    fn record(&self, r: PbsReply) -> PbsReply {
        let line = match serde_json::to_string(&r) {
            Ok(l) => l,
            Err(_) => return r,
        };
        let res = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.out)
            .and_then(|mut f| {
                f.write_all(format!("{line}\n").as_bytes())?;
                f.sync_data()
            });
        match res {
            Ok(()) => r,
            // 記録できない原票は返さない（後で raw に入らない応答で controller を進めない）。
            Err(e) => PbsReply {
                outcome: CallOutcome::SpawnFailed,
                stderr: format!("recording {}: {e}", self.out.display()),
                ..r
            },
        }
    }
}

impl<T: PbsTransport> PbsTransport for RecordingTransport<T> {
    fn origin(&self) -> Origin {
        self.inner.origin()
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        self.record(self.inner.qsub(req))
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        self.record(self.inner.qstat_full(job_id))
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        self.record(self.inner.find_by_marker(m))
    }
}

/// 子の記録した原票を読む（末尾の壊れ行は捨てる）。
pub fn read_replies(path: &Path) -> io::Result<Vec<PbsReply>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    drop_torn_tail(path)?;
    fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(io::Error::other))
        .collect()
}

fn load_spec_study(spec: &Path, study: &Path) -> Result<(RecoverySpec, Study), String> {
    let spec = RecoverySpec::parse(
        &fs::read_to_string(spec).map_err(|e| format!("{}: {e}", spec.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let study: Study =
        serde_json::from_slice(&fs::read(study).map_err(|e| format!("{}: {e}", study.display()))?)
            .map_err(|e| format!("study: {e}"))?;
    Ok((spec, study))
}

fn epoch(arm: Arm, cond: Condition, incarnation: u64) -> String {
    format!("{}-{}-{incarnation}", arm.as_str(), cond.as_str())
}

/// 子（測定専用 controller）の本体。完走で Ok。incarnation 1 は境界で合図し、stdin の `CONTINUE` を待つ。
pub fn child_trial(cfg: &ChildConfig, ctl: &mut dyn Controller) -> Result<(), String> {
    let (spec, study) = load_spec_study(&cfg.spec_path, &cfg.study_path)?;
    let trial_key = study.collector(&spec, cfg.arm, cfg.condition).trial_key();
    let transport = Arc::new(RecordingTransport::new(
        cfg.access.transport(&spec, &cfg.dir)?,
        cfg.replies_file(),
    ));
    let clock = cfg.access.clock()?;
    let site = cfg.access.site(&spec, &cfg.trial_dir)?;
    let ext = PbsExt::open(
        &cfg.dir,
        spec.clone(),
        cfg.arm,
        &trial_key,
        &epoch(cfg.arm, cfg.condition, cfg.incarnation),
        transport,
        clock,
    )
    .map_err(|e| e.to_string())?;
    let fault = match std::env::var(CHILD_FAULT_ENV) {
        Ok(v) => Some(ChildFault::parse(&v)?),
        Err(_) => None,
    };
    let incarnation = cfg.incarnation;
    let file_state = match &cfg.access {
        PbsAccess::File { state } => Some(state.clone()),
        PbsAccess::Ssh { .. } => None,
    };
    if fault.is_some() && file_state.is_none() {
        return Err(format!("{CHILD_FAULT_ENV} is for the file transport only"));
    }
    let dup = (fault == Some(ChildFault::DuplicateSubmit)).then(|| {
        (
            cfg.dir.clone(),
            file_state.clone().unwrap_or_default(),
            cfg.replies_file(),
            spec.clone(),
            cfg.arm,
            trial_key.clone(),
        )
    });
    let announce: Option<super::entry::Announce> = (incarnation == 1).then(|| {
        Box::new(move |logical: &str, job_id: &str, _snapshot: &Value| {
            let line =
                json!({"logical_job_id": logical, "job_id": job_id, "incarnation": incarnation});
            let mut out = io::stdout().lock();
            writeln!(out, "\n{WAIT_SAVED_LINE} {line}")?;
            out.flush()?;
            drop(out);
            let mut reply = String::new();
            io::stdin().lock().read_line(&mut reply)?;
            if reply.trim() == CONTINUE_LINE {
                if let Some((dir, state, replies, spec, arm, trial_key)) = &dup {
                    let mut again = spec.intent_spec(*arm, trial_key, logical);
                    again.identity.workload_attempt += 1;
                    let pbs = RecordingTransport::new(FilePbs::open(state)?, replies.clone());
                    IntentLedger::open(dir, "fault-duplicate-submit")?
                        .submit(&again, &pbs, &FileClock::open(state)?)
                        .map_err(|e| io::Error::other(e.to_string()))?;
                }
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "runner did not continue after {WAIT_SAVED_LINE}: {reply:?}"
                )))
            }
        }) as super::entry::Announce
    });
    let gated = child_gated(
        ext,
        &cfg.trial_dir,
        site,
        ctl.saved_reader(),
        announce,
        fault == Some(ChildFault::AnnounceBeforeSave),
    );
    if ctl.run(gated)? {
        if fault == Some(ChildFault::DuplicateConsume) {
            duplicate_last_consumed(&cfg.dir)?;
        }
        Ok(())
    } else {
        Err("controller did not complete".into())
    }
}

/// [`ChildFault::DuplicateConsume`]: 最後の消費行を seq だけ変えて追記する（台帳の重複防止を迂回する故障）。
fn duplicate_last_consumed(dir: &Path) -> Result<(), String> {
    let rows = read_consumed(dir)?;
    let mut row = rows
        .last()
        .cloned()
        .ok_or("duplicate-consume: nothing consumed")?;
    row.seq += 1;
    let mut line = serde_json::to_vec(&row).map_err(e2s)?;
    line.push(b'\n');
    let mut f = OpenOptions::new()
        .append(true)
        .open(dir.join(super::backend::CONSUMED_LEDGER))
        .map_err(e2s)?;
    f.write_all(&line).and_then(|()| f.sync_data()).map_err(e2s)
}

/// 親が子と独立の接続で読み直した保存済み Waiting。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWait {
    /// `state.json`（baseline）/ `sqlite_ro`（Celeris、読み取り専用の接続）。
    pub source: String,
    pub wait_id: Option<String>,
    pub job_id: String,
    pub snapshot: Value,
}

/// 親の読み直し（arm ごとの保存先を、子の handle を使わずに開く）。Celeris の read-only SQLite は呼び出し側が注入する。
pub type SavedWaitReader = dyn Fn(Arm, &Path, &str) -> io::Result<Option<SavedWait>>;

/// baseline の `state.json` を新しく open して、`job_id` を待つ Waiting を探す。
pub fn read_saved_state_json(trial_dir: &Path, job_id: &str) -> io::Result<Option<SavedWait>> {
    let path = trial_dir.join("state.json");
    let state: Value = match fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(state["nodes"].as_object().and_then(|nodes| {
        nodes.iter().find_map(|(id, n)| {
            (n["status"] == "waiting" && n["physical_id"] == job_id).then(|| SavedWait {
                source: "state.json".into(),
                wait_id: None,
                job_id: job_id.into(),
                snapshot: json!({"node": id, "state": n}),
            })
        })
    }))
}

/// 1 trial（arm × condition）の親。
pub struct ProcTrial<'a> {
    /// 両 arm 共通の測定 dir（絶対 path）。
    pub dir: &'a Path,
    pub spec: &'a RecoverySpec,
    pub spec_path: &'a Path,
    pub study: &'a Study,
    pub study_path: &'a Path,
    pub arm: Arm,
    pub condition: Condition,
    /// PBS への経路（子にも同じ値を渡す）。
    pub access: &'a PbsAccess,
    /// harness の `Params`（子へそのまま渡す）。
    pub params: Value,
    /// 子 process の command（test binary の `pbs_control_child`）。env は proc が足す。
    pub child: &'a dyn Fn() -> Command,
    pub read_saved: &'a SavedWaitReader,
}

pub use super::recovery::{ControllerRun, RejectReason};

/// A2 の証跡（`a2.json`、`celeris.recovery-a2/1`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcA2Evidence {
    pub schema: String,
    pub logical_job_id: String,
    pub job_id: String,
    pub saved_wait: SavedWait,
    pub saved_wait_file: String,
    pub saved_wait_sha256: String,
    pub killed_pid: u32,
    pub killed_starttime: u64,
    pub exit_signal: Option<i32>,
    /// 停止中に親が取った同じ job の原票（`scheduler-events.jsonl` の seq）。
    pub stopped_qstat_seqs: Vec<u64>,
    pub intent_rows_before_stop: usize,
    pub intent_rows_after_resume: usize,
    pub consumed_job_ref: String,
}

#[derive(Clone, Debug)]
pub struct ProcReport {
    pub logical_job_id: String,
    pub job_id: String,
    pub job_ref: String,
    pub binding: TrialBinding,
    pub saved_wait: SavedWait,
    pub controllers: Vec<ControllerRun>,
    pub a2: Option<ProcA2Evidence>,
    /// `recovery-trials.jsonl` に追記した行。
    pub recovery: RecoveryRecord,
}

fn sha256_bytes(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn rows(path: &Path) -> usize {
    fs::read_to_string(path).map_or(0, |t| t.lines().filter(|l| !l.trim().is_empty()).count())
}

/// `/proc/<pid>/stat` の starttime（第 22 欄）。pid の再利用を見分ける。
pub fn proc_starttime(pid: u32) -> io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let rest = stat
        .rsplit_once(')')
        .map(|(_, r)| r)
        .ok_or_else(|| io::Error::other("stat: no comm"))?;
    rest.split_whitespace()
        .nth(19)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| io::Error::other("stat: no starttime"))
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn e2s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl ProcTrial<'_> {
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

    fn child_cmd(&self, incarnation: u64) -> Result<Command, String> {
        let cfg = ChildConfig {
            params: self.params.clone(),
            spec_path: self.spec_path.to_path_buf(),
            study_path: self.study_path.to_path_buf(),
            access: self.access.clone(),
            dir: self.dir.to_path_buf(),
            trial_dir: self.trial_dir(),
            arm: self.arm,
            condition: self.condition,
            incarnation,
        };
        let mut c = (self.child)();
        c.env(CHILD_ENV, serde_json::to_string(&cfg).map_err(e2s)?);
        Ok(c)
    }

    /// 1 trial を流して raw に結合する。境界・停止・回収の順は module 文書のとおり。
    /// 拒否（付記 I14 (5)）は Err で終え、理由を trial dir の [`REJECT_FILE`] に残す（測定成功にしない）。
    pub fn run(&self, raw: &mut RawWriter) -> Result<ProcReport, String> {
        let trial_dir = self.trial_dir();
        if trial_dir.exists() {
            return Err(format!(
                "{}: trial already exists (not overwritten)",
                trial_dir.display()
            ));
        }
        fs::create_dir_all(&trial_dir).map_err(e2s)?;
        self.run_trial(raw, &trial_dir).map_err(|e| {
            let reason = e
                .split_once(':')
                .and_then(|(k, _)| serde_json::from_value::<RejectReason>(json!(k)).ok());
            let rec = json!({"schema": REJECT_SCHEMA, "trial_key": self.trial_key(),
                "arm": self.arm.as_str(), "condition": self.condition.as_str(),
                "reject_reason": reason, "detail": e});
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(trial_dir.join(REJECT_FILE))
                .and_then(|mut f| {
                    f.write_all(rec.to_string().as_bytes())?;
                    f.sync_data()
                }) {
                Ok(()) => e,
                Err(w) => format!("{e} (and {REJECT_FILE} was not written: {w})"),
            }
        })
    }

    fn run_trial(&self, raw: &mut RawWriter, trial_dir: &Path) -> Result<ProcReport, String> {
        let trial_dir = trial_dir.to_path_buf();
        let deadline = Duration::from_secs(self.spec.limits.trial_deadline_secs);
        let log = EventLog::create(&trial_dir).map_err(e2s)?;
        let mut sup = Supervisor::new(
            log.clone(),
            Limits {
                wall: deadline,
                logical_secs: self.spec.limits.trial_deadline_secs,
                max_restarts: 1,
                ipc_io_timeout: Duration::from_secs(5),
                ipc_retries: 0,
                stop_grace: deadline,
                kill_wait: Duration::from_secs(30),
            },
        )
        .map_err(e2s)?;
        let t0 = Instant::now();
        let remaining = || deadline.saturating_sub(t0.elapsed());

        let mut c1 = sup.spawn(Role::Control, self.child_cmd(1)?).map_err(e2s)?;
        let pid1 = c1.pid();
        let start1 = proc_starttime(pid1).map_err(e2s)?;
        let announced: Value = loop {
            let line = match c1.next_line(remaining()) {
                Ok(l) => l,
                Err(e) => {
                    let _ = c1.kill("no_wait_saved");
                    return Err(format!(
                        "controller did not announce a saved wait: {e} (see {})",
                        trial_dir.join("control-1.stderr").display()
                    ));
                }
            };
            // libtest は `test <name> ... ` を改行なしで先に出すので、行の途中の合図も拾う。
            if let Some(i) = line.find(WAIT_SAVED_LINE) {
                break serde_json::from_str(line[i + WAIT_SAVED_LINE.len()..].trim())
                    .map_err(e2s)?;
            }
        };
        let job_id = announced["job_id"]
            .as_str()
            .ok_or("announcement without job_id")?
            .to_string();
        let logical = announced["logical_job_id"]
            .as_str()
            .ok_or("announcement without logical_job_id")?
            .to_string();

        // 子と独立に保存を読み直す（申告は合図だけ）。
        let saved = match (self.read_saved)(self.arm, &trial_dir, &job_id).map_err(e2s)? {
            Some(s) if s.job_id == job_id => s,
            _ => {
                let _ = c1.kill("wait_not_saved");
                return Err(format!(
                    "wait_not_saved: no saved Waiting with job {job_id} in the controller state"
                ));
            }
        };
        let intent = self.spec.intent_spec(self.arm, &self.trial_key(), &logical);
        let ledger_job = IntentLedger::open(self.dir, "proc-reader")
            .map_err(e2s)?
            .replay()
            .map_err(e2s)?
            .get(&intent.submit_intent_id())
            .and_then(|st| match &st.state {
                IntentState::Accepted { job_id } | IntentState::Reconciled { job_id } => {
                    Some(job_id.clone())
                }
                _ => None,
            });
        if ledger_job.as_deref() != Some(job_id.as_str()) {
            let _ = c1.kill("job_mismatch");
            return Err(format!(
                "job_mismatch: intent ledger has {ledger_job:?}, the saved wait has {job_id}"
            ));
        }
        let replies1 = read_replies(&replies_file(&trial_dir, 1)).map_err(e2s)?;
        if !replies1.iter().any(|r| {
            r.call == PbsCall::Qsub && r.outcome == CallOutcome::Ok && r.stdout.trim() == job_id
        }) {
            let _ = c1.kill("saved_evidence_missing");
            return Err(format!(
                "saved_evidence_missing: replies-1.jsonl has no qsub reply for {job_id}"
            ));
        }
        let saved_bytes = serde_json::to_vec_pretty(&saved).map_err(e2s)?;
        let saved_file = trial_dir.join(DURABLE_WAIT_FILE);
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&saved_file)
            .and_then(|mut f| {
                f.write_all(&saved_bytes)?;
                f.sync_data()
            })
            .map_err(e2s)?;
        let saved_sha = sha256_bytes(&saved_bytes);
        let wall_wait_saved = ms(t0.elapsed());
        let logical_wait_saved = self.access.logical_now();
        log.barrier(
            "wait_saved",
            json!({"job_id": job_id, "logical_job_id": logical, "source": saved.source,
                "wait_id": saved.wait_id, "file": DURABLE_WAIT_FILE, "sha256": saved_sha,
                "pid": pid1, "starttime": start1}),
        )
        .map_err(e2s)?;
        let intent_rows_before = rows(&self.dir.join(INTENT_LEDGER));
        let identity = raw
            .identity(self.arm, &self.trial_key(), &logical)
            .map_err(e2s)?;
        let site = self.access.site(self.spec, &trial_dir)?;
        let mut controllers = Vec::new();
        let mut stopped_seqs = Vec::new();
        let mut killed = None;
        let (mut wall_stop, mut wall_recovery) = (None, None);
        let (mut logical_stop, mut logical_resume) = (None, None);

        match self.condition {
            Condition::Normal => {
                site.release(&job_id, "normal: wait saved").map_err(e2s)?;
                log.barrier(
                    "released",
                    json!({"job_id": job_id, "reason": "normal: wait saved"}),
                )
                .map_err(e2s)?;
                c1.send_line(CONTINUE_LINE).map_err(e2s)?;
                c1.wait_exit(remaining()).map_err(e2s)?;
                let rep = c1.stop(None, "controller_exited").map_err(e2s)?;
                controllers.push(ControllerRun {
                    incarnation: 1,
                    pid: pid1,
                    starttime: start1,
                    exit_code: rep.exit_code,
                    exit_signal: rep.exit_signal,
                    killed_by_runner: rep.killed,
                });
                if rep.exit_code != Some(0) {
                    return Err(format!("normal: controller exited {rep:?}"));
                }
                log.barrier("controller_exited", json!({"incarnation": 1, "pid": pid1}))
                    .map_err(e2s)?;
                raw.record_replies(
                    &identity,
                    read_replies(&replies_file(&trial_dir, 1)).map_err(e2s)?,
                )
                .map_err(e2s)?;
            }
            Condition::A2 => {
                let now_start = proc_starttime(pid1).map_err(e2s)?;
                if now_start != start1 {
                    return Err(format!(
                        "stop_boundary_invalid: pid {pid1} starttime {now_start} differs from {start1}"
                    ));
                }
                let rep = c1.kill("a2-wait-saved").map_err(e2s)?;
                let t_kill = Instant::now();
                logical_stop = self.access.logical_now();
                controllers.push(ControllerRun {
                    incarnation: 1,
                    pid: pid1,
                    starttime: start1,
                    exit_code: rep.exit_code,
                    exit_signal: rep.exit_signal,
                    killed_by_runner: rep.killed,
                });
                if rep.exit_signal != Some(9) || !rep.killed {
                    return Err(format!(
                        "stop_boundary_invalid: controller was not SIGKILLed: {rep:?}"
                    ));
                }
                log.barrier(
                    "controller_killed",
                    json!({"pid": pid1, "starttime": start1, "exit_signal": rep.exit_signal}),
                )
                .map_err(e2s)?;
                killed = Some((pid1, start1, rep.exit_signal));
                site.release(&job_id, "a2: controller stopped")
                    .map_err(e2s)?;
                log.barrier(
                    "released",
                    json!({"job_id": job_id, "reason": "a2: controller stopped"}),
                )
                .map_err(e2s)?;
                // 子の原票を先に raw へ（順序は 1 → 停止中の親の照会 → 2）。
                raw.record_replies(
                    &identity,
                    read_replies(&replies_file(&trial_dir, 1)).map_err(e2s)?,
                )
                .map_err(e2s)?;
                let stopped = self.observe_stopped(&job_id, &intent)?;
                stopped_seqs = raw
                    .record_replies(&identity, stopped)
                    .map_err(e2s)?
                    .iter()
                    .map(|e| e.seq)
                    .collect();
                log.barrier(
                    "observed_finished_while_stopped",
                    json!({"job_id": job_id, "seqs": stopped_seqs}),
                )
                .map_err(e2s)?;
                wall_stop = Some(ms(t_kill.elapsed()));
                logical_resume = self.access.logical_now();
                let mut c2 = sup.spawn(Role::Control, self.child_cmd(2)?).map_err(e2s)?;
                let pid2 = c2.pid();
                let start2 = proc_starttime(pid2).unwrap_or(0);
                log.barrier("controller_resumed", json!({"incarnation": 2, "pid": pid2}))
                    .map_err(e2s)?;
                c2.wait_exit(remaining()).map_err(e2s)?;
                let rep2 = c2.stop(None, "controller_exited").map_err(e2s)?;
                wall_recovery = Some(ms(t_kill.elapsed()));
                controllers.push(ControllerRun {
                    incarnation: 2,
                    pid: pid2,
                    starttime: start2,
                    exit_code: rep2.exit_code,
                    exit_signal: rep2.exit_signal,
                    killed_by_runner: rep2.killed,
                });
                if rep2.exit_code != Some(0) {
                    return Err(format!(
                        "a2: resumed controller exited {rep2:?} (see {})",
                        trial_dir.join("control-2.stderr").display()
                    ));
                }
                log.barrier("controller_exited", json!({"incarnation": 2, "pid": pid2}))
                    .map_err(e2s)?;
                raw.record_replies(
                    &identity,
                    read_replies(&replies_file(&trial_dir, 2)).map_err(e2s)?,
                )
                .map_err(e2s)?;
            }
        }

        // 重複投入・重複消費・job 不一致。
        let intent_rows_after = rows(&self.dir.join(INTENT_LEDGER));
        if intent_rows_after != intent_rows_before {
            return Err(format!(
                "duplicate_submit: intent ledger grew from {intent_rows_before} to {intent_rows_after} rows after the saved wait"
            ));
        }
        let job_ref = intent.job_ref(&job_id);
        let consumed: Vec<ConsumedRow> = read_consumed(self.dir)?
            .into_iter()
            .filter(|r| r.submit_intent_id == intent.submit_intent_id())
            .collect();
        match consumed.as_slice() {
            [] => return Err("not_consumed: the controller did not consume the job".into()),
            [one] if one.job_ref != job_ref => {
                return Err(format!(
                    "job_mismatch: consumed {}, not the saved {job_ref}",
                    one.job_ref
                ));
            }
            [_] => {}
            many => {
                return Err(format!(
                    "duplicate_consume: {} consumed rows for {}",
                    many.len(),
                    intent.submit_intent_id()
                ));
            }
        }

        let t_grade = Instant::now();
        let att_dir = trial_dir.join("attempt-1");
        fs::create_dir_all(&att_dir).map_err(e2s)?;
        let paths = site.fetch_outputs(&job_id, &att_dir).map_err(e2s)?;
        let att = AttemptArtifacts {
            attempt: 1,
            dir: att_dir,
            paths,
        };
        let g = grade(self.spec, &att, &trial_dir.join(GRADE_FILE)).map_err(e2s)?;
        let wall_grade = ms(t_grade.elapsed());
        let t_collect = Instant::now();
        let observer = self.access.transport(self.spec, self.dir)?;
        let binding = raw
            .collect_trial(
                QstatView::new(observer.as_ref(), &self.spec.cluster),
                &TrialInput {
                    arm: self.arm,
                    logical_job_id: logical.clone(),
                    collector: self.study.collector(self.spec, self.arm, self.condition),
                    attempts: vec![att],
                    grade: Some(g),
                },
            )
            .map_err(e2s)?;
        let wall_collect = ms(t_collect.elapsed());
        let a2 = match killed {
            None => None,
            Some((killed_pid, killed_starttime, exit_signal)) => {
                let ev = ProcA2Evidence {
                    schema: A2_SCHEMA.into(),
                    logical_job_id: logical.clone(),
                    job_id: job_id.clone(),
                    saved_wait: saved.clone(),
                    saved_wait_file: DURABLE_WAIT_FILE.into(),
                    saved_wait_sha256: saved_sha.clone(),
                    killed_pid,
                    killed_starttime,
                    exit_signal,
                    stopped_qstat_seqs: stopped_seqs,
                    intent_rows_before_stop: intent_rows_before,
                    intent_rows_after_resume: intent_rows_after,
                    consumed_job_ref: job_ref.clone(),
                };
                fs::write(
                    trial_dir.join(A2_FILE),
                    serde_json::to_vec_pretty(&ev).map_err(e2s)?,
                )
                .map_err(e2s)?;
                Some(ev)
            }
        };
        let rel_trial = trial_dir.strip_prefix(self.dir).map_err(e2s)?;
        let recovery = raw
            .record_recovery(
                &binding,
                RecoveryInput {
                    condition: self.condition.as_str().into(),
                    controller: controllers.clone(),
                    saved_wait: Some(SavedWaitRef {
                        source: saved.source.clone(),
                        wait_id: saved.wait_id.clone(),
                        job_id: saved.job_id.clone(),
                        file: rel_trial.join(DURABLE_WAIT_FILE).display().to_string(),
                        sha256: saved_sha.clone(),
                    }),
                    stopped_qstat_seqs: a2
                        .as_ref()
                        .map(|a| a.stopped_qstat_seqs.clone())
                        .unwrap_or_default(),
                    wall_spawn_to_wait_saved_ms: Some(wall_wait_saved),
                    wall_stop_ms: wall_stop,
                    wall_recovery_ms: wall_recovery,
                    wall_collect_ms: Some(wall_collect),
                    wall_grade_ms: Some(wall_grade),
                    wall_trial_ms: Some(ms(t0.elapsed())),
                    logical_wait_saved_secs: logical_wait_saved,
                    logical_stop_secs: logical_stop,
                    logical_resume_secs: logical_resume,
                    // file の論理欄は state dir の偽の時計（FileClock）で実秒ではない。ssh は実時計。
                    clock_kind: self.access.clock_kind(),
                    reject_reason: None,
                },
            )
            .map_err(e2s)?;
        Ok(ProcReport {
            recovery,
            logical_job_id: logical,
            job_id,
            job_ref,
            binding,
            saved_wait: saved,
            controllers,
            a2,
        })
    }

    /// 停止中の照会: 親自身が開いた transport（子と別の handle・別 pid）で、同じ job を `F` まで照会する。
    /// poll 間隔は file では [`FileClock`] を進めるだけで待たず、ssh では実時計で待つ。停止中に `F` が取れなければ Err（A2 不成立）。
    fn observe_stopped(
        &self,
        job_id: &str,
        intent: &super::intent::IntentSpec,
    ) -> Result<Vec<PbsReply>, String> {
        let observer = self.access.transport(self.spec, self.dir)?;
        let clock = self.access.clock()?;
        let deadline = clock
            .now_unix()
            .saturating_add(self.spec.limits.trial_deadline_secs);
        let mut out = Vec::new();
        loop {
            let r = observer.qstat_full(job_id);
            let class = classify_qstat(&r, job_id);
            out.push(r);
            if let QstatClass::Record { attrs } = &class {
                // 停止中の原票が保存済み Waiting の job（同じ trial・同じ intent）のものか。
                intent
                    .check_record(job_id, attrs)
                    .map_err(|e| format!("job_mismatch: stopped qstat of {job_id}: {e}"))?;
            }
            match class {
                QstatClass::Record { attrs }
                    if attrs.get("job_state").map(String::as_str) == Some("F") =>
                {
                    return Ok(out);
                }
                QstatClass::Record { .. } => {}
                QstatClass::UnknownJob { detail } => {
                    return Err(format!(
                        "unobserved: {job_id} unknown while stopped: {detail}"
                    ));
                }
                QstatClass::Failed { reason, detail } => {
                    return Err(format!(
                        "collector_failed: qstat {job_id} ({reason:?}): {detail}"
                    ));
                }
            }
            let next = clock.now_unix().saturating_add(self.spec.limits.poll_secs);
            if next > deadline {
                return Err(format!(
                    "unobserved: {job_id} did not finish before the trial deadline"
                ));
            }
            clock.sleep_until(next);
        }
    }
}
