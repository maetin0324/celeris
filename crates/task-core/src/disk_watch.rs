//! ADR 2026-10-07-build-tmp-hygiene D4.4: ディスク使用率の監視の状態（表 `disk_watch_state`。migration 0058）。
//!
//! path ごとに 1 行（`level`・`since`・`last_pct`・`last_notified_at`）。状態は daemon のメモリに持たず、
//! ここに置く。どの使用率でどの level にするか・いつ通知するかの判定は `task_dispatch::disk_watch` にあり、
//! ここは型と SQL だけ（LLM は呼ばない）。受信箱の `disk_full` はこの表の `critical` の行から派生する
//! （受信箱は DB に行を持たない派生の一覧。level が下がれば項目は構造的に消える）。

use rusqlite::{Connection, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::feed::{NoticeEvent, NoticeRecordOutcome};
use crate::store::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

/// 使用率の段。宣言順が重さの順（`Unavailable` は測れなかった path で、段の比較には使わない）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DiskLevel {
    Ok,
    Warn,
    Critical,
    /// path が無い・測れない（D4.2: 1 回だけ記録する）。
    Unavailable,
}

impl DiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            DiskLevel::Ok => "ok",
            DiskLevel::Warn => "warn",
            DiskLevel::Critical => "critical",
            DiskLevel::Unavailable => "unavailable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            DiskLevel::Ok,
            DiskLevel::Warn,
            DiskLevel::Critical,
            DiskLevel::Unavailable,
        ]
        .into_iter()
        .find(|l| l.as_str() == s)
    }
}

impl std::fmt::Display for DiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `disk_watch_state` の 1 行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiskWatchState {
    pub path: String,
    pub level: DiskLevel,
    /// 今の level になった時刻。
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub since: OffsetDateTime,
    /// 直近に測った使用率（%）。測れなかったら `None`。
    pub last_pct: Option<f64>,
    /// 直近に通知した時刻（同じ level の再通知は 24 時間に 1 回）。
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>")]
    pub last_notified_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub updated_at: OffsetDateTime,
}

/// `disk_watch_state` の読み書き。実装は `SqliteStore` のみ。
pub trait DiskWatchStore: Send + Sync {
    /// 全 path の状態（path 順）。読み取り接続だけを使う。
    fn disk_watch_states(&self) -> Result<Vec<DiskWatchState>, StoreError>;

    /// 状態を 1 行書き（upsert）、`notice` があれば同じ transaction で通知の束に足す
    /// （`NoticeStore::notice_record` と同じ扱い。同じ `source_key` は 1 回だけ）。
    fn disk_watch_apply(
        &self,
        state: &DiskWatchState,
        notice: Option<&NoticeEvent>,
    ) -> Result<Option<NoticeRecordOutcome>, StoreError>;
}

fn query_states(conn: &Connection) -> Result<Vec<DiskWatchState>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT path, level, since, last_pct, last_notified_at, updated_at \
         FROM disk_watch_state ORDER BY path",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<f64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (path, level, since, last_pct, last_notified_at, updated_at) = row?;
        let level = DiskLevel::parse(&level)
            .ok_or_else(|| StoreError::Invalid(format!("invalid disk watch level: {level}")))?;
        out.push(DiskWatchState {
            path,
            level,
            since: parse_rfc3339(&since)?,
            last_pct,
            last_notified_at: last_notified_at
                .map(|raw| parse_rfc3339(&raw))
                .transpose()?,
            updated_at: parse_rfc3339(&updated_at)?,
        });
    }
    Ok(out)
}

impl DiskWatchStore for SqliteStore {
    fn disk_watch_states(&self) -> Result<Vec<DiskWatchState>, StoreError> {
        self.with_read_conn(query_states)
    }

    fn disk_watch_apply(
        &self,
        state: &DiskWatchState,
        notice: Option<&NoticeEvent>,
    ) -> Result<Option<NoticeRecordOutcome>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let outcome = notice
            .map(|n| crate::feed::record_in_tx(&tx, n))
            .transpose()?;
        let last_notified_at = state.last_notified_at.map(format_rfc3339).transpose()?;
        tx.execute(
            "INSERT INTO disk_watch_state (path, level, since, last_pct, last_notified_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(path) DO UPDATE SET level = excluded.level, since = excluded.since, \
             last_pct = excluded.last_pct, last_notified_at = excluded.last_notified_at, \
             updated_at = excluded.updated_at",
            params![
                state.path,
                state.level.as_str(),
                format_rfc3339(state.since)?,
                state.last_pct,
                last_notified_at,
                format_rfc3339(state.updated_at)?,
            ],
        )?;
        tx.commit()?;
        Ok(outcome)
    }
}
