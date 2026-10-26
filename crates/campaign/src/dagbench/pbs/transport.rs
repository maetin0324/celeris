//! recovery 比較の PBS transport（ADR 2026-10-25-recovery-scheduler-backend 実装付記 I2）。
//!
//! - [`PbsTransport`] は qsub（1 回だけ・再試行しない）、`qstat -x -f <完全な ID>`、自分の job に限る marker 完全一致検索。
//! - [`SshPbs`] は `ssh -o BatchMode=yes -o ConnectTimeout=<n> -o ControlMaster=no -- <cluster> <cmd>`。
//!   ControlMaster は利用者の ssh 設定の既存 master を使うだけで、起動・停止しない（D5）。1 回の呼出し全体に timeout を持ち、
//!   過ぎたら子 process を SIGKILL して回収し、読めた途中の出力をそのまま原票に残す。
//! - [`SshSite`] は実 cluster の解放（`touch -- <dir>/release/<id>`）と成果物の取得（`cat -- <dir>/out/<id>/out.bin`）。
//!   `<dir>` は検査済みの `dirname(spec.payload.script)` だけで、呼び出しごとに `site-calls.jsonl` へ 1 行残す（付記 I15 (2)）。
//! - [`FakePbs`] は試験用の台本応答。ssh・qsub・外部 network を使わない。
//!
//! 応答の解釈は [`classify_qsub`]・[`classify_qstat`]・[`classify_marker`] が型で返す。timeout・切断・ID 不正は
//! unknown（未投入にも成功にもしない）。PBS 原票の解析は `observe` の既存パーサを共用する。
use std::{
    collections::{BTreeMap, VecDeque},
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    observe::{
        Origin, SchedulerReply, SchedulerTransport, TRIAL_MARKER_VAR, is_scheduler_job_id,
        job_variable, parse_qstat,
    },
    ssh_qstat::is_safe_cluster,
};

/// qsub に渡す submit intent の印（`-v CELERIS_SUBMIT_INTENT=<submit_intent_id>`）。
pub const SUBMIT_INTENT_VAR: &str = "CELERIS_SUBMIT_INTENT";

/// payload に渡す cluster 上の測定 dir（`-v RECOVERY_SMOKE_DIR=dirname(script)`。付記 I15 (4)）。
pub const SMOKE_DIR_VAR: &str = "RECOVERY_SMOKE_DIR";

/// timeout で殺した後、pipe の残りを読み切るまで待つ上限（孫 process が pipe を握っても止まらない）。
const KILL_GRACE: Duration = Duration::from_secs(2);

/// 呼び出しの種別。原票に残す。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PbsCall {
    Qsub,
    QstatFull,
    FindByMarker,
}

/// 呼び出しの結果。`SpawnFailed` は PBS に届いていないことが確実（引数検査で起動しなかった場合も含む）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Ok,
    Timeout,
    SpawnFailed,
    NonZero,
    /// 検索手段が未確認（D4.4）。照合不能として停止させる。
    Unsupported,
}

/// 1 回の呼び出しの原票。推定で補わず、読めたものだけを持つ。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PbsReply {
    pub call: PbsCall,
    /// 実際に起動した（起動しようとした）command。起動しなかったときは remote 側の argv。
    pub argv: Vec<String>,
    pub started_utc: String,
    pub finished_utc: String,
    /// 子 process の終了 code。timeout で殺した・起動できなかった・signal 終了では None。
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub outcome: CallOutcome,
}

/// qsub の要求。spec 由来の欄だけを持ち、自由文字列の引数を受けない。rerunnable は常に false（`-r n`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QsubRequest {
    pub queue: String,
    pub account: String,
    /// `-l select=<値>`（例 `1:ncpus=4`）。
    pub select: String,
    /// `-l place=<値>`（例 `excl`）。
    pub place: String,
    /// `-l walltime=<値>`（`HH:MM:SS`）。
    pub walltime: String,
    /// `-N`。表示用で照合には使わない。
    pub job_name: String,
    pub trial_key: String,
    pub submit_intent: String,
    /// cluster 上の job script の絶対 path。
    pub script: String,
}

/// 英数字と `.`・`_`・`-` だけ（先頭 `-` 不可）。cluster 名と同じ文字種。
fn token(field: &str, v: &str) -> Result<(), String> {
    if is_safe_cluster(v) {
        Ok(())
    } else {
        Err(format!(
            "{field} {v:?}: only [A-Za-z0-9._-] (not starting with '-')"
        ))
    }
}

