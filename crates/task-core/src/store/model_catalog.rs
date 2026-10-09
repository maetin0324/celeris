//! ADR 2026-10-06 D4: `model_catalog` / `model_catalog_overrides` / `model_catalog_discovery`（migration 0052）。
//!
//! 発見結果の反映（`model_catalog_apply`）は 1 トランザクションで、upsert・消失の印・変化 event・発見記録を
//! まとめて書く。取得に失敗した source は `model_catalog_record_failure` で記録だけし、catalog は変えない
//! （消えたと誤認しない）。上書きは別表で、反映は触らない。LLM は呼ばない。

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Tier;
use crate::model::Event;
use crate::model_catalog::assignments::{
    AssignmentView, RoleAssignment, RoleAssignmentReader, RoleMember, tier_from_str, tier_str,
};
use crate::model_catalog::{
    CatalogDelta, CatalogEntry, CatalogOverride, CatalogOverrideRow, CatalogSource,
    DiscoveredModel, DiscoveryRecord, catalog_event_task_id,
};

use super::{SqliteStore, StoreError};

use tier_str as tier_to_str;

fn parse_caps(json: &str) -> serde_json::Value {
    serde_json::from_str(json).unwrap_or_else(|_| serde_json::json!({}))
}

fn read_entries(conn: &Connection) -> Result<Vec<CatalogEntry>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT source, model_id, display_name, first_seen, last_seen, available, capabilities \
         FROM model_catalog ORDER BY source ASC, model_id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (source, model_id, display_name, first_seen, last_seen, available, caps) = row?;
        out.push(CatalogEntry {
            source: CatalogSource(source),
            model_id,
            display_name,
            first_seen,
            last_seen,
            available: available != 0,
            capabilities: parse_caps(&caps),
        });
    }
    Ok(out)
}

fn override_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogOverrideRow> {
    let tier: Option<String> = r.get(3)?;
    Ok(CatalogOverrideRow {
        source: CatalogSource(r.get(0)?),
        model_id: r.get(1)?,
        value: CatalogOverride {
            disabled: r.get::<_, i64>(2)? != 0,
            tier: tier.as_deref().and_then(tier_from_str),
            alias: r.get(4)?,
            note: r.get(5)?,
        },
        updated_at: r.get(6)?,
    })
}

const OVERRIDE_COLUMNS: &str = "source, model_id, disabled, tier, alias, note, updated_at";

