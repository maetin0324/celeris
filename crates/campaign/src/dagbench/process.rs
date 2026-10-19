//! 独立観測 runner と所有 process の管理（ExtSim を制御 child から分離する）。
//!
//! # 役割（[`Role`]）
//! - `External`: [`ExtSim`](super::ext::ExtSim) と ledger を **唯一** 所有する独立 process（[`run_external_role`]）。
//! - `Control`: 制御 child。baseline / CelerisSide を走らせる。kill されても External は生き続ける。
//! - `Observer`: この runner（[`Supervisor`] を持つ側）。control 接続とは別の接続・別の client 名で、
//!   `observe` op（通知を消費せず ack もしない）で外部時計を進め、job 完了と ledger 追記を control 不在でも進める。
//!
//! 同一 process 内の thread は外部 process として数えない。External / Control は test executable か bin を
//! role で再起動した別 process（[`ROLE_ENV`] 等）で、起動した `Child` を [`OwnedProc`] が所有する。
//!
//! # 所有と停止
//! 停止・回収できるのは [`Supervisor::spawn`] が返した [`OwnedProc`]（= 自分が起こした `Child` の handle）だけ。
//! PID 番号を受け取って signal を送る API は無い。[`OwnedProc::stop`] は
//! 「graceful 要求 → `stop_grace` まで exit 待ち → kill → `kill_wait` まで wait」の有界手順で、
//! 全ての結果と理由を [`EventLog`] に残す。
//!
//! # 証跡（[`EventLog`]）
//! 呼び出し側が渡す **絶対** dir の `process-events.jsonl`（1 行 1 event、`seq` は単調増加、各行 sync）。
//! 相対 dir は拒否し、tempdir への fallback はしない。event 種別:
//! `limits` / `process_started`（role・pid・incarnation・owner_pid・program）/ `process_stopped`
//! （reason・graceful_error・killed・exit code/signal・elapsed_ms）/ `barrier`（連番 `barrier_seq`）/
//! `observer_advance` / `observer_finished`。wall / logical / restart / IPC の上限は [`Limits`] で `limits` event に残る。
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    ext::{DATASET, ExtBackend, ExtConfig, ExtSim, read_ledger},
    ipc::{self, IpcExt},
    metrics::{PROCESS_RAW_SCHEMA, RAW_SCHEMA, raw_schema_of},
};

/// role 再起動の入口で見る環境変数（`external` / 試験が決めたその他の値）。
pub const ROLE_ENV: &str = "DAGBENCH_ROLE";
pub const SOCKET_ENV: &str = "DAGBENCH_SOCKET";
pub const LEDGER_ENV: &str = "DAGBENCH_LEDGER";
/// `ExtConfig` の JSON。
pub const EXT_CFG_ENV: &str = "DAGBENCH_EXT_CFG";
/// External が listen を始めたら stdout に 1 行出す（親はこれを出来事として待つ。libtest 経由の role 再起動では
/// 行頭に `test ... ` が付くので、親は行末一致で見る）。
pub const READY_LINE: &str = "ready";

/// 確認の刻み（出来事待ちの合間に exit を見るだけ。順序は作らない）。
const WATCH_GAP: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Observer,
    External,
    Control,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Observer => "observer",
            Role::External => "external",
            Role::Control => "control",
        }
    }
}

/// wall / logical / restart / IPC の上限と有界停止の期限。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub wall: Duration,
    /// 外部時計がここを超えて進む要求は拒否する（論理秒）。
    pub logical_secs: u64,
    pub max_restarts: u32,
    pub ipc_io_timeout: Duration,
    pub ipc_retries: u32,
    /// graceful 要求後に exit を待つ上限。
    pub stop_grace: Duration,
    /// kill 後に wait する上限。
    pub kill_wait: Duration,
}

impl Limits {
    pub fn ipc_options(&self) -> ipc::IpcOptions {
        ipc::IpcOptions {
            io_timeout: self.ipc_io_timeout,
            retries: self.ipc_retries,
        }
    }
}

