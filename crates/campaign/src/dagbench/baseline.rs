//! 合理的な非同期 DAG baseline（公開仕様。`docs/ops/dag-fault-comparison.md` の「baseline 仕様」と同じ内容）。
//!
//! 単一 process・論理時計の実行器で、わざと弱くしない。
//! - 依存解決: 全依存が `done` の node を ready にし、FIFO（scenario の node 定義順）で worker 枠へ割り当てる。
//! - 資源枠: worker 枠（外部 job の投入前後の作業も 1 枠を使う）と外部 job 枠（ext-sim 側が強制）。
//! - 非同期待機: 外部 job を投入したら worker 枠を即返し、`poll_secs` ごとに照会する。待機中も他の node を進める。
//! - 永続 state: 遷移ごとに write-temp → fsync → rename → fsync(dir) の原子的書込みで JSON に残す。
//!   外部投入は「投入する」意図を先に書き（`submitted`）、投入後に結果を書く。
//! - 再開: [`Baseline::open`] が state を読む。`running` の worker 作業は最初からやり直し、
//!   `submitted`（応答不明）と `waiting` は論理 ID（冪等キー）で ext-sim に再照合する。
//!   見つかれば追跡を続け、見つからなければ同じ論理 ID で再投入する（ext-sim が既存の物理起動に寄せる）。
//! - 重複通知: 完了済み・待機でない node への通知は捨てる。消費は ledger 側でも論理 job ごとに 1 件。
//! - 手動復旧: 制御 process が止まったら、同じ state file と ext ledger で再起動するだけで続きから進む。
//!   state が壊れて読めない場合は読み込みエラーにし、勝手に作り直さない。
//!
//! 持たないもの: 複数 process の lease、計画・レビューの再作成、人の受信箱（機構比較の対象外）。
//! 実 LLM・実 job は呼ばない。時間は注入された論理時計で進み、sleep しない。
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::ext::{ExtSim, JobState, Lookup, SubmitError};

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioFile {
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub resources: Resources,
    pub nodes: Vec<NodeSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resources {
    pub worker_slots: usize,
    pub external_job_slots: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeSpec {
    pub id: String,
    pub kind: String,
    pub deps: Vec<String>,
    pub work_secs: u64,
    #[serde(default)]
    pub external: Option<ExternalSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalSpec {
    pub logical_job_id: String,
    pub post_wait_work_secs: u64,
}

pub fn load_scenario(path: &Path, id: &str) -> Result<Scenario, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let f: ScenarioFile = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    f.scenarios
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("scenario {id} not found"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Ready,
    Running,
    /// 投入の意図を書いた後（応答が不明）。再開時は照会で解く。
    Submitted,
    /// 外部 job 枠が空かず、まだ起動していない。
    SubmitPending,
    Waiting,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 通常の worker node。
    Main,
    /// 外部 node の投入前の作業。
    Pre,
    /// 外部 node の完了後の作業。
    Post,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub status: Status,
    pub phase: Phase,
    #[serde(default)]
    pub ready_at: Option<u64>,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub phase_end: Option<u64>,
    #[serde(default)]
    pub done_at: Option<u64>,
    #[serde(default)]
    pub physical_id: Option<String>,
    /// 再開で最初からやり直した回数。
    #[serde(default)]
    pub reruns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub schema: String,
    pub scenario: String,
    pub now: u64,
    #[serde(default)]
    pub next_poll: Option<u64>,
    pub nodes: BTreeMap<String, NodeState>,
    #[serde(default)]
    pub duplicate_notices_ignored: u64,
}

const STATE_SCHEMA: &str = "celeris.dagbench-baseline-state/1";

#[derive(Debug, Clone)]
pub struct BaselineConfig {
    pub poll_secs: u64,
    pub ext_job_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    /// `stop_after_saves` に達して止めた（制御 process 停止の模擬）。state は保存済み。
    Interrupted,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    State(String),
    Stuck(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub struct Baseline {
    scenario: Scenario,
    cfg: BaselineConfig,
    path: PathBuf,
    state: State,
    saves: u64,
    stop_after_saves: Option<u64>,
    progress: Option<(File, u64)>,
}

impl Baseline {
    /// state file があれば読んで再開し、無ければ新規に作る。壊れた state はエラー。
    pub fn open(scenario: Scenario, cfg: BaselineConfig, path: &Path) -> Result<Self, Error> {
        let state = if path.exists() {
            let s: State = serde_json::from_str(&fs::read_to_string(path)?)
                .map_err(|e| Error::State(e.to_string()))?;
            if s.schema != STATE_SCHEMA || s.scenario != scenario.id {
                return Err(Error::State("state is for another scenario/schema".into()));
            }
            s
        } else {
            let nodes = scenario
                .nodes
                .iter()
                .map(|n| {
                    let phase = if n.external.is_some() {
                        Phase::Pre
                    } else {
                        Phase::Main
                    };
                    (
                        n.id.clone(),
                        NodeState {
                            status: Status::Pending,
                            phase,
                            ready_at: None,
                            started_at: None,
                            phase_end: None,
                            done_at: None,
                            physical_id: None,
                            reruns: 0,
                        },
                    )
                })
                .collect();
            State {
                schema: STATE_SCHEMA.into(),
                scenario: scenario.id.clone(),
                now: 0,
                next_poll: None,
                nodes,
                duplicate_notices_ignored: 0,
            }
        };
        let mut b = Baseline {
            scenario,
            cfg,
            path: path.to_path_buf(),
            state,
            saves: 0,
            stop_after_saves: None,
            progress: None,
        };
        // 実行中だった作業は止まった時点で失われているので、最初からやり直す。
        for n in b.state.nodes.values_mut() {
            if n.status == Status::Running {
                n.status = Status::Ready;
                n.reruns += 1;
            }
        }
        Ok(b)
    }

    /// この run で `n` 回目の state 保存の直後に止める（制御 process の停止を模す）。
    pub fn stop_after_saves(&mut self, n: u64) {
        self.stop_after_saves = Some(n);
    }

    /// worker 作業の開始・終了を追記する進捗 log（celeris 側の worker ledger と同じ形、campaign は `c0`）。
    /// 再実行（失われた進捗）を ledger から数えるために使う。既存の file には追記する。
    pub fn set_progress_log(&mut self, path: &Path) -> Result<(), Error> {
        super::ext::drop_torn_tail(path)?;
        let seq = if path.exists() {
            fs::read_to_string(path)?
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count() as u64
        } else {
            0
        };
        let f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        self.progress = Some((f, seq));
        Ok(())
    }

    fn log_progress(&mut self, event: &str, node: &str, phase: Phase) -> Result<(), Error> {
        let now = self.state.now;
        let Some((f, seq)) = self.progress.as_mut() else {
            return Ok(());
        };
        *seq += 1;
        let line = serde_json::json!({
            "seq": *seq, "logical_secs": now, "event": event,
            "dataset": super::ext::DATASET, "campaign": "c0", "node": node,
            "detail": {"phase": phase},
        });
        f.write_all(format!("{line}\n").as_bytes())?;
        f.sync_data()?;
        Ok(())
    }

    /// この run で行った state 保存の回数。
    pub fn saves(&self) -> u64 {
        self.saves
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// 原子的書込み。
    fn save(&mut self) -> Result<bool, Error> {
        let tmp = self.path.with_extension("tmp");
        let mut f = File::create(&tmp)?;
        f.write_all(
            serde_json::to_string_pretty(&self.state)
                .map_err(|e| Error::State(e.to_string()))?
                .as_bytes(),
        )?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        if let Some(dir) = self.path.parent() {
            File::open(dir)?.sync_all()?;
        }
        self.saves += 1;
        Ok(self.stop_after_saves == Some(self.saves))
    }

    fn spec(&self, id: &str) -> &NodeSpec {
        self.scenario
            .nodes
            .iter()
            .find(|n| n.id == id)
            .expect("node id comes from the scenario")
    }

    fn used_workers(&self) -> usize {
        self.state
            .nodes
            .values()
            .filter(|n| n.status == Status::Running)
            .count()
    }

    fn logical_id(&self, node: &str) -> String {
        self.spec(node)
            .external
            .as_ref()
            .map(|e| e.logical_job_id.clone())
            .unwrap_or_else(|| node.to_string())
    }

    /// 完了まで（または停止点まで）進める。外部側の事実は `ext`（独立 ledger）が持つ。
    pub fn run(&mut self, ext: &mut ExtSim) -> Result<Outcome, Error> {
        self.state.now = self.state.now.max(ext.now());
        // 再開時の再照合: 応答不明の投入と待機中の job を論理 ID で確かめる。
        let ids: Vec<String> = self.scenario.nodes.iter().map(|n| n.id.clone()).collect();
        for id in &ids {
            let st = self.state.nodes[id].status;
            if st != Status::Submitted && st != Status::Waiting {
                continue;
            }
            let logical = self.logical_id(id);
            match ext.lookup(&logical) {
                Lookup::Known { physical_id, state } => {
                    ext.wait_started(&logical)?;
                    let n = self.state.nodes.get_mut(id).expect("node");
                    n.physical_id = Some(physical_id);
                    n.status = Status::Waiting;
                    if state == JobState::Completed && self.complete_wait(ext, id)? {
                        return Ok(Outcome::Interrupted);
                    }
                }
                Lookup::Unknown => {
                    self.state.nodes.get_mut(id).expect("node").status = Status::SubmitPending;
                }
            }
        }
        let mut retry_submit = true;
        let mut started: Vec<(String, Phase)> = Vec::new();
        loop {
            let mut changed = false;
            // 依存解決
            for id in &ids {
                if self.state.nodes[id].status != Status::Pending {
                    continue;
                }
                let ok = self
                    .spec(id)
                    .deps
                    .iter()
                    .all(|d| self.state.nodes[d].status == Status::Done);
                if ok {
                    let now = self.state.now;
                    let n = self.state.nodes.get_mut(id).expect("node");
                    n.status = Status::Ready;
                    n.ready_at = Some(now);
                    changed = true;
                }
            }
            // 外部投入の再試行（枠待ちと再照合後の再投入）
            if retry_submit {
                retry_submit = false;
                for id in &ids {
                    if self.state.nodes[id].status == Status::SubmitPending {
                        if self.submit(ext, id)? {
                            return Ok(Outcome::Interrupted);
                        }
                        changed |= self.state.nodes[id].status != Status::SubmitPending;
                    }
                }
            }
            // 割当（FIFO = node 定義順）
            for id in &ids {
                if self.used_workers() >= self.scenario.resources.worker_slots {
                    break;
                }
                if self.state.nodes[id].status != Status::Ready {
                    continue;
                }
                let secs = {
                    let sp = self.spec(id);
                    match (self.state.nodes[id].phase, &sp.external) {
                        (Phase::Post, Some(e)) => e.post_wait_work_secs,
                        _ => sp.work_secs,
                    }
                };
                let now = self.state.now;
                let n = self.state.nodes.get_mut(id).expect("node");
                n.status = Status::Running;
                n.started_at.get_or_insert(now);
                n.phase_end = Some(now + secs);
                let phase = n.phase;
                started.push((id.clone(), phase));
                changed = true;
            }
            if changed {
                let stop = self.save()?;
                for (id, phase) in started.drain(..) {
                    self.log_progress("node_started", &id, phase)?;
                }
                if stop {
                    return Ok(Outcome::Interrupted);
                }
                continue;
            }
            if self.state.nodes.values().all(|n| n.status == Status::Done) {
                return Ok(Outcome::Complete);
            }
            // 次の出来事: worker 作業の終了 or 次の poll 時刻。
            let waiting = self.state.nodes.values().any(|n| {
                matches!(
                    n.status,
                    Status::Waiting | Status::Submitted | Status::SubmitPending
                )
            });
            if waiting && self.state.next_poll.is_none() {
                self.state.next_poll = Some(self.state.now + self.cfg.poll_secs);
            }
            let next_end = self
                .state
                .nodes
                .values()
                .filter(|n| n.status == Status::Running)
                .filter_map(|n| n.phase_end)
                .min();
            let poll = if waiting { self.state.next_poll } else { None };
            let Some(t) = [next_end, poll].into_iter().flatten().min() else {
                return Err(Error::Stuck(
                    "no runnable node and nothing to wait for".into(),
                ));
            };
            self.state.now = t.max(self.state.now);
            let notices = ext.advance(self.state.now)?;
            for nt in notices {
                let hit = ids
                    .iter()
                    .find(|i| self.logical_id(i) == nt.logical_job_id)
                    .cloned();
                match hit {
                    Some(id) if self.state.nodes[&id].status == Status::Waiting => {
                        if self.complete_wait(ext, &id)? {
                            return Ok(Outcome::Interrupted);
                        }
                    }
                    _ => self.state.duplicate_notices_ignored += 1,
                }
            }
            if poll == Some(t) {
                self.state.next_poll = Some(t + self.cfg.poll_secs);
                retry_submit = true;
                for id in &ids {
                    if self.state.nodes[id].status == Status::Waiting {
                        let l = self.logical_id(id);
                        if matches!(
                            ext.lookup(&l),
                            Lookup::Known {
                                state: JobState::Completed,
                                ..
                            }
                        ) && self.complete_wait(ext, id)?
                        {
                            return Ok(Outcome::Interrupted);
                        }
                    }
                }
            }
            // 作業の終了
            let now = self.state.now;
            let mut finished: Vec<(String, Phase)> = Vec::new();
            for id in &ids {
                let n = &self.state.nodes[id];
                if n.status != Status::Running || n.phase_end.is_none_or(|e| e > now) {
                    continue;
                }
                let phase = n.phase;
                if phase == Phase::Pre {
                    let n = self.state.nodes.get_mut(id).expect("node");
                    n.status = Status::SubmitPending;
                    n.phase_end = None;
                    if self.submit(ext, id)? {
                        return Ok(Outcome::Interrupted);
                    }
                } else {
                    let n = self.state.nodes.get_mut(id).expect("node");
                    n.status = Status::Done;
                    n.done_at = Some(now);
                    n.phase_end = None;
                    finished.push((id.clone(), phase));
                }
            }
            if !self
                .state
                .nodes
                .values()
                .any(|n| n.status == Status::Waiting)
            {
                // 待機が無ければ poll 時刻は次の待機開始で決め直す。
                let any = self
                    .state
                    .nodes
                    .values()
                    .any(|n| matches!(n.status, Status::Submitted | Status::SubmitPending));
                if !any {
                    self.state.next_poll = None;
                }
            }
            let stop = self.save()?;
            for (id, phase) in finished {
                self.log_progress("node_finished", &id, phase)?;
            }
            if stop {
                return Ok(Outcome::Interrupted);
            }
        }
    }

    /// 外部 job を論理 ID（冪等キー）で投入する。意図を先に保存し、結果を保存する。
    fn submit(&mut self, ext: &mut ExtSim, id: &str) -> Result<bool, Error> {
        let logical = self.logical_id(id);
        self.state.nodes.get_mut(id).expect("node").status = Status::Submitted;
        if self.save()? {
            return Ok(true);
        }
        let reply = ext.submit(&logical, self.cfg.ext_job_secs);
        if reply.is_ok() {
            ext.wait_started(&logical)?;
        }
        let n = self.state.nodes.get_mut(id).expect("node");
        match reply {
            Ok(r) => {
                n.physical_id = Some(r.physical_id);
                n.status = Status::Waiting;
            }
            // 応答不明: 再開時（または次の poll）に論理 ID で照会して解く。
            Err(SubmitError::ResponseLost) => {
                n.status = Status::Submitted;
                if self.state.next_poll.is_none() {
                    self.state.next_poll = Some(self.state.now + self.cfg.poll_secs);
                }
                // 同じ run の中でも次の poll で再照合する。
                self.save()?;
                return self.reconcile(ext, id);
            }
            Err(SubmitError::NoSlot) => n.status = Status::SubmitPending,
        }
        self.save()
    }

    /// 応答不明の投入を照会で解く。見つからなければ再投入に戻す。
    fn reconcile(&mut self, ext: &mut ExtSim, id: &str) -> Result<bool, Error> {
        let logical = self.logical_id(id);
        let found = ext.lookup(&logical);
        if matches!(found, Lookup::Known { .. }) {
            ext.wait_started(&logical)?;
        }
        let n = self.state.nodes.get_mut(id).expect("node");
        match found {
            Lookup::Known { physical_id, .. } => {
                n.physical_id = Some(physical_id);
                n.status = Status::Waiting;
            }
            Lookup::Unknown => n.status = Status::SubmitPending,
        }
        self.save()
    }

    /// 完了した外部 job の結果を消費し、post 作業の ready にする。ledger への消費記録は 1 件。
    fn complete_wait(&mut self, ext: &mut ExtSim, id: &str) -> Result<bool, Error> {
        let logical = self.logical_id(id);
        ext.record_consumed(&logical)?;
        let now = self.state.now;
        let n = self.state.nodes.get_mut(id).expect("node");
        n.status = Status::Ready;
        n.phase = Phase::Post;
        n.ready_at = Some(now);
        self.save()
    }

    /// 全 node の完了時刻の最大（makespan。起点は 0）。
    pub fn makespan(&self) -> Option<u64> {
        self.state
            .nodes
            .values()
            .map(|n| n.done_at)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .max()
    }
}