/// モデル catalog の読み書き。`TaskStore` の supertrait（ディスパッチャの `Arc<dyn TaskStore>` から使える）。
pub trait ModelCatalogStore: Send + Sync {
    /// `source` の発見結果を反映する。新規は `available = 1`・`first_seen = last_seen = now`、既存は
    /// `last_seen = now`・`available = 1`、今回見えなかった既存は `available = 0`（行は残す）。
    /// 追加・消失・復活があれば `Event::ModelCatalogChanged` を追記する。発見記録（ok）も同じ
    /// トランザクションで書く。
    fn model_catalog_apply(
        &self,
        source: &CatalogSource,
        models: &[DiscoveredModel],
        now: i64,
    ) -> Result<CatalogDelta, StoreError>;
    /// 取得に失敗したことを記録する（catalog は変えない）。直前の成功時の `count` は残す。
    fn model_catalog_record_failure(
        &self,
        source: &CatalogSource,
        error: &str,
        now: i64,
    ) -> Result<(), StoreError>;
    /// 全 entry（source・model_id 昇順）。
    fn model_catalog_list(&self) -> Result<Vec<CatalogEntry>, StoreError>;
    /// 全上書き（source・model_id 昇順）。
    fn model_catalog_overrides(&self) -> Result<Vec<CatalogOverrideRow>, StoreError>;
    fn model_catalog_override_get(
        &self,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<Option<CatalogOverrideRow>, StoreError>;
    /// source ごとの最終発見記録（source 昇順）。
    fn model_catalog_discovery_records(&self) -> Result<Vec<DiscoveryRecord>, StoreError>;
    /// 上書きを置く（あれば置き換える）。catalog に無いモデルにも置ける（発見前に決めておける）。
    fn model_catalog_set_override(
        &self,
        source: &CatalogSource,
        model_id: &str,
        value: &CatalogOverride,
        now: i64,
    ) -> Result<(), StoreError>;
    /// 上書きを消す。行が無ければ `Ok(false)`。
    fn model_catalog_delete_override(
        &self,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<bool, StoreError>;
    /// ADR 2026-10-06 model-role-assignments D1: 全割り当て（source・tier 昇順）。
    fn model_role_assignments(&self) -> Result<Vec<RoleAssignment>, StoreError>;
    /// 割り当てを置く（あれば置き換える）。同じトランザクションで `Event::ModelRoleAssignmentChanged`
    /// （`previous` は置き換え前の model_id）を catalog の疑似 task に追記する。catalog の自動更新は
    /// この表を触らない。
    fn model_role_assignment_set(
        &self,
        source: &CatalogSource,
        tier: Tier,
        model_id: &str,
        note: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<RoleAssignment, StoreError>;
    /// 割り当てを外す。行が無ければ `Ok(false)`（event も書かない）。外したら
    /// `model_id: None, previous: Some(旧)` の event を同じトランザクションで追記する。
    fn model_role_assignment_delete(
        &self,
        source: &CatalogSource,
        tier: Tier,
        actor: &str,
        now: i64,
    ) -> Result<bool, StoreError>;
    /// Atomically replace a role's entire membership, retaining explicitly empty scopes.
    fn model_role_members_replace(
        &self,
        tier: Tier,
        members: &[RoleMember],
        sources: &[CatalogSource],
        actor: &str,
        now: i64,
    ) -> Result<(), StoreError>;
    /// 3 表（割り当て・catalog・上書き）から実効の view を組む（D2）。
    fn model_role_assignment_view(&self) -> Result<AssignmentView, StoreError>;
}

fn assignment_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<RoleAssignment>> {
    let tier: String = r.get(1)?;
    let Some(tier) = tier_from_str(&tier) else {
        return Ok(None);
    };
    Ok(Some(RoleAssignment {
        priority: r.get(6)?,
        source: CatalogSource(r.get(0)?),
        tier,
        model_id: r.get(2)?,
        note: r.get(3)?,
        updated_at: r.get(4)?,
        updated_by: r.get(5)?,
    }))
}

const ASSIGNMENT_COLUMNS: &str = "source, tier, model_id, note, updated_at, updated_by, priority";

fn read_assignments(conn: &Connection) -> Result<Vec<RoleAssignment>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ASSIGNMENT_COLUMNS} FROM model_role_assignments ORDER BY priority ASC, source ASC, model_id ASC, tier ASC"
    ))?;
    let rows = stmt.query_map([], assignment_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        // 知らない tier 名の行（手書きなど）は読み飛ばす。
        if let Some(a) = row? {
            out.push(a);
        }
    }
    Ok(out)
}

impl RoleAssignmentReader for SqliteStore {
    fn assignment_view(&self) -> Result<AssignmentView, String> {
        self.model_role_assignment_view().map_err(|e| e.to_string())
    }
}

