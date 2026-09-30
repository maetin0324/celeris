//! ADR-0088 D2: browser・celeris-browser-sandboxd・接続ごとの celeris-browser-egress を 1 runtime として
//! 起動・記録・停止する supervisor。
//!
//! 1 runtime = 1 本の専用の長寿命 std::thread（`celeris-browser-rt-<session>`）。bwrap の spawn・
//! egress の spawn（relay thread 経由）・記録・停止・回収はこの thread が行い、bwrap を回収し終えるまで
//! thread は終わらない（`PR_SET_PDEATHSIG` は親 thread の終了で発火するので、idle で消える
//! `spawn_blocking` の thread からは起動しない）。async 側はこの handle だけを持つ。
//!
//! 記録（`<record_dir>/<session>.pid`）は bwrap を 1 行目に、sandbox 内の process（sandboxd・browser
//! など）と稼働中の egress を pid+starttime で並べ、変化のたびに tmp+rename で書き直す。controller が
//! SIGKILL された後は `reap_on_start` が記録を読み、starttime が一致する本人だけを回収する。

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use nix::libc;
use task_core::browser_isolation::{
    IsolationAttestation, IsolationViolation, LiveIsolation, LiveSessionEntry, LiveSessions,
    RuntimeKind,
};

use crate::browser_runtime::{
    IsolatedRuntime, RecordedProcess, RuntimeError, RuntimeSpec, process_starttime, read_record,
    reap_recorded, same_process_alive, write_record,
};

/// 停止時の SIGTERM から SIGKILL までの猶予（ADR-0088 D2）。
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// 記録を採り直す間隔。
const REFRESH: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct SupervisorOptions {
    /// daemon 所有の記録 dir（ADR-0088 D3。sandbox に bind しない）。
    pub record_dir: PathBuf,
    /// false なら bwrap に PDEATHSIG を掛けない（取りこぼしを再現する試験専用。本番は true）。
    pub arm_parent_death: bool,
    /// ADR-0088 D5: 稼働中 session の registry。起動後に登録し、停止の最初に外す。
    pub registry: Option<Arc<LiveSessions>>,
}

impl SupervisorOptions {
    pub fn new(record_dir: impl Into<PathBuf>) -> Self {
        Self {
            record_dir: record_dir.into(),
            arm_parent_death: true,
            registry: None,
        }
    }
}

enum Cmd {
    Stop(mpsc::Sender<()>),
    Attest(mpsc::Sender<Result<IsolationAttestation, Vec<IsolationViolation>>>),
}

/// registry に載る supervisor 管理の session（ADR-0088 D5）。attestation は runtime thread に
/// 採り直させる。controller への state 投入口は持たない（CDP pipe は `Supervisor` の持ち主が
/// 持つか、agent-browser が駆動する）ので、復元は開封前に `isolation_required` で止まる。
struct SupervisedEntry {
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl LiveIsolation for SupervisedEntry {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        let (reply, rx) = mpsc::channel();
        let sent = self
            .tx
            .lock()
            .map(|tx| tx.send(Cmd::Attest(reply)).is_ok())
            .unwrap_or(false);
        if !sent {
            return Err(vec![IsolationViolation::NoProcessGroup]);
        }
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| Err(vec![IsolationViolation::NoProcessGroup]))
    }
}

impl LiveSessionEntry for SupervisedEntry {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Isolated
    }

    fn accepts_state(&self) -> bool {
        false
    }

    fn deliver_state(
        &self,
        _state: &[u8],
    ) -> Result<(), task_core::browser_isolation::StateRejected> {
        Err(task_core::browser_isolation::StateRejected)
    }
}

/// 稼働中 runtime の handle。drop でも停止する。
pub struct Supervisor {
    session_id: String,
    runtime_pid: i32,
    tx: mpsc::Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
    procs: Arc<Mutex<Vec<RecordedProcess>>>,
    record_path: PathBuf,
    registry: Option<Arc<LiveSessions>>,
    /// controller が持つ CDP pipe（`cdp_pipe` の runtime だけ）。
    pub cdp_write: Option<File>,
    pub cdp_read: Option<File>,
}

struct Started {
    runtime_pid: i32,
    cdp_write: Option<File>,
    cdp_read: Option<File>,
}