// ---- 証跡 ----

/// role・pid・開始/終了・barrier の追記専用証跡。
pub struct EventLog {
    path: PathBuf,
    dir: PathBuf,
    file: Mutex<File>,
    seq: AtomicU64,
    barrier_seq: AtomicU64,
    t0: Instant,
}

impl EventLog {
    /// `dir`（絶対 path。run の成果物 dir）に `process-events.jsonl` を新規に作る。
    pub fn create(dir: &Path) -> io::Result<Arc<Self>> {
        if !dir.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("event log dir must be absolute: {}", dir.display()),
            ));
        }
        let path = dir.join("process-events.jsonl");
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        Ok(Arc::new(Self {
            path,
            dir: dir.to_path_buf(),
            file: Mutex::new(file),
            seq: AtomicU64::new(0),
            barrier_seq: AtomicU64::new(0),
            t0: Instant::now(),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 1 行追記して sync し、その `seq` を返す。
    pub fn append(&self, kind: &str, detail: Value) -> io::Result<u64> {
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut line = json!({
            "seq": seq, "kind": kind, "unix_ms": unix_ms,
            "t_ms": self.t0.elapsed().as_millis() as u64,
        });
        if let (Some(m), Value::Object(d)) = (line.as_object_mut(), detail) {
            m.extend(d);
        }
        let mut s = serde_json::to_string(&line).map_err(io::Error::other)?;
        s.push('\n');
        f.write_all(s.as_bytes())?;
        f.sync_data()?;
        Ok(seq)
    }

    /// 連番つきの barrier（`barrier_seq` は 1 から欠番なし）。戻り値は `barrier_seq`。
    pub fn barrier(&self, name: &str, detail: Value) -> io::Result<u64> {
        let n = self.barrier_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut d = json!({"name": name, "barrier_seq": n});
        if let (Some(m), Value::Object(x)) = (d.as_object_mut(), detail) {
            m.extend(x);
        }
        self.append("barrier", d)?;
        Ok(n)
    }
}

/// 証跡 file を読む（試験・集計用）。
pub fn read_events(path: &Path) -> io::Result<Vec<Value>> {
    std::fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(io::Error::other))
        .collect()
}

// ---- 所有 process ----

/// 停止の結果（`process_stopped` event と同じ内容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StopReport {
    pub role: Role,
    pub pid: u32,
    pub incarnation: u64,
    /// `already_exited` / `graceful` / `killed` / `kill_unreaped`。
    pub reason: String,
    pub graceful_error: Option<String>,
    pub killed: bool,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<i32>,
    pub elapsed_ms: u64,
}

/// 自分が起こした `Child` の所有 handle。signal を送れるのはこの handle の `Child` だけ。
pub struct OwnedProc {
    role: Role,
    pid: u32,
    incarnation: u64,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    limits: Limits,
    log: Arc<EventLog>,
    stopped: bool,
}