fn chars(field: &str, v: &str, extra: &[u8]) -> Result<(), String> {
    if !v.is_empty()
        && !v.starts_with('-')
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b) || extra.contains(&b))
    {
        Ok(())
    } else {
        Err(format!("{field} {v:?}: contains characters not allowed"))
    }
}

impl QsubRequest {
    /// 検査済みの remote 側 argv（`qsub ... <script>`）。文字種が外れれば Err（起動しない）。
    pub fn argv(&self) -> Result<Vec<String>, String> {
        token("queue", &self.queue)?;
        token("account", &self.account)?;
        token("job_name", &self.job_name)?;
        token("trial_key", &self.trial_key)?;
        token("submit_intent", &self.submit_intent)?;
        chars("select", &self.select, b":=+")?;
        chars("place", &self.place, b":=+")?;
        if self.walltime.is_empty()
            || !self
                .walltime
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b':')
        {
            return Err(format!("walltime {:?}: expected HH:MM:SS", self.walltime));
        }
        if !self.script.starts_with('/') || self.script.split('/').any(|c| c == "..") {
            return Err(format!(
                "script {:?}: expected an absolute path without '..'",
                self.script
            ));
        }
        chars("script", &self.script[1..], b"/")?;
        let dir = self.dir()?;
        Ok(vec![
            "qsub".into(),
            "-q".into(),
            self.queue.clone(),
            "-A".into(),
            self.account.clone(),
            "-l".into(),
            format!("select={}", self.select),
            "-l".into(),
            format!("place={}", self.place),
            "-l".into(),
            format!("walltime={}", self.walltime),
            "-r".into(),
            "n".into(),
            "-N".into(),
            self.job_name.clone(),
            "-v".into(),
            format!(
                "{TRIAL_MARKER_VAR}={},{SUBMIT_INTENT_VAR}={},{SMOKE_DIR_VAR}={dir}",
                self.trial_key, self.submit_intent
            ),
            self.script.clone(),
        ])
    }

    /// cluster 上の測定 dir（`dirname(script)`）。root 直下の script は Err。
    pub fn dir(&self) -> Result<&str, String> {
        match self.script.rsplit_once('/') {
            Some((d, f)) if !d.is_empty() && !f.is_empty() => Ok(d),
            _ => Err(format!(
                "script {:?}: expected <dir>/<file> below the root",
                self.script
            )),
        }
    }
}

/// marker 検索の条件。`owner` の job だけを、両 marker の完全一致で探す。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerQuery {
    pub owner: String,
    pub trial_key: String,
    pub submit_intent: String,
}

/// PBS への呼び出し。各 call は transport 側の全体 timeout で必ず戻る。
pub trait PbsTransport {
    /// 取得元（Live / Synthetic）を raw に残す。
    fn origin(&self) -> Origin;
    /// 1 回だけ実行する。再試行しない。
    fn qsub(&self, req: &QsubRequest) -> PbsReply;
    /// `qstat -x -f <完全な ID>`（実行中と履歴）。
    fn qstat_full(&self, job_id: &str) -> PbsReply;
    /// 自分の job（実行中＋履歴）から marker 完全一致を探す。
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply;
}

/// 共有した transport（親子で同じ経路を包む）。
impl<T: PbsTransport + ?Sized> PbsTransport for Arc<T> {
    fn origin(&self) -> Origin {
        (**self).origin()
    }
    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        (**self).qsub(req)
    }
    fn qstat_full(&self, job_id: &str) -> PbsReply {
        (**self).qstat_full(job_id)
    }
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        (**self).find_by_marker(m)
    }
}

/// unknown の理由。どれも「未投入」「成功」の根拠にならない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    Timeout,
    /// ssh の切断・接続失敗（exit 255）や signal 終了。
    Disconnected,
    /// 非 0 終了（PBS が拒否したのか届かなかったのか区別できない）。
    NonZero,
    /// 応答が完全な PBS ID・期待した job の原票でない（PID 形・空・複数行・別 job）。
    InvalidId,
    Unsupported,
}

