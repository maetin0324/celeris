//! 障害注入 runner（`docs/ops/dag-fault-comparison.md`）。
//!
//! 両方式（baseline / celeris-side）に同じ障害条件を与える。制御 process は呼び出し側が渡す command で
//! この runner が子 process として起こし、`Child` を握る（測定側が所有する process だけ）。
//! - (a) 制御 process の停止・再起動: ext-sim の [`FaultKind::StopControl`] が trigger event（`external_submit_accepted`
//!   / `external_wait_started`）の nth 件目で `fault_fired` を ledger に確定させ、制御 process 側の停止 hook で
//!   止まる。runner は ext ledger にその行が現れたのを見て、握っている `Child` にだけ SIGKILL を送り、同じ永続
//!   state（baseline の state file / Celeris の SQLite と ledger）で同じ command を起こし直す。本番 daemon・
//!   無関係な process には signal を送らない（pid 番号で探して kill しない）。
//! - (b) 応答喪失・(c) 完了通知の重複: ext-sim の hook（[`FaultKind::DropResponse`] / [`FaultKind::DuplicateNotice`]）。
//!
//! 観測側（この runner）は制御 process と独立した process で、自分の観測 ledger（`fault.jsonl`）に追記する。
//! 「制御（daemon）停止」は `control_stopped`、「評価 runner 停止」は `runner_stopped` という別 event 種別で残す。
//! 待ちは出来事待ち（ledger の行・子の終了）で、刻みの短い確認と長い保険（`wall_cap`）だけを使う（ADR-0125）。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    celeris_side::WorkerEntry,
    ext::{DATASET, Entry, FaultKind, FaultSpec},
};

/// scenarios.json の `faults.list[].class`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultClass {
    ControlRestart,
    SubmitResponseLost,
    CompletionDuplicate,
}

/// 1 回の障害注入の条件（両方式で同じ値を使う）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultCase {
    pub fault_id: String,
    pub class: FaultClass,
    pub trigger_event: String,
    pub seed: u64,
    /// 正常 run での trigger event の出現回数（論理 job ごとに 1 回なので campaign 数 × 外部 node 数）。
    pub occurrences: u64,
    /// (c) の通知の総数（元の 1 通を含む）。
    pub copies: u32,
}

impl FaultCase {
    /// scenarios.json の `faults.list` から `fault_id` の定義を読む。
    pub fn from_scenarios(
        path: &Path,
        fault_id: &str,
        seed: u64,
        occurrences: u64,
    ) -> Result<Self, String> {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let f = v["faults"]["list"]
            .as_array()
            .and_then(|l| l.iter().find(|f| f["id"] == fault_id))
            .ok_or_else(|| format!("unknown fault {fault_id}"))?;
        let class: FaultClass =
            serde_json::from_value(f["class"].clone()).map_err(|e| format!("class: {e}"))?;
        let trigger_event = f["trigger"]["event"]
            .as_str()
            .ok_or("fault without trigger.event")?
            .to_string();
        Ok(FaultCase {
            fault_id: fault_id.into(),
            class,
            trigger_event,
            seed,
            occurrences,
            copies: f["action"]["copies"].as_u64().unwrap_or(2) as u32,
        })
    }

    /// ext-sim に渡す fault（トリガ位置 `nth = 1 + seed % occurrences`）。
    pub fn ext_fault(&self) -> FaultSpec {
        let kind = match self.class {
            FaultClass::ControlRestart => FaultKind::StopControl,
            FaultClass::SubmitResponseLost => FaultKind::DropResponse,
            FaultClass::CompletionDuplicate => FaultKind::DuplicateNotice {
                extra: self.copies.saturating_sub(1),
            },
        };
        FaultSpec {
            fault_id: self.fault_id.clone(),
            kind,
            event: self.trigger_event.clone(),
            seed: self.seed,
            occurrences: self.occurrences,
        }
    }
}

/// 起こす制御 process（測定側の所有物。runner が `Child` を握る）。
#[derive(Debug, Clone)]
pub struct ControlCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// 観測 ledger の 1 行（runner だけが書く）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObsEntry {
    pub seq: u64,
    /// 実時間（unix ms）。復旧の実時間に使う。
    pub wall_ms: u64,
    pub event: String,
    pub dataset: String,
    pub detail: Value,
}

pub fn read_obs(path: &Path) -> std::io::Result<Vec<ObsEntry>> {
    read_jsonl(path)
}