impl Supervisor {
    /// 専用 thread の上で runtime を起動し、最初の記録を書いてから返す。
    pub fn start(spec: RuntimeSpec, opts: SupervisorOptions) -> Result<Self, RuntimeError> {
        std::fs::create_dir_all(&opts.record_dir)?;
        let session_id = spec.session_id.clone();
        let record_path = opts.record_dir.join(format!("{session_id}.pid"));
        let procs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Started, RuntimeError>>();
        let thread_procs = procs.clone();
        let registry = opts.registry.clone();
        let thread = std::thread::Builder::new()
            .name(format!("celeris-browser-rt-{session_id}"))
            .spawn(move || runtime_thread(spec, opts, thread_procs, rx, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(started)) => {
                if let Some(reg) = &registry {
                    let entry = SupervisedEntry {
                        tx: Mutex::new(tx.clone()),
                    };
                    reg.insert(&session_id, Arc::new(entry));
                }
                Ok(Self {
                    session_id,
                    runtime_pid: started.runtime_pid,
                    tx,
                    thread: Some(thread),
                    procs,
                    record_path,
                    registry,
                    cdp_write: started.cdp_write,
                    cdp_read: started.cdp_read,
                })
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(RuntimeError::NotRunning)
            }
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The namespace child used for broker-side isolation fact collection.
    pub fn runtime_pid(&self) -> i32 {
        self.runtime_pid
    }

    pub fn record_path(&self) -> &Path {
        &self.record_path
    }

    /// 最新の記録（bwrap・sandbox 内の process・稼働中の egress）。
    pub fn processes(&self) -> Vec<RecordedProcess> {
        self.procs.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// SIGTERM → 猶予 → process group へ SIGKILL → wait → 記録を消す。戻った時点で全 process は回収済み。
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(reg) = self.registry.take() {
            reg.remove(&self.session_id);
        }
        let (done_tx, done_rx) = mpsc::channel();
        if self.tx.send(Cmd::Stop(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn runtime_thread(
    spec: RuntimeSpec,
    opts: SupervisorOptions,
    procs: Arc<Mutex<Vec<RecordedProcess>>>,
    rx: mpsc::Receiver<Cmd>,
    ready: mpsc::Sender<Result<Started, RuntimeError>>,
) {
    let mut rt = match IsolatedRuntime::launch_with(&spec, opts.arm_parent_death) {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let session = spec.session_id.clone();
    let mut last = Vec::new();
    if let Err(e) = refresh(&rt, &opts.record_dir, &session, &procs, &mut last) {
        // 記録できない runtime は残さない（ADR-0088 D2）。
        rt.signal_group(libc::SIGKILL);
        rt.reap();
        let _ = ready.send(Err(RuntimeError::Io(e)));
        return;
    }
    let started = Started {
        runtime_pid: rt.inner_pid(),
        cdp_write: rt.cdp_write.take(),
        cdp_read: rt.cdp_read.take(),
    };
    if ready.send(Ok(started)).is_err() {
        stop_runtime(&mut rt, &opts.record_dir, &session, &procs);
        return;
    }
    loop {
        match rx.recv_timeout(REFRESH) {
            Ok(Cmd::Attest(reply)) => {
                let _ = reply.send(rt.attest());
            }
            Ok(Cmd::Stop(done)) => {
                stop_runtime(&mut rt, &opts.record_dir, &session, &procs);
                let _ = done.send(());
                return;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_runtime(&mut rt, &opts.record_dir, &session, &procs);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if rt.exited_nowait() {
                    // runtime が自分で終わった。残りを process group ごと止めて回収する。
                    stop_runtime(&mut rt, &opts.record_dir, &session, &procs);
                    return;
                }
                let _ = refresh(&rt, &opts.record_dir, &session, &procs, &mut last);
            }
        }
    }
}

/// 停止。bwrap を回収する前（zombie でも pgid は保持される）にだけ process group へ signal を送り、
/// 回収した後は何も送らない。
fn stop_runtime(
    rt: &mut IsolatedRuntime,
    dir: &Path,
    session: &str,
    procs: &Arc<Mutex<Vec<RecordedProcess>>>,
) {
    drop(rt.cdp_write.take());
    drop(rt.cdp_read.take());
    if !rt.exited_nowait() {
        rt.signal_group(libc::SIGTERM);
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && !rt.exited_nowait() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    // bwrap は未回収なので pgid は再利用されていない。同じ group の egress もここで止まる。
    rt.signal_group(libc::SIGKILL);
    rt.reap();
    // egress は relay 側の waiter thread が回収する。回収を待つだけで signal は送らない。
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        match rt.egress_stats() {
            Some(s) if s.exited.len() < s.spawned.len() => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => break,
        }
    }
    // bwrap の子（pid namespace の init = bwrap-init）とその下は bwrap の回収後に PDEATHSIG と
    // pid namespace の後始末で非同期に消える。高負荷ではここで戻るとまだ生きて見えるので、
    // 記録した本人（pid+starttime）が全て消えるまで待つ。signal は追加で送らない。
    let recorded: Vec<RecordedProcess> = procs.lock().map(|p| p.clone()).unwrap_or_default();
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline
        && recorded
            .iter()
            .any(|p| same_process_alive(p.pid, p.starttime))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = std::fs::remove_file(dir.join(format!("{session}.pid")));
    if let Ok(mut p) = procs.lock() {
        p.clear();
    }
}

/// bwrap の子孫（sandbox 内）と稼働中の egress を採り直し、変わっていれば記録を書き直す。
fn refresh(
    rt: &IsolatedRuntime,
    dir: &Path,
    session: &str,
    procs: &Arc<Mutex<Vec<RecordedProcess>>>,
    last: &mut Vec<RecordedProcess>,
) -> std::io::Result<()> {
    let bwrap = rt.bwrap_pid();
    let st = process_starttime(bwrap).ok_or_else(|| std::io::Error::other("no starttime"))?;
    let mut now = vec![RecordedProcess {
        pid: bwrap,
        starttime: st,
        role: "bwrap".into(),
    }];
    for pid in descendants(bwrap) {
        if let Some(st) = process_starttime(pid) {
            now.push(RecordedProcess {
                pid,
                starttime: st,
                role: role_of(pid),
            });
        }
    }
    if let Some(s) = rt.egress_stats() {
        for pid in s.spawned.iter().filter(|p| !s.exited.contains(p)) {
            if let Some(st) = process_starttime(*pid) {
                now.push(RecordedProcess {
                    pid: *pid,
                    starttime: st,
                    role: "egress".into(),
                });
            }
        }
    }
    if now != *last {
        write_record(dir, session, &now)?;
        if let Ok(mut p) = procs.lock() {
            p.clone_from(&now);
        }
        *last = now;
    }
    Ok(())
}

/// `/proc/*/stat` の ppid から `root` の子孫を集める（root 自身は含まない）。
fn descendants(root: i32) -> Vec<i32> {
    let mut children: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let ppid = stat
                .rfind(')')
                .and_then(|i| stat[i + 1..].split_whitespace().nth(1)?.parse::<i32>().ok());
            if let Some(ppid) = ppid {
                children.entry(ppid).or_default().push(pid);
            }
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        for c in children.get(&p).into_iter().flatten() {
            out.push(*c);
            stack.push(*c);
        }
    }
    out.sort_unstable();
    out
}

fn role_of(pid: i32) -> String {
    let argv0 = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .and_then(|b| b.split(|c| *c == 0).next().map(|a| a.to_vec()))
        .map(|a| String::from_utf8_lossy(&a).into_owned())
        .unwrap_or_default();
    let base = argv0.rsplit('/').next().unwrap_or("");
    match base {
        "celeris-browser-sandboxd" => "sandboxd",
        "bwrap" => "bwrap-init",
        b if b.contains("chrome") => "browser",
        _ => "child",
    }
    .to_owned()
}

/// 起動時回収（ADR-0088 D3）: 記録 dir の runtime を starttime 照合つきで SIGKILL し、記録にあった
/// 本人がすべて消えるまで `timeout` だけ待つ（待つ間は signal を送らない）。戻り値は signal を送った pid。
pub fn reap_on_start(dir: &Path, timeout: Duration) -> std::io::Result<Vec<i32>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut recorded = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) == Some("pid") {
            recorded.extend(read_record(&path));
        }
    }
    let killed = reap_recorded(dir)?;
    let deadline = Instant::now() + timeout;
    while recorded
        .iter()
        .any(|p| same_process_alive(p.pid, p.starttime))
    {
        if Instant::now() >= deadline {
            return Err(std::io::Error::other(
                "recorded runtime processes survived reap",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(killed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrip_and_legacy_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let procs = vec![
            RecordedProcess {
                pid: 10,
                starttime: 5,
                role: "bwrap".into(),
            },
            RecordedProcess {
                pid: 11,
                starttime: 6,
                role: "egress".into(),
            },
        ];
        write_record(dir.path(), "s", &procs).expect("write");
        assert_eq!(read_record(&dir.path().join("s.pid")), procs);
        std::fs::write(dir.path().join("old.pid"), "42 7\n").expect("write");
        let old = read_record(&dir.path().join("old.pid"));
        assert_eq!(old[0].role, "bwrap");
    }

    #[test]
    fn reap_on_start_without_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = reap_on_start(&dir.path().join("none"), Duration::from_secs(1)).expect("reap");
        assert!(r.is_empty());
    }
}
