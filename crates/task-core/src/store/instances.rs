use rusqlite::{TransactionBehavior, params};
use time::OffsetDateTime;

use crate::instance::{DaemonInstance, InstanceRole, SELECT_INSTANCE, row_to_instance};

use super::{SqliteStore, StoreError, format_rfc3339};

impl SqliteStore {
    // ---- ADR-0040 D4（Phase 47）: celeris のインスタンスの役割（`daemon_instances`）----
    //
    // ここにあるのは「行を読み書きする」だけの操作で、役割を決める規則（誰が active になるか、いつ
    // drain するか）は celeris 側（`celeris::instance`）にある。`verify` のインスタンスはこの表に触れない。
    /// 自分の行を作る（既にあれば上書きする＝同じ `instance_id` で起動し直したとき）。
    /// `handoff_requested_at` / `drained_at` は NULL に戻る。
    pub fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_instances \
             (instance_id, \"release\", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                instance.instance_id,
                instance.release,
                i64::from(instance.pid),
                instance.role.as_str(),
                format_rfc3339(instance.started_at)?,
                format_rfc3339(instance.heartbeat_at)?,
                instance.handoff_requested_at.map(format_rfc3339).transpose()?,
                instance.drained_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        Ok(())
    }

    /// 自分の行の `heartbeat_at` を更新する。行が無ければ `Ok(false)`（呼び出し側は登録し直す）。
    pub fn instance_heartbeat(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET heartbeat_at = ?1 WHERE instance_id = ?2",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    /// `instance_id` の行に `handoff_requested_at` を書く（既に入っていれば**上書きしない**。
    /// 引き継ぎの要求は 1 回だけ）。書いたら `Ok(true)`。
    pub fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET handoff_requested_at = ?1 \
             WHERE instance_id = ?2 AND handoff_requested_at IS NULL",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    /// 役割を変える（`heartbeat_at` も同時に更新する）。行が無ければ `Ok(false)`。
    pub fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET role = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![role.as_str(), format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    /// 手元の run が 0 になったので `drained_at` を書く（役割は `draining` のまま）。行が無ければ `Ok(false)`。
    pub fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let ts = format_rfc3339(at)?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET drained_at = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![ts.clone(), ts, instance_id],
        )?;
        Ok(affected == 1)
    }

    /// 全インスタンスを `started_at` 昇順（同時刻は `instance_id` 昇順）で返す。
    pub fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{SELECT_INSTANCE} ORDER BY started_at ASC, instance_id ASC"
            ))?;
            let rows = stmt.query_map([], row_to_instance)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    /// 1 行消す。無い id は `Ok(false)`。
    pub fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM daemon_instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        Ok(affected == 1)
    }

    /// 他のインスタンスの行のうち `removable` が真のものを消す（`keep` は消さない）。`removable` は
    /// 同じ transaction の中で 1 行ずつ呼ばれる。消した `instance_id` を昇順で返す。
    /// 何を消すかの規則は celeris 側（`celeris::instance::removable_row`）にある。
    pub fn instance_delete_where(
        &self,
        keep: &str,
        removable: &dyn Fn(&DaemonInstance) -> bool,
    ) -> Result<Vec<String>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed: Vec<String> = read_instances(&tx)?
            .into_iter()
            .filter(|row| row.instance_id != keep && removable(row))
            .map(|row| row.instance_id)
            .collect();
        removed.sort();
        for id in &removed {
            tx.execute(
                "DELETE FROM daemon_instances WHERE instance_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// ADR-0040 D4 付記（2026-10-05）: `instance` の行を作る／上書きするが、**同じ transaction の中で**
    /// 全行を `admit` に渡し、`true` を返したときだけ書く。`false` なら何も書かず `Ok(false)`。
    /// 「active になってよいか」の判断と書き込みを 1 つの writer transaction に入れるための口
    /// （判断の中身は celeris 側にある。store は読んで書くだけ）。
    pub fn instance_register_if(
        &self,
        instance: &DaemonInstance,
        admit: &dyn Fn(&[DaemonInstance]) -> bool,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !admit(&read_instances(&tx)?) {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR REPLACE INTO daemon_instances \
             (instance_id, \"release\", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                instance.instance_id,
                instance.release,
                i64::from(instance.pid),
                instance.role.as_str(),
                format_rfc3339(instance.started_at)?,
                format_rfc3339(instance.heartbeat_at)?,
                instance.handoff_requested_at.map(format_rfc3339).transpose()?,
                instance.drained_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// ADR-0040 D4 付記（2026-10-05）: `instance_id` の行の役割を `role` にする。`admit` は
    /// `instance_register_if` と同じく同じ transaction で全行（この行を含む）を見て、`true` のときだけ書く。
    /// 行が無い、または `admit` が `false` なら `Ok(false)`。
    pub fn instance_set_role_if(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
        admit: &dyn Fn(&[DaemonInstance]) -> bool,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !admit(&read_instances(&tx)?) {
            return Ok(false);
        }
        let affected = tx.execute(
            "UPDATE daemon_instances SET role = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![role.as_str(), format_rfc3339(at)?, instance_id],
        )?;
        tx.commit()?;
        Ok(affected == 1)
    }
}

/// 全行を `started_at` 昇順（同時刻は `instance_id` 昇順）で読む（transaction の中から使う）。
fn read_instances(conn: &rusqlite::Connection) -> Result<Vec<DaemonInstance>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "{SELECT_INSTANCE} ORDER BY started_at ASC, instance_id ASC"
    ))?;
    let rows = stmt.query_map([], row_to_instance)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row??);
    }
    Ok(out)
}
