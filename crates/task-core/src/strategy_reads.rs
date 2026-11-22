//! ADR 2026-11-22-parent-checkpoint-evaluator D1.1: `cos_operations` の strategy 付きの `task.create` の照会（1 か所）。
//!
//! 子 2 D3.4 の「ある task がどの戦略を実行するか」の引き口。子 5 の gate（`task_api::cos::strategy_gate`）は全件で
//! X(v) の元を読み、親の途中評価の controller は `Some(root)` で root の参照を読む。読み取り専用の接続
//! （`with_read_conn`、ADR-0064 D4）で SELECT だけを行い、表も索引も足さない。
//! `cos_inbox` と同じく store の外に置く: store の module は `size.responsibility-growth` の上限に達しており、
//! store に置くと 1 行の増加でも repo health が落ちる（人の決定 `strategy-read-store-size`、CoS の代答）。
//! この module は store と `strategy_ref` に一方向に依存し、task-core の中でこれを use する module は無い。

use rusqlite::params;

use crate::model::TaskId;
use crate::store::{SqliteStore, StoreError};
use crate::strategy_ref::StrategyRef;

/// 照会の SQL（全件と task で絞る形の両方）。`?1` が NULL なら全件、そうでなければ `target_id = ?1` の行だけ。
/// `sha256`・`task_id` の欠けは空文字にする（子 5 の gate の照会の扱いのまま）。
const STRATEGY_TASK_CREATES: &str = "SELECT target_id, \
     json_extract(payload_json, '$.strategy.task_id'), \
     json_extract(payload_json, '$.strategy.name'), \
     json_extract(payload_json, '$.strategy.sha256'), \
     json_extract(payload_json, '$.strategy.candidate') FROM cos_operations \
     WHERE action = 'task.create' AND state = 'applied' \
     AND json_extract(payload_json, '$.strategy.name') IS NOT NULL \
     AND (?1 IS NULL OR target_id = ?1) \
     ORDER BY created_at, id";

/// `cos_operations` の `action = 'task.create'`・`state = 'applied'`・`payload_json.strategy` のある行の
/// `(target_id, 参照)`。順は `created_at, id`。`None` は全件、`Some(id)` は `target_id = id` の行だけ。
pub fn strategy_task_creates(
    store: &SqliteStore,
    target: Option<TaskId>,
) -> Result<Vec<(String, StrategyRef)>, StoreError> {
    let target = target.map(|id| id.to_string());
    store.with_read_conn(|conn| {
        let mut stmt = conn.prepare(STRATEGY_TASK_CREATES)?;
        let rows = stmt
            .query_map(params![target], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    StrategyRef {
                        task_id: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        name: r.get(2)?,
                        sha256: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        candidate: r.get(4)?,
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

#[cfg(test)]
#[path = "strategy_reads_tests.rs"]
mod tests;