impl OwnedProc {
    pub fn role(&self) -> Role {
        self.role
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    /// child が stdout に出した次の 1 行を `timeout` まで待つ（reader thread からの出来事待ち）。
    pub fn next_line(&self, timeout: Duration) -> io::Result<String> {
        self.lines.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => {
                io::Error::new(io::ErrorKind::TimedOut, "no line from child in time")
            }
            mpsc::RecvTimeoutError::Disconnected => {
                io::Error::new(io::ErrorKind::UnexpectedEof, "child closed stdout")
            }
        })
    }

    /// child の stdin へ 1 行送る（試験が child に「進んでよい」を伝える出来事。child は stdin の行を待つ）。
    pub fn send_line(&mut self, line: &str) -> io::Result<()> {
        let w = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "child stdin closed"))?;
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
        w.flush()
    }

    /// 自分で終わったか（回収はしない側の確認ではなく、回収込み）。
    pub fn try_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// exit を `deadline` まで待つ。`Ok(None)` は期限切れ（child は生きている）。
    pub fn wait_exit(&mut self, deadline: Duration) -> io::Result<Option<ExitStatus>> {
        let end = Instant::now() + deadline;
        loop {
            if let Some(st) = self.child.try_wait()? {
                return Ok(Some(st));
            }
            if Instant::now() >= end {
                return Ok(None);
            }
            std::thread::sleep(WATCH_GAP);
        }
    }

    /// 有界の停止: graceful 要求 → `stop_grace` まで exit 待ち → kill → `kill_wait` まで wait。
    /// `graceful` の失敗（IPC 切断・timeout 等）は kill へ進む理由として記録する。
    pub fn stop(
        &mut self,
        graceful: Option<&mut dyn FnMut() -> io::Result<()>>,
        cause: &str,
    ) -> io::Result<StopReport> {
        self.stop_inner(graceful, false, cause)
    }

    fn stop_inner(
        &mut self,
        graceful: Option<&mut dyn FnMut() -> io::Result<()>>,
        immediate_kill: bool,
        cause: &str,
    ) -> io::Result<StopReport> {
        let t = Instant::now();
        let mut graceful_error = None;
        let mut killed = false;
        let mut status = self.child.try_wait()?;
        let reason;
        if status.is_some() {
            reason = "already_exited";
        } else {
            if let Some(g) = graceful
                && let Err(e) = g()
            {
                graceful_error = Some(format!("{:?}: {e}", e.kind()));
            }
            // stdin を閉じるのも graceful 要求（読んで止まる child 向け）。
            self.stdin = None;
            status = if immediate_kill {
                None
            } else {
                self.wait_exit(self.limits.stop_grace)?
            };
            if status.is_some() {
                reason = "graceful";
            } else {
                // 握っている Child にだけ SIGKILL（std の kill は unix で SIGKILL）。
                let _ = self.child.kill();
                killed = true;
                status = self.wait_exit(self.limits.kill_wait)?;
                reason = if status.is_some() {
                    "killed"
                } else {
                    "kill_unreaped"
                };
            }
        }
        self.stopped = status.is_some();
        let report = StopReport {
            role: self.role,
            pid: self.pid,
            incarnation: self.incarnation,
            reason: reason.into(),
            graceful_error,
            killed,
            exit_code: status.and_then(|s| s.code()),
            exit_signal: status.and_then(|s| exit_signal(&s)),
            elapsed_ms: t.elapsed().as_millis() as u64,
        };
        self.log.append(
            "process_stopped",
            json!({
                "role": report.role, "pid": report.pid, "incarnation": report.incarnation,
                "reason": report.reason, "cause": cause,
                "graceful_error": report.graceful_error, "killed": report.killed,
                "exit_code": report.exit_code, "exit_signal": report.exit_signal,
                "elapsed_ms": report.elapsed_ms,
            }),
        )?;
        Ok(report)
    }

    /// 制御 child を fault などで強制終了する（所有 handle にだけ SIGKILL、回収まで有界）。
    pub fn kill(&mut self, cause: &str) -> io::Result<StopReport> {
        self.stop_inner(None, true, cause)
    }
}

impl Drop for OwnedProc {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        // 取りこぼしても孤児を残さない（有界）。
        let _ = self.stop(None, "dropped_unstopped");
    }
}

fn exit_signal(st: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    st.signal()
}

/// role ごとの process を起こし、incarnation を数え、起動を証跡に残す。
pub struct Supervisor {
    log: Arc<EventLog>,
    limits: Limits,
    incarnations: BTreeMap<Role, u64>,
    started: Instant,
}