/// JSONL を読む。書きかけの末尾行（他 process が追記中）は読み飛ばす。
fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> std::io::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let complete = match raw.rfind('\n') {
        Some(i) => &raw[..i],
        None => "",
    };
    complete
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(std::io::Error::other))
        .collect()
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct ObsLedger {
    file: File,
    seq: u64,
}

impl ObsLedger {
    fn append(&mut self, event: &str, detail: Value) -> std::io::Result<()> {
        self.seq += 1;
        let e = ObsEntry {
            seq: self.seq,
            wall_ms: wall_ms(),
            event: event.into(),
            dataset: DATASET.into(),
            detail,
        };
        let mut line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()
    }
}

/// runner の止め方（評価 runner 停止の再現用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStop {
    /// 止めない。
    Never,
    /// 制御 process を SIGKILL して終了を確かめた直後、再起動せずに runner が止まる。
    AfterControlStopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerOutcome {
    /// 制御 process が exit 0 で終わった（scenario 完走）。
    Completed { restarts: u32 },
    /// runner 自身が止まった（[`RunnerStop`]）。次の [`FaultRunner::run`] が続きをやる。
    RunnerStopped,
}

pub struct FaultRunner {
    pub case: FaultCase,
    pub system: String,
    pub cmd: ControlCommand,
    pub ext_ledger: PathBuf,
    pub obs_path: PathBuf,
    /// 制御 process の stdout / stderr を書く dir。
    pub log_dir: PathBuf,
    pub max_restarts: u32,
    pub wall_cap: Duration,
    pub stop: RunnerStop,
}

/// 出来事待ちの確認の刻み（実時間。時間の進みは制御 process の論理時計が決める）。
const WATCH_GAP: Duration = Duration::from_millis(10);

