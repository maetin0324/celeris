//! 外部 job の stub サービス（測定専用の所有資源）と独立観測 ledger。
//!
//! 制御側（Celeris / baseline）とは別の JSONL file に、論理 job ID・物理起動 ID・
//! 受理・物理起動・完了・消費・fault 発火を追記する。ledger を読み直せば状態を復元できる。
//! 時間は注入された論理時計（`advance`）だけで進み、sleep は使わない。
//! 実 process は起こさないので、本番 daemon や無関係 job には触れない。
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const DATASET: &str = "control_measured";

/// 注入する障害。`event` の `nth` 件目に到達した時に 1 回だけ発火する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FaultKind {
    /// (b) 受理・物理起動の後で応答を返さない。
    DropResponse,
    /// (c) 完了通知を `extra` 回余分に送る（2 回目以降は +poll_secs ずつ遅れる）。
    DuplicateNotice { extra: u32 },
    /// (a) 制御 process の停止点。`fault_fired` を ledger に確定させてから停止 hook（[`ExtSim::set_stop_hook`]）を
    /// 呼ぶ。hook は制御 process 側が差し、観測側（`dagbench::fault`）が SIGKILL するまで戻らない。
    StopControl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultSpec {
    pub fault_id: String,
    pub kind: FaultKind,
    /// 数える event 名（`external_submit_accepted` / `external_wait_started` / `external_job_completed`）。
    pub event: String,
    pub seed: u64,
    /// event の出現回数 occurrences から `nth = 1 + seed % occurrences` で決める。
    pub occurrences: u64,
}

