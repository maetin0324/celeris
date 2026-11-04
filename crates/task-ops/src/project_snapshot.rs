//! ADR 2026-11-04-project-controller（A2）: 案件の [`ProjectSnapshot`] を store から組み立てる。
//!
//! - task ledger: `Project.request`（正本）と、その中の Outcome block（[`parse_outcomes`]）、root の acceptance。
//! - progress ledger: root の状態・reviewer の合格・取り込み・失敗の分類・待ち・子の数を **events と行から毎回**作る。
//!   独立の Goal も別のイベントストアも持たない（D1・I2）。worker の報告・ログの文は入れない（I11）。
//! - controller の状態（revision・human_epoch・mode）と過去の判断は A1 の表が正本なので、呼び出し側が渡す。
//! - controller が起票した root は冪等キー `pc:<project>:<root.key>`（`task_idempotency_keys`）で引き、
//!   その行の `result_json` に起票時の `outcome_ids`・`continues_root_ids` を持つ（[`LaunchRootMeta`]）。
//!
//! 起票の DTO 変換（[`launch_root_spec`]・[`launch_root_claim`]）もここに置く。task の作成そのものは既存の
//! `add::build_task_with_roles` と `SqliteStore::create_task_with`＋`idempotency_record_tx` が行う。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use task_core::project_proposal::{
    self as pp, CONTROLLER_LABEL, LAUNCH_ROOT_OPERATION, ProjectSnapshot, RootCheck, RootSpec,
    SnapshotController, SnapshotCriterion, SnapshotDecision, SnapshotLimits, SnapshotProject,
    SnapshotRoot, SnapshotWait, SnapshotWake, parse_outcomes,
};
use task_core::store::{IdempotencyClaim, SqliteStore};
use task_core::{Check, Event, ProjectId, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;
use crate::add::{CriterionSpec, NewTaskSpec};

/// 呼び出し側（A1 の tick・試験）が渡す controller の値。
#[derive(Debug, Clone)]
pub struct SnapshotContext {
    pub controller: SnapshotController,
    pub decisions: Vec<SnapshotDecision>,
    pub wakes: Vec<SnapshotWake>,
    pub limits: SnapshotLimits,
    pub now: OffsetDateTime,
}

/// 冪等キーの行の `result_json`（起票時の controller の値）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRootMeta {
    pub root_key: String,
    pub outcome_ids: Vec<String>,
    pub continues_root_ids: Vec<String>,
}

fn status_str(s: Status) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn check_kind(c: &Check) -> &'static str {
    match c {
        Check::Command { .. } => "command",
        Check::ArtifactExists { .. } => "artifact_exists",
        Check::KnowledgePage { .. } => "knowledge_page",
        Check::Reviewer => "reviewer",
        Check::Human => "human",
    }
}