impl FaultRunner {
    /// 制御 process を起こし、fault を観測し、(a) なら SIGKILL → 再起動して完走まで見届ける。
    pub fn run(&self) -> Result<RunnerOutcome, String> {
        if self.case.trigger_event == "durable_wait_saved" {
            return Err(
                "durable_wait_saved requires the formal independent-process A2 harness".into(),
            );
        }
        let prior = read_obs(&self.obs_path).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.obs_path)
            .map_err(|e| e.to_string())?;
        let mut obs = ObsLedger {
            file,
            seq: prior.iter().map(|e| e.seq).max().unwrap_or(0),
        };
        let io = |e: std::io::Error| e.to_string();
        // 前の runner session が終わりの記録なしに居なくなっていれば、評価 runner の停止として残す。
        let sessions = prior.iter().filter(|e| e.event == "runner_started").count();
        let finished = prior
            .iter()
            .filter(|e| e.event == "runner_finished")
            .count();
        let detected = prior.iter().filter(|e| e.event == "runner_stopped").count();
        if sessions > finished + detected {
            obs.append(
                "runner_stopped",
                json!({"session": sessions, "detected_at_resume": true}),
            )
            .map_err(io)?;
        }
        let session = sessions + 1;
        let mut generation = prior
            .iter()
            .filter(|e| e.event == "control_started")
            .count() as u64;
        let mut restarts = prior
            .iter()
            .filter(|e| e.event == "control_started" && e.detail["restart"] == true)
            .count() as u32;
        let mut observed: BTreeSet<u64> = prior
            .iter()
            .filter(|e| e.event == "trigger_observed")
            .filter_map(|e| e.detail["ext_seq"].as_u64())
            .collect();
        let fault = self.case.ext_fault();
        obs.append(
            "runner_started",
            json!({
                "session": session, "system": self.system, "fault": fault, "nth": fault.nth(),
                "owned_resources_only": true,
            }),
        )
        .map_err(io)?;
        let started = Instant::now();
        loop {
            let restart = generation > 0;
            generation += 1;
            let ext_seq_at_start = read_jsonl::<Entry>(&self.ext_ledger)
                .map_err(io)?
                .last()
                .map(|e| e.seq)
                .unwrap_or(0);
            let mut child = self.spawn(generation)?;
            obs.append(
                "control_started",
                json!({
                    "pid": child.id(), "generation": generation, "restart": restart,
                    "ext_seq_at_start": ext_seq_at_start,
                }),
            )
            .map_err(io)?;
            if restart {
                restarts += 1;
            }
            let mut killed = false;
            let status = loop {
                // 終了を先に見てから ledger を末尾まで読む（読み切り→判定）。逆順だと発火直後に完走した
                // 制御 process の fault_fired を読まずに抜ける。
                let exited = child.try_wait().map_err(io)?;
                for e in read_jsonl::<Entry>(&self.ext_ledger).map_err(io)? {
                    if e.event != "fault_fired" || observed.contains(&e.seq) {
                        continue;
                    }
                    let d = e.detail.clone().unwrap_or(Value::Null);
                    if d["fault_id"] != fault.fault_id.as_str() {
                        continue;
                    }
                    observed.insert(e.seq);
                    obs.append(
                        "trigger_observed",
                        json!({
                            "fault_id": fault.fault_id, "kind": d["kind"], "seed": d["seed"],
                            "nth": d["nth"], "event": d["event"], "event_seq": d["event_seq"],
                            "ext_seq": e.seq, "logical_secs": e.logical_secs,
                            "logical_job_id": e.logical_job_id, "pid": child.id(),
                        }),
                    )
                    .map_err(io)?;
                    if fault.kind == FaultKind::StopControl && !killed && exited.is_none() {
                        // 握っている Child にだけ SIGKILL（std の kill は unix で SIGKILL）。
                        child.kill().map_err(io)?;
                        killed = true;
                        obs.append(
                            "control_stopped",
                            json!({
                                "pid": child.id(), "signal": "SIGKILL", "cause": "fault",
                                "fault_id": fault.fault_id, "generation": generation,
                            }),
                        )
                        .map_err(io)?;
                    }
                }
                if let Some(st) = exited {
                    break st;
                }
                if started.elapsed() > self.wall_cap {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("wall cap {:?} exceeded", self.wall_cap));
                }
                std::thread::sleep(WATCH_GAP);
            };
            obs.append(
                "control_exited",
                json!({
                    "pid": child.id(), "generation": generation, "code": status.code(),
                    "signal": exit_signal(&status), "killed_by_runner": killed,
                }),
            )
            .map_err(io)?;
            if status.success() && !killed {
                obs.append("control_completed", json!({"generation": generation}))
                    .map_err(io)?;
                obs.append("runner_finished", json!({"session": session}))
                    .map_err(io)?;
                return Ok(RunnerOutcome::Completed { restarts });
            }
            if killed && self.stop == RunnerStop::AfterControlStopped {
                // runner_finished を書かずに止まる（次の run が runner_stopped を残す）。
                return Ok(RunnerOutcome::RunnerStopped);
            }
            if restarts >= self.max_restarts {
                return Err(format!(
                    "control process failed (status {status:?}) after {restarts} restarts; see {}",
                    self.log_dir.display()
                ));
            }
        }
    }

    fn spawn(&self, generation: u64) -> Result<Child, String> {
        std::fs::create_dir_all(&self.log_dir).map_err(|e| e.to_string())?;
        let log = |name: &str| {
            File::create(self.log_dir.join(format!("control-{generation}.{name}")))
                .map_err(|e| e.to_string())
        };
        let mut c = Command::new(&self.cmd.program);
        c.args(&self.cmd.args)
            .envs(self.cmd.env.iter().map(|(k, v)| (k, v)))
            // 停止 hook は stdin を読んで止まる（runner が居なくなれば EOF で終わる）。
            .stdin(Stdio::piped())
            .stdout(log("stdout")?)
            .stderr(log("stderr")?);
        c.spawn()
            .map_err(|e| format!("spawn {}: {e}", self.cmd.program.display()))
    }
}

fn exit_signal(st: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    st.signal()
}

/// 制御 process 側の停止 hook: stdin が閉じるまで（= runner の SIGKILL か runner の消滅まで）止まる。
/// runner が居なくなって EOF になったら、続きを勝手に進めずに exit する。
pub fn block_until_killed() -> super::ext::StopHook {
    Box::new(|_f: &FaultSpec| {
        let mut sink = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut sink);
        std::process::exit(86);
    })
}

/// ledger だけから求める障害の結果（制御側の自己申告は使わない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FaultReport {
    pub dataset: String,
    pub fault_id: String,
    pub seed: u64,
    pub nth: u64,
    pub trigger_event_seq: u64,
    pub trigger_logical_secs: u64,
    pub trigger_logical_job_id: Option<String>,
    pub logical_jobs: usize,
    pub physical_launches: usize,
    pub consumed: usize,
    /// 同じ論理 job の消費記録が 2 件以上（二重計上）の数。
    pub double_counted: usize,
    pub consume_ignored: usize,
    pub submit_deduped: usize,
    pub notices_sent: usize,
    pub lookups: usize,
    pub control_stops: usize,
    pub control_restarts: usize,
    pub runner_stops: usize,
    pub interventions: usize,
    /// fault 発火（(a) は再起動）後、その論理 job の状態が再確定した最初の外部側 event までの論理秒。
    pub recovery_logical_secs: Option<u64>,
    /// (a) の SIGKILL から再起動した制御 process の起動までの実時間（ms）。
    pub recovery_wall_ms: Option<u64>,
    /// 再実行された worker 作業と、余分な物理起動の job 時間の和（論理秒）。
    pub lost_progress_secs: u64,
}

