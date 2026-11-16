//! ADR 2026-10-04-multi-objective-model-routing Phase 2: routing の相関欄。
//! `llm_proxy_requests`（0022）の行に、決定・snapshot・run・task・供給元・モデルを結ぶ欄（0048）の型。
//! 行と相関欄は llm-proxy（`log.rs` の `insert_routed`）が 1 文で書く。

use rusqlite::params;

use crate::model::TaskId;

use super::{SqliteStore, StoreError};

/// 要求 1 件の相関欄。`None` は「未記録」（0048 より前の行は全部 `None`）。
/// `request_id` は `llm_proxy_requests.id`、`account` は既存欄（0022）をそのまま使う。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingCorrelation {
    pub decision_id: Option<String>,
    pub snapshot_id: Option<String>,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub source_id: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
}

/// `routing_decided` event のうち task の分（seq 昇順）を引く SQL。0048 の部分索引と同じ式にする
/// （試験は EXPLAIN QUERY PLAN でこの SQL が索引を使うことを確かめる）。
pub(crate) const ROUTING_DECIDED_ROWS_SQL: &str = "SELECT seq FROM events \
     WHERE task_id = ?1 AND json_extract(json,'$.type') = 'routing_decided' ORDER BY seq";

impl SqliteStore {
    /// task の `routing_decided` event の seq（昇順）。task events から決定を引く入口。
    pub fn routing_decided_seqs(&self, task_id: TaskId) -> Result<Vec<i64>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(ROUTING_DECIDED_ROWS_SQL)?;
            let seqs = stmt
                .query_map(params![task_id.to_string()], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(seqs)
        })
    }
}
