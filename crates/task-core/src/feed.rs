//! 通知（アプリ内の知らせ。ADR-0133 D3。migration 0040）。判断の要らない知らせを、既読と束ねを持つ
//! 一覧として保存する。
//!
//! `crate::notify`（`notifications` 表）は Discord 等への**送り出しの待ち行列**で、これとは別物
//! （ADR-0133 用語「送り出し」）。名前の衝突を避けるため、ここでは `Notice` / `NoticeKind` /
//! `NoticeStore` と呼ぶ。
//!
//! ここにあるのは型と SQL だけで、**どの出来事を通知にするかの判定は `task_ops::feed` にある**
//! （ADR-0133 D3.3）。ストアは LLM を呼ばない。
//!
//! 束ねの規則（D3.1）:
//! - 同じ `source_key`（出来事）は 1 回しか数えない（`feed_sources` の主キー）。
//! - 未読の束は `group_key` ごとに高々 1 行。同じ `group_key` の出来事は件数を 1 増やし、題名・対象を
//!   最新の 1 件で置き換える。
//! - 既読にした束に同じ `group_key` の出来事が来たら、新しい束（行）を作る。

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::store::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

/// 通知（束）の id（ULID）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct NoticeId(#[schemars(with = "String")] pub Ulid);

impl NoticeId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for NoticeId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for NoticeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for NoticeId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self)
    }
}

/// 通知の種類（ADR-0133 D3.1 の 9 種。追加は ADR で）。`Disk` は ADR 2026-10-07-build-tmp-hygiene D4.3。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    TaskDone,
    Report,
    BadNews,
    SecretaryReply,
    Delivery,
    Release,
    CronRun,
    AutoRecovered,
    RequeueLimitNear,
    // ディスク使用率の警告（group_key `disk:<path>`）と target sweep の上限超え（`disk:target_sweep`）。
    Disk,
}

impl NoticeKind {
    pub const ALL: [NoticeKind; 10] = [
        NoticeKind::TaskDone,
        NoticeKind::Report,
        NoticeKind::BadNews,
        NoticeKind::SecretaryReply,
        NoticeKind::Delivery,
        NoticeKind::Release,
        NoticeKind::CronRun,
        NoticeKind::AutoRecovered,
        NoticeKind::RequeueLimitNear,
        NoticeKind::Disk,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NoticeKind::TaskDone => "task_done",
            NoticeKind::Report => "report",
            NoticeKind::BadNews => "bad_news",
            NoticeKind::SecretaryReply => "secretary_reply",
            NoticeKind::Delivery => "delivery",
            NoticeKind::Release => "release",
            NoticeKind::CronRun => "cron_run",
            NoticeKind::AutoRecovered => "auto_recovered",
            NoticeKind::RequeueLimitNear => "requeue_limit_near",
            NoticeKind::Disk => "disk",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for NoticeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 通知の対象（最新の 1 件）。`kind` は `task` / `report` / `release` / `delivery` / `cron_job` 等の
/// 短い名前で、`id` はその領域の id（文字列のまま。領域の型には依存しない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoticeTarget {
    pub kind: String,
    pub id: String,
}

/// 通知に付けるリンク（GUI / web が開く先）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoticeLink {
    pub label: String,
    pub href: String,
}

/// 通知の束（`feed_notices` の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notice {
    pub id: NoticeId,
    pub kind: NoticeKind,
    /// 束ね key（`<kind>:<範囲>`。ADR-0133 D3.3）。
    pub group_key: String,
    /// 最新の 1 件の題名。
    pub title: String,
    /// 最新の 1 件の要約（`count > 1` なら末尾に「ほか n−1 件」）。
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NoticeTarget>,
    #[serde(default)]
    pub links: Vec<NoticeLink>,
    /// 束ねた出来事の件数。
    pub count: u32,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub first_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub last_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(with = "Option<String>")]
    pub read_at: Option<OffsetDateTime>,
}

impl Notice {
    pub fn is_unread(&self) -> bool {
        self.read_at.is_none()
    }
}