/// qsub 応答の分類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QsubClass {
    /// 完全な PBS ID を受け取った。
    Accepted { job_id: String },
    /// 子 process を起動していない（PBS に届いていないことが確実）。
    RejectedBeforeSend { detail: String },
    /// 送ったか分からない。marker 照合まで未投入扱いにしない。
    Unknown {
        reason: UnknownReason,
        detail: String,
    },
}

fn detail(r: &PbsReply) -> String {
    format!(
        "{:?} exit={:?} stdout={:?} stderr={:?}",
        r.outcome,
        r.exit,
        r.stdout.trim(),
        r.stderr.trim()
    )
}

fn unknown_reason(r: &PbsReply) -> UnknownReason {
    match r.outcome {
        CallOutcome::Timeout => UnknownReason::Timeout,
        CallOutcome::Unsupported => UnknownReason::Unsupported,
        CallOutcome::NonZero if r.exit.is_none_or(|c| c == 255) => UnknownReason::Disconnected,
        CallOutcome::NonZero => UnknownReason::NonZero,
        CallOutcome::Ok | CallOutcome::SpawnFailed => UnknownReason::InvalidId,
    }
}

pub fn classify_qsub(r: &PbsReply) -> QsubClass {
    match r.outcome {
        CallOutcome::SpawnFailed => QsubClass::RejectedBeforeSend { detail: detail(r) },
        CallOutcome::Ok => {
            let out = r.stdout.trim();
            if !out.contains(char::is_whitespace) && is_scheduler_job_id(out) {
                QsubClass::Accepted { job_id: out.into() }
            } else {
                QsubClass::Unknown {
                    reason: UnknownReason::InvalidId,
                    detail: detail(r),
                }
            }
        }
        _ => QsubClass::Unknown {
            reason: unknown_reason(r),
            detail: detail(r),
        },
    }
}

/// `qstat -x -f` 応答の分類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QstatClass {
    /// 要求した job の原票（`key = value`）。
    Record { attrs: BTreeMap<String, String> },
    /// scheduler が知らないと答えた（照会は届いた）。成功・未投入に補完しない。
    UnknownJob { detail: String },
    /// 照会自体が不確定（起動失敗・timeout・切断・別 job の原票）。
    Failed {
        reason: QueryFailure,
        detail: String,
    },
}

/// 照会の失敗。`NotStarted` は子 process を起動しなかった（引数不正・起動失敗）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryFailure {
    NotStarted,
    Unknown(UnknownReason),
}

fn query_failure(r: &PbsReply) -> QueryFailure {
    if r.outcome == CallOutcome::SpawnFailed {
        QueryFailure::NotStarted
    } else {
        QueryFailure::Unknown(unknown_reason(r))
    }
}

pub fn classify_qstat(r: &PbsReply, job_id: &str) -> QstatClass {
    if r.outcome == CallOutcome::NonZero && r.stderr.contains("Unknown Job Id") {
        return QstatClass::UnknownJob { detail: detail(r) };
    }
    if r.outcome != CallOutcome::Ok {
        return QstatClass::Failed {
            reason: query_failure(r),
            detail: detail(r),
        };
    }
    match parse_qstat(&r.stdout) {
        (Some(id), attrs) if id == job_id => QstatClass::Record { attrs },
        (id, _) => QstatClass::Failed {
            reason: QueryFailure::Unknown(UnknownReason::InvalidId),
            detail: format!("record is for {id:?}, not {job_id}"),
        },
    }
}

/// marker 検索の分類。候補の数（0・1・複数）の判断は呼び出し側（intent 台帳）がする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkerClass {
    /// owner と両 marker が完全一致した job id（出現順）。0 件は「未投入」の証拠ではない。
    Candidates(Vec<String>),
    Failed {
        reason: QueryFailure,
        detail: String,
    },
}

/// `qstat -x -f` 形式の複数原票を `Job Id:` で分けて既存パーサに渡す。
fn split_records(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        if l.trim_start().starts_with("Job Id:") || out.is_empty() {
            out.push(String::new());
        }
        if let Some(cur) = out.last_mut() {
            cur.push_str(l);
            cur.push('\n');
        }
    }
    out.retain(|r| !r.trim().is_empty());
    out
}