impl ModelCatalogStore for SqliteStore {
    /// `source` の発見結果を反映する。新規は `available = 1`・`first_seen = last_seen = now`、既存は
    /// `last_seen = now`・`available = 1`、今回見えなかった既存は `available = 0`（行は残す）。
    /// 追加・消失・復活があれば `Event::ModelCatalogChanged` を追記する。発見記録（ok）も同じ
    /// トランザクションで書く。
    fn model_catalog_apply(
        &self,
        source: &CatalogSource,
        models: &[DiscoveredModel],
        now: i64,
    ) -> Result<CatalogDelta, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut existing: BTreeMap<String, bool> = BTreeMap::new();
        {
            let mut stmt =
                tx.prepare("SELECT model_id, available FROM model_catalog WHERE source = ?1")?;
            let rows = stmt.query_map(params![source.as_str()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, available) = row?;
                existing.insert(id, available != 0);
            }
        }
        let mut delta = CatalogDelta {
            source: source.0.clone(),
            ..CatalogDelta::default()
        };
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for model in models {
            if !seen.insert(model.model_id.as_str()) {
                continue;
            }
            let caps = serde_json::to_string(&model.capabilities)?;
            match existing.get(&model.model_id) {
                None => {
                    delta.added.push(model.model_id.clone());
                    tx.execute(
                        "INSERT INTO model_catalog (source, model_id, display_name, first_seen, \
                         last_seen, available, capabilities) VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5)",
                        params![
                            source.as_str(),
                            model.model_id,
                            model.display_name,
                            now,
                            caps
                        ],
                    )?;
                }
                Some(was_available) => {
                    if !was_available {
                        delta.restored.push(model.model_id.clone());
                    }
                    tx.execute(
                        "UPDATE model_catalog SET display_name = ?3, last_seen = ?4, available = 1, \
                         capabilities = ?5 WHERE source = ?1 AND model_id = ?2",
                        params![source.as_str(), model.model_id, model.display_name, now, caps],
                    )?;
                }
            }
        }
        for (id, was_available) in &existing {
            if *was_available && !seen.contains(id.as_str()) {
                delta.removed.push(id.clone());
                tx.execute(
                    "UPDATE model_catalog SET available = 0 WHERE source = ?1 AND model_id = ?2",
                    params![source.as_str(), id],
                )?;
            }
        }
        let count = i64::try_from(seen.len()).unwrap_or(i64::MAX);
        tx.execute(
            "INSERT INTO model_catalog_discovery (source, at, ok, error, count) \
             VALUES (?1, ?2, 1, NULL, ?3) \
             ON CONFLICT(source) DO UPDATE SET at = ?2, ok = 1, error = NULL, count = ?3",
            params![source.as_str(), now, count],
        )?;
        if !delta.is_empty() {
            Self::append_event_tx(
                &tx,
                catalog_event_task_id(),
                &Event::ModelCatalogChanged {
                    source: delta.source.clone(),
                    added: delta.added.clone(),
                    removed: delta.removed.clone(),
                    restored: delta.restored.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(delta)
    }

    /// 取得に失敗したことを記録する（catalog は変えない）。直前の成功時の `count` は残す。
    fn model_catalog_record_failure(
        &self,
        source: &CatalogSource,
        error: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO model_catalog_discovery (source, at, ok, error, count) \
             VALUES (?1, ?2, 0, ?3, 0) \
             ON CONFLICT(source) DO UPDATE SET at = ?2, ok = 0, error = ?3",
            params![source.as_str(), now, error],
        )?;
        Ok(())
    }

    /// 全 entry（source・model_id 昇順）。
    fn model_catalog_list(&self) -> Result<Vec<CatalogEntry>, StoreError> {
        self.with_read_conn(read_entries)
    }

    fn model_catalog_overrides(&self) -> Result<Vec<CatalogOverrideRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {OVERRIDE_COLUMNS} FROM model_catalog_overrides \
                 ORDER BY source ASC, model_id ASC"
            ))?;
            let rows = stmt.query_map([], override_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    fn model_catalog_override_get(
        &self,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<Option<CatalogOverrideRow>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(conn
                .query_row(
                    &format!(
                        "SELECT {OVERRIDE_COLUMNS} FROM model_catalog_overrides \
                         WHERE source = ?1 AND model_id = ?2"
                    ),
                    params![source.as_str(), model_id],
                    override_from_row,
                )
                .optional()?)
        })
    }

    /// 上書きを置く（あれば置き換える）。catalog に無いモデルにも置ける（発見前に決めておける）。
    fn model_catalog_set_override(
        &self,
        source: &CatalogSource,
        model_id: &str,
        value: &CatalogOverride,
        now: i64,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::model_catalog_set_override_tx(&conn, source, model_id, value, now)
    }

    /// 上書きを消す。行が無ければ `Ok(false)`。
    fn model_catalog_delete_override(
        &self,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::model_catalog_delete_override_tx(&conn, source, model_id)
    }

    fn model_role_assignments(&self) -> Result<Vec<RoleAssignment>, StoreError> {
        self.with_read_conn(read_assignments)
    }

    fn model_role_assignment_set(
        &self,
        source: &CatalogSource,
        tier: Tier,
        model_id: &str,
        note: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<RoleAssignment, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let assignment = SqliteStore::model_role_assignment_set_tx(
            &tx, source, tier, model_id, note, actor, now,
        )?;
        tx.commit()?;
        Ok(assignment)
    }

    fn model_role_assignment_delete(
        &self,
        source: &CatalogSource,
        tier: Tier,
        actor: &str,
        _now: i64,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let deleted = SqliteStore::model_role_assignment_delete_tx(&tx, source, tier, actor)?;
        tx.commit()?;
        Ok(deleted)
    }

    fn model_role_members_replace(
        &self,
        tier: Tier,
        members: &[RoleMember],
        sources: &[CatalogSource],
        actor: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::model_role_members_replace_tx(&tx, tier, members, sources, actor, now)?;
        tx.commit()?;
        Ok(())
    }

    fn model_role_assignment_view(&self) -> Result<AssignmentView, StoreError> {
        self.with_read_conn(|conn| {
            let assignments = read_assignments(conn)?;
            let entries = read_entries(conn)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {OVERRIDE_COLUMNS} FROM model_catalog_overrides \
                 ORDER BY source ASC, model_id ASC"
            ))?;
            let rows = stmt.query_map([], override_from_row)?;
            let mut overrides = Vec::new();
            for row in rows {
                overrides.push(row?);
            }
            let mut view = AssignmentView::build(&assignments, &entries, &overrides);
            let mut stmt =
                conn.prepare("SELECT source, tier FROM model_role_scopes ORDER BY source, tier")?;
            let rows = stmt.query_map([], |r| {
                Ok((CatalogSource(r.get(0)?), r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (source, tier) = row?;
                if let Some(tier) = tier_from_str(&tier) {
                    view.managed.push((source, tier));
                }
            }
            Ok(view)
        })
    }

    /// source ごとの最終発見記録（source 昇順）。
    fn model_catalog_discovery_records(&self) -> Result<Vec<DiscoveryRecord>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source, at, ok, error, count FROM model_catalog_discovery ORDER BY source ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(DiscoveryRecord {
                    source: CatalogSource(r.get(0)?),
                    at: r.get(1)?,
                    ok: r.get::<_, i64>(2)? != 0,
                    error: r.get(3)?,
                    count: u32::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }
}

/// 割り当ての書き込みを呼び出し側の transaction の中で行う（ADR 2026-10-09 D5 の CoS 監査経路）。
impl SqliteStore {
    /// `model_catalog_set_override` in the caller's transaction (CoS `model_override.put`).
    pub fn model_catalog_set_override_tx(
        tx: &Connection,
        source: &CatalogSource,
        model_id: &str,
        value: &CatalogOverride,
        now: i64,
    ) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO model_catalog_overrides (source, model_id, disabled, tier, alias, note, \
             updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(source, model_id) DO UPDATE SET disabled = ?3, tier = ?4, alias = ?5, \
             note = ?6, updated_at = ?7",
            params![
                source.as_str(),
                model_id,
                i64::from(value.disabled),
                value.tier.map(tier_to_str),
                value.alias,
                value.note,
                now
            ],
        )?;
        Ok(())
    }

    /// `model_catalog_delete_override` in the caller's transaction (CoS `model_override.delete`).
    pub fn model_catalog_delete_override_tx(
        tx: &Connection,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<bool, StoreError> {
        let n = tx.execute(
            "DELETE FROM model_catalog_overrides WHERE source = ?1 AND model_id = ?2",
            params![source.as_str(), model_id],
        )?;
        Ok(n > 0)
    }

    /// `model_role_members_replace` in the caller's transaction (CoS `model_role.replace`).
    pub fn model_role_members_replace_tx(
        tx: &Connection,
        tier: Tier,
        members: &[RoleMember],
        sources: &[CatalogSource],
        actor: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut unique = std::collections::HashSet::new();
        for m in members {
            if !m.source.is_valid()
                || m.model_id.trim().is_empty()
                || !unique.insert((m.source.as_str(), m.model_id.as_str()))
            {
                return Err(StoreError::Invalid(
                    "invalid or duplicate role membership".into(),
                ));
            }
        }
        let old = read_assignments(tx)?;
        let scopes = sources
            .iter()
            .chain(members.iter().map(|m| &m.source))
            .chain(old.iter().filter(|a| a.tier == tier).map(|a| &a.source));
        for source in scopes {
            tx.execute(
                "INSERT OR IGNORE INTO model_role_scopes (source, tier) VALUES (?1, ?2)",
                params![source.as_str(), tier_str(tier)],
            )?;
        }
        tx.execute(
            "DELETE FROM model_role_assignments WHERE tier = ?1",
            [tier_str(tier)],
        )?;
        for m in members {
            let prior = old
                .iter()
                .find(|a| a.tier == tier && a.source == m.source && a.model_id == m.model_id);
            tx.execute("INSERT INTO model_role_assignments (source, tier, model_id, priority, note, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![m.source.as_str(), tier_str(tier), m.model_id, m.priority, prior.and_then(|a| a.note.as_deref()), now, actor])?;
            Self::append_event_tx(
                tx,
                catalog_event_task_id(),
                &Event::ModelRoleAssignmentChanged {
                    source: m.source.0.clone(),
                    tier,
                    model_id: Some(m.model_id.clone()),
                    previous: prior.map(|a| a.model_id.clone()),
                    actor: actor.into(),
                },
            )?;
        }
        for a in old.iter().filter(|a| a.tier == tier) {
            if !members
                .iter()
                .any(|m| m.source == a.source && m.model_id == a.model_id)
            {
                Self::append_event_tx(
                    tx,
                    catalog_event_task_id(),
                    &Event::ModelRoleAssignmentChanged {
                        source: a.source.0.clone(),
                        tier,
                        model_id: None,
                        previous: Some(a.model_id.clone()),
                        actor: actor.into(),
                    },
                )?;
            }
        }
        // Config-only scopes also need an event when cleared, so cached routing catalogs refresh.
        for source in sources {
            if !old.iter().any(|a| a.tier == tier && &a.source == source)
                && !members.iter().any(|m| &m.source == source)
            {
                Self::append_event_tx(
                    tx,
                    catalog_event_task_id(),
                    &Event::ModelRoleAssignmentChanged {
                        source: source.0.clone(),
                        tier,
                        model_id: None,
                        previous: None,
                        actor: actor.into(),
                    },
                )?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn model_role_assignment_set_tx(
        tx: &Connection,
        source: &CatalogSource,
        tier: Tier,
        model_id: &str,
        note: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<RoleAssignment, StoreError> {
        let previous: Option<String> = tx
            .query_row(
                "SELECT model_id FROM model_role_assignments WHERE source = ?1 AND tier = ?2",
                params![source.as_str(), tier_str(tier)],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "DELETE FROM model_role_assignments WHERE source = ?1 AND tier = ?2",
            params![source.as_str(), tier_str(tier)],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO model_role_scopes (source, tier) VALUES (?1, ?2)",
            params![source.as_str(), tier_str(tier)],
        )?;
        tx.execute(
            "INSERT INTO model_role_assignments (source, tier, model_id, note, updated_at, \
             updated_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(source, tier, model_id) DO UPDATE SET model_id = ?3, note = ?4, updated_at = ?5, \
             updated_by = ?6",
            params![source.as_str(), tier_str(tier), model_id, note, now, actor],
        )?;
        Self::append_event_tx(
            tx,
            catalog_event_task_id(),
            &Event::ModelRoleAssignmentChanged {
                source: source.0.clone(),
                tier,
                model_id: Some(model_id.to_string()),
                previous,
                actor: actor.to_string(),
            },
        )?;
        Ok(RoleAssignment {
            priority: 0,
            source: source.clone(),
            tier,
            model_id: model_id.to_string(),
            note: note.map(str::to_string),
            updated_at: now,
            updated_by: actor.to_string(),
        })
    }

    pub fn model_role_assignment_delete_tx(
        tx: &Connection,
        source: &CatalogSource,
        tier: Tier,
        actor: &str,
    ) -> Result<bool, StoreError> {
        let previous: Option<String> = tx
            .query_row(
                "SELECT model_id FROM model_role_assignments WHERE source = ?1 AND tier = ?2",
                params![source.as_str(), tier_str(tier)],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "DELETE FROM model_role_scopes WHERE source = ?1 AND tier = ?2",
            params![source.as_str(), tier_str(tier)],
        )?;
        let Some(previous) = previous else {
            return Ok(false);
        };
        tx.execute(
            "DELETE FROM model_role_assignments WHERE source = ?1 AND tier = ?2",
            params![source.as_str(), tier_str(tier)],
        )?;
        Self::append_event_tx(
            tx,
            catalog_event_task_id(),
            &Event::ModelRoleAssignmentChanged {
                source: source.0.clone(),
                tier,
                model_id: None,
                previous: Some(previous),
                actor: actor.to_string(),
            },
        )?;
        Ok(true)
    }
}