/// 束に足す 1 件の出来事（`NoticeStore::notice_record` の入力）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeEvent {
    /// 出来事の key（`event:<seq>` / `report:<id>` / `cron_run:<id>` 等）。同じ key は 1 回だけ数える。
    pub source_key: String,
    pub kind: NoticeKind,
    pub group_key: String,
    pub title: String,
    pub summary: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub target: Option<NoticeTarget>,
    pub links: Vec<NoticeLink>,
    pub at: OffsetDateTime,
}

/// `notice_record` の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeRecordOutcome {
    /// 新しい束を作った。
    Created(NoticeId),
    /// 未読の束の件数を増やした。
    Bundled(NoticeId),
    /// 同じ出来事は記録済み（何もしない）。
    Duplicate(NoticeId),
}

impl NoticeRecordOutcome {
    pub fn notice_id(self) -> NoticeId {
        match self {
            NoticeRecordOutcome::Created(id)
            | NoticeRecordOutcome::Bundled(id)
            | NoticeRecordOutcome::Duplicate(id) => id,
        }
    }
}

/// 一覧の絞り込みとページ（新しい順。`last_at` 降順、同値は id 降順）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoticeQuery {
    pub unread_only: bool,
    /// 空なら全種類。
    pub kinds: Vec<NoticeKind>,
    /// 0 なら既定（50）。上限 500。
    pub limit: usize,
    pub offset: usize,
}

/// 一覧の既定の件数と上限。
pub const NOTICE_PAGE_DEFAULT: usize = 50;
pub const NOTICE_PAGE_MAX: usize = 500;

/// 一覧の 1 ページ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoticePage {
    pub items: Vec<Notice>,
    /// 絞り込みに合う全件数（ページ前）。
    pub total: u64,
}

/// 未読数（全体と種類ごと）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoticeUnreadCount {
    /// 未読の束の数。
    pub total: u64,
    /// 種類ごとの未読の束の数（0 の種類は載せない）。
    pub by_kind: BTreeMap<String, u64>,
}

/// 束の要約に「ほか n−1 件」を決定的に付ける（D3.1）。
pub fn bundled_summary(latest: &str, count: u32) -> String {
    if count <= 1 {
        latest.to_string()
    } else {
        format!("{latest}（ほか {} 件）", count - 1)
    }
}

/// ADR-0133 D3.2: `feed_notices` / `feed_sources` / `feed_cursor` の読み書き。実装は `SqliteStore` のみ。
pub trait NoticeStore: Send + Sync {
    /// 出来事を 1 件束に足す（冪等）。同じ `source_key` は 1 回だけ、同じ `group_key` の未読の束が
    /// あれば件数を増やし、無ければ新しい束を作る。`feed_sources` の挿入と件数の加算は同じ transaction。
    fn notice_record(&self, event: &NoticeEvent) -> Result<NoticeRecordOutcome, StoreError>;

    /// ADR-0133 付記: 複数の出来事の記録と走査位置（`feed_cursor`）の更新を 1 つの書き込み transaction で行う
    /// （同期 1 回の書き込みを 1 回にする。NFS 上の DB では commit の回数がそのまま tick の時間になる）。
    /// 各出来事の扱いは `notice_record` と同じ。戻り値は `events` と同じ順。
    fn notice_record_batch(
        &self,
        events: &[NoticeEvent],
        cursors: &[(&str, String)],
    ) -> Result<Vec<NoticeRecordOutcome>, StoreError>;

    fn notice_get(&self, id: NoticeId) -> Result<Option<Notice>, StoreError>;

    fn notice_list(&self, query: &NoticeQuery) -> Result<NoticePage, StoreError>;

    /// 1 件を既読にする。既に既読・無い id は `Ok(false)`。
    fn notice_mark_read(&self, id: NoticeId, at: OffsetDateTime) -> Result<bool, StoreError>;

