//! ADR 2026-11-04-project-controller 付記 A5: Roadmap の組み立てと Outcome の状態の記録。
//!
//! - 画面の [`Roadmap`] は版の行（`project_roadmap_versions`）＋ Outcome の記録（`project_roadmap_outcome_events`）
//!   ＋ controller の起票の記録（`pc:` の冪等キーの `outcome_ids`）から毎回作る。task の状態は写さない（I1）。
//! - `reached` の evidence は [`project_snapshot::build`](crate::project_snapshot::build) の root の行で
//!   [`root_satisfies`] を Rust が確かめる。worker・CoS の自己申告だけでは reached にしない（D6）。
//! - 旧 milestones（`Task.milestone_id`）と ExecutionPlan の stage・WorkUnit の phase は読まない。

use std::collections::BTreeMap;

use task_core::org::ProjectId;
use task_core::project_proposal::{self as pp, root_satisfies};
use task_core::roadmap::{
    OutcomeStatus, Roadmap, RoadmapHistory, RoadmapInvalid, RoadmapVersion, build_view,
    snapshot_outcomes,
};
use task_core::store::{NewOutcomeEvent, RoadmapWriteError, SqliteStore};
use task_core::{Task, TaskStore};
use time::OffsetDateTime;

use crate::OpsError;
use crate::project_snapshot::{
    LaunchRootMeta, SnapshotContext, build as build_snapshot, parse_task_id,
};

/// controller が起票した root と、その root の `outcome_ids`（root の作成の古い順）。
fn launched(
    tasks: &[Task],
    metas: &BTreeMap<String, LaunchRootMeta>,
) -> Vec<(String, Vec<String>)> {
    tasks
        .iter()
        .filter_map(|t| {
            let id = t.id.to_string();
            let meta = metas.get(&id)?;
            Some((id, meta.outcome_ids.clone()))
        })
        .collect()
}

/// snapshot の組み立てから呼ぶ（root と冪等キーの記録は呼び出し側が読んだもの）。
pub(crate) fn view_of(
    store: &SqliteStore,
    version: &RoadmapVersion,
    tasks: &[Task],
    metas: &BTreeMap<String, LaunchRootMeta>,
) -> Result<Roadmap, OpsError> {
    let events = store.roadmap_outcome_events(&version.project_id)?;
    Ok(build_view(version, &events, &launched(tasks, metas)))
}

fn project_roots_and_metas(
    store: &SqliteStore,
    project_id: ProjectId,
) -> Result<(Vec<Task>, BTreeMap<String, LaunchRootMeta>), OpsError> {
    let pid = project_id.to_string();
    let metas = store
        .idempotency_keys_with_prefix(&format!("{}{pid}:", pp::ROOT_KEY_PREFIX))?
        .into_iter()
        .filter_map(|(_, task_id, json)| {
            let meta: LaunchRootMeta = serde_json::from_str(json.as_deref()?).ok()?;
            Some((task_id, meta))
        })
        .collect();
    let mut tasks: Vec<Task> = store
        .list(None)?
        .into_iter()
        .filter(|t| t.project_id == Some(project_id) && task_core::is_root_task(t))
        .collect();
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok((tasks, metas))
}