impl Supervisor {
    /// 作ると同時に `limits` event（上限）と observer の `process_started` を記録する。
    pub fn new(log: Arc<EventLog>, limits: Limits) -> io::Result<Self> {
        log.append(
            "limits",
            json!({
                "wall_secs": limits.wall.as_secs_f64(), "logical_secs": limits.logical_secs,
                "max_restarts": limits.max_restarts,
                "ipc_io_timeout_ms": limits.ipc_io_timeout.as_millis() as u64,
                "ipc_retries": limits.ipc_retries,
                "stop_grace_ms": limits.stop_grace.as_millis() as u64,
                "kill_wait_ms": limits.kill_wait.as_millis() as u64,
            }),
        )?;
        log.append(
            "process_started",
            json!({
                "role": Role::Observer, "pid": std::process::id(), "incarnation": 1,
                "owner_pid": Value::Null, "program": "observer-runner",
            }),
        )?;
        Ok(Self {
            log,
            limits,
            incarnations: BTreeMap::new(),
            started: Instant::now(),
        })
    }

    pub fn log(&self) -> &Arc<EventLog> {
        &self.log
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// 起動からの wall 上限の期限。
    pub fn wall_deadline(&self) -> Instant {
        self.started + self.limits.wall
    }

    /// role の process を起こす。`restart` は同じ role の 2 回目以降で、`max_restarts` を超えると拒否する。
    /// stdin / stdout は pipe（stdout は reader thread が行ごとに [`OwnedProc::next_line`] へ渡す）。
    /// stderr は `<log dir>/<role>-<incarnation>.stderr`。
    pub fn spawn(&mut self, role: Role, mut cmd: Command) -> io::Result<OwnedProc> {
        let inc = self.incarnations.get(&role).copied().unwrap_or(0) + 1;
        if inc > 1 + u64::from(self.limits.max_restarts) {
            self.log.append(
                "restart_refused",
                json!({"role": role, "incarnation": inc, "max_restarts": self.limits.max_restarts}),
            )?;
            return Err(io::Error::other(format!(
                "{} restart limit {} exceeded",
                role.as_str(),
                self.limits.max_restarts
            )));
        }
        let err = File::create(
            self.log
                .dir()
                .join(format!("{}-{inc}.stderr", role.as_str())),
        )?;
        let program = format!("{:?}", cmd.get_program());
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(err)
            .spawn()?;
        self.incarnations.insert(role, inc);
        let pid = child.id();
        let stdin = child.stdin.take();
        let (tx, lines) = mpsc::channel();
        if let Some(out) = child.stdout.take() {
            std::thread::spawn(move || {
                for l in BufReader::new(out).lines() {
                    let Ok(l) = l else { return };
                    if tx.send(l).is_err() {
                        return;
                    }
                }
            });
        }
        self.log.append(
            "process_started",
            json!({
                "role": role, "pid": pid, "incarnation": inc,
                "owner_pid": std::process::id(), "program": program,
                "restart": inc > 1,
            }),
        )?;
        Ok(OwnedProc {
            role,
            pid,
            incarnation: inc,
            child,
            stdin,
            lines,
            limits: self.limits.clone(),
            log: Arc::clone(&self.log),
            stopped: false,
        })
    }
}

// ---- 独立観測 ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObserverOutcome {
    pub advances: u64,
    pub final_now: u64,
    pub high_seq: u64,
    pub unacked: usize,
    /// 最後の `observe` 応答時点で sync 済みの External ledger の最終 seq（完了 ledger の sync ack）。
    pub ledger_seq: u64,
}

