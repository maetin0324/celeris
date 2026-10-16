//! 試行台帳（追記専用 JSONL）。キー (campaign sha256, problem, arm, trial_no) → task/run ID を
//! 投入『前』に fsync で永続化し、投入後に task_id を追記する。本番 SQLite には書かない。
//! 行は状態の履歴で、キーごとに最後の行が現在値。再起動・応答喪失後はこの台帳から続きを追跡する。
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::manifest::ConfigId;

pub const LEDGER_FILE: &str = "ledger.jsonl";
pub const LEDGER_SCHEMA: &str = "celeris.campaign-ledger/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerState {
    Planned,
    Submitted,
    Waiting,
    Done,
    Cancelled,
}

impl LedgerState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrialKey {
    pub manifest_sha256: String,
    pub problem_id: String,
    pub arm: ConfigId,
    pub trial_no: u32,
    /// 独立反復の番号（1 始まり）。
    #[serde(default = "one")]
    pub rep: u32,
}

fn one() -> u32 {
    1
}

impl TrialKey {
    /// 外部（Celeris の task 投入 API 等）に渡す冪等キー。同じ試行は常に同じ文字列になる。
    pub fn idempotency_key(&self) -> String {
        let mut s = format!(
            "{}\n{}\n{:?}\n{}",
            self.manifest_sha256, self.problem_id, self.arm, self.trial_no
        );
        // rep 1 は従来と同じ文字列（既存の投入との冪等性を保つ）。
        if self.rep != 1 {
            s.push_str(&format!("\nrep{}", self.rep));
        }
        let d = Sha256::digest(s);
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        format!("campaign-{hex}")
    }
}

/// 外部実行（task / run / 子 task / クラスタ job）への参照。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRef {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// 子 task の ID（追跡と打ち切り時の cancel 対象）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_task_ids: Vec<String>,
    /// クラスタ job の ID。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cluster_job_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LedgerRow {
    pub schema: String,
    #[serde(flatten)]
    pub key: TrialKey,
    pub state: LedgerState,
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub child_task_ids: Vec<String>,
    #[serde(default)]
    pub cluster_job_ids: Vec<String>,
    /// 試行ごとに固定した routing 設定（投入前に決めた値）。
    #[serde(default)]
    pub routing: Value,
    /// out からの相対 path。台帳が参照中の間は cleanup しない。
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl LedgerRow {
    pub fn task_ref(&self) -> Option<TaskRef> {
        self.task_id.as_ref().map(|t| TaskRef {
            task_id: t.clone(),
            run_id: self.run_id.clone(),
            child_task_ids: self.child_task_ids.clone(),
            cluster_job_ids: self.cluster_job_ids.clone(),
        })
    }
}

pub struct Ledger {
    path: PathBuf,
    rows: BTreeMap<TrialKey, LedgerRow>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Ledger {
    /// `out/ledger.jsonl` を開く。crash で残った末尾の不完全な行は切り落とす。
    pub fn open(out: &Path) -> Result<Self, String> {
        let path = out.join(LEDGER_FILE);
        let mut rows = BTreeMap::new();
        if path.exists() {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let keep = text.rfind('\n').map_or(0, |i| i + 1);
            if keep < text.len() {
                OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .and_then(|f| f.set_len(keep as u64).and_then(|_| f.sync_all()))
                    .map_err(|e| e.to_string())?;
            }
            for (i, l) in text[..keep].lines().enumerate() {
                let r: LedgerRow = serde_json::from_str(l)
                    .map_err(|e| format!("{LEDGER_FILE} line {}: {e}", i + 1))?;
                rows.insert(r.key.clone(), r);
            }
        }
        Ok(Self { path, rows })
    }

    pub fn get(&self, key: &TrialKey) -> Option<&LedgerRow> {
        self.rows.get(key)
    }

    pub fn rows(&self) -> impl Iterator<Item = &LedgerRow> {
        self.rows.values()
    }

    /// 未完了（done/cancelled 以外）の行。打ち切り時の cancel 対象。
    pub fn unfinished(&self) -> Vec<&LedgerRow> {
        self.rows
            .values()
            .filter(|r| !r.state.is_terminal())
            .collect()
    }

    /// 台帳が参照中（未完了）の workspace（out からの相対 path）。cleanup はこれを消さない。
    pub fn referenced_workspaces(&self) -> Vec<&str> {
        self.unfinished()
            .into_iter()
            .map(|r| r.workspace.as_str())
            .collect()
    }

    pub fn is_referenced(&self, workspace_rel: &str) -> bool {
        self.rows
            .values()
            .any(|r| !r.state.is_terminal() && r.workspace == workspace_rel)
    }

    fn append(&mut self, row: LedgerRow) -> Result<(), String> {
        let mut line = serde_json::to_string(&row).map_err(|e| e.to_string())?;
        line.push('\n');
        let created = !self.path.exists();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        if created && let Some(dir) = self.path.parent() {
            // 新規 file の directory entry も永続化する。
            File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(|e| e.to_string())?;
        }
        self.rows.insert(row.key.clone(), row);
        Ok(())
    }

    /// 投入前に planned 行を永続化する（既にあれば何もしない）。task_id はまだ無い。
    pub fn plan(
        &mut self,
        key: &TrialKey,
        workspace: &str,
        routing: Value,
    ) -> Result<&LedgerRow, String> {
        if !self.rows.contains_key(key) {
            self.append(LedgerRow {
                schema: LEDGER_SCHEMA.into(),
                key: key.clone(),
                state: LedgerState::Planned,
                idempotency_key: key.idempotency_key(),
                task_id: None,
                run_id: None,
                child_task_ids: Vec::new(),
                cluster_job_ids: Vec::new(),
                routing,
                workspace: workspace.into(),
                submitted_at: None,
                reason: None,
            })?;
        }
        self.rows
            .get(key)
            .ok_or_else(|| "ledger row missing".into())
    }

    fn update(&mut self, key: &TrialKey, f: impl FnOnce(&mut LedgerRow)) -> Result<(), String> {
        let mut row = self
            .rows
            .get(key)
            .cloned()
            .ok_or_else(|| format!("no ledger row for {key:?}; plan() first"))?;
        f(&mut row);
        self.append(row)
    }

    fn apply_ref(row: &mut LedgerRow, t: &TaskRef) {
        row.task_id = Some(t.task_id.clone());
        row.run_id = t.run_id.clone();
        row.child_task_ids = t.child_task_ids.clone();
        row.cluster_job_ids = t.cluster_job_ids.clone();
    }

    /// 投入後に task_id を追記する（planned → submitted）。
    pub fn mark_submitted(&mut self, key: &TrialKey, t: &TaskRef) -> Result<(), String> {
        self.update(key, |r| {
            Self::apply_ref(r, t);
            r.state = LedgerState::Submitted;
            r.submitted_at.get_or_insert_with(now_secs);
        })
    }

    /// 外部待ち。子 task / job が増えたらここで追記する。
    pub fn mark_waiting(&mut self, key: &TrialKey, t: &TaskRef) -> Result<(), String> {
        self.update(key, |r| {
            Self::apply_ref(r, t);
            r.state = LedgerState::Waiting;
        })
    }

    pub fn mark_done(&mut self, key: &TrialKey) -> Result<(), String> {
        self.update(key, |r| r.state = LedgerState::Done)
    }

    pub fn mark_cancelled(&mut self, key: &TrialKey, reason: &str) -> Result<(), String> {
        self.update(key, |r| {
            r.state = LedgerState::Cancelled;
            r.reason = Some(reason.into());
        })
    }
}