/// 版 `version`（`None` なら今の approved）の Roadmap。無ければ `None`。
pub fn view(
    store: &SqliteStore,
    project_id: ProjectId,
    version: Option<i64>,
) -> Result<Option<Roadmap>, OpsError> {
    let pid = project_id.to_string();
    let row = match version {
        None => store.roadmap_approved(&pid)?,
        Some(v) => store.roadmap_version(&pid, v)?,
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let (tasks, metas) = project_roots_and_metas(store, project_id)?;
    Ok(Some(view_of(store, &row, &tasks, &metas)?))
}

/// 版と Outcome の記録の全部。
pub fn history(store: &SqliteStore, project_id: ProjectId) -> Result<RoadmapHistory, OpsError> {
    let pid = project_id.to_string();
    Ok(RoadmapHistory {
        versions: store.roadmap_versions(&pid)?,
        outcome_events: store.roadmap_outcome_events(&pid)?,
    })
}

/// `POST …/roadmap/outcomes/{oid}/status` の本文。
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeStatusInput {
    /// `open`・`in_progress`・`reached`（`dropped` は版で付ける）。
    pub status: OutcomeStatus,
    /// `task:<root id>`。`reached` では 1 件以上要り、全部を Rust が確かめる。`in_progress` では担う root。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub reason: String,
}

/// 確かめた状態の記録の入力（[`check_status`] の結果）。
#[derive(Debug, Clone)]
pub struct CheckedStatus {
    evidence_token: (i64, task_core::store::RootEventMark),
    pub status: OutcomeStatus,
    pub roots: Vec<String>,
    pub evidence: Vec<String>,
    pub reason: String,
}

fn invalid(code: &'static str, detail: impl Into<String>) -> RoadmapWriteError {
    RoadmapWriteError::Invalid(RoadmapInvalid::new(code, detail))
}

/// 状態の記録の前の検査（読み取りだけ）。`reached` の evidence は、案件の root で、approved の版の Outcome に
/// [`root_satisfies`]（done・合格・取り込み済みで、Outcome を名指すか acceptance を全部含む）なものだけ。
pub fn check_status(
    store: &SqliteStore,
    project_id: ProjectId,
    outcome_id: &str,
    input: &OutcomeStatusInput,
    now: OffsetDateTime,
) -> Result<CheckedStatus, RoadmapWriteError> {
    if input.reason.chars().count() > task_core::roadmap::ROADMAP_TEXT_MAX {
        return Err(invalid("roadmap_invalid", "reason is too long"));
    }
    let pid = project_id.to_string();
    let evidence_token = store.roadmap_evidence_token(&pid)?;
    let approved = store.roadmap_approved(&pid)?.ok_or_else(|| {
        invalid(
            "roadmap_not_approved",
            "the project has no approved roadmap version",
        )
    })?;
    let outcome = snapshot_outcomes(&approved)
        .into_iter()
        .find(|o| o.id == outcome_id)
        .ok_or_else(|| {
            invalid(
                "roadmap_outcome_unknown",
                format!("outcome {outcome_id:?} is not a live outcome of the approved version"),
            )
        })?;
    let snapshot = build_snapshot(
        store,
        project_id,
        SnapshotContext {
            controller: pp::SnapshotController {
                mode: "off".into(),
                phase: "active".into(),
                revision: 0,
                human_epoch: 0,
                retry_count: 0,
                progress_epoch: 0,
            },
            decisions: Vec::new(),
            wakes: Vec::new(),
            limits: pp::SnapshotLimits::default(),
            now,
        },
    )
    .map_err(|e| match e {
        OpsError::Store(s) => RoadmapWriteError::Store(s),
        other => invalid("roadmap_invalid", other.to_string()),
    })?;
    let mut roots = Vec::new();
    for r in &input.evidence_refs {
        let id = r
            .strip_prefix("task:")
            .filter(|id| parse_task_id(id).is_some())
            .ok_or_else(|| {
                invalid(
                    "roadmap_evidence_invalid",
                    format!("{r:?} must be task:<root id>"),
                )
            })?;
        let Some(root) = snapshot.roots.iter().find(|x| x.id == id) else {
            return Err(invalid(
                "roadmap_evidence_unknown",
                format!("{r:?} is not a root task of this project"),
            ));
        };
        if input.status == OutcomeStatus::Reached && !root_satisfies(root, &outcome) {
            return Err(invalid(
                "roadmap_evidence_unverified",
                format!(
                    "root {id} is not done, reviewer-passed and integrated for outcome {outcome_id:?}"
                ),
            ));
        }
        if !roots.iter().any(|x| x == id) {
            roots.push(id.to_owned());
        }
    }
    match input.status {
        OutcomeStatus::Reached if roots.is_empty() => Err(invalid(
            "roadmap_evidence_required",
            "reached needs evidence_refs that name done, passed roots",
        )),
        OutcomeStatus::Dropped => Err(invalid(
            "roadmap_drop_needs_version",
            "dropping an outcome is a roadmap change; propose a version with dropped: true",
        )),
        status => Ok(CheckedStatus {
            evidence_token,
            status,
            evidence: if status == OutcomeStatus::Reached {
                roots.iter().map(|r| format!("task:{r}")).collect()
            } else {
                Vec::new()
            },
            roots,
            reason: input.reason.clone(),
        }),
    }
}

/// [`check_status`] の結果を呼び出し側の transaction で書く（root との対応、状態の順）。状態の記録の id を返す。
pub fn record_status_tx(
    conn: &rusqlite::Connection,
    project: &str,
    outcome_id: &str,
    checked: &CheckedStatus,
    actor: &str,
    now: OffsetDateTime,
) -> Result<i64, RoadmapWriteError> {
    if SqliteStore::roadmap_evidence_token_tx(conn, project)? != checked.evidence_token {
        return Err(RoadmapWriteError::Stale {
            current: Some(SqliteStore::roadmap_evidence_token_tx(conn, project)?.0),
        });
    }
    // controller の同じ版・到達・根拠は重複させない。人の状態操作と reason は従来どおり追記する。
    let existing: Option<i64> = if checked.status == OutcomeStatus::Reached
        && actor == "project_controller"
    {
        rusqlite::OptionalExtension::optional(conn.query_row(
        "SELECT id FROM project_roadmap_outcome_events WHERE project_id = ?1 AND outcome_id = ?2 AND kind = 'status' AND id = (SELECT MAX(id) FROM project_roadmap_outcome_events WHERE project_id = ?1 AND outcome_id = ?2 AND kind = 'status') AND roadmap_version = ?3 AND status = ?4 AND evidence_json = ?5",
        rusqlite::params![project, outcome_id, checked.evidence_token.0, checked.status.as_str(), serde_json::to_string(&checked.evidence).map_err(task_core::store::StoreError::from)?], |r| r.get(0)))?
    } else {
        None
    };
    if let Some(id) = existing {
        return Ok(id);
    }
    for root in &checked.roots {
        SqliteStore::roadmap_outcome_event_tx(
            conn,
            &NewOutcomeEvent {
                project_id: project,
                outcome_id,
                status: None,
                root_task_id: Some(root),
                evidence: &[],
                reason: "",
                actor,
            },
            now,
        )?;
    }
    SqliteStore::roadmap_outcome_event_tx(
        conn,
        &NewOutcomeEvent {
            project_id: project,
            outcome_id,
            status: Some(checked.status),
            root_task_id: None,
            evidence: &checked.evidence,
            reason: &checked.reason,
            actor,
        },
        now,
    )?
    .ok_or_else(|| invalid("roadmap_invalid", "the status record was not written"))
}
