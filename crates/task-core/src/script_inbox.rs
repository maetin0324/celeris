//! ADR 2026-11-02-ts-control-layer-boundary 付記 S5-2・S5-6・S5-7: 受信箱 `script_control` の元（migration 0106）。
//!
//! daemon の script-guard（`crates/celeris/src/daemon/script_guard.rs`）が `script_domain_events`・
//! `script_generation_events` を cursor で読み、actor `system:script-startup`・`system:script-guard` の行 1 件に
//! 項目 1 件を作る。ここは型と SQL だけ（LLM は呼ばない）。
//!
//! - 冪等: 項目の id は `<source>-<event id>`（`source` は記録の表）。`INSERT OR IGNORE` なので、cursor が
//!   戻っても同じ event で 2 件作らない。cursor は前へだけ進む。
//! - 閉じる: 人の actor の後の記録が来たら `resolved_at` を書く（行は消さない）。規則は [`ScriptInboxOp`]。
//! - 項目の作成・閉じる・cursor の更新は 1 つの IMMEDIATE transaction（[`ScriptInboxStore::script_inbox_apply`]）。

use rusqlite::{Connection, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::store::{
    ScriptDomainEventRow, ScriptGenerationEventRow, SqliteStore, StoreError, format_rfc3339,
    parse_rfc3339,
};

/// 受信箱に出す自動の記録の actor（S5-2 の起動時の戻し、S5-6・S5-7 の戻し・無効化・停止）。
pub const SCRIPT_INBOX_ACTORS: [&str; 2] = ["system:script-startup", "system:script-guard"];

/// 全体の停止の記録の種類（`script_generation_events` の `type`）。閉じるのは人の解除だけ。
pub const SCRIPT_GUARD_HALTED: &str = "script_guard_halted";

/// 項目の元の記録の表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptInboxSource {
    /// `script_domain_events`（mode の変更）。
    Domain,
    /// `script_generation_events`（generation の遷移、全体の停止）。
    Generation,
}

impl ScriptInboxSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ScriptInboxSource::Domain => "domain",
            ScriptInboxSource::Generation => "generation",
        }
    }

    fn parse(s: &str) -> Result<Self, StoreError> {
        match s {
            "domain" => Ok(Self::Domain),
            "generation" => Ok(Self::Generation),
            other => Err(StoreError::Invalid(format!(
                "invalid script inbox source: {other}"
            ))),
        }
    }

    /// 項目の id（`<source>-<event id>`。URL にそのまま使える）。
    pub fn item_id(self, event_id: u64) -> String {
        format!("{}-{event_id}", self.as_str())
    }
}

/// 表 `script_inbox_items` の 1 行（開いているものは受信箱の `script_control` に出る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ScriptInboxItem {
    /// `<source>-<event id>`。
    pub id: String,
    pub source: ScriptInboxSource,
    pub event_id: u64,
    /// 記録の `type`（`script_domain_mode_changed`・`script_generation_invalidated`・`script_guard_halted` 等）。
    pub event_type: String,
    /// mode の変更なら領域名。generation の記録では `None`。
    pub domain: Option<String>,
    /// 戻した元の generation（mode の変更）か、遷移した generation（全体の停止は `*`）。
    pub generation_id: Option<String>,
    /// 変更の要約（`canary → rust`・`active → invalid` 等）。
    pub change: Option<String>,
    pub actor: String,
    /// 記録の `reason` の全文（code と計数）。
    pub reason: String,
    /// `reason` の先頭の code（`guard_*`・`script_effect_unverified`・起動時の失敗の理由）。
    pub code: String,
    /// 元の記録の時刻。
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub created_at: OffsetDateTime,
}

/// 閉じる操作の元の人の記録。閉じるのはこの記録より前の項目だけ（同じ表は id、別の表は ms の時刻で比べ、
/// 同じ ms は前と見なさない）。
/// cursor を失って古い記録を読み直しても、後の項目を閉じない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInboxResolver {
    pub source: ScriptInboxSource,
    pub event_id: u64,
    pub at: OffsetDateTime,
}