/// root の progress ledger の 1 行を events から作る。
fn root_row(
    store: &SqliteStore,
    task: &Task,
    meta: Option<&LaunchRootMeta>,
    now: OffsetDateTime,
    waits: &mut Vec<SnapshotWait>,
) -> Result<SnapshotRoot, OpsError> {
    let events = store.events_for(task.id)?;
    let mut verdicts: BTreeMap<usize, bool> = BTreeMap::new();
    let mut reviewer_passed = false;
    let mut merged = false;
    let mut last_fail_reason: Option<String> = None;
    let mut attempts = 0u32;
    let mut open_questions: Vec<(String, &'static str)> = Vec::new();
    for (seq, e) in &events {
        match e {
            Event::ReviewVerdict {
                criterion_idx,
                pass,
                ..
            } => {
                verdicts.insert(*criterion_idx, *pass);
            }
            Event::ReviewerPassed { .. } | Event::ReviewerPassReused { .. } => {
                reviewer_passed = true
            }
            Event::IntegrationMerged { .. } | Event::IntegrationNotApplicable { .. } => {
                merged = true
            }
            Event::IntegrationReopened { .. } => merged = false,
            Event::Transitioned { to, reason, .. } => {
                if *to == Status::Done && reason == "review_pass" {
                    reviewer_passed = true;
                }
                if *to == Status::Failed {
                    last_fail_reason = Some(reason.clone());
                }
                if !to.is_terminal() && *to != Status::Reviewing {
                    last_fail_reason = None;
                }
            }
            Event::Retried { .. } => attempts = attempts.saturating_add(1),
            Event::QuestionRaised { .. } => open_questions.push((seq.to_string(), "question")),
            Event::Answered { .. } => {
                open_questions.retain(|(_, k)| *k != "question");
            }
            Event::ApprovalRequested => open_questions.push((seq.to_string(), "approval")),
            Event::ApprovalDecided { .. } => open_questions.retain(|(_, k)| *k != "approval"),
            _ => {}
        }
    }
    let status = status_str(task.status);
    let failure_class = match task.status {
        Status::Failed => Some(
            match last_fail_reason.as_deref() {
                Some("review_fail") => "review_failed",
                _ => "infra",
            }
            .to_owned(),
        ),
        Status::Cancelled => Some("cancelled".to_owned()),
        _ => None,
    };
    let acceptance = task
        .acceptance
        .iter()
        .enumerate()
        .map(|(i, c)| SnapshotCriterion {
            text: c.text.clone(),
            kind: check_kind(&c.check).to_owned(),
            result: match verdicts.get(&i) {
                Some(true) => "pass",
                Some(false) => "fail",
                None => "unknown",
            }
            .to_owned(),
        })
        .collect();
    let children = store.children(task.id)?;
    let (leaf_done, leaf_total) = if children.is_empty() {
        (u32::from(task.status == Status::Done), 1)
    } else {
        let done = children.iter().filter(|c| c.status == Status::Done).count();
        (
            u32::try_from(done).unwrap_or(u32::MAX),
            u32::try_from(children.len()).unwrap_or(u32::MAX),
        )
    };
    // 終端の root に開いた質問は残っていても待ちに数えない。
    if task.status.is_terminal() {
        open_questions.clear();
    }
    let age = (now - task.updated_at).whole_seconds().max(0);
    for (seq, kind) in &open_questions {
        waits.push(SnapshotWait {
            kind: (*kind).to_owned(),
            id: format!("{}:{seq}", task.id),
            task_id: task.id.to_string(),
            age_secs: u64::try_from(age).unwrap_or(0),
        });
    }
    Ok(SnapshotRoot {
        id: task.id.to_string(),
        status,
        title: task.title.clone(),
        controller_key: meta.map(|m| m.root_key.clone()),
        outcome_ids: meta.map(|m| m.outcome_ids.clone()).unwrap_or_default(),
        acceptance,
        reviewer_passed,
        integrated: task.status == Status::Done && (task.repos.is_empty() || merged),
        failure_class,
        attempts,
        open_waits: open_questions
            .iter()
            .map(|(_, k)| (*k).to_owned())
            .collect(),
        leaf_done,
        leaf_total,
        continues_root_ids: meta
            .map(|m| m.continues_root_ids.clone())
            .unwrap_or_default(),
    })
}

/// 案件 `project_id` の snapshot を作る。上限（1 MiB）を超える分は古い done の root から落とし `truncated` を真にする。
pub fn build(
    store: &SqliteStore,
    project_id: ProjectId,
    ctx: SnapshotContext,
) -> Result<ProjectSnapshot, OpsError> {
    let project = store
        .project_get(project_id)?
        .ok_or(OpsError::ProjectNotFound(project_id))?;
    let pid = project.id.to_string();
    let brief = store
        .project_brief_get(project_id)?
        .and_then(|b| serde_json::to_value(b).ok());
    let (mut outcomes, mut outcomes_invalid) = match parse_outcomes(&project.request) {
        Ok(o) => (o, false),
        Err(_) => (Vec::new(), true),
    };
    let metas: BTreeMap<String, LaunchRootMeta> = store
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
    let mut waits = Vec::new();
    let mut roots = Vec::with_capacity(tasks.len());
    for t in &tasks {
        roots.push(root_row(
            store,
            t,
            metas.get(&t.id.to_string()),
            ctx.now,
            &mut waits,
        )?);
    }
    // 付記 A5: approved の Roadmap があれば request の block より優先する（block は読まない）。
    let roadmap = match store.roadmap_approved(&pid)? {
        Some(version) => {
            outcomes = task_core::roadmap::snapshot_outcomes(&version);
            outcomes_invalid = false;
            let view = crate::roadmap::view_of(store, &version, &tasks, &metas)?;
            serde_json::to_value(view).ok()
        }
        None => None,
    };
    let mut snapshot = ProjectSnapshot {
        now: ctx.now.format(&Rfc3339).unwrap_or_default(),
        truncated: false,
        project: SnapshotProject {
            id: pid,
            title: project.title.clone(),
            status: serde_json::to_value(project.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            request_sha256: pp::sha256_hex(&project.request),
            request: project.request.clone(),
            auto_integrate: project.auto_integrate,
        },
        brief,
        outcomes,
        outcomes_invalid,
        roots,
        waits,
        stagnation: Vec::new(),
        wakes: ctx.wakes,
        decisions: ctx.decisions,
        controller: ctx.controller,
        limits: ctx.limits,
        roadmap,
        authority: None,
    };
    while serde_json::to_vec(&snapshot).map_or(0, |v| v.len()) > pp::SNAPSHOT_MAX_BYTES {
        let Some(i) = snapshot.roots.iter().position(|r| r.status == "done") else {
            break;
        };
        snapshot.roots.remove(i);
        snapshot.truncated = true;
    }
    Ok(snapshot)
}

/// `launch_root` の root を `POST /tasks` と同じ DTO にする。acceptance は Outcome の文のまま（I8）、label は
/// `project-controller`。親・依存・担当は持たない（root）。
pub fn launch_root_spec(project_id: ProjectId, root: &RootSpec) -> Result<NewTaskSpec, OpsError> {
    let acceptance: Vec<CriterionSpec> = root
        .acceptance
        .iter()
        .map(|c| match c.check {
            RootCheck::Reviewer => CriterionSpec::Reviewer {
                text: c.text.clone(),
            },
            RootCheck::Human => CriterionSpec::Human {
                text: c.text.clone(),
            },
        })
        .collect();
    let body = serde_json::json!({
        "title": root.title,
        "objective": root.objective,
        "acceptance": acceptance,
        "project_id": project_id,
        "labels": [CONTROLLER_LABEL],
    });
    serde_json::from_value(body).map_err(|e| OpsError::Validation(format!("root spec: {e}")))
}

/// 起票の冪等キーの claim と、行に残す [`LaunchRootMeta`] の JSON。`request_hash` は root の中身の SHA-256
/// （同じ key・同じ中身の再送は既存を返し、中身が違えば衝突）。
pub fn launch_root_claim(
    project_id: ProjectId,
    root: &RootSpec,
) -> Result<(IdempotencyClaim, String), OpsError> {
    let body = serde_json::to_string(root).map_err(|e| OpsError::Validation(e.to_string()))?;
    let meta = LaunchRootMeta {
        root_key: root.key.clone(),
        outcome_ids: root.outcome_ids.clone(),
        continues_root_ids: root.continues_root_ids.clone(),
    };
    let meta = serde_json::to_string(&meta).map_err(|e| OpsError::Validation(e.to_string()))?;
    Ok((
        IdempotencyClaim {
            key: pp::root_idempotency_key(&project_id.to_string(), &root.key),
            operation: LAUNCH_ROOT_OPERATION.to_owned(),
            request_hash: pp::sha256_hex(&body),
        },
        meta,
    ))
}

/// snapshot の root の id のうち、`project_controller:read_root_evidence` が読んでよいもの（op の scope）。
pub fn evidence_scope(snapshot: &ProjectSnapshot) -> BTreeSet<String> {
    snapshot.roots.iter().map(|r| r.id.clone()).collect()
}

/// `TaskId` の文字列を読む（snapshot の id から）。
pub fn parse_task_id(id: &str) -> Option<TaskId> {
    id.parse().ok()
}
