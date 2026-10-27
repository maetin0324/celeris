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

use crate::manifest::{Cap, ConfigId, TokenUsage};

pub const LEDGER_FILE: &str = "ledger.jsonl";
pub const LEDGER_SCHEMA: &str = "celeris.campaign-ledger/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerState {
    Planned,
    Submitted,
    Waiting,
    /// 投入の結果が不明（POST 応答喪失など）。再 submit せず、冪等キーの照会で追跡を続ける。
    SubmitUnknown,
    /// cancel を要求した（または失敗した）が、外部の終端をまだ確認していない。terminal でも予約解除でもない。
    CancelPending,
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

/// 実行中に最後に観測した累積使用量。tokens が None は欠測（0 ではない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedUsage {
    pub tokens: Option<TokenUsage>,
    pub wall_secs: u64,
}

/// 投入前に確保した予約額（この試行の実効上限）。外部の終端を確認するまで解除しない。
pub type Reserved = Cap;

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
    /// 投入を試みた時刻（planned にした時）。投入結果が不明でも壁時計はここから数える。旧行は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    /// 最後に観測した累積使用量。通信断の間もここに残る。旧行は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_usage: Option<ObservedUsage>,
    /// 予約額。旧行は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved: Option<Reserved>,
    /// 連続した通信失敗の回数（成功で 0 に戻る）と、最後の失敗の本文。
    #[serde(default)]
    pub comm_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 試行の開始時（planned にした時）に観測した版。契約 revision の manifest の試行だけ。再開しても観測し直さない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<crate::versions::ObservedVersions>,
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

    /// 読み取り専用で開く（他の out の台帳を数えるだけ。末尾の欠けた行も直さない）。
    pub fn open_readonly(out: &Path) -> Result<Self, String> {
        let path = out.join(LEDGER_FILE);
        let mut rows = BTreeMap::new();
        if path.exists() {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let keep = text.rfind('\n').map_or(0, |i| i + 1);
            for (i, l) in text[..keep].lines().enumerate() {
                let r: LedgerRow = serde_json::from_str(l)
                    .map_err(|e| format!("{}: line {}: {e}", path.display(), i + 1))?;
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
        self.plan_with(key, workspace, routing, None)
    }

    /// `plan` に開始時の観測版を載せる（投入前の同じ 1 行に fsync する）。
    pub fn plan_with(
        &mut self,
        key: &TrialKey,
        workspace: &str,
        routing: Value,
        versions: Option<crate::versions::ObservedVersions>,
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
                started_at: Some(now_secs()),
                observed_usage: None,
                reserved: None,
                comm_failures: 0,
                last_error: None,
                versions,
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

    /// 状態は変えずに参照（子 task・job）だけを更新する。終了確認の直後に受理記録を取り込む時に使う。
    pub fn record_ref(&mut self, key: &TrialKey, t: &TaskRef) -> Result<(), String> {
        self.update(key, |r| Self::apply_ref(r, t))
    }

    pub fn mark_done(&mut self, key: &TrialKey) -> Result<(), String> {
        self.update(key, |r| r.state = LedgerState::Done)
    }

    /// 予約額を確保する（既にあれば何もしない）。投入の前に呼ぶ。
    pub fn reserve(&mut self, key: &TrialKey, cap: Reserved) -> Result<(), String> {
        if self.rows.get(key).is_some_and(|r| r.reserved.is_some()) {
            return Ok(());
        }
        self.update(key, |r| r.reserved = Some(cap))
    }

    /// 投入結果が不明。task_id は無いまま、冪等キーでの照会を続ける。
    pub fn mark_submit_unknown(&mut self, key: &TrialKey, err: &str) -> Result<(), String> {
        self.update(key, |r| {
            r.state = LedgerState::SubmitUnknown;
            r.comm_failures += 1;
            r.last_error = Some(err.into());
        })
    }

    /// 通信失敗の記録（state・task・usage・予約は変えない）。
    pub fn note_comm_failure(&mut self, key: &TrialKey, err: &str) -> Result<(), String> {
        self.update(key, |r| {
            r.comm_failures += 1;
            r.last_error = Some(err.into());
        })
    }

    /// 通信が回復した（失敗の連続を 0 に戻す）。
    pub fn note_comm_ok(&mut self, key: &TrialKey) -> Result<(), String> {
        if self.rows.get(key).is_some_and(|r| r.comm_failures == 0) {
            return Ok(());
        }
        self.update(key, |r| {
            r.comm_failures = 0;
            r.last_error = None;
        })
    }

    /// 観測した累積使用量を残す（変化があるときだけ追記）。
    pub fn record_usage(&mut self, key: &TrialKey, u: ObservedUsage) -> Result<(), String> {
        if self
            .rows
            .get(key)
            .is_some_and(|r| r.observed_usage == Some(u))
        {
            return Ok(());
        }
        self.update(key, |r| r.observed_usage = Some(u))
    }

    /// cancel を要求した。外部の終端を確認する（`mark_cancelled`）まで terminal にしない。
    pub fn mark_cancel_pending(
        &mut self,
        key: &TrialKey,
        reason: &str,
        err: Option<&str>,
    ) -> Result<(), String> {
        self.update(key, |r| {
            r.state = LedgerState::CancelPending;
            r.reason = Some(reason.into());
            if let Some(e) = err {
                r.comm_failures += 1;
                r.last_error = Some(e.into());
            }
        })
    }

    /// 外部の停止を確認した後だけ呼ぶ。
    pub fn mark_cancelled(&mut self, key: &TrialKey, reason: &str) -> Result<(), String> {
        self.update(key, |r| {
            r.state = LedgerState::Cancelled;
            r.reason = Some(reason.into());
            r.comm_failures = 0;
            r.last_error = None;
        })
    }
}
