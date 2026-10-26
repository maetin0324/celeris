//! process 間で共有する決定的な偽 PBS（ADR 2026-10-25-recovery-scheduler-backend 付記 I14 (2)）。
//!
//! 状態は state dir の追記専用 JSONL だけに置き、in-memory の状態は持たない（呼び出しごとに `pbs.lock` の排他の中で
//! 読み直す）。controller の子 process・runner・observer が別々に開いても同じ cluster を見る。実 ssh・network は使わず、
//! 原票は [`Origin::Synthetic`]。時計 [`FileClock`] も state dir の file で、`sleep_until` は待たずに跳ぶ。
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    entry::Site,
    intent::IntentClock,
    transport::{
        CallOutcome, FakeCounts, MarkerQuery, PbsCall, PbsReply, PbsTransport, QsubRequest,
    },
};
use crate::{dagbench::ext::drop_torn_tail, observe::Origin};

pub const JOBS_FILE: &str = "jobs.jsonl";
pub const JOB_SCHEMA: &str = "celeris.file-pbs-job/1";
pub const CALLS_FILE: &str = "calls.jsonl";
pub const CALL_SCHEMA: &str = "celeris.file-pbs-call/1";
pub const FAULTS_FILE: &str = "faults.json";
pub const CLOCK_FILE: &str = "clock.json";
pub const LOCK_FILE: &str = "pbs.lock";
/// 偽 cluster の時計の初期値（unix 秒）。
pub const CLOCK_T0: u64 = 1_800_000_000;
/// 成果物の既定（`faults.output` が無いとき）。
pub const DEFAULT_OUTPUT: &str = "recovery payload output seed=17\n";