impl ScriptInboxResolver {
    fn by(&self) -> String {
        self.source.item_id(self.event_id)
    }
}

/// 1 回の取り込みで当てる操作（記録の時刻順に当てる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptInboxOp {
    /// 項目を作る（同じ id があれば何もしない）。
    Open(ScriptInboxItem),
    /// 人がその領域の mode を変えた: その領域の項目と、領域を持たない項目（停止を除く）を閉じる。
    ResolveDomain {
        domain: String,
        by: ScriptInboxResolver,
    },
    /// 人がその generation を遷移させた: その generation の項目（停止を除く）を閉じる。
    ResolveGeneration {
        generation_id: String,
        by: ScriptInboxResolver,
    },
    /// 人が全体の停止を解いた: 停止の項目を閉じる。
    ResolveHalt { by: ScriptInboxResolver },
}

/// 1 回の取り込み（cursor は記録の表ごとに読み終えた最大の id）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptInboxBatch {
    pub domain_cursor: u64,
    pub generation_cursor: u64,
    pub ops: Vec<ScriptInboxOp>,
}

/// 記録の表ごとの読み終えた id。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptInboxCursors {
    pub domain: u64,
    pub generation: u64,
}

/// 取り込みの結果（新しく作った項目と閉じた項目の数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScriptInboxApplied {
    pub opened: usize,
    pub resolved: usize,
}

/// `script_inbox_items`・`script_inbox_cursors` の読み書き。実装は `SqliteStore` のみ。
pub trait ScriptInboxStore: Send + Sync {
    /// 開いている項目（古い順）。読み取り接続だけを使う。
    fn script_inbox_open(&self) -> Result<Vec<ScriptInboxItem>, StoreError>;
    /// 読み終えた id（行が無ければ 0）。
    fn script_inbox_cursors(&self) -> Result<ScriptInboxCursors, StoreError>;
    /// 操作を順に当て、cursor を進める（1 つの transaction）。
    fn script_inbox_apply(
        &self,
        batch: &ScriptInboxBatch,
    ) -> Result<ScriptInboxApplied, StoreError>;
    /// `script_domain_events` の `after` より後の行（古い順に `limit` 行。`SqliteStore` の同名 method へ転送）。
    fn script_inbox_domain_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptDomainEventRow>, StoreError>;
    /// `script_generation_events` の `after` より後の行（古い順に `limit` 行）。
    fn script_inbox_generation_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptGenerationEventRow>, StoreError>;
}

fn unix_ms(t: OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// `by` より前の開いている項目のうち `filter` に合うものを閉じる。`filter` は `?6`（`key`。空文字は NULL）と
/// `?7`（停止の記録の種類）を使う。
fn resolve(
    tx: &Connection,
    now: &str,
    by: &ScriptInboxResolver,
    filter: &str,
    key: &str,
) -> Result<usize, StoreError> {
    let key = (!key.is_empty()).then_some(key);
    let sql = format!(
        "UPDATE script_inbox_items SET resolved_at = ?1, resolved_by = ?2 \
         WHERE resolved_at IS NULL \
           AND ((source = ?3 AND event_id < ?4) OR (source <> ?3 AND created_unix_ms < ?5)) \
           AND {filter}"
    );
    Ok(tx.execute(
        &sql,
        params![
            now,
            by.by(),
            by.source.as_str(),
            to_i64(by.event_id)?,
            unix_ms(by.at),
            key,
            SCRIPT_GUARD_HALTED,
        ],
    )?)
}

fn to_i64(v: u64) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| StoreError::Invalid(format!("script inbox id {v}")))
}