impl FaultSpec {
    pub fn nth(&self) -> u64 {
        1 + self.seed % self.occurrences.max(1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtConfig {
    pub poll_secs: u64,
    pub job_slots: usize,
    pub faults: Vec<FaultSpec>,
}

/// ledger の 1 行。`seq` は単調増加（読み直しで次の seq を復元する）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub logical_secs: u64,
    pub event: String,
    pub dataset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmitError {
    /// 受理・起動は済んだが応答は届かなかった（再照合で解く）。
    ResponseLost,
    /// job 枠が空いていない（何も起こしていない）。
    NoSlot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitReply {
    pub physical_id: String,
    /// 同じ論理 ID の既存の物理起動に寄せた。
    pub deduped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lookup {
    Unknown,
    Known {
        physical_id: String,
        state: JobState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub logical_job_id: String,
    pub physical_id: String,
    /// 0 が元の通知、1 以降が重複。
    pub copy: u32,
    pub due_secs: u64,
}

#[derive(Debug, Clone)]
struct Job {
    physical_id: String,
    due_secs: u64,
    state: JobState,
    consumed: bool,
}

/// ExtSim の操作面。baseline / CelerisSide はこの trait 越しに外部 stub を使うので、同じ process の
/// [`ExtSim`]（in-process 既定）でも、別 process の ExtSim を持つ IPC 実装（`dagbench::ipc::IpcExt`）でも動く。
/// IPC では切断・期限超過が起こり得るので、全操作が `io::Result`（有界に Err を返す）。
pub trait ExtBackend {
    fn submit(
        &mut self,
        logical: &str,
        job_secs: u64,
    ) -> std::io::Result<Result<SubmitReply, SubmitError>>;
    fn wait_started(&mut self, logical: &str) -> std::io::Result<()>;
    fn lookup(&mut self, logical: &str) -> std::io::Result<Lookup>;
    fn advance(&mut self, now_secs: u64) -> std::io::Result<Vec<Notice>>;
    fn record_consumed(&mut self, logical: &str) -> std::io::Result<bool>;
    fn now(&mut self) -> std::io::Result<u64>;
    fn due_secs(&mut self, logical: &str) -> std::io::Result<Option<u64>>;
}

impl ExtBackend for ExtSim {
    fn submit(
        &mut self,
        logical: &str,
        job_secs: u64,
    ) -> std::io::Result<Result<SubmitReply, SubmitError>> {
        Ok(ExtSim::submit(self, logical, job_secs))
    }
    fn wait_started(&mut self, logical: &str) -> std::io::Result<()> {
        ExtSim::wait_started(self, logical)
    }
    fn lookup(&mut self, logical: &str) -> std::io::Result<Lookup> {
        Ok(ExtSim::lookup(self, logical))
    }
    fn advance(&mut self, now_secs: u64) -> std::io::Result<Vec<Notice>> {
        ExtSim::advance(self, now_secs)
    }
    fn record_consumed(&mut self, logical: &str) -> std::io::Result<bool> {
        ExtSim::record_consumed(self, logical)
    }
    fn now(&mut self) -> std::io::Result<u64> {
        Ok(ExtSim::now(self))
    }
    fn due_secs(&mut self, logical: &str) -> std::io::Result<Option<u64>> {
        Ok(ExtSim::due_secs(self, logical))
    }
}

/// fault 発火時に制御 process 側で呼ぶ停止 hook（[`FaultKind::StopControl`]）。
pub type StopHook = Box<dyn FnMut(&FaultSpec) + Send>;

pub struct ExtSim {
    path: PathBuf,
    stop_hook: Option<StopHook>,
    /// `external_wait_started` を記録済みの論理 job（論理 job ごとに 1 回だけ数える）。
    wait_started: std::collections::BTreeSet<String>,
    file: File,
    cfg: ExtConfig,
    now: u64,
    seq: u64,
    jobs: BTreeMap<String, Job>,
    event_counts: BTreeMap<String, u64>,
    fired: Vec<String>,
    pending_notices: Vec<Notice>,
}

impl ExtSim {
    /// ledger を開く（無ければ作る）。既存なら全行を再生して状態を復元する。
    pub fn open(path: &Path, cfg: ExtConfig) -> std::io::Result<Self> {
        drop_torn_tail(path)?;
        let entries = read_ledger(path)?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut sim = ExtSim {
            path: path.to_path_buf(),
            stop_hook: None,
            wait_started: Default::default(),
            file,
            cfg,
            now: 0,
            seq: 0,
            jobs: BTreeMap::new(),
            event_counts: BTreeMap::new(),
            fired: Vec::new(),
            pending_notices: Vec::new(),
        };
        for e in &entries {
            sim.replay(e);
        }
        Ok(sim)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// [`FaultKind::StopControl`] の発火時に呼ぶ hook を差す（制御 process の中だけで使う）。
    pub fn set_stop_hook(&mut self, hook: StopHook) {
        self.stop_hook = Some(hook);
    }

    fn replay(&mut self, e: &Entry) {
        self.seq = self.seq.max(e.seq);
        self.now = self.now.max(e.logical_secs);
        *self.event_counts.entry(e.event.clone()).or_default() += 1;
        let (Some(l), phys) = (e.logical_job_id.clone(), e.physical_id.clone()) else {
            if e.event == "fault_fired"
                && let Some(id) = e.detail.as_ref().and_then(|d| d["fault_id"].as_str())
            {
                self.fired.push(id.to_string());
            }
            return;
        };
        match e.event.as_str() {
            "physical_launch" => {
                let due = e
                    .detail
                    .as_ref()
                    .and_then(|d| d["due_secs"].as_u64())
                    .unwrap_or(e.logical_secs);
                self.jobs.insert(
                    l,
                    Job {
                        physical_id: phys.unwrap_or_default(),
                        due_secs: due,
                        state: JobState::Running,
                        consumed: false,
                    },
                );
            }
            "external_job_completed" => {
                if let Some(j) = self.jobs.get_mut(&l) {
                    j.state = JobState::Completed;
                }
            }
            "external_wait_started" => {
                self.wait_started.insert(l);
            }
            "consumed" => {
                if let Some(j) = self.jobs.get_mut(&l) {
                    j.consumed = true;
                }
            }
            "fault_fired" => {
                if let Some(id) = e.detail.as_ref().and_then(|d| d["fault_id"].as_str()) {
                    self.fired.push(id.to_string());
                }
            }
            _ => {}
        }
    }

    fn append(
        &mut self,
        event: &str,
        logical: Option<&str>,
        physical: Option<&str>,
        detail: Option<serde_json::Value>,
    ) -> std::io::Result<u64> {
        self.seq += 1;
        let e = Entry {
            seq: self.seq,
            logical_secs: self.now,
            event: event.to_string(),
            dataset: DATASET.to_string(),
            logical_job_id: logical.map(str::to_string),
            physical_id: physical.map(str::to_string),
            detail,
        };
        let mut line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()?;
        *self.event_counts.entry(event.to_string()).or_default() += 1;
        Ok(self.seq)
    }

    /// event に到達した時、該当 fault が未発火で nth 件目なら発火して返す。
    fn reach(&mut self, event: &str, logical: &str) -> std::io::Result<Option<FaultSpec>> {
        let n = self.event_counts.get(event).copied().unwrap_or(0);
        let hit = self
            .cfg
            .faults
            .iter()
            .find(|f| f.event == event && f.nth() == n && !self.fired.contains(&f.fault_id))
            .cloned();
        if let Some(f) = &hit {
            self.fired.push(f.fault_id.clone());
            let event_seq = self.seq;
            self.append(
                "fault_fired",
                Some(logical),
                None,
                Some(serde_json::json!({
                    "fault_id": f.fault_id, "kind": f.kind, "seed": f.seed,
                    "nth": f.nth(), "event": event, "event_seq": event_seq,
                    "logical_secs": self.now,
                })),
            )?;
            if f.kind == FaultKind::StopControl
                && let Some(hook) = self.stop_hook.as_mut()
            {
                hook(f);
            }
        }
        Ok(hit)
    }

    fn running(&self) -> usize {
        self.jobs
            .values()
            .filter(|j| j.state == JobState::Running)
            .count()
    }

    /// 論理 ID（冪等キー）で投入する。`job_secs` は論理秒。
    pub fn submit(&mut self, logical: &str, job_secs: u64) -> Result<SubmitReply, SubmitError> {
        let io = |_| SubmitError::NoSlot;
        if let Some(j) = self.jobs.get(logical) {
            let physical_id = j.physical_id.clone();
            self.append("submit_deduped", Some(logical), Some(&physical_id), None)
                .map_err(io)?;
            return Ok(SubmitReply {
                physical_id,
                deduped: true,
            });
        }
        if self.running() >= self.cfg.job_slots {
            self.append("submit_rejected", Some(logical), None, None)
                .map_err(io)?;
            return Err(SubmitError::NoSlot);
        }
        let physical_id = format!("phys-{}", self.jobs.len() + 1);
        self.append(
            "external_submit_accepted",
            Some(logical),
            Some(&physical_id),
            None,
        )
        .map_err(io)?;
        let due = self.now + job_secs;
        self.jobs.insert(
            logical.to_string(),
            Job {
                physical_id: physical_id.clone(),
                due_secs: due,
                state: JobState::Running,
                consumed: false,
            },
        );
        self.append(
            "physical_launch",
            Some(logical),
            Some(&physical_id),
            Some(serde_json::json!({"due_secs": due, "job_secs": job_secs})),
        )
        .map_err(io)?;
        let fault = self
            .reach("external_submit_accepted", logical)
            .map_err(io)?;
        if matches!(fault.map(|f| f.kind), Some(FaultKind::DropResponse)) {
            return Err(SubmitError::ResponseLost);
        }
        Ok(SubmitReply {
            physical_id,
            deduped: false,
        })
    }

    /// 制御側が外部 job の待機に入った（論理 job ごとに 1 回だけ記録・計数する。再開後の再追跡は数えない）。
    pub fn wait_started(&mut self, logical: &str) -> std::io::Result<()> {
        if !self.wait_started.insert(logical.to_string()) {
            return Ok(());
        }
        let phys = self.jobs.get(logical).map(|j| j.physical_id.clone());
        self.append(
            "external_wait_started",
            Some(logical),
            phys.as_deref(),
            None,
        )?;
        self.reach("external_wait_started", logical)?;
        Ok(())
    }

    /// 冪等キーによる再照合（アクセスも ledger に残す）。
    pub fn lookup(&mut self, logical: &str) -> Lookup {
        let r = match self.jobs.get(logical) {
            None => Lookup::Unknown,
            Some(j) => Lookup::Known {
                physical_id: j.physical_id.clone(),
                state: j.state,
            },
        };
        let phys = match &r {
            Lookup::Known { physical_id, .. } => Some(physical_id.clone()),
            Lookup::Unknown => None,
        };
        let _ = self.append(
            "lookup",
            Some(logical),
            phys.as_deref(),
            Some(serde_json::json!({"result": match &r {
                Lookup::Unknown => "unknown",
                Lookup::Known{state: JobState::Running, ..} => "running",
                Lookup::Known{..} => "completed",
            }})),
        );
        r
    }

    /// 論理時計を進め、期限の来た job を完了させて通知を返す。重複通知は +poll_secs ずつ遅れ、
    /// 遅れた分は `advance` が届いた時に返る。
    pub fn advance(&mut self, now_secs: u64) -> std::io::Result<Vec<Notice>> {
        self.now = self.now.max(now_secs);
        let due: Vec<(String, Job)> = self
            .jobs
            .iter()
            .filter(|(_, j)| j.state == JobState::Running && j.due_secs <= self.now)
            .map(|(k, j)| (k.clone(), j.clone()))
            .collect();
        for (l, j) in due {
            if let Some(m) = self.jobs.get_mut(&l) {
                m.state = JobState::Completed;
            }
            self.append(
                "external_job_completed",
                Some(&l),
                Some(&j.physical_id),
                None,
            )?;
            self.pending_notices.push(Notice {
                logical_job_id: l.clone(),
                physical_id: j.physical_id.clone(),
                copy: 0,
                due_secs: self.now,
            });
            if let Some(FaultSpec {
                kind: FaultKind::DuplicateNotice { extra },
                ..
            }) = self.reach("external_job_completed", &l)?
            {
                for c in 1..=extra {
                    self.pending_notices.push(Notice {
                        logical_job_id: l.clone(),
                        physical_id: j.physical_id.clone(),
                        copy: c,
                        due_secs: self.now + u64::from(c) * self.cfg.poll_secs,
                    });
                }
            }
        }
        let now = self.now;
        let (ready, later): (Vec<_>, Vec<_>) = self
            .pending_notices
            .drain(..)
            .partition(|n| n.due_secs <= now);
        self.pending_notices = later;
        for n in &ready {
            self.append(
                "notice_sent",
                Some(&n.logical_job_id),
                Some(&n.physical_id),
                Some(serde_json::json!({"copy": n.copy})),
            )?;
        }
        Ok(ready)
    }

    /// 制御側が結果を消費した（二重計上の照合用）。何度呼んでも消費記録は論理 job ごとに 1 件。
    /// 初回なら true。
    pub fn record_consumed(&mut self, logical: &str) -> std::io::Result<bool> {
        let Some(j) = self.jobs.get_mut(logical) else {
            return Ok(false);
        };
        let phys = j.physical_id.clone();
        if j.consumed {
            self.append("consume_ignored", Some(logical), Some(&phys), None)?;
            return Ok(false);
        }
        j.consumed = true;
        self.append("consumed", Some(logical), Some(&phys), None)?;
        Ok(true)
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    /// ledger に sync 済みの最後の行の seq（追記は 1 行ごとに `sync_data` するので、返った時点で永続）。
    pub fn ledger_seq(&self) -> u64 {
        self.seq
    }

    /// 論理 job の物理起動の期限（論理秒）。照会と違い ledger には残さない（driver 内部の時計合わせ用）。
    pub fn due_secs(&self, logical: &str) -> Option<u64> {
        self.jobs.get(logical).map(|j| j.due_secs)
    }
}

/// SIGKILL で書きかけのまま残った末尾行（改行で終わっていない行）を捨てる。書き終わっていない追記は
/// 起きなかったものとして扱う（追記の続きが壊れた行に連結しないように、開き直す前に呼ぶ）。
pub fn drop_torn_tail(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read(path)?;
    if raw.is_empty() || raw.ends_with(b"\n") {
        return Ok(());
    }
    let keep = raw.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(keep as u64)
}

pub fn read_ledger(path: &Path) -> std::io::Result<Vec<Entry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).map_err(std::io::Error::other)?);
    }
    Ok(out)
}

/// ledger から数える独立集計（制御側の自己申告を使わない）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LedgerCounts {
    pub physical_launches: usize,
    pub logical_jobs: usize,
    pub consumed: usize,
    pub notices_sent: usize,
    pub lookups: usize,
}

pub fn count(entries: &[Entry]) -> LedgerCounts {
    let n = |ev: &str| entries.iter().filter(|e| e.event == ev).count();
    let logical: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|e| e.event == "physical_launch")
        .filter_map(|e| e.logical_job_id.clone())
        .collect();
    LedgerCounts {
        physical_launches: n("physical_launch"),
        logical_jobs: logical.len(),
        consumed: n("consumed"),
        notices_sent: n("notice_sent"),
        lookups: n("lookup"),
    }
}

// ---- 通知の処理証跡（独立 ledger）----

/// 通知処理 ledger の 1 行。制御側が通知を **消費した** 事実（`role = control_consume`）を持つ。
/// 外部側の独立観測（[`Entry`] の `notice_sent`、observer の `observe`）とは別 file・別 role で、混ぜて数えない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeEntry {
    pub seq: u64,
    pub dataset: String,
    /// `control_consume` 固定（独立観測と区別する）。
    pub role: String,
    /// `celeris` / `baseline`。
    pub side: String,
    /// `notice_received` / `notice_held` / `notice_processed` / `notice_duplicate_suppressed` /
    /// `notice_continuation` / `notice_acked` / `startup_lookup`。
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physical_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

pub const NOTICE_ROLE: &str = "control_consume";

/// 通知の行き先 ID（wait / task / work-unit / run）。baseline には wait が無いので全て空でよい。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoticeIds {
    pub wait_id: Option<String>,
    pub task_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub run_id: Option<String>,
}

/// 通知処理の追記専用 ledger（1 行ごとに sync）。開き直すと全行から「処理済み seq」「消費済み論理 job」
/// 「受信時に解決した ID」を復元するので、再起動後の重複抑止はこの file だけが根拠になる。
pub struct NoticeLedger {
    file: File,
    path: PathBuf,
    side: String,
    seq: u64,
    entries: Vec<NoticeEntry>,
}

impl NoticeLedger {
    pub fn open(path: &Path, side: &str) -> std::io::Result<Self> {
        drop_torn_tail(path)?;
        let entries = read_notice_ledger(path)?;
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            side: side.to_string(),
            seq: entries.iter().map(|e| e.seq).max().unwrap_or(0),
            entries,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 通知 seq を `notice_processed` まで済ませたか（再配送の重複抑止）。
    pub fn processed(&self, notice_seq: u64) -> bool {
        self.entries
            .iter()
            .any(|e| e.event == "notice_processed" && e.notice_seq == Some(notice_seq))
    }

    /// 論理 job の continuation / 消費が済んだか（同じ job の重複通知 copy の抑止）。
    pub fn logical_consumed(&self, logical: &str) -> bool {
        self.entries.iter().any(|e| {
            e.event == "notice_processed"
                && e.logical_job_id.as_deref() == Some(logical)
                && matches!(e.outcome.as_deref(), Some("finished" | "consumed"))
        })
    }

    /// 受信時に解決済みの wait ID（適用と ledger 記録の間で止まった後の再配送で、閉じた wait を引く）。
    pub fn resolved_wait(&self, notice_seq: u64) -> Option<String> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.notice_seq == Some(notice_seq) && e.wait_id.is_some())
            .and_then(|e| e.wait_id.clone())
    }

