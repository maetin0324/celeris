//! ADR 2026-10-11-llm-source-model-routing-evaluation D3: genre×harness の段階（`routing_stage`）と
//! 根拠表（`routing_evidence`）の読み取り view と、人の段階変更。
//!
//! 読み取りは明示の DB path を読み取り専用で開く（store の trait は 1 組ずつしか読めないため、
//! 全組の列挙と直近の `routing_stage_changed` はここで引く）。書き込みは store の
//! `set_routing_stage` だけを通す。`enforce` へ進めるのは人だけで、D1 の「根拠が立つ」を満たす
//! 根拠表の行が無ければ拒否する（[`StageChangeError::EvidenceInsufficient`]）。
use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use task_core::model::TaskId;
use task_core::route_select::{RoutingStage, RoutingStageChange};
use task_core::routing_evidence::EvidenceRow;
use task_core::{StoreError, TaskStore};

/// D1: 候補の有効な結果がこの数以上（30 問×2 のうち 24 問）。
pub const ENFORCE_MIN_VALID_RESULTS: u64 = 24;
/// D1: pass^1 の Wilson 95% 下限がこの値以上。
pub const ENFORCE_MIN_PASS_LOWER: f64 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum StageViewError {
    #[error("failed to read routing database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("failed to decode routing record: {0}")]
    Json(#[from] serde_json::Error),
}

/// genre×harness 1 組の段階。表に行が無い組（根拠表だけある組）は `shadow`・`since` 無し。
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingStageView {
    pub genre: String,
    pub harness_id: String,
    /// `shadow` | `enforce`。
    pub stage: String,
    /// 今の段階に入った時刻（RFC 3339）。段階表に行が無ければ無い。
    pub since: Option<String>,
    /// 段階を決めたときの根拠表の版。
    pub evidence_version: Option<String>,
    /// 根拠表の最新版（段階を決めた版より新しいことがある）。
    pub latest_evidence_version: Option<String>,
    /// 直近の `routing_stage_changed`。
    pub last_change: Option<RoutingStageChangeView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingStageChangeView {
    /// event を記録した task。
    pub task_id: String,
    pub ts: String,
    pub from: Option<String>,
    pub to: String,
    pub reason: String,
    pub window: serde_json::Value,
    /// `human` | `daemon`。
    pub actor: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingStageList {
    pub stages: Vec<RoutingStageView>,
}

/// 根拠表 1 行（区間つき）。
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingEvidenceRowView {
    pub harness_version: String,
    pub source_id: String,
    pub model_id: String,
    pub n: u64,
    pub pass: u64,
    pub pass_lower: f64,
    pub pass_upper: f64,
    /// 結果の無い cell は `null`。
    pub wall_p90_s: Option<f64>,
    /// 定価の無い source・結果の無い cell は `null`。
    pub usd_per_pass: Option<f64>,
    /// `[quota 窓, 1 run あたりの消費率]`。
    pub quota_per_run: Vec<(String, f64)>,
    pub timeout_rate: f64,
    /// D1 の「根拠が立つ」（有効な結果 24 以上かつ Wilson 下限 0.5 以上）を満たすか。
    pub qualifies: bool,
}

/// genre×harness 1 組の最新版の根拠表。
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingEvidenceGroup {
    pub genre: String,
    pub harness_id: String,
    pub evidence_version: Option<String>,
    pub rows: Vec<RoutingEvidenceRowView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RoutingEvidenceList {
    pub groups: Vec<RoutingEvidenceGroup>,
}

/// D1 の「根拠が立つ」行か。
pub fn row_qualifies(row: &EvidenceRow) -> bool {
    row.n >= ENFORCE_MIN_VALID_RESULTS && row.pass_lower >= ENFORCE_MIN_PASS_LOWER
}

fn open_read_only(db_path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
}

/// 段階表と根拠表に現れる genre×harness の組（`(genre, harness_id)` 順）。
pub fn stage_keys(db_path: &Path) -> Result<Vec<(String, String)>, StageViewError> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT genre, harness_id FROM routing_stage \
         UNION SELECT DISTINCT genre, harness_id FROM routing_evidence \
         ORDER BY 1, 2",
    )?;
    let keys = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(keys)
}

/// 全組の段階。`genre`・`harness_id` を渡すとその組だけ（組が無ければ空）。
pub fn list_stages(
    db_path: &Path,
    genre: Option<&str>,
    harness_id: Option<&str>,
) -> Result<RoutingStageList, StageViewError> {
    let conn = open_read_only(db_path)?;
    let mut stages: BTreeMap<(String, String), RoutingStageView> = BTreeMap::new();
    for (g, h) in stage_keys(db_path)? {
        if genre.is_some_and(|x| x != g) || harness_id.is_some_and(|x| x != h) {
            continue;
        }
        let row: Option<(String, String, Option<String>)> = conn
            .query_row(
                "SELECT stage, since, evidence_version FROM routing_stage WHERE genre=?1 AND harness_id=?2",
                params![g, h],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        let latest: Option<String> = conn.query_row(
            "SELECT MAX(evidence_version) FROM routing_evidence WHERE genre=?1 AND harness_id=?2",
            params![g, h],
            |r| r.get(0),
        )?;
        let (stage, since, evidence_version) = match row {
            Some((stage, since, version)) => (stage, Some(since), version),
            None => ("shadow".to_string(), None, None),
        };
        stages.insert(
            (g.clone(), h.clone()),
            RoutingStageView {
                genre: g,
                harness_id: h,
                stage,
                since,
                evidence_version,
                latest_evidence_version: latest,
                last_change: None,
            },
        );
    }
    // 直近の routing_stage_changed（新しい順に見て、組ごとに最初の 1 件）。
    let mut stmt = conn.prepare(
        "SELECT task_id, ts, json FROM events \
         WHERE json_extract(json, '$.type') = 'routing_stage_changed' ORDER BY id DESC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let task_id: String = row.get(0)?;
        let ts: String = row.get(1)?;
        let value: serde_json::Value = serde_json::from_str(&row.get::<_, String>(2)?)?;
        let text = |k: &str| value.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (Some(g), Some(h)) = (text("genre"), text("harness_id")) else {
            continue;
        };
        if let Some(view) = stages.get_mut(&(g, h))
            && view.last_change.is_none()
        {
            view.last_change = Some(RoutingStageChangeView {
                task_id,
                ts,
                from: text("from"),
                to: text("to").unwrap_or_default(),
                reason: text("reason").unwrap_or_default(),
                window: value
                    .get("window")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                actor: text("actor").unwrap_or_default(),
            });
        }
    }
    Ok(RoutingStageList {
        stages: stages.into_values().collect(),
    })
}

fn evidence_group(
    store: &impl TaskStore,
    genre: &str,
    harness_id: &str,
) -> Result<RoutingEvidenceGroup, StoreError> {
    let rows = store.latest_routing_evidence(genre, harness_id)?;
    Ok(RoutingEvidenceGroup {
        genre: genre.into(),
        harness_id: harness_id.into(),
        evidence_version: rows.first().map(|r| r.evidence_version.clone()),
        rows: rows
            .iter()
            .map(|r| RoutingEvidenceRowView {
                harness_version: r.key.harness_version.clone(),
                source_id: r.key.source_id.clone(),
                model_id: r.key.model_id.clone(),
                n: r.n,
                pass: r.pass,
                pass_lower: r.pass_lower,
                pass_upper: r.pass_upper,
                wall_p90_s: r.wall_p90_s,
                usd_per_pass: r.usd_per_pass,
                quota_per_run: r
                    .quota_per_run
                    .iter()
                    .map(|(window, rate)| (window.clone(), *rate))
                    .collect(),
                timeout_rate: r.timeout_rate,
                qualifies: row_qualifies(r),
            })
            .collect(),
    })
}

/// 最新版の根拠表。`genre`・`harness_id` で絞れる。
pub fn list_evidence(
    store: &impl TaskStore,
    db_path: &Path,
    genre: Option<&str>,
    harness_id: Option<&str>,
) -> Result<RoutingEvidenceList, StageChangeError> {
    let mut groups = Vec::new();
    for (g, h) in stage_keys(db_path)? {
        if genre.is_some_and(|x| x != g) || harness_id.is_some_and(|x| x != h) {
            continue;
        }
        let group = evidence_group(store, &g, &h)?;
        if !group.rows.is_empty() {
            groups.push(group);
        }
    }
    Ok(RoutingEvidenceList { groups })
}

/// 人の段階変更の依頼。
#[derive(Debug, Clone, PartialEq)]
pub struct HumanStageChange {
    /// event を記録する task（受信箱の承認の対象の task など）。
    pub task_id: TaskId,
    pub genre: String,
    pub harness_id: String,
    pub stage: RoutingStage,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StageChangeError {
    #[error("task {0} not found")]
    TaskNotFound(TaskId),
    #[error("reason must not be empty")]
    EmptyReason,
    #[error(
        "enforce requires qualifying evidence for {genre}×{harness_id}: at least one row with \
         n >= {ENFORCE_MIN_VALID_RESULTS} and Wilson lower bound >= {ENFORCE_MIN_PASS_LOWER} (ADR D1)"
    )]
    EvidenceInsufficient { genre: String, harness_id: String },
    #[error(transparent)]
    View(#[from] StageViewError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// 人（actor `human`）の段階変更。`enforce` は根拠表の最新版に D1 を満たす行が要る。
/// `since` は呼び手が渡す（RFC 3339）。返り値は変更後の段階の view。
pub fn set_stage_by_human(
    store: &impl TaskStore,
    db_path: &Path,
    change: &HumanStageChange,
    since: &str,
) -> Result<RoutingStageView, StageChangeError> {
    if change.reason.trim().is_empty() {
        return Err(StageChangeError::EmptyReason);
    }
    if store.get(change.task_id)?.is_none() {
        return Err(StageChangeError::TaskNotFound(change.task_id));
    }
    let rows = store.latest_routing_evidence(&change.genre, &change.harness_id)?;
    if change.stage == RoutingStage::Enforce && !rows.iter().any(row_qualifies) {
        return Err(StageChangeError::EvidenceInsufficient {
            genre: change.genre.clone(),
            harness_id: change.harness_id.clone(),
        });
    }
    store.set_routing_stage(
        change.task_id,
        &RoutingStageChange {
            genre: change.genre.clone(),
            harness_id: change.harness_id.clone(),
            stage: change.stage,
            since: since.into(),
            evidence_version: rows.first().map(|r| r.evidence_version.clone()),
            reason: change.reason.clone(),
            window: serde_json::json!({}),
            actor: "human".into(),
        },
    )?;
    let list = list_stages(db_path, Some(&change.genre), Some(&change.harness_id))?;
    list.stages.into_iter().next().ok_or_else(|| {
        StageChangeError::Store(StoreError::Invalid(
            "routing stage row missing after write".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::SqliteStore;
    use task_core::routing_evidence::EvidenceKey;

    fn row(n: u64, pass_lower: f64) -> EvidenceRow {
        EvidenceRow {
            quota_cost_per_pass: None,
            key: EvidenceKey {
                lane: String::new(),
                genre: "coding".into(),
                harness_id: "codex".into(),
                harness_version: "1.0.0".into(),
                source_id: "openai".into(),
                model_id: "gpt".into(),
            },
            evidence_version: "v1".into(),
            n,
            pass: n,
            pass_lower,
            pass_upper: 0.95,
            pass_boot_lower: pass_lower,
            pass_boot_upper: 0.95,
            wall_p90_s: Some(10.0),
            usd_per_pass: Some(0.2),
            quota_per_run: [("7d".to_string(), 0.001)].into(),
            timeout_rate: 0.0,
            low_confidence: false,
        }
    }

    #[test]
    fn enforce_requires_d1_evidence_and_records_human_change() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let store = SqliteStore::open(&db).unwrap();
        let spec: crate::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title": "t", "objective": "o",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
        }))
        .unwrap();
        let task = crate::add::create_task(&store, spec, time::OffsetDateTime::now_utc()).unwrap();
        let change = HumanStageChange {
            task_id: task.id,
            genre: "coding".into(),
            harness_id: "codex".into(),
            stage: RoutingStage::Enforce,
            reason: "approve".into(),
        };
        assert!(matches!(
            set_stage_by_human(&store, &db, &change, "2026-10-13T00:00:00Z"),
            Err(StageChangeError::EvidenceInsufficient { .. })
        ));
        store
            .replace_routing_evidence("v1", &[row(23, 0.6)])
            .unwrap();
        assert!(matches!(
            set_stage_by_human(&store, &db, &change, "2026-10-13T00:00:00Z"),
            Err(StageChangeError::EvidenceInsufficient { .. })
        ));
        store
            .replace_routing_evidence("v2", &[row(30, 0.55)])
            .unwrap();
        let view = set_stage_by_human(&store, &db, &change, "2026-10-13T00:00:00Z").unwrap();
        assert_eq!(view.stage, "enforce");
        assert_eq!(view.evidence_version.as_deref(), Some("v2"));
        let last = view.last_change.unwrap();
        assert_eq!(
            (last.actor.as_str(), last.to.as_str()),
            ("human", "enforce")
        );
        assert_eq!(last.task_id, task.id.to_string());
        let evidence = list_evidence(&store, &db, None, None).unwrap();
        assert_eq!(evidence.groups.len(), 1);
        assert!(evidence.groups[0].rows[0].qualifies);
    }
}
