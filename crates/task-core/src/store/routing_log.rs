//! ADR 2026-10-04-multi-objective-model-routing Phase 2: routing の相関欄。
//! `llm_proxy_requests`（0022）の行に、決定・snapshot・run・task・供給元・モデルを結ぶ欄（0048）を付ける・読む。
//! 行そのものは llm-proxy（`log.rs`）が書く。ここでは相関欄だけを触る。

use rusqlite::{OptionalExtension, params};

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
    /// 要求 `request_id` の相関欄を書く（既存の行を更新し、欄は丸ごと置き換える）。
    /// 行が無ければ `Ok(false)`。
    pub fn routing_correlation_set(
        &self,
        request_id: &str,
        correlation: &RoutingCorrelation,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE llm_proxy_requests SET decision_id = ?2, snapshot_id = ?3, run_id = ?4, \
             task_id = ?5, source_id = ?6, model = ?7, account = ?8 WHERE id = ?1",
            params![
                request_id,
                correlation.decision_id,
                correlation.snapshot_id,
                correlation.run_id,
                correlation.task_id,
                correlation.source_id,
                correlation.model,
                correlation.account,
            ],
        )?;
        Ok(changed > 0)
    }

    /// 要求 `request_id` の相関欄を読む。行が無ければ `None`。
    pub fn routing_correlation_get(
        &self,
        request_id: &str,
    ) -> Result<Option<RoutingCorrelation>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT decision_id, snapshot_id, run_id, task_id, source_id, model, account \
                     FROM llm_proxy_requests WHERE id = ?1",
                    params![request_id],
                    |row| {
                        Ok(RoutingCorrelation {
                            decision_id: row.get(0)?,
                            snapshot_id: row.get(1)?,
                            run_id: row.get(2)?,
                            task_id: row.get(3)?,
                            source_id: row.get(4)?,
                            model: row.get(5)?,
                            account: row.get(6)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    /// 決定 `decision_id` で記録された要求 id（昇順）。
    pub fn routing_request_ids_for_decision(
        &self,
        decision_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM llm_proxy_requests WHERE decision_id = ?1 ORDER BY id")?;
            let ids = stmt
                .query_map(params![decision_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

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