    /// 未読を全て（`kinds` が空でなければその種類だけ）既読にし、既読にした件数を返す。
    fn notice_mark_all_read(
        &self,
        kinds: &[NoticeKind],
        at: OffsetDateTime,
    ) -> Result<u64, StoreError>;

    fn notice_unread_count(&self) -> Result<NoticeUnreadCount, StoreError>;

    /// 既読が `before` より前の束（とその出来事の記録）を消し、消した束の数を返す（D3.2 の保持）。
    fn notice_prune_read(&self, before: OffsetDateTime) -> Result<u64, StoreError>;

    /// ADR-0133 付記: `keys` のうち既に `feed_sources` にある `source_key`。読み取り接続だけを使う
    /// （同期は記録済みの出来事に書き込みの transaction を開かない）。
    fn notice_sources_known(
        &self,
        keys: &[String],
    ) -> Result<std::collections::HashSet<String>, StoreError>;

    /// 走査位置（`feed_cursor`）。
    fn feed_cursor_get(&self, name: &str) -> Result<Option<String>, StoreError>;
    fn feed_cursor_set(&self, name: &str, value: &str) -> Result<(), StoreError>;
}

/// `notice_sources_known` の 1 文あたりの key 数。
const KNOWN_CHUNK: usize = 400;

const SELECT_NOTICE: &str = "SELECT id, kind, group_key, title, summary, project_id, task_id, \
     target_kind, target_id, links_json, count, first_at, last_at, read_at FROM feed_notices";