pub fn classify_marker(r: &PbsReply, q: &MarkerQuery) -> MarkerClass {
    if r.outcome != CallOutcome::Ok {
        return MarkerClass::Failed {
            reason: query_failure(r),
            detail: detail(r),
        };
    }
    let mut found = Vec::new();
    for rec in split_records(&r.stdout) {
        let (Some(id), kv) = parse_qstat(&rec) else {
            return MarkerClass::Failed {
                reason: QueryFailure::Unknown(UnknownReason::InvalidId),
                detail: format!(
                    "search output has text outside a job record: {:?}",
                    rec.trim()
                ),
            };
        };
        if !is_scheduler_job_id(&id) {
            return MarkerClass::Failed {
                reason: QueryFailure::Unknown(UnknownReason::InvalidId),
                detail: format!("search output has a non-PBS job id {id:?}"),
            };
        }
        let mine = kv
            .get("Job_Owner")
            .and_then(|o| o.split('@').next())
            .is_some_and(|o| o == q.owner);
        let marked = job_variable(&kv, TRIAL_MARKER_VAR).as_deref() == Some(q.trial_key.as_str())
            && job_variable(&kv, SUBMIT_INTENT_VAR).as_deref() == Some(q.submit_intent.as_str());
        if mine && marked {
            found.push(id);
        }
    }
    MarkerClass::Candidates(found)
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// 子 process の終了（pipe の EOF）を待つ時計。試験は出来事で発火する実装を注入する。
pub trait CallTimer: Send + Sync {
    /// `event` に通知が来れば true、`limit` を過ぎれば false。
    fn wait(&self, event: &mpsc::Receiver<()>, limit: Duration) -> bool;
}

/// 実時計。
pub struct RealTimer;

impl CallTimer for RealTimer {
    fn wait(&self, event: &mpsc::Receiver<()>, limit: Duration) -> bool {
        event.recv_timeout(limit).is_ok()
    }
}

/// 1 回の起動の結果（byte のまま）。`status` が None なら timeout（子は SIGKILL 済み）。
struct BoundedRun {
    started_utc: String,
    finished_utc: String,
    spawn_error: Option<String>,
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// 起動から回収までの全体を `limit` で区切って 1 回実行する。
fn run_bounded(
    call: PbsCall,
    argv: Vec<String>,
    limit: Duration,
    timer: &dyn CallTimer,
) -> PbsReply {
    let r = run_bounded_bytes(&argv, limit, timer);
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let (exit, outcome, stderr) = match (&r.spawn_error, r.status) {
        (Some(e), _) => (None, CallOutcome::SpawnFailed, e.clone()),
        (None, None) => (None, CallOutcome::Timeout, text(&r.stderr)),
        (None, Some(s)) if s.success() => (s.code(), CallOutcome::Ok, text(&r.stderr)),
        (None, Some(s)) => (s.code(), CallOutcome::NonZero, text(&r.stderr)),
    };
    PbsReply {
        call,
        argv,
        started_utc: r.started_utc,
        finished_utc: r.finished_utc,
        exit,
        stdout: text(&r.stdout),
        stderr,
        outcome,
    }
}

fn run_bounded_bytes(argv: &[String], limit: Duration, timer: &dyn CallTimer) -> BoundedRun {
    let started_utc = now_utc();
    let failed = |e: String| BoundedRun {
        started_utc: started_utc.clone(),
        finished_utc: now_utc(),
        spawn_error: Some(e),
        status: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let Some((prog, args)) = argv.split_first() else {
        return failed("empty command".into());
    };
    let start = Instant::now();
    let mut child = match Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return failed(format!("spawn {prog}: {e}")),
    };
    let (tx, rx) = mpsc::channel();
    let mut bufs = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ] {
        let buf = Arc::new(Mutex::new(Vec::new()));
        bufs.push(buf.clone());
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Some(mut p) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(n) = p.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buf.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend_from_slice(&chunk[..n]);
                }
            }
            let _ = tx.send(());
        });
    }
    drop(tx);
    let mut eofs = 0;
    let mut timed_out = false;
    while eofs < 2 {
        if !timer.wait(&rx, limit.saturating_sub(start.elapsed())) {
            timed_out = true;
            break;
        }
        eofs += 1;
    }
    let mut status = None;
    if !timed_out {
        // 両 pipe が閉じても process が残ることがある。残りの時間だけ回収を待つ。
        loop {
            match child.try_wait() {
                Ok(Some(s)) => {
                    status = Some(s);
                    break;
                }
                Ok(None) if start.elapsed() < limit => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    timed_out = true;
                    break;
                }
            }
        }
    }
    if timed_out {
        let _ = child.kill();
        let _ = child.wait();
        while eofs < 2 && rx.recv_timeout(KILL_GRACE).is_ok() {
            eofs += 1;
        }
    }
    let take = |i: usize| std::mem::take(&mut *bufs[i].lock().unwrap_or_else(|e| e.into_inner()));
    BoundedRun {
        started_utc,
        finished_utc: now_utc(),
        spawn_error: None,
        status,
        stdout: take(0),
        stderr: take(1),
    }
}

