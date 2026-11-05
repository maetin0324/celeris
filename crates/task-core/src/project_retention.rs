//! 第 4 フェーズ P4-D（ADR 2026-11-05-phase4-p4d-retention）: wake・判断・compare の記録の保持と GC。
//!
//! 読み書きは `store/project_retention.rs`（migration 0117）。events は追記専用の正本のまま触らない。
//! 消してよい行の条件はここの定数と store の SQL が持ち、DB の trigger が二重に守る。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// GC の対象の表（処理の順）。
pub const RETENTION_TABLES: [&str; 3] = [
    "project_wakes",
    "project_controller_decisions",
    "script_compare_log",
];

/// 保持の規則（既定は ADR の値）。秒は「この秒数より古い行だけを候補にする」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// 消費済みの `cron` の wake を残す秒数（既定 7 日）。
    pub wake_keep_secs: u64,
    /// 効果 `none` の判断を残す秒数（既定 30 日）。
    pub decision_keep_secs: u64,
    /// 案件ごとに、状態を問わず新しい順に必ず残す判断の数（既定 256。guard と再開が読む窓と同じ）。
    pub decision_keep_latest: usize,
    /// TS が適用していない compare の記録を残す秒数（既定 30 日）。
    pub compare_keep_secs: u64,
    /// 1 transaction で消す行の上限（書き込みの lock を短く保つ）。
    pub batch_rows: usize,
    /// 1 回の GC で表ごとに消す行の上限（tick を長くしない）。
    pub max_rows_per_pass: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            wake_keep_secs: 7 * 86_400,
            decision_keep_secs: 30 * 86_400,
            decision_keep_latest: 256,
            compare_keep_secs: 30 * 86_400,
            batch_rows: 2_000,
            max_rows_per_pass: 5_000,
        }
    }
}

impl RetentionPolicy {
    /// 0 の値は設定の誤り（「すぐ消す」「上限なし」にしない）。
    pub fn validate(&self) -> Result<(), String> {
        let fields = [
            ("wake_keep_secs", self.wake_keep_secs),
            ("decision_keep_secs", self.decision_keep_secs),
            ("decision_keep_latest", self.decision_keep_latest as u64),
            ("compare_keep_secs", self.compare_keep_secs),
            ("batch_rows", self.batch_rows as u64),
            ("max_rows_per_pass", self.max_rows_per_pass as u64),
        ];
        match fields.iter().find(|(_, v)| *v == 0) {
            Some((name, _)) => Err(format!("{name} must be greater than 0")),
            None => Ok(()),
        }
    }
}

/// 1 つの表の GC の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RetentionTableReport {
    pub table: String,
    /// archive に書いた行。
    pub archived: u64,
    /// 実際に消した行（並行の GC が先に消した行は数えない）。
    pub deleted: u64,
    pub batches: u64,
    pub archive_paths: Vec<String>,
}

/// 1 回の GC の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RetentionReport {
    pub tables: Vec<RetentionTableReport>,
}

impl RetentionReport {
    pub fn deleted(&self) -> u64 {
        self.tables.iter().map(|t| t.deleted).sum()
    }
}

/// `project_retention_runs` の 1 行（GC の追記専用の記録）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionRun {
    pub id: i64,
    pub ts: String,
    pub actor: String,
    pub table_name: String,
    pub archived: u64,
    pub deleted: u64,
    pub first_key: Option<String>,
    pub last_key: Option<String>,
    pub cutoff: String,
    pub archive_path: Option<String>,
    pub archive_sha256: Option<String>,
}

/// 表の行数と DB の大きさ（可視化）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RetentionStats {
    pub wakes_total: u64,
    pub wakes_pending: u64,
    pub wakes_consumed_cron: u64,
    pub decisions_total: u64,
    /// `effect_state` ごとの判断の数。
    pub decisions_by_effect: BTreeMap<String, u64>,
    pub compare_log_total: u64,
    /// TS が適用した compare の記録（GC は消さない）。
    pub compare_log_ts_applied: u64,
    pub page_size: u64,
    pub page_count: u64,
    /// 消した行の空き page（file は縮まず、次の書き込みが使う）。
    pub freelist_count: u64,
    /// 新しい順の GC の記録（最大 10 件）。
    pub last_runs: Vec<RetentionRun>,
}

/// 案件 1 件の行数（`GET …/controller` の `summary.retention`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectRetentionCounts {
    pub wakes_total: u64,
    pub wakes_pending: u64,
    pub decisions_total: u64,
    pub decisions_effect_none: u64,
    pub decisions_pending: u64,
}