fn query_open(conn: &Connection) -> Result<Vec<ScriptInboxItem>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, event_id, event_type, domain, generation_id, change, actor, reason, code, \
         created_at FROM script_inbox_items WHERE resolved_at IS NULL ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            id,
            source,
            event_id,
            event_type,
            domain,
            generation_id,
            change,
            actor,
            reason,
            code,
            created_at,
        ) = row?;
        out.push(ScriptInboxItem {
            id,
            source: ScriptInboxSource::parse(&source)?,
            event_id: u64::try_from(event_id)
                .map_err(|_| StoreError::Invalid(format!("script inbox event id {event_id}")))?,
            event_type,
            domain,
            generation_id,
            change,
            actor,
            reason,
            code,
            created_at: parse_rfc3339(&created_at)?,
        });
    }
    Ok(out)
}

fn query_cursors(conn: &Connection) -> Result<ScriptInboxCursors, StoreError> {
    let mut stmt = conn.prepare("SELECT source, last_event_id FROM script_inbox_cursors")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut out = ScriptInboxCursors::default();
    for row in rows {
        let (source, last) = row?;
        let last = u64::try_from(last)
            .map_err(|_| StoreError::Invalid(format!("script inbox cursor {last}")))?;
        match ScriptInboxSource::parse(&source)? {
            ScriptInboxSource::Domain => out.domain = last,
            ScriptInboxSource::Generation => out.generation = last,
        }
    }
    Ok(out)
}

impl ScriptInboxStore for SqliteStore {
    fn script_inbox_domain_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptDomainEventRow>, StoreError> {
        self.script_domain_events_after(after, limit)
    }

    fn script_inbox_generation_events_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<ScriptGenerationEventRow>, StoreError> {
        self.script_generation_events_after(after, limit)
    }

    fn script_inbox_open(&self) -> Result<Vec<ScriptInboxItem>, StoreError> {
        self.with_read_conn(query_open)
    }

    fn script_inbox_cursors(&self) -> Result<ScriptInboxCursors, StoreError> {
        self.with_read_conn(query_cursors)
    }

    fn script_inbox_apply(
        &self,
        batch: &ScriptInboxBatch,
    ) -> Result<ScriptInboxApplied, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let mut applied = ScriptInboxApplied::default();
        for op in &batch.ops {
            match op {
                ScriptInboxOp::Open(item) => {
                    applied.opened += tx.execute(
                        "INSERT OR IGNORE INTO script_inbox_items \
                         (id, source, event_id, event_type, domain, generation_id, change, actor, reason, \
                          code, created_at, created_unix_ms) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        params![
                            item.id,
                            item.source.as_str(),
                            to_i64(item.event_id)?,
                            item.event_type,
                            item.domain,
                            item.generation_id,
                            item.change,
                            item.actor,
                            item.reason,
                            item.code,
                            format_rfc3339(item.created_at)?,
                            unix_ms(item.created_at),
                        ],
                    )?;
                }
                ScriptInboxOp::ResolveDomain { domain, by } => {
                    applied.resolved += resolve(
                        &tx,
                        &now,
                        by,
                        "(domain = ?6 OR (domain IS NULL AND event_type <> ?7))",
                        domain,
                    )?;
                }
                ScriptInboxOp::ResolveGeneration { generation_id, by } => {
                    applied.resolved += resolve(
                        &tx,
                        &now,
                        by,
                        "generation_id = ?6 AND event_type <> ?7",
                        generation_id,
                    )?;
                }
                ScriptInboxOp::ResolveHalt { by } => {
                    applied.resolved +=
                        resolve(&tx, &now, by, "event_type = ?7 AND ?6 IS NULL", "")?;
                }
            }
        }
        for (source, last) in [
            (ScriptInboxSource::Domain, batch.domain_cursor),
            (ScriptInboxSource::Generation, batch.generation_cursor),
        ] {
            tx.execute(
                "INSERT INTO script_inbox_cursors (source, last_event_id) VALUES (?1, ?2) \
                 ON CONFLICT(source) DO UPDATE SET last_event_id = MAX(last_event_id, excluded.last_event_id)",
                params![source.as_str(), to_i64(last)?],
            )?;
        }
        tx.commit()?;
        Ok(applied)
    }
}
