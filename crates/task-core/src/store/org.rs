use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::model::Task;
use crate::org::{OrgKind, OrgNode};

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

fn browser_settings_snapshot(node: &OrgNode) -> serde_json::Value {
    let browser = node.profile.browser.as_ref();
    serde_json::json!({
        "allowed_domains": browser.map(|b| &b.allowed_domains),
        "allowed_actions": browser.and_then(|b| b.allowed_actions.as_ref()),
        "credential_policy_ids": browser.map(|b| &b.credential_policy_ids),
        "credential_identity_ids": browser.map(|b| &b.credential_identity_ids),
        "harnesses": &node.profile.harnesses,
        "budget": &node.profile.budget,
    })
}

impl SqliteStore {
    // ---- ADR-0046 D7（Phase 59）: `celerisctl org migrate-v2` のための低レベルの書き換え。
    // 通常の経路（`org_upsert` / `update_task`）は状態機械と検証を通すが、移行は「id の付け替え」だけを
    // まとめて行うので、ここに専用の関数を置く（`celerisctl` からしか呼ばない）。

    /// ノード id を参照している行を `from` → `to` に書き換え、書き換えた行の主キーを返す。
    /// `tasks` は `assignee` 列と `json`（正本）の両方を直す。
    pub fn migrate_node_refs(
        &self,
        table: &str,
        column: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<String>, StoreError> {
        if !is_known_node_ref(table, column) {
            return Err(StoreError::Invalid(format!(
                "unknown node reference: {table}.{column}"
            )));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: Vec<String> = {
            let sql = format!("SELECT id FROM {table} WHERE {column} = ?1");
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(params![from], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        for id in &ids {
            set_node_ref(&tx, table, column, id, to)?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// `migrate_node_refs` の逆（`--rollback`）。1 行だけを元の値に戻す。
    pub fn restore_node_ref(
        &self,
        table: &str,
        column: &str,
        id: &str,
        old: &str,
    ) -> Result<(), StoreError> {
        if !is_known_node_ref(table, column) {
            return Err(StoreError::Invalid(format!(
                "unknown node reference: {table}.{column}"
            )));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        set_node_ref(&tx, table, column, id, old)?;
        tx.commit()?;
        Ok(())
    }

    /// `org_nodes` をまるごと差し替える（親が先に来る並びで渡すこと）。1 トランザクション。
    pub fn replace_org_nodes(&self, nodes: &[OrgNode]) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM org_nodes", [])?;
        let mut existing: Vec<OrgNode> = Vec::with_capacity(nodes.len());
        for node in nodes {
            crate::org::validate_upsert(&existing, node)?;
            tx.execute(
                "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
                 profile_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    node.id,
                    node.parent_id,
                    node.name,
                    node.kind.as_str(),
                    node.genre,
                    node.brief,
                    node.position,
                    format_rfc3339(node.created_at)?,
                    format_rfc3339(node.updated_at)?,
                    profile_json(&node.profile)?,
                ],
            )?;
            existing.push(node.clone());
        }
        tx.commit()?;
        Ok(())
    }

    // ---- ADR-0033 D1/D2: 組織・案件の行と型の間の変換（rusqlite の行変換は `rusqlite::Error` しか
    // 返せないので、解析の失敗は内側の `Result<_, StoreError>` に載せて返す）----
    pub(super) fn org_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<OrgNode, StoreError>> {
        let id: String = row.get(0)?;
        let kind_col: String = row.get(3)?;
        let created_at: String = row.get(7)?;
        let updated_at: String = row.get(8)?;
        // ADR-0046 D1（Phase 59）: 10 列目は `profile_json`（NULL なら空の profile）。
        let profile_col: Option<String> = row.get(9)?;
        let Some(kind) = OrgKind::parse(&kind_col) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid org node kind in org_nodes: {kind_col}"
            ))));
        };
        let profile = match profile_col.as_deref() {
            Some(raw) if !raw.trim().is_empty() => {
                match serde_json::from_str::<crate::profile::Profile>(raw) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        return Ok(Err(StoreError::Invalid(format!(
                            "invalid org profile for {id}: {e}"
                        ))));
                    }
                }
            }
            _ => crate::profile::Profile::default(),
        };
        Ok((|| {
            Ok(OrgNode {
                id,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                kind,
                genre: row.get(4)?,
                brief: row.get(5)?,
                profile,
                position: row.get(6)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    pub(super) fn org_list_tx(conn: &Connection) -> Result<Vec<OrgNode>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, name, kind, genre, brief, position, created_at, updated_at, profile_json \
             FROM org_nodes ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], Self::org_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub(super) fn org_list_impl(&self) -> Result<Vec<OrgNode>, StoreError> {
        self.with_read_conn(Self::org_list_tx)
    }

    pub(super) fn org_get_impl(&self, id: &str) -> Result<Option<OrgNode>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, parent_id, name, kind, genre, brief, position, created_at, updated_at, profile_json \
                     FROM org_nodes WHERE id = ?1",
                    params![id],
                    Self::org_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    pub(super) fn org_upsert_impl(&self, node: &OrgNode) -> Result<OrgNode, StoreError> {
        self.org_upsert_with_actor(node, None)
    }

    /// Persist a browser settings change and its org audit event atomically.
    pub fn org_upsert_browser_settings(
        &self,
        node: &OrgNode,
        actor: &str,
    ) -> Result<OrgNode, StoreError> {
        self.org_upsert_with_actor(node, Some(actor))
    }

    fn org_upsert_with_actor(
        &self,
        node: &OrgNode,
        actor: Option<&str>,
    ) -> Result<OrgNode, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = Self::org_list_tx(&tx)?;
        crate::org::validate_upsert(&existing, node)?;
        let previous = existing.iter().find(|n| n.id == node.id);
        let mut stored = node.clone();
        if let Some(previous) = previous {
            stored.created_at = previous.created_at;
        }
        tx.execute(
            "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
             profile_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name, \
             kind = excluded.kind, genre = excluded.genre, brief = excluded.brief, \
             position = excluded.position, updated_at = excluded.updated_at, \
             profile_json = excluded.profile_json",
            params![
                stored.id,
                stored.parent_id,
                stored.name,
                stored.kind.as_str(),
                stored.genre,
                stored.brief,
                stored.position,
                format_rfc3339(stored.created_at)?,
                format_rfc3339(stored.updated_at)?,
                profile_json(&stored.profile)?,
            ],
        )?;
        if let Some(actor) = actor {
            let before = previous.map(browser_settings_snapshot);
            let after = browser_settings_snapshot(&stored);
            tx.execute(
                "INSERT INTO org_browser_events (node_id, ts, actor, before_json, after_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    stored.id,
                    format_rfc3339(stored.updated_at)?,
                    actor,
                    serde_json::to_string(&before)?,
                    serde_json::to_string(&after)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(stored)
    }

    pub(super) fn org_seed_impl(&self, nodes: &[OrgNode]) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut existing = Self::org_list_tx(&tx)?;
        for node in nodes {
            crate::org::validate_upsert(&existing, node)?;
            tx.execute(
                "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
                 profile_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name, \
                 kind = excluded.kind, genre = excluded.genre, brief = excluded.brief, \
                 position = excluded.position, updated_at = excluded.updated_at, \
                 profile_json = excluded.profile_json",
                params![
                    node.id,
                    node.parent_id,
                    node.name,
                    node.kind.as_str(),
                    node.genre,
                    node.brief,
                    node.position,
                    format_rfc3339(node.created_at)?,
                    format_rfc3339(node.updated_at)?,
                    profile_json(&node.profile)?,
                ],
            )?;
            existing.push(node.clone());
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn org_delete_impl(&self, id: &str) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM org_nodes WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        // ADR-0033 D1: 「消すときに仕事を抱えていたら 409」。抱えている＝未終了のタスクの assignee。
        let open_tasks: i64 = tx.query_row(
            &format!(
                "SELECT COUNT(*) FROM tasks WHERE assignee = ?1 AND {}",
                Self::NON_TERMINAL_SQL
            ),
            params![id],
            |row| row.get(0),
        )?;
        if open_tasks > 0 {
            return Err(StoreError::InUse {
                kind: "org node",
                id: id.to_string(),
                detail: format!("{open_tasks} task(s) assigned to it have not finished"),
            });
        }
        let children: i64 = tx.query_row(
            "SELECT COUNT(*) FROM org_nodes WHERE parent_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 {
            return Err(StoreError::InUse {
                kind: "org node",
                id: id.to_string(),
                detail: format!("{children} child node(s) still report to it"),
            });
        }
        tx.execute("DELETE FROM org_nodes WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(true)
    }
}

/// ADR-0046 D7: ノード id を持つ表と列（`celerisctl org migrate-v2` が触ってよいものだけ）。
fn is_known_node_ref(table: &str, column: &str) -> bool {
    matches!(
        (table, column),
        ("tasks", "assignee")
            | ("messages", "node_id")
            | ("reports", "node_id")
            | ("approvals", "node_id")
            | ("standing_rules", "node_id")
    )
}

/// ADR-0046 D7: 1 行のノード id を書き換える。`tasks` は `json`（正本）も直す。
fn set_node_ref(
    tx: &Connection,
    table: &str,
    column: &str,
    id: &str,
    value: &str,
) -> Result<(), StoreError> {
    if table == "tasks" {
        let json: Option<String> = tx
            .query_row("SELECT json FROM tasks WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(json) = json else { return Ok(()) };
        let mut task: Task = serde_json::from_str(&json)?;
        task.assignee = Some(value.to_string());
        let updated = serde_json::to_string(&task)?;
        tx.execute(
            "UPDATE tasks SET assignee = ?1, json = ?2 WHERE id = ?3",
            params![value, updated, id],
        )?;
        return Ok(());
    }
    let sql = format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2");
    tx.execute(&sql, params![value, id])?;
    Ok(())
}

/// ADR-0046 D1（Phase 59）: `org_nodes.profile_json` に書く値。空の profile は NULL
/// （導入前のノードの行と 1 バイトも変わらない）。
pub(super) fn profile_json(
    profile: &crate::profile::Profile,
) -> Result<Option<String>, StoreError> {
    if profile.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(profile)?))
}