/// 起動前に拒否した呼び出しの原票（PBS に届いていない）。
fn refused(call: PbsCall, argv: Vec<String>, why: String) -> PbsReply {
    let t = now_utc();
    PbsReply {
        call,
        argv,
        started_utc: t.clone(),
        finished_utc: t,
        exit: None,
        stdout: String::new(),
        stderr: format!("not started: {why}"),
        outcome: CallOutcome::SpawnFailed,
    }
}

/// 本番の PBS transport（ssh 越し）。
pub struct SshPbs {
    cluster: String,
    timeout: Duration,
    program: PathBuf,
    timer: Arc<dyn CallTimer>,
}

impl SshPbs {
    /// `timeout` は 1 回の呼出し全体（接続・実行・回収）の上限（D2 の 60 秒、`RecoverySpec` から渡す）。
    pub fn new(cluster: &str, timeout: Duration) -> Result<Self, String> {
        if !is_safe_cluster(cluster) {
            return Err(format!("refusing cluster {cluster:?}"));
        }
        if timeout.is_zero() {
            return Err("ssh timeout must be positive".into());
        }
        Ok(Self {
            cluster: cluster.into(),
            timeout,
            program: PathBuf::from("ssh"),
            timer: Arc::new(RealTimer),
        })
    }

    /// 起動する ssh を差し替える（試験の stub）。
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    /// 待ちの時計を差し替える（試験で timeout を出来事で発火させる）。
    pub fn with_timer(mut self, timer: Arc<dyn CallTimer>) -> Self {
        self.timer = timer;
        self
    }

    fn ssh_argv(&self, remote: &[String]) -> Vec<String> {
        let mut v = vec![
            self.program.to_string_lossy().into_owned(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!("ConnectTimeout={}", self.timeout.as_secs().max(1)),
            "-o".into(),
            "ControlMaster=no".into(),
            "--".into(),
            self.cluster.clone(),
        ];
        v.extend(remote.iter().cloned());
        v
    }

    fn run(&self, call: PbsCall, remote: Vec<String>) -> PbsReply {
        run_bounded(
            call,
            self.ssh_argv(&remote),
            self.timeout,
            self.timer.as_ref(),
        )
    }
}

impl PbsTransport for SshPbs {
    fn origin(&self) -> Origin {
        Origin::Live
    }

    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        match req.argv() {
            Ok(remote) => self.run(PbsCall::Qsub, remote),
            Err(e) => refused(PbsCall::Qsub, vec!["qsub".into()], e),
        }
    }

    fn qstat_full(&self, job_id: &str) -> PbsReply {
        let remote = vec!["qstat".into(), "-x".into(), "-f".into(), job_id.into()];
        if !is_scheduler_job_id(job_id) {
            return refused(
                PbsCall::QstatFull,
                remote,
                format!("not a PBS job id {job_id:?}"),
            );
        }
        self.run(PbsCall::QstatFull, remote)
    }

    /// 検索の具体 command は接続後の read-only 確認で決める（D4.4）。それまでは起動せず `Unsupported`。
    fn find_by_marker(&self, _m: &MarkerQuery) -> PbsReply {
        let t = now_utc();
        PbsReply {
            call: PbsCall::FindByMarker,
            argv: Vec::new(),
            started_utc: t.clone(),
            finished_utc: t,
            exit: None,
            stdout: String::new(),
            stderr: "marker search command is not confirmed for this cluster (D4.4)".into(),
            outcome: CallOutcome::Unsupported,
        }
    }
}

/// [`SshSite`] の原票（trial dir、追記専用）。
pub const SITE_CALLS_FILE: &str = "site-calls.jsonl";
pub const SITE_CALL_SCHEMA: &str = "celeris.recovery-site-call/1";
/// `fetch_outputs` の stdout の上限。
pub const FETCH_LIMIT_BYTES: usize = 1 << 20;

/// [`SshSite`] の呼び出しの種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteCall {
    Release,
    FetchOutputs,
}

