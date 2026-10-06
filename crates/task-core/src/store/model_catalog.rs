//! ADR 2026-10-06 D4: `model_catalog` / `model_catalog_overrides` / `model_catalog_discovery`（migration 0052）。
//!
//! 発見結果の反映（`model_catalog_apply`）は 1 トランザクションで、upsert・消失の印・変化 event・発見記録を
//! まとめて書く。取得に失敗した source は `model_catalog_record_failure` で記録だけし、catalog は変えない
//! （消えたと誤認しない）。上書きは別表で、反映は触らない。LLM は呼ばない。

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Tier;
use crate::model::Event;
use crate::model_catalog::{
    CatalogDelta, CatalogEntry, CatalogOverride, CatalogOverrideRow, CatalogSource,
    DiscoveredModel, DiscoveryRecord, catalog_event_task_id,
};

use super::{SqliteStore, StoreError};

fn tier_to_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

fn tier_from_str(s: &str) -> Option<Tier> {
    match s {
        "frontier" => Some(Tier::Frontier),
        "standard" => Some(Tier::Standard),
        "cheap" => Some(Tier::Cheap),
        _ => None,
    }
}

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
        conn.execute(
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

    /// 上書きを消す。行が無ければ `Ok(false)`。
    fn model_catalog_delete_override(
        &self,
        source: &CatalogSource,
        model_id: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "DELETE FROM model_catalog_overrides WHERE source = ?1 AND model_id = ?2",
            params![source.as_str(), model_id],
        )?;
        Ok(n > 0)
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