/// control 不在でも、`logicals` の各 job の期限まで外部時計を進めて完了させる（ledger は External が sync 追記する）。
/// 通知は受け取らず ack もしない（`observe` op）ので、control 向けの通知は outbox に残る。
/// `ipc` は control とは別の接続・別の client 名で作る。wall 期限・logical 上限を超える前に Err を返す。
pub fn observe_until_complete(
    ipc: &mut IpcExt,
    logicals: &[String],
    limits: &Limits,
    wall_deadline: Instant,
    log: &EventLog,
) -> io::Result<ObserverOutcome> {
    let mut out = ObserverOutcome {
        advances: 0,
        final_now: 0,
        high_seq: 0,
        unacked: 0,
        ledger_seq: 0,
    };
    for l in logicals {
        if Instant::now() >= wall_deadline {
            log.append(
                "observer_aborted",
                json!({"cause": "wall_cap", "logical": l}),
            )?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "observer wall cap exceeded",
            ));
        }
        let Some(due) = ExtBackend::due_secs(ipc, l)? else {
            log.append(
                "observer_aborted",
                json!({"cause": "unknown_job", "logical": l}),
            )?;
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("unknown logical job {l}"),
            ));
        };
        if due > limits.logical_secs {
            log.append(
                "observer_aborted",
                json!({"cause": "logical_cap", "logical": l, "due": due, "cap": limits.logical_secs}),
            )?;
            return Err(io::Error::other(format!(
                "logical cap {} exceeded by job {l} (due {due})",
                limits.logical_secs
            )));
        }
        let (now, high_seq, unacked, ledger_seq) = ipc.observe(due)?;
        out.advances += 1;
        (out.final_now, out.high_seq, out.unacked, out.ledger_seq) =
            (now, high_seq, unacked, ledger_seq);
        log.barrier(
            "observer_advance",
            json!({
                "logical": l, "target_secs": due, "now": now,
                "high_seq": high_seq, "unacked": unacked, "ledger_seq": ledger_seq,
            }),
        )?;
    }
    log.append("observer_finished", json!(out))?;
    Ok(out)
}

// ---- External role の入口 ----

/// External role の本体（独立 process から呼ぶ）。ExtSim と ledger を開いて listen し、準備ができたら
/// stdout に [`READY_LINE`] を出して、`Shutdown` 要求まで serve する。
pub fn run_external_role(socket: &Path, ledger: &Path, cfg: ExtConfig) -> io::Result<()> {
    let sim = ExtSim::open(ledger, cfg)?;
    let _ = std::fs::remove_file(socket);
    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    let mut out = io::stdout();
    writeln!(out, "{READY_LINE}")?;
    out.flush()?;
    ipc::serve(listener, sim)
}

/// 環境変数（[`SOCKET_ENV`] / [`LEDGER_ENV`] / [`EXT_CFG_ENV`]）から External role を走らせる。
pub fn run_external_role_from_env() -> io::Result<()> {
    let var = |k: &str| std::env::var(k).map_err(|_| io::Error::other(format!("missing env {k}")));
    let cfg: ExtConfig = serde_json::from_str(&var(EXT_CFG_ENV)?).map_err(io::Error::other)?;
    run_external_role(
        Path::new(&var(SOCKET_ENV)?),
        Path::new(&var(LEDGER_ENV)?),
        cfg,
    )
}

// ---- process raw（版 /2）と barrier 順序の検証 ----

/// barrier 名（[`verify_survival_barriers`] が順序を検証する）。
pub const B_CONTROL_ACKED: &str = "control_acked";
pub const B_CONTROL_EXIT_ACK: &str = "control_exit_ack";
pub const B_OBSERVER_ADVANCE: &str = "observer_advance";
pub const B_LEDGER_SYNC_ACK: &str = "ledger_sync_ack";
pub const B_CONTROL_RESUME: &str = "control_resume";
pub const B_CONTROL_DONE: &str = "control_done";

/// process raw の `run.json`（版 [`PROCESS_RAW_SCHEMA`]）。旧 in-process の `RunHeader`（/1）とは欄も版も別。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessRunHeader {
    pub schema: String,
    pub dataset: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    /// 障害（control kill）の位置の seed・event・nth（`nth = 1 + seed % occurrences`）。
    pub seed: u64,
    pub event: String,
    pub nth: u64,
    pub occurrences: u64,
    pub poll_secs: u64,
    pub ext_job_secs: u64,
    pub worker_slots: usize,
    pub external_job_slots: usize,
    /// wall / logical / restart / IPC の上限。
    pub limits: Limits,
}