/// [`SITE_CALLS_FILE`] の 1 行（付記 I15 (2)）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteCallRow {
    pub schema: String,
    pub seq: u64,
    pub call: SiteCall,
    pub job_id: String,
    pub argv: Vec<String>,
    pub started_utc: String,
    pub finished_utc: String,
    pub exit: Option<i32>,
    pub outcome: CallOutcome,
    pub stdout_sha256: String,
    pub stdout_bytes: u64,
}

/// cluster 上の測定 dir（`dirname(spec.payload.script)`）を検査して返す。絶対 path・`..` と空要素なし・
/// 文字種 `[A-Za-z0-9._/-]`・basename が `measurement_id`。
pub fn cluster_measurement_dir(script: &str, measurement_id: &str) -> Result<String, String> {
    let bad = |why: &str| Err(format!("payload.script {script:?}: {why}"));
    let Some(rest) = script.strip_prefix('/') else {
        return bad("not an absolute path");
    };
    if !script
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
    {
        return bad("only [A-Za-z0-9._/-] is allowed");
    }
    let parts: Vec<&str> = rest.split('/').collect();
    if parts
        .iter()
        .any(|c| c.is_empty() || *c == "." || *c == "..")
    {
        return bad("has an empty, '.' or '..' component");
    }
    let [dirs @ .., _file] = parts.as_slice() else {
        return bad("has no directory");
    };
    match dirs.last() {
        Some(base) if *base == measurement_id => Ok(format!("/{}", dirs.join("/"))),
        _ => bad(&format!(
            "the directory basename must be the measurement_id {measurement_id:?}"
        )),
    }
}

/// 実 cluster の [`super::entry::Site`]（付記 I15 (2)）。`release` は `touch -- <dir>/release/<job_id>`、
/// `fetch_outputs` は `cat -- <dir>/out/<job_id>/out.bin`。argv は [`SshPbs`] と同じ ssh で、全体 timeout を持つ。
/// 呼び出しごとに [`SITE_CALLS_FILE`] へ 1 行を追記する。qdel 等・`<dir>` の外への書き込みは持たない。
pub struct SshSite {
    ssh: SshPbs,
    dir: String,
    calls: PathBuf,
}

impl SshSite {
    /// `script`・`measurement_id` は spec の値。`calls` は trial dir の [`SITE_CALLS_FILE`]。
    pub fn new(
        cluster: &str,
        timeout: Duration,
        script: &str,
        measurement_id: &str,
        calls: PathBuf,
    ) -> Result<Self, String> {
        Ok(Self {
            ssh: SshPbs::new(cluster, timeout)?,
            dir: cluster_measurement_dir(script, measurement_id)?,
            calls,
        })
    }

    /// 起動する ssh を差し替える（試験の stub）。
    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.ssh = self.ssh.with_program(program);
        self
    }

    /// 待ちの時計を差し替える（試験で timeout を出来事で発火させる）。
    pub fn with_timer(mut self, timer: Arc<dyn CallTimer>) -> Self {
        self.ssh = self.ssh.with_timer(timer);
        self
    }

    /// cluster 上の測定 dir（検査済み）。
    pub fn dir(&self) -> &str {
        &self.dir
    }

    fn call(
        &self,
        call: SiteCall,
        job_id: &str,
        remote: Vec<String>,
    ) -> std::io::Result<(CallOutcome, Vec<u8>, String)> {
        use std::io::Write as _;
        let argv = self.ssh.ssh_argv(&remote);
        let r = run_bounded_bytes(&argv, self.ssh.timeout, self.ssh.timer.as_ref());
        let outcome = match (&r.spawn_error, r.status) {
            (Some(_), _) => CallOutcome::SpawnFailed,
            (None, None) => CallOutcome::Timeout,
            (None, Some(s)) if s.success() => CallOutcome::Ok,
            (None, Some(_)) => CallOutcome::NonZero,
        };
        let seq = std::fs::read_to_string(&self.calls)
            .map_or(0, |t| t.lines().filter(|l| !l.trim().is_empty()).count())
            as u64
            + 1;
        let row = SiteCallRow {
            schema: SITE_CALL_SCHEMA.into(),
            seq,
            call,
            job_id: job_id.into(),
            argv,
            started_utc: r.started_utc,
            finished_utc: r.finished_utc,
            exit: r.status.and_then(|s| s.code()),
            outcome,
            stdout_sha256: sha256_hex(&r.stdout),
            stdout_bytes: r.stdout.len() as u64,
        };
        let mut line = serde_json::to_vec(&row).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.calls)?;
        f.write_all(&line)?;
        f.sync_data()?;
        let why = r
            .spawn_error
            .unwrap_or_else(|| String::from_utf8_lossy(&r.stderr).trim().to_string());
        Ok((outcome, r.stdout, why))
    }
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn site_job(job_id: &str) -> std::io::Result<()> {
    if is_scheduler_job_id(job_id) {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "not a PBS job id {job_id:?} (not started)"
        )))
    }
}