fn row_to_notice(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Notice, StoreError>> {
    let id: String = row.get(0)?;
    let kind_col: String = row.get(1)?;
    let group_key: String = row.get(2)?;
    let title: String = row.get(3)?;
    let summary: String = row.get(4)?;
    let project_id: Option<String> = row.get(5)?;
    let task_id: Option<String> = row.get(6)?;
    let target_kind: Option<String> = row.get(7)?;
    let target_id: Option<String> = row.get(8)?;
    let links_json: String = row.get(9)?;
    let count: i64 = row.get(10)?;
    let first_at: String = row.get(11)?;
    let last_at: String = row.get(12)?;
    let read_at: Option<String> = row.get(13)?;
    Ok((|| {
        let id = id
            .parse::<NoticeId>()
            .map_err(|_| StoreError::Invalid(format!("invalid notice id: {id}")))?;
        let kind = NoticeKind::parse(&kind_col)
            .ok_or_else(|| StoreError::Invalid(format!("invalid notice kind: {kind_col}")))?;
        let links: Vec<NoticeLink> = serde_json::from_str(&links_json)
            .map_err(|e| StoreError::Invalid(format!("invalid notice links_json: {e}")))?;
        let target = match (target_kind, target_id) {
            (Some(kind), Some(id)) => Some(NoticeTarget { kind, id }),
            _ => None,
        };
        Ok(Notice {
            id,
            kind,
            group_key,
            title,
            summary,
            project_id,
            task_id,
            target,
            links,
            count: u32::try_from(count).unwrap_or(u32::MAX),
            first_at: parse_rfc3339(&first_at)?,
            last_at: parse_rfc3339(&last_at)?,
            read_at: read_at.map(|raw| parse_rfc3339(&raw)).transpose()?,
        })
    })())
}

/// `kinds` の絞り込みを `kind IN (...)` に写す（値は `NoticeKind::as_str` だけなので埋め込んでよい）。
fn kinds_clause(kinds: &[NoticeKind]) -> Option<String> {
    if kinds.is_empty() {
        return None;
    }
    let list: Vec<String> = kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
    Some(format!("kind IN ({})", list.join(", ")))
}

/// 1 件の記録（`notice_record` / `notice_record_batch` 共通。呼び出し側が transaction を持つ）。
pub(crate) fn record_in_tx(
    tx: &rusqlite::Transaction<'_>,
    event: &NoticeEvent,
) -> Result<NoticeRecordOutcome, StoreError> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT notice_id FROM feed_sources WHERE source_key = ?1",
            params![event.source_key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(raw) = existing {
        let id = raw
            .parse::<NoticeId>()
            .map_err(|_| StoreError::Invalid(format!("invalid notice id: {raw}")))?;
        return Ok(NoticeRecordOutcome::Duplicate(id));
    }
    let at = format_rfc3339(event.at)?;
    let links_json = serde_json::to_string(&event.links)
        .map_err(|e| StoreError::Invalid(format!("notice links: {e}")))?;
    let (target_kind, target_id) = match &event.target {
        Some(t) => (Some(t.kind.as_str()), Some(t.id.as_str())),
        None => (None, None),
    };
    let open: Option<(String, i64)> = tx
        .query_row(
            "SELECT id, count FROM feed_notices WHERE group_key = ?1 AND read_at IS NULL",
            params![event.group_key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let outcome = match open {
        Some((raw, count)) => {
            let id = raw
                .parse::<NoticeId>()
                .map_err(|_| StoreError::Invalid(format!("invalid notice id: {raw}")))?;
            let count = count.saturating_add(1);
            let summary = bundled_summary(&event.summary, u32::try_from(count).unwrap_or(u32::MAX));
            // 題名・対象は最新の 1 件。時刻が前後して届いても `last_at` は戻さない。
            tx.execute(
                "UPDATE feed_notices SET count = ?2, title = ?3, summary = ?4, \
                 project_id = ?5, task_id = ?6, target_kind = ?7, target_id = ?8, \
                 links_json = ?9, \
                 last_at = CASE WHEN julianday(?10) > julianday(last_at) THEN ?10 ELSE last_at END \
                 WHERE id = ?1",
                params![
                    raw,
                    count,
                    event.title,
                    summary,
                    event.project_id,
                    event.task_id,
                    target_kind,
                    target_id,
                    links_json,
                    at
                ],
            )?;
            NoticeRecordOutcome::Bundled(id)
        }
        None => {
            let id = NoticeId::new();
            tx.execute(
                "INSERT INTO feed_notices (id, kind, group_key, title, summary, project_id, \
                 task_id, target_kind, target_id, links_json, count, first_at, last_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11, ?11)",
                params![
                    id.to_string(),
                    event.kind.as_str(),
                    event.group_key,
                    event.title,
                    event.summary,
                    event.project_id,
                    event.task_id,
                    target_kind,
                    target_id,
                    links_json,
                    at
                ],
            )?;
            NoticeRecordOutcome::Created(id)
        }
    };
    tx.execute(
        "INSERT INTO feed_sources (source_key, notice_id, recorded_at) VALUES (?1, ?2, ?3)",
        params![event.source_key, outcome.notice_id().to_string(), at],
    )?;
    Ok(outcome)
}

impl NoticeStore for SqliteStore {
    fn notice_record(&self, event: &NoticeEvent) -> Result<NoticeRecordOutcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let outcome = record_in_tx(&tx, event)?;
        tx.commit()?;
        Ok(outcome)
    }

    fn notice_record_batch(
        &self,
        events: &[NoticeEvent],
        cursors: &[(&str, String)],
    ) -> Result<Vec<NoticeRecordOutcome>, StoreError> {
        if events.is_empty() && cursors.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut outcomes = Vec::with_capacity(events.len());
        for event in events {
            outcomes.push(record_in_tx(&tx, event)?);
        }
        for (name, value) in cursors {
            tx.execute(
                "INSERT INTO feed_cursor (name, value) VALUES (?1, ?2) \
                 ON CONFLICT(name) DO UPDATE SET value = excluded.value",
                params![name, value],
            )?;
        }
        tx.commit()?;
        Ok(outcomes)
    }

    fn notice_get(&self, id: NoticeId) -> Result<Option<Notice>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    &format!("{SELECT_NOTICE} WHERE id = ?1"),
                    params![id.to_string()],
                    row_to_notice,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn notice_list(&self, query: &NoticeQuery) -> Result<NoticePage, StoreError> {
        let mut conds: Vec<String> = Vec::new();
        if query.unread_only {
            conds.push("read_at IS NULL".to_string());
        }
        if let Some(c) = kinds_clause(&query.kinds) {
            conds.push(c);
        }
        let where_sql = if conds.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conds.join(" AND "))
        };
        let limit = match query.limit {
            0 => NOTICE_PAGE_DEFAULT,
            n => n.min(NOTICE_PAGE_MAX),
        };
        self.with_read_conn(|conn| {
            let total: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM feed_notices{where_sql}"),
                [],
                |row| row.get(0),
            )?;
            let mut stmt = conn.prepare(&format!(
                "{SELECT_NOTICE}{where_sql} ORDER BY last_at DESC, id DESC LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(
                params![
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    i64::try_from(query.offset).unwrap_or(i64::MAX)
                ],
                row_to_notice,
            )?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row??);
            }
            Ok(NoticePage {
                items,
                total: u64::try_from(total).unwrap_or(0),
            })
        })
    }

    fn notice_mark_read(&self, id: NoticeId, at: OffsetDateTime) -> Result<bool, StoreError> {
        let n = self.lock()?.execute(
            "UPDATE feed_notices SET read_at = ?2 WHERE id = ?1 AND read_at IS NULL",
            params![id.to_string(), format_rfc3339(at)?],
        )?;
        Ok(n > 0)
    }

    fn notice_mark_all_read(
        &self,
        kinds: &[NoticeKind],
        at: OffsetDateTime,
    ) -> Result<u64, StoreError> {
        let extra = kinds_clause(kinds)
            .map(|c| format!(" AND {c}"))
            .unwrap_or_default();
        let n = self.lock()?.execute(
            &format!("UPDATE feed_notices SET read_at = ?1 WHERE read_at IS NULL{extra}"),
            params![format_rfc3339(at)?],
        )?;
        Ok(n as u64)
    }

    fn notice_unread_count(&self) -> Result<NoticeUnreadCount, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT kind, COUNT(*) FROM feed_notices WHERE read_at IS NULL GROUP BY kind",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut out = NoticeUnreadCount::default();
            for row in rows {
                let (kind, n) = row?;
                let n = u64::try_from(n).unwrap_or(0);
                out.total += n;
                out.by_kind.insert(kind, n);
            }
            Ok(out)
        })
    }

    fn notice_prune_read(&self, before: OffsetDateTime) -> Result<u64, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let before = format_rfc3339(before)?;
        // `foreign_keys` は off なので、出来事の記録は明示的に消す。
        tx.execute(
            "DELETE FROM feed_sources WHERE notice_id IN (SELECT id FROM feed_notices \
             WHERE read_at IS NOT NULL AND julianday(read_at) < julianday(?1))",
            params![before],
        )?;
        let n = tx.execute(
            "DELETE FROM feed_notices WHERE read_at IS NOT NULL AND julianday(read_at) < julianday(?1)",
            params![before],
        )?;
        tx.commit()?;
        Ok(n as u64)
    }

    fn notice_sources_known(
        &self,
        keys: &[String],
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut known = std::collections::HashSet::new();
        if keys.is_empty() {
            return Ok(known);
        }
        self.with_read_conn(|conn| {
            // 主キー（`source_key`）の索引で引く。変数の上限を超えないよう束ごとに 1 文。
            for chunk in keys.chunks(KNOWN_CHUNK) {
                let marks = vec!["?"; chunk.len()].join(", ");
                let mut stmt = conn.prepare(&format!(
                    "SELECT source_key FROM feed_sources WHERE source_key IN ({marks})"
                ))?;
                let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    row.get::<_, String>(0)
                })?;
                for row in rows {
                    known.insert(row?);
                }
            }
            Ok(known)
        })
    }

    fn feed_cursor_get(&self, name: &str) -> Result<Option<String>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM feed_cursor WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    fn feed_cursor_set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        self.lock()?.execute(
            "INSERT INTO feed_cursor (name, value) VALUES (?1, ?2) \
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![name, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