/// 再実行の数え方に使う node の作業時間（scenario から）。
pub struct WorkTable {
    /// node → (work_secs, post_wait_work_secs)
    pub nodes: BTreeMap<String, (u64, u64)>,
    pub job_secs: u64,
}

pub fn report(
    obs: &[ObsEntry],
    ext: &[Entry],
    worker: &[WorkerEntry],
    work: &WorkTable,
) -> Result<FaultReport, String> {
    let fired = ext
        .iter()
        .find(|e| e.event == "fault_fired")
        .ok_or("no fault_fired in the external ledger")?;
    let d = fired.detail.clone().unwrap_or(Value::Null);
    let n = |ev: &str| ext.iter().filter(|e| e.event == ev).count();
    let o = |ev: &str| obs.iter().filter(|e| e.event == ev).count();
    let logical: BTreeSet<&str> = ext
        .iter()
        .filter(|e| e.event == "physical_launch")
        .filter_map(|e| e.logical_job_id.as_deref())
        .collect();
    let mut consumed_per: BTreeMap<&str, usize> = BTreeMap::new();
    for e in ext.iter().filter(|e| e.event == "consumed") {
        *consumed_per
            .entry(e.logical_job_id.as_deref().unwrap_or(""))
            .or_default() += 1;
    }
    let target = fired.logical_job_id.as_deref();
    // (a) は再起動した制御 process の起動時点の ext seq 以降、(b)(c) は発火以降。
    let restart = obs
        .iter()
        .find(|e| e.event == "control_started" && e.detail["restart"] == true);
    let from_seq = restart
        .and_then(|r| r.detail["ext_seq_at_start"].as_u64())
        .unwrap_or(fired.seq);
    let recovered = ext.iter().find(|e| {
        e.seq > from_seq
            && e.logical_job_id.as_deref() == target
            && matches!(
                e.event.as_str(),
                "lookup" | "submit_deduped" | "external_wait_started" | "consumed"
            )
    });
    let recovery_logical_secs =
        recovered.map(|e| e.logical_secs.saturating_sub(fired.logical_secs));
    let stopped = obs.iter().find(|e| e.event == "control_stopped");
    let recovery_wall_ms = match (stopped, restart) {
        (Some(s), Some(r)) => Some(r.wall_ms.saturating_sub(s.wall_ms)),
        _ => None,
    };
    // 再実行: 同じ (campaign, node, 位相) の開始が 2 件以上。
    let mut starts: BTreeMap<(String, String, bool), u64> = BTreeMap::new();
    for e in worker {
        let post = match e.event.as_str() {
            "post_wait_started" => true,
            "node_started" => e.detail.as_ref().is_some_and(|d| d["phase"] == "post"),
            _ => continue,
        };
        *starts
            .entry((e.campaign.clone(), e.node.clone(), post))
            .or_default() += 1;
    }
    let mut lost = 0u64;
    for ((_, node, post), c) in &starts {
        let (w, p) = work.nodes.get(node).copied().unwrap_or((0, 0));
        lost += c.saturating_sub(1) * if *post { p } else { w };
    }
    let launches = n("physical_launch");
    lost += (launches.saturating_sub(logical.len()) as u64) * work.job_secs;
    Ok(FaultReport {
        dataset: DATASET.into(),
        fault_id: d["fault_id"].as_str().unwrap_or_default().to_string(),
        seed: d["seed"].as_u64().unwrap_or_default(),
        nth: d["nth"].as_u64().unwrap_or_default(),
        trigger_event_seq: d["event_seq"].as_u64().unwrap_or_default(),
        trigger_logical_secs: fired.logical_secs,
        trigger_logical_job_id: fired.logical_job_id.clone(),
        logical_jobs: logical.len(),
        physical_launches: launches,
        consumed: n("consumed"),
        double_counted: consumed_per.values().filter(|c| **c > 1).count(),
        consume_ignored: n("consume_ignored"),
        submit_deduped: n("submit_deduped"),
        notices_sent: n("notice_sent"),
        lookups: n("lookup"),
        control_stops: o("control_stopped"),
        control_restarts: obs
            .iter()
            .filter(|e| e.event == "control_started" && e.detail["restart"] == true)
            .count(),
        runner_stops: o("runner_stopped"),
        interventions: o("intervention"),
        recovery_logical_secs,
        recovery_wall_ms,
        lost_progress_secs: lost,
    })
}
