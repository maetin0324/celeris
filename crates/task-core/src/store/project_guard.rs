//! ADR 2026-11-04-project-controller A6: 長時間の運用の store の口（phase の同期・最後の入力の時刻・token の和）。
//! 判定（どの guard が効くか）は `task_dispatch::dispatcher::project_guard`、予算の数え方は
//! `task_ops::project_budget` が持つ。ここは読み書きだけ。

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::project_wake::ControllerPhase;

/// [`SqliteStore::project_controller_sync_phases`] が変えた 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSync {
    pub project_id: String,
    pub from: ControllerPhase,
    pub to: ControllerPhase,
    /// `paused`・`cancelled` にしたときに消費した（評価せずに畳んだ）wake の数。
    pub held_wakes: usize,
}

/// `Project.status`（と `archived_at`）から決まる phase。`None` は「controller の判断に任せる」。
fn phase_of_project(status: &str, archived: bool) -> Option<ControllerPhase> {
    if archived {
        return Some(ControllerPhase::Cancelled);
    }
    match status {
        "paused" => Some(ControllerPhase::Paused),
        "cancelled" | "done" => Some(ControllerPhase::Cancelled),
        _ => None,
    }
}

impl SqliteStore {
    /// A6: controller の phase を `Project.status` に合わせる（D12「`paused`・`cancelled` は `Project.status`
    /// に従う」）。Task の running とは独立に、案件の状態だけを読む。
    /// - 案件が paused → `paused`、cancelled・done・archived → `cancelled`。未消費の wake は `hold:<phase>` で
    ///   消費し、timer（`next_wake_at`・`coalesce_until`）を消す（評価 0。止まっている間に積もった wake で
    ///   再開の後に何度も評価しない）。
    /// - 案件が active（proposed）に戻ったのに phase が `paused`・`cancelled` → `active`、`next_wake_at = now`
    ///   （再開の後に 1 回だけ評価する）。
    /// - mode `off` の案件は触らない（I13）。1 回の transaction で全案件を見る。
    pub fn project_controller_sync_phases(
        &self,
        now: OffsetDateTime,
    ) -> Result<Vec<PhaseSync>, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows: Vec<(String, String, String, bool)> = {
            let mut st = tx.prepare(
                "SELECT s.project_id, s.phase, p.status, p.archived_at IS NOT NULL \
                 FROM project_controller_state s JOIN projects p ON p.id = s.project_id \
                 WHERE s.mode != 'off'",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for (project_id, phase, status, archived) in rows {
            let Some(from) = ControllerPhase::parse(&phase) else {
                continue;
            };
            let wanted = phase_of_project(&status, archived);
            let to = match wanted {
                Some(p) => p,
                None if matches!(from, ControllerPhase::Paused | ControllerPhase::Cancelled) => {
                    ControllerPhase::Active
                }
                None => from,
            };
            let holds = matches!(to, ControllerPhase::Paused | ControllerPhase::Cancelled);
            // 止まっている間に来た wake も評価せずに畳む（毎 tick。再開の後に積もった分で何度も評価しない）。
            let held_wakes = if holds {
                tx.execute(
                    "UPDATE project_wakes SET consumed_by = ?2 \
                     WHERE project_id = ?1 AND consumed_by IS NULL",
                    params![project_id, format!("hold:{}", to.as_str())],
                )?
            } else {
                0
            };
            if to == from {
                continue;
            }
            if holds {
                tx.execute(
                    "UPDATE project_controller_state SET phase = ?2, next_wake_at = NULL, \
                     coalesce_until = NULL, updated_at = ?3, updated_by = 'daemon' \
                     WHERE project_id = ?1",
                    params![project_id, to.as_str(), at],
                )?;
            } else {
                tx.execute(
                    "UPDATE project_controller_state SET phase = ?2, next_wake_at = ?3, \
                     updated_at = ?3, updated_by = 'daemon' WHERE project_id = ?1",
                    params![project_id, to.as_str(), at],
                )?;
            }
            out.push(PhaseSync {
                project_id,
                from,
                to,
                held_wakes,
            });
        }
        tx.commit()?;
        Ok(out)
    }

    /// A6 の TTL: 案件に最後に「外からの入力」（cron 以外の wake。消費済みを含む）が来た時刻。無ければ最初の
    /// 判断の時刻。どちらも無ければ `None`。
    pub fn project_controller_last_input_at(
        &self,
        project_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.with_read_conn(|c| {
            let wake: Option<String> = c
                .query_row(
                    "SELECT MAX(created_at) FROM project_wakes \
                     WHERE project_id = ?1 AND kind != 'cron'",
                    [project_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if wake.is_some() {
                return Ok(wake);
            }
            Ok(c.query_row(
                "SELECT MIN(created_at) FROM project_controller_decisions WHERE project_id = ?1",
                [project_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// A6 の token の予算: 案件の task の run（`started_at >= since`）の `usage_json`。和は
    /// `task_ops::project_budget` が `tree_metrics` と同じ規則（input + output、cache を含めない）で取る。
    pub fn project_run_usage_since(
        &self,
        project_id: &str,
        since: &str,
    ) -> Result<Vec<crate::model::Usage>, StoreError> {
        self.with_read_conn(|c| {
            let mut st = c.prepare(
                "SELECT r.usage_json FROM runs r JOIN tasks t ON t.id = r.task_id \
                 WHERE t.project_id = ?1 AND r.started_at >= ?2 AND r.usage_json IS NOT NULL",
            )?;
            let raw: Vec<String> = st
                .query_map(params![project_id, since], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok(raw
                .iter()
                .filter_map(|s| serde_json::from_str(s).ok())
                .collect())
        })
    }

    /// A6（crash の B5）: 問いの notice（`source_key`）が既に記録されているか。`escalate` は notice と
    /// phase `blocked` を同じ tx で書くので、記録があれば効果は効いている。
    pub fn project_controller_notice_recorded(&self, source_key: &str) -> Result<bool, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM feed_sources WHERE source_key = ?1)",
                [source_key],
                |r| r.get(0),
            )?)
        })
    }
}