impl super::entry::Site for SshSite {
    fn release(&self, job_id: &str, _reason: &str) -> std::io::Result<()> {
        site_job(job_id)?;
        let marker = format!("{}/release/{job_id}", self.dir);
        let (outcome, _, why) = self.call(
            SiteCall::Release,
            job_id,
            vec!["touch".into(), "--".into(), marker],
        )?;
        match outcome {
            CallOutcome::Ok => Ok(()),
            o => Err(std::io::Error::other(format!(
                "release {job_id}: {o:?}: {why}"
            ))),
        }
    }

    fn fetch_outputs(&self, job_id: &str, dest: &std::path::Path) -> std::io::Result<Vec<String>> {
        use std::io::Write as _;
        site_job(job_id)?;
        let src = format!("{}/out/{job_id}/out.bin", self.dir);
        let (outcome, out, why) = self.call(
            SiteCall::FetchOutputs,
            job_id,
            vec!["cat".into(), "--".into(), src],
        )?;
        if outcome != CallOutcome::Ok {
            return Err(std::io::Error::other(format!(
                "fetch_outputs {job_id}: {outcome:?}: {why}"
            )));
        }
        if out.is_empty() || out.len() > FETCH_LIMIT_BYTES {
            return Err(std::io::Error::other(format!(
                "fetch_outputs {job_id}: {} bytes (expected 1..={FETCH_LIMIT_BYTES})",
                out.len()
            )));
        }
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dest.join("out.bin"))?;
        f.write_all(&out)?;
        f.sync_all()?;
        Ok(vec!["out.bin".into()])
    }
}

/// `PbsTransport::qstat_full` だけを `observe::SchedulerTransport` に写す view。qsub を呼べない（再収集は read-only）。
/// 受けた原票は取得時刻付きで溜め、raw 保存に渡す。
pub struct QstatView<'a> {
    transport: &'a dyn PbsTransport,
    cluster: String,
    replies: Mutex<Vec<PbsReply>>,
}

impl<'a> QstatView<'a> {
    pub fn new(transport: &'a dyn PbsTransport, cluster: &str) -> Self {
        Self {
            transport,
            cluster: cluster.into(),
            replies: Mutex::new(Vec::new()),
        }
    }

    /// これまでの qstat 原票（呼んだ順）。
    pub fn into_replies(self) -> Vec<PbsReply> {
        self.replies.into_inner().unwrap_or_else(|e| e.into_inner())
    }
}

impl SchedulerTransport for QstatView<'_> {
    fn origin(&self) -> Origin {
        self.transport.origin()
    }

    fn qstat(
        &self,
        scheduler: &str,
        cluster: &str,
        job_id: &str,
    ) -> Result<SchedulerReply, String> {
        if scheduler != "pbs" || cluster != self.cluster {
            return Err(format!("this transport only answers pbs:{}", self.cluster));
        }
        let r = self.transport.qstat_full(job_id);
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(r.clone());
        // 失敗の文言は observe の分類（`Unknown Job Id` → Unobserved、他は FetchFailed）に渡る。
        if r.outcome != CallOutcome::Ok {
            return Err(detail(&r));
        }
        Ok(SchedulerReply {
            cluster: self.cluster.clone(),
            raw: r.stdout,
        })
    }
}