impl ProcessRunHeader {
    pub fn write(&self, raw: &Path) -> io::Result<()> {
        std::fs::write(
            raw.join("run.json"),
            serde_json::to_string_pretty(self).map_err(io::Error::other)?,
        )
    }
}

/// barrier から読み取れる「制御不在での外部完了」の証明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurvivalProof {
    pub control_pid: u32,
    pub external_pid: u32,
    pub external_incarnation: u64,
    pub observer_advances: usize,
    pub ledger_seq: u64,
    pub completed: Vec<String>,
    pub resumed_incarnation: u64,
}

fn u(v: &Value, k: &str) -> Result<u64, String> {
    v[k].as_u64().ok_or_else(|| format!("missing {k} in {v}"))
}

/// 証跡 event の列から barrier 順序を検証する:
/// `control_acked` → control の `process_stopped`（SIGKILL・回収済）→ `control_exit_ack` → `observer_advance`+ →
/// `ledger_sync_ack` → `control_resume` → control 再起動（incarnation 2）→ `control_done`。
/// 終了 ack から完了 ack の間に control の `process_started` が無く、同じ External（pid/incarnation）が
/// `control_acked` と `ledger_sync_ack` の両方に載り、その間に停止していないこと。
pub fn verify_survival_barriers(events: &[Value]) -> Result<SurvivalProof, String> {
    let barrier = |name: &str| -> Result<&Value, String> {
        events
            .iter()
            .find(|e| e["kind"] == "barrier" && e["name"] == name)
            .ok_or_else(|| format!("barrier {name} missing"))
    };
    let seq = |v: &Value| u(v, "seq");
    let acked = barrier(B_CONTROL_ACKED)?;
    let exit = barrier(B_CONTROL_EXIT_ACK)?;
    let sync = barrier(B_LEDGER_SYNC_ACK)?;
    let resume = barrier(B_CONTROL_RESUME)?;
    let done = barrier(B_CONTROL_DONE)?;
    let ordered = [acked, exit, sync, resume, done]
        .iter()
        .map(|b| seq(b))
        .collect::<Result<Vec<_>, _>>()?;
    if !ordered.windows(2).all(|w| w[0] < w[1]) {
        return Err(format!("barrier order violated: {ordered:?}"));
    }
    let advances: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "barrier" && e["name"] == B_OBSERVER_ADVANCE)
        .collect();
    if advances.is_empty() {
        return Err("no observer_advance barrier".into());
    }
    for a in &advances {
        let s = seq(a)?;
        if s <= seq(exit)? || s >= seq(sync)? {
            return Err(format!(
                "observer_advance seq {s} is outside (control_exit_ack, ledger_sync_ack)"
            ));
        }
    }
    let control_pid = u(exit, "pid")? as u32;
    let stopped = events
        .iter()
        .find(|e| {
            e["kind"] == "process_stopped"
                && e["role"] == "control"
                && e["incarnation"] == 1
                && e["pid"] == control_pid
        })
        .ok_or("control incarnation 1 process_stopped missing")?;
    if !(seq(acked)? < seq(stopped)? && seq(stopped)? < seq(exit)?) {
        return Err(
            "control exit must be reaped (process_stopped) between acked and exit ack".into(),
        );
    }
    if stopped["exit_signal"] != 9 {
        return Err(format!("control was not SIGKILLed: {stopped}"));
    }
    // 終了 ack〜再開の間に control は居ない。
    let hi = seq(resume)?;
    if events.iter().any(|e| {
        e["kind"] == "process_started"
            && e["role"] == "control"
            && seq(e).is_ok_and(|s| s > seq(acked).unwrap_or(0) && s < hi)
            && e["incarnation"] != 1
    }) {
        return Err("a control process started before control_resume".into());
    }
    // 同じ External が両 barrier に載り、その間に止まっていない。
    let (xp, xi) = (u(acked, "external_pid")?, u(acked, "external_incarnation")?);
    if (xp, xi) != (u(sync, "external_pid")?, u(sync, "external_incarnation")?) {
        return Err("external identity changed between control_acked and ledger_sync_ack".into());
    }
    if events.iter().any(|e| {
        e["kind"] == "process_stopped"
            && e["pid"] == xp
            && seq(e).is_ok_and(|s| s < seq(sync).unwrap_or(0))
    }) {
        return Err("external stopped before ledger_sync_ack".into());
    }
    if !events
        .iter()
        .any(|e| e["kind"] == "process_started" && e["role"] == "external" && e["pid"] == xp)
    {
        return Err("external process_started missing".into());
    }
    let completed: Vec<String> = sync["completed"]
        .as_array()
        .ok_or("ledger_sync_ack.completed missing")?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if completed.is_empty() {
        return Err("ledger_sync_ack has no completed job".into());
    }
    let ledger_seq = u(sync, "ledger_seq")?;
    if u(sync, "max_completed_seq")? > ledger_seq || u(sync, "ledger_file_seq")? < ledger_seq {
        return Err("ledger_sync_ack: completion not covered by the synced ledger seq".into());
    }
    let restarted = events
        .iter()
        .find(|e| {
            e["kind"] == "process_started"
                && e["role"] == "control"
                && e["incarnation"] != 1
                && seq(e).is_ok_and(|s| s > hi)
        })
        .ok_or("control was not restarted after control_resume")?;
    Ok(SurvivalProof {
        control_pid,
        external_pid: xp as u32,
        external_incarnation: xi,
        observer_advances: advances.len(),
        ledger_seq,
        completed,
        resumed_incarnation: u(restarted, "incarnation")?,
    })
}