    /// 同じ seq の受信を以前にも記録したか。
    pub fn received_before(&self, notice_seq: u64) -> bool {
        self.entries
            .iter()
            .any(|e| e.event == "notice_received" && e.notice_seq == Some(notice_seq))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self,
        event: &str,
        notice_seq: Option<u64>,
        copy: Option<u32>,
        logical: Option<&str>,
        physical: Option<&str>,
        ids: &NoticeIds,
        outcome: Option<&str>,
    ) -> std::io::Result<()> {
        self.seq += 1;
        let e = NoticeEntry {
            seq: self.seq,
            dataset: DATASET.to_string(),
            role: NOTICE_ROLE.to_string(),
            side: self.side.clone(),
            event: event.to_string(),
            notice_seq,
            copy,
            logical_job_id: logical.map(str::to_string),
            physical_id: physical.map(str::to_string),
            wait_id: ids.wait_id.clone(),
            task_id: ids.task_id.clone(),
            work_unit_id: ids.work_unit_id.clone(),
            run_id: ids.run_id.clone(),
            outcome: outcome.map(str::to_string),
        };
        let mut line = serde_json::to_string(&e).map_err(std::io::Error::other)?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()?;
        self.entries.push(e);
        Ok(())
    }
}

pub fn read_notice_ledger(path: &Path) -> std::io::Result<Vec<NoticeEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).map_err(std::io::Error::other)?);
    }
    Ok(out)
}

/// 通知 ledger の独立集計（制御側の自己申告を使わない）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NoticeCounts {
    pub received: usize,
    pub held: usize,
    pub processed: usize,
    pub suppressed: usize,
    pub continuations: usize,
    pub acked: usize,
    pub startup_lookups: usize,
}

pub fn count_notices(entries: &[NoticeEntry]) -> NoticeCounts {
    let n = |ev: &str| entries.iter().filter(|e| e.event == ev).count();
    NoticeCounts {
        received: n("notice_received"),
        held: n("notice_held"),
        processed: n("notice_processed"),
        suppressed: n("notice_duplicate_suppressed"),
        continuations: n("notice_continuation"),
        acked: n("notice_acked"),
        startup_lookups: n("startup_lookup"),
    }
}