/// 試験が起動前に書く故障（`faults.json`）。無ければ全て無効。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FilePbsFaults {
    /// qsub の応答喪失（job は作らず Timeout を返す）。
    pub qsub_timeout: bool,
    /// qstat の collector 障害（`SpawnFailed`/`NonZero`/`Timeout`）。
    pub qstat_outcome: Option<CallOutcome>,
    /// 原票の `Variable_List` の `CELERIS_TRIAL_KEY` を差し替える（job/trial 不一致）。
    pub variable_list_trial_key: Option<String>,
    /// 解放後の照会で履歴が消える（`Unknown Job Id`）。
    pub forget_after_release: bool,
    /// 成果物の差し替え。
    pub output: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum JobEvent {
    Submitted {
        job_id: String,
        request: Box<QsubRequest>,
    },
    Released {
        job_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct JobRow {
    schema: String,
    #[serde(flatten)]
    event: JobEvent,
}

/// 試験の assert 用の job の写し（raw には使わない）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileJob {
    pub job_id: String,
    pub request: QsubRequest,
    pub released: bool,
}

/// file 永続の偽 PBS（`impl PbsTransport + Site`）。
#[derive(Clone, Debug)]
pub struct FilePbs {
    dir: PathBuf,
}

fn read_rows<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Vec<T>> {
    drop_torn_tail(path)?;
    match fs::read_to_string(path) {
        Ok(t) => t
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(io::Error::other))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn append_row(path: &Path, v: &Value) -> io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(format!("{v}\n").as_bytes())?;
    f.sync_data()
}

fn now_utc() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn reply(
    call: PbsCall,
    argv: Vec<String>,
    outcome: CallOutcome,
    stdout: String,
    stderr: String,
) -> PbsReply {
    let at = now_utc();
    PbsReply {
        call,
        argv,
        started_utc: at.clone(),
        finished_utc: at,
        exit: match outcome {
            CallOutcome::Ok => Some(0),
            CallOutcome::NonZero => Some(1),
            _ => None,
        },
        stdout,
        stderr,
        outcome,
    }
}

fn failed(call: PbsCall, argv: Vec<String>, e: &io::Error) -> PbsReply {
    reply(
        call,
        argv,
        CallOutcome::SpawnFailed,
        String::new(),
        format!("file-pbs state: {e}"),
    )
}

impl FilePbs {
    /// state dir を開く（無ければ作る）。process ごとに開き直してよい。
    pub fn open(state: &Path) -> io::Result<Self> {
        fs::create_dir_all(state)?;
        Ok(Self {
            dir: state.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 起動前に故障を書く（試験用）。
    pub fn write_faults(&self, f: &FilePbsFaults) -> io::Result<()> {
        fs::write(
            self.dir.join(FAULTS_FILE),
            serde_json::to_vec_pretty(f).map_err(io::Error::other)?,
        )
    }

    fn faults(&self) -> io::Result<FilePbsFaults> {
        match fs::read(self.dir.join(FAULTS_FILE)) {
            Ok(b) => serde_json::from_slice(&b).map_err(io::Error::other),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(FilePbsFaults::default()),
            Err(e) => Err(e),
        }
    }

    /// `pbs.lock` の排他の中で `f` を走らせる（lock は file を閉じると外れる）。
    fn locked<T>(&self, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let lock = File::create(self.dir.join(LOCK_FILE))?;
        lock.lock()?;
        let r = f();
        drop(lock);
        r
    }

    fn job_rows(&self) -> io::Result<Vec<JobRow>> {
        read_rows(&self.dir.join(JOBS_FILE))
    }

    fn jobs_unlocked(&self) -> io::Result<Vec<FileJob>> {
        let mut out: Vec<FileJob> = Vec::new();
        for r in self.job_rows()? {
            match r.event {
                JobEvent::Submitted { job_id, request } => out.push(FileJob {
                    job_id,
                    request: *request,
                    released: false,
                }),
                JobEvent::Released { job_id } => {
                    if let Some(j) = out.iter_mut().find(|j| j.job_id == job_id) {
                        j.released = true;
                    }
                }
            }
        }
        Ok(out)
    }

    /// 投入された job（試験の assert 用）。
    pub fn jobs(&self) -> io::Result<Vec<FileJob>> {
        self.locked(|| self.jobs_unlocked())
    }

    /// `calls.jsonl` から数えた呼び出し数（物理 qsub 数・照会数）。
    pub fn counts(&self) -> io::Result<FakeCounts> {
        self.locked(|| {
            let rows: Vec<Value> = read_rows(&self.dir.join(CALLS_FILE))?;
            let mut c = FakeCounts::default();
            for r in rows {
                match r["call"].as_str() {
                    Some("qsub") => c.qsub += 1,
                    Some("qstat_full") => c.qstat_full += 1,
                    Some("find_by_marker") => c.find_by_marker += 1,
                    _ => {}
                }
            }
            Ok(c)
        })
    }

    /// `calls.jsonl` の行（pid で呼んだ process を区別する）。
    pub fn calls(&self) -> io::Result<Vec<Value>> {
        self.locked(|| read_rows(&self.dir.join(CALLS_FILE)))
    }

    fn record_call(&self, r: &PbsReply) -> io::Result<()> {
        let path = self.dir.join(CALLS_FILE);
        let seq = read_rows::<Value>(&path)?.len() + 1;
        append_row(
            &path,
            &json!({"schema": CALL_SCHEMA, "seq": seq, "call": r.call, "argv": r.argv,
                "pid": std::process::id(), "outcome": r.outcome}),
        )
    }

    fn qsub_locked(&self, req: &QsubRequest, argv: Vec<String>) -> io::Result<PbsReply> {
        let faults = self.faults()?;
        let r = if faults.qsub_timeout {
            reply(
                PbsCall::Qsub,
                argv,
                CallOutcome::Timeout,
                String::new(),
                "timed out".into(),
            )
        } else {
            let n = self
                .job_rows()?
                .iter()
                .filter(|r| matches!(r.event, JobEvent::Submitted { .. }))
                .count();
            let id = format!("{}.sirius", 4300 + n + 1);
            append_row(
                &self.dir.join(JOBS_FILE),
                &serde_json::to_value(JobRow {
                    schema: JOB_SCHEMA.into(),
                    event: JobEvent::Submitted {
                        job_id: id.clone(),
                        request: Box::new(req.clone()),
                    },
                })
                .map_err(io::Error::other)?,
            )?;
            reply(
                PbsCall::Qsub,
                argv,
                CallOutcome::Ok,
                format!("{id}\n"),
                String::new(),
            )
        };
        self.record_call(&r)?;
        Ok(r)
    }

    fn qstat_locked(&self, job_id: &str, argv: Vec<String>) -> io::Result<PbsReply> {
        let faults = self.faults()?;
        let jobs = self.jobs_unlocked()?;
        let r = if let Some(o) = faults.qstat_outcome {
            reply(
                PbsCall::QstatFull,
                argv,
                o,
                String::new(),
                "qstat: collector fault".into(),
            )
        } else {
            match jobs
                .iter()
                .find(|j| j.job_id == job_id && !(j.released && faults.forget_after_release))
            {
                None => reply(
                    PbsCall::QstatFull,
                    argv,
                    CallOutcome::NonZero,
                    String::new(),
                    format!("qstat: Unknown Job Id {job_id}"),
                ),
                Some(j) => reply(
                    PbsCall::QstatFull,
                    argv,
                    CallOutcome::Ok,
                    qstat_text(j, faults.variable_list_trial_key.as_deref()),
                    String::new(),
                ),
            }
        };
        self.record_call(&r)?;
        Ok(r)
    }
}

/// `qstat -x -f` の原票（`R` は解放まで、以後 `F`・`Exit_status = 0`）。
fn qstat_text(j: &FileJob, trial_key: Option<&str>) -> String {
    let req = &j.request;
    let state = if j.released { "F" } else { "R" };
    let mut kv = vec![
        ("Job_Name".to_string(), req.job_name.clone()),
        ("Job_Owner".into(), "rmaeda@sirius-login1".into()),
        ("job_state".into(), state.into()),
        ("queue".into(), req.queue.clone()),
        ("server".into(), "sirius".into()),
        ("Account_Name".into(), req.account.clone()),
        ("Rerunable".into(), "False".into()),
        ("Resource_List.place".into(), req.place.clone()),
        ("Resource_List.select".into(), req.select.clone()),
        ("Resource_List.walltime".into(), req.walltime.clone()),
        (
            "Variable_List".into(),
            format!(
                "PBS_O_HOME=/home/rmaeda,CELERIS_TRIAL_KEY={},CELERIS_SUBMIT_INTENT={},RECOVERY_SMOKE_DIR={}",
                trial_key.unwrap_or(&req.trial_key),
                req.submit_intent,
                req.dir().unwrap_or_default()
            ),
        ),
        (
            "Submit_arguments".into(),
            format!("-q {} -A {} -r n {}", req.queue, req.account, req.script),
        ),
    ];
    if j.released {
        kv.push(("Exit_status".into(), "0".into()));
    }
    let mut out = format!("Job Id: {}\n", j.job_id);
    for (k, v) in kv {
        out.push_str(&format!("    {k} = {v}\n"));
    }
    out.push('\n');
    out
}

impl PbsTransport for FilePbs {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }

    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        let argv = match req.argv() {
            Ok(a) => a,
            Err(e) => {
                return reply(
                    PbsCall::Qsub,
                    vec!["qsub".into()],
                    CallOutcome::SpawnFailed,
                    String::new(),
                    e,
                );
            }
        };
        self.locked(|| self.qsub_locked(req, argv.clone()))
            .unwrap_or_else(|e| failed(PbsCall::Qsub, argv, &e))
    }

    fn qstat_full(&self, job_id: &str) -> PbsReply {
        let argv = vec!["qstat".into(), "-x".into(), "-f".into(), job_id.into()];
        self.locked(|| self.qstat_locked(job_id, argv.clone()))
            .unwrap_or_else(|e| failed(PbsCall::QstatFull, argv, &e))
    }

    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        let r = reply(
            PbsCall::FindByMarker,
            vec!["find-by-marker".into(), m.submit_intent.clone()],
            CallOutcome::Unsupported,
            String::new(),
            "marker search capability not confirmed".into(),
        );
        match self.locked(|| self.record_call(&r)) {
            Ok(()) => r,
            Err(e) => failed(PbsCall::FindByMarker, r.argv, &e),
        }
    }
}

impl Site for FilePbs {
    fn release(&self, job_id: &str, _reason: &str) -> io::Result<()> {
        self.locked(|| {
            let jobs = self.jobs_unlocked()?;
            let j = jobs
                .iter()
                .find(|j| j.job_id == job_id)
                .ok_or_else(|| io::Error::other(format!("release: no job {job_id}")))?;
            if j.released {
                return Ok(());
            }
            let out = self.dir.join("outputs").join(job_id);
            fs::create_dir_all(&out)?;
            let output = self.faults()?.output;
            fs::write(
                out.join("out.bin"),
                output.as_deref().unwrap_or(DEFAULT_OUTPUT),
            )?;
            append_row(
                &self.dir.join(JOBS_FILE),
                &serde_json::to_value(JobRow {
                    schema: JOB_SCHEMA.into(),
                    event: JobEvent::Released {
                        job_id: job_id.into(),
                    },
                })
                .map_err(io::Error::other)?,
            )
        })
    }

    fn fetch_outputs(&self, job_id: &str, dest: &Path) -> io::Result<Vec<String>> {
        self.locked(|| {
            let src = self.dir.join("outputs").join(job_id).join("out.bin");
            if !src.exists() {
                return Err(io::Error::other(format!("{job_id} has not finished")));
            }
            fs::copy(&src, dest.join("out.bin"))?;
            Ok(vec!["out.bin".into()])
        })
    }
}

/// state dir の `clock.json` を単調に進める時計（replay/偽の時計。実秒ではない）。
#[derive(Clone, Debug)]
pub struct FileClock {
    path: PathBuf,
    lock: PathBuf,
}

impl FileClock {
    pub fn open(state: &Path) -> io::Result<Self> {
        fs::create_dir_all(state)?;
        Ok(Self {
            path: state.join(CLOCK_FILE),
            lock: state.join(LOCK_FILE),
        })
    }

    fn update(&self, f: impl FnOnce(u64) -> u64) -> io::Result<u64> {
        let lock = File::create(&self.lock)?;
        lock.lock()?;
        let now = match fs::read(&self.path) {
            Ok(b) => serde_json::from_slice::<Value>(&b).map_err(io::Error::other)?["now_unix"]
                .as_u64()
                .ok_or_else(|| io::Error::other("clock.json: now_unix"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => CLOCK_T0,
            Err(e) => return Err(e),
        };
        let next = f(now).max(now);
        if next != now || !self.path.exists() {
            let tmp = self.path.with_extension("json.pending");
            let mut t = File::create(&tmp)?;
            t.write_all(json!({"now_unix": next}).to_string().as_bytes())?;
            t.sync_data()?;
            fs::rename(&tmp, &self.path)?;
        }
        Ok(next)
    }

    pub fn now(&self) -> io::Result<u64> {
        self.update(|n| n)
    }
}

impl IntentClock for FileClock {
    fn now_unix(&self) -> u64 {
        // 読めない時計は進めない（初期値）。state dir の故障は呼び出し側の file 検査で出る。
        self.now().unwrap_or(CLOCK_T0)
    }

    fn sleep_until(&self, t: u64) {
        let _ = self.update(|n| n.max(t));
    }
}