/// process raw の集計結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessSummary {
    pub schema: String,
    pub arm: String,
    pub seed: u64,
    pub event: String,
    pub nth: u64,
    pub proof: SurvivalProof,
    pub physical_launches: usize,
    pub logical_jobs: usize,
    pub consumed: usize,
    pub notices_sent: usize,
}

/// process raw dir（版 [`PROCESS_RAW_SCHEMA`]）だけを集計する入口。旧 in-process の `/1` raw は
/// 新方式として読み替えず、明示的に拒否する（`dagbench::metrics::compute` は逆に /2 を拒否する）。
pub fn compute_process(raw: &Path) -> Result<ProcessSummary, String> {
    let run_json =
        std::fs::read_to_string(raw.join("run.json")).map_err(|e| format!("run.json: {e}"))?;
    let schema = raw_schema_of(&run_json)?;
    if schema == RAW_SCHEMA {
        return Err(format!(
            "{RAW_SCHEMA} is an in-process raw; process aggregation reads only {PROCESS_RAW_SCHEMA} \
             and does not reinterpret the old raw as the process method"
        ));
    }
    if schema != PROCESS_RAW_SCHEMA {
        return Err(format!(
            "unknown raw schema {schema} (process aggregation reads only {PROCESS_RAW_SCHEMA})"
        ));
    }
    let h: ProcessRunHeader =
        serde_json::from_str(&run_json).map_err(|e| format!("run.json: {e}"))?;
    if h.dataset != DATASET {
        return Err(format!("raw dataset {} is not {DATASET}", h.dataset));
    }
    let events = read_events(&raw.join("process-events.jsonl")).map_err(|e| e.to_string())?;
    let proof = verify_survival_barriers(&events)?;
    let ledger = read_ledger(&raw.join("ext.jsonl")).map_err(|e| e.to_string())?;
    if ledger.iter().any(|e| e.dataset != DATASET) {
        return Err(format!("ext.jsonl has a record that is not {DATASET}"));
    }
    let c = super::ext::count(&ledger);
    Ok(ProcessSummary {
        schema,
        arm: h.arm,
        seed: h.seed,
        event: h.event,
        nth: h.nth,
        proof,
        physical_launches: c.physical_launches,
        logical_jobs: c.logical_jobs,
        consumed: c.consumed,
        notices_sent: c.notices_sent,
    })
}