/// 偽 transport の台本の 1 応答。
#[derive(Clone, Debug)]
pub struct Scripted {
    pub outcome: CallOutcome,
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Scripted {
    pub fn ok(stdout: &str) -> Self {
        Self {
            outcome: CallOutcome::Ok,
            exit: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    /// 全体 timeout。`partial` は殺す前に読めた出力。
    pub fn timeout(partial: &str) -> Self {
        Self {
            outcome: CallOutcome::Timeout,
            exit: None,
            stdout: partial.into(),
            stderr: String::new(),
        }
    }
    pub fn nonzero(exit: i32, stderr: &str) -> Self {
        Self {
            outcome: CallOutcome::NonZero,
            exit: Some(exit),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
    pub fn spawn_failed() -> Self {
        Self {
            outcome: CallOutcome::SpawnFailed,
            exit: None,
            stdout: String::new(),
            stderr: "fake: not started".into(),
        }
    }
    pub fn unsupported() -> Self {
        Self {
            outcome: CallOutcome::Unsupported,
            exit: None,
            stdout: String::new(),
            stderr: "fake: marker search not confirmed".into(),
        }
    }
}

/// 呼び出し数。`qsub` は引数検査を通って PBS に送った（送ったつもりの）回数＝物理投入数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FakeCounts {
    pub qsub: u32,
    pub qstat_full: u32,
    pub find_by_marker: u32,
}

#[derive(Default)]
struct FakeState {
    qsub: VecDeque<Scripted>,
    qstat: VecDeque<Scripted>,
    find: VecDeque<Scripted>,
    counts: FakeCounts,
    submitted: Vec<QsubRequest>,
    tick: u64,
}

/// 試験用の偽 transport（`Origin::Synthetic`）。call 種別ごとの台本を順に返す。台本が尽きたら `SpawnFailed`。
/// 時刻は 2026-01-01T00:00:00Z から呼び出しごとに 1 秒進む合成時計。
#[derive(Default)]
pub struct FakePbs {
    state: Mutex<FakeState>,
}

impl FakePbs {
    pub fn new() -> Self {
        Self::default()
    }

    fn with(&self, f: impl FnOnce(&mut FakeState)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
    }

    pub fn script_qsub(&self, r: Scripted) -> &Self {
        self.with(|s| s.qsub.push_back(r));
        self
    }
    pub fn script_qstat(&self, r: Scripted) -> &Self {
        self.with(|s| s.qstat.push_back(r));
        self
    }
    pub fn script_find(&self, r: Scripted) -> &Self {
        self.with(|s| s.find.push_back(r));
        self
    }

    pub fn counts(&self) -> FakeCounts {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).counts
    }

    /// 物理投入した要求（順）。
    pub fn submitted(&self) -> Vec<QsubRequest> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .submitted
            .clone()
    }

    fn answer(
        &self,
        call: PbsCall,
        argv: Vec<String>,
        pick: impl FnOnce(&mut FakeState) -> Option<Scripted>,
    ) -> PbsReply {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let r = pick(&mut s).unwrap_or_else(Scripted::spawn_failed);
        let at = |n: u64| {
            (OffsetDateTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600 + n))
                .format(&Rfc3339)
                .unwrap_or_default()
        };
        s.tick += 1;
        let t = s.tick;
        PbsReply {
            call,
            argv,
            started_utc: at(t),
            finished_utc: at(t),
            exit: r.exit,
            stdout: r.stdout,
            stderr: r.stderr,
            outcome: r.outcome,
        }
    }
}

impl PbsTransport for FakePbs {
    fn origin(&self) -> Origin {
        Origin::Synthetic
    }

    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        match req.argv() {
            Ok(argv) => self.answer(PbsCall::Qsub, argv, |s| {
                s.counts.qsub += 1;
                s.submitted.push(req.clone());
                s.qsub.pop_front()
            }),
            Err(e) => refused(PbsCall::Qsub, vec!["qsub".into()], e),
        }
    }

    fn qstat_full(&self, job_id: &str) -> PbsReply {
        let argv = vec!["qstat".into(), "-x".into(), "-f".into(), job_id.into()];
        if !is_scheduler_job_id(job_id) {
            return refused(
                PbsCall::QstatFull,
                argv,
                format!("not a PBS job id {job_id:?}"),
            );
        }
        self.answer(PbsCall::QstatFull, argv, |s| {
            s.counts.qstat_full += 1;
            s.qstat.pop_front()
        })
    }

    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        let argv = vec![
            "find-by-marker".into(),
            m.owner.clone(),
            m.trial_key.clone(),
            m.submit_intent.clone(),
        ];
        self.answer(PbsCall::FindByMarker, argv, |s| {
            s.counts.find_by_marker += 1;
            s.find.pop_front()
        })
    }
}
