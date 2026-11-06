//! 第 3 フェーズ P2-J: `GET /projects/{id}/controller` の `summary`（未達・達成・待機の理由・許可の範囲・次の wake・
//! guard の上限・最後の判断の追跡）を store から組み立てる。
//!
//! - 到達は判定と同じ規則で数える（[`recorded_outcomes`] と `project_proposal::root_satisfies`。I1）。snapshot は
//!   [`crate::project_snapshot::build`] で作る（dispatcher の判定が読むのと同じ root・Outcome・Roadmap）。
//! - 待機の理由は最後の判断の規則（判断の行の `validation`）と、記録の中の TS の `wait`・`escalate`・guard から読む。
//!   判定の根拠には使わない（画面の写し）。
//! - 許可の範囲は `authority_section` と同じ生きている許可（`grant_denial` が `None`）。

use std::collections::BTreeSet;

use serde_json::Value;
use task_core::ProjectId;
use task_core::authority::{ActionClass, GrantScope, ScopeKind, grant_denial};
use task_core::project_proposal::{
    self as pp, ProjectSnapshot, SnapshotController, SnapshotLimits,
};
use task_core::project_wake::{
    ControllerAuthority, ControllerBudgetLimits, ControllerGrant, ControllerMode,
    ControllerNextWake, ControllerOutcomeStatus, ControllerPhase, ControllerTrace,
    ControllerWaitReason, ProjectControllerDecision, ProjectControllerState,
    ProjectControllerSummary, ProjectWake, ProjectWakeKind,
};
use task_core::store::SqliteStore;
use time::OffsetDateTime;

use crate::OpsError;
use crate::project_budget::BudgetLimits;
use crate::project_snapshot::{self as ps, SnapshotContext};

/// 到達の記録を探す判断の件数の上限（`mark_milestone` は Outcome ごとに 1 回なので十分に大きい）。
const RECORDED_SCAN: usize = 100_000;

/// 第 3 フェーズ I1・I2: 到達を記録済みの Outcome の id。
/// - approved の Roadmap がある案件: Roadmap の最後の状態が `reached`（人の API か controller の効果が書いたもの）。
/// - Roadmap の無い案件: 今の依頼文の版（`request_sha256`）での `mark_milestone` の判断で、効果が `applied`。
/// - `shadow`・`dry_run` は効果を書かない（`suppressed`）ので、その mode では `suppressed` の `mark_milestone` も
///   記録に数える（enabled なら何が起きるかの写し。enabled の判定には効かない）。
pub fn recorded_outcomes(
    store: &SqliteStore,
    mode: ControllerMode,
    snapshot: &ProjectSnapshot,
) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    if let Some(view) = &snapshot.roadmap {
        let view: task_core::roadmap::Roadmap =
            serde_json::from_value(view.clone()).map_err(|e| format!("roadmap view: {e}"))?;
        for o in view.outcomes {
            if o.status == task_core::roadmap::OutcomeStatus::Reached {
                out.insert(o.id);
            }
        }
    }
    let rows = store
        .project_controller_decisions(&snapshot.project.id, None, RECORDED_SCAN)
        .map_err(|e| e.to_string())?;
    for row in &rows {
        let counted = match row.effect_state.as_str() {
            "applied" => snapshot.roadmap.is_none(),
            "suppressed" => mode != ControllerMode::Enabled,
            _ => false,
        };
        if !counted || row.request_sha256 != snapshot.project.request_sha256 {
            continue;
        }
        if let Some(id) = record_outcome_id(&row.effect_json) {
            out.insert(id);
        }
    }
    Ok(out)
}

/// 効果 `{"kind":"record_outcome","outcome_id":…}`（`task_dispatch::ProjectEffect::RecordOutcome`）の id。
fn record_outcome_id(effect_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(effect_json).ok()?;
    if v.get("kind")?.as_str()? != "record_outcome" {
        return None;
    }
    Some(v.get("outcome_id")?.as_str()?.to_owned())
}

impl From<&BudgetLimits> for ControllerBudgetLimits {
    fn from(l: &BudgetLimits) -> Self {
        Self {
            max_cos_inputs_per_day: l.max_cos_inputs_per_day,
            max_roots_per_day: l.max_roots_per_day,
            max_no_progress_decisions: l.max_no_progress_decisions,
            max_same_failure_repeats: l.max_same_failure_repeats,
            max_invalid_decisions: l.max_invalid_decisions,
            max_usd_per_day: l.max_usd_per_day_micros as f64 / 1_000_000.0,
            ttl_secs: l.ttl_secs,
        }
    }
}

fn json_kind(text: &str, pointer: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(v.pointer(pointer)?.as_str()?.to_owned())
}

/// 最後の判断から待機の種類を決める（`ControllerWaitReason.kind`）。規則の id を先に見て、どれでもなければ
/// 効果か TS の案が問い（`open_escalation`・`escalate`）なら `escalation`、他は `wait`。
pub fn wait_kind(phase: ControllerPhase, last: Option<&ProjectControllerDecision>) -> &'static str {
    match phase {
        ControllerPhase::Paused => return "paused",
        ControllerPhase::Cancelled => return "cancelled",
        _ => {}
    }
    let Some(d) = last else {
        return "other";
    };
    let rule = d.validation.as_str();
    if rule.starts_with("pc/guard/") {
        "guard"
    } else if rule == task_core::store::AUTHORITY_REQUIRED_RULE {
        "authority"
    } else if rule.starts_with("pc/achieved-unverified") {
        "achievement_unverified"
    } else if rule == "pc/script-unavailable" {
        "script_unavailable"
    } else if rule.starts_with("pc/invalid") {
        "invalid_proposal"
    } else if json_kind(&d.effect_json, "/kind").as_deref() == Some("open_escalation")
        || json_kind(&d.proposal_json, "/proposal/action/kind").as_deref() == Some("escalate")
    {
        "escalation"
    } else {
        "wait"
    }
}

/// 判断の記録（`proposal_json`）から待ち相手を 1 行ずつ読む。
/// TS の記録は `proposal.action`（`wait` の `waiting_on`・`escalate` の `summary`）、guard の記録は `guard`・`detail`。
pub fn wait_details(decision: &ProjectControllerDecision) -> Vec<String> {
    let Ok(rec) = serde_json::from_str::<Value>(&decision.proposal_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(name) = rec.get("guard").and_then(Value::as_str) {
        let detail = rec.get("detail").map(Value::to_string).unwrap_or_default();
        out.push(format!("guard {name}: {detail}"));
    }
    if let Some(reason) = rec.get("unavailable").and_then(Value::as_str) {
        out.push(format!("script unavailable: {reason}"));
    }
    if let Some(action) = rec.get("proposal").and_then(|p| p.get("action")) {
        match action.get("kind").and_then(Value::as_str) {
            Some("wait") => {
                for w in action
                    .get("waiting_on")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let kind = w.get("kind").and_then(Value::as_str).unwrap_or("?");
                    let note = w.get("note").and_then(Value::as_str).unwrap_or("");
                    match w.get("target").and_then(Value::as_str) {
                        Some(t) => out.push(format!("{kind}: {note} ({t})")),
                        None => out.push(format!("{kind}: {note}")),
                    }
                }
            }
            Some("escalate") => {
                let summary = action.get("summary").and_then(Value::as_str).unwrap_or("");
                let key = action
                    .get("question_key")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                out.push(format!("escalate {key}: {summary}"));
            }
            _ => {}
        }
    }
    if let Some(ids) = rec.get("achievement").and_then(|a| a.get("unverified"))
        && ids.as_array().is_some_and(|a| !a.is_empty())
    {
        out.push(format!("unverified outcomes: {ids}"));
    }
    if let Ok(effect) = serde_json::from_str::<Value>(&decision.effect_json)
        && effect.get("kind").and_then(Value::as_str) == Some("open_escalation")
    {
        let key = effect
            .get("question_key")
            .and_then(Value::as_str)
            .unwrap_or("");
        let summary = effect.get("summary").and_then(Value::as_str).unwrap_or("");
        let line = format!("escalate {key}: {summary}");
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

fn authority(store: &SqliteStore, project: &str, now: OffsetDateTime) -> ControllerAuthority {
    let verdict = store.authority_check(
        project,
        ActionClass::LaunchRoot.as_str(),
        &GrantScope {
            kind: ScopeKind::Project,
            reference: project.to_owned(),
        },
        now,
    );
    let grants = store
        .authority_grants(project)
        .unwrap_or_default()
        .iter()
        .filter(|g| grant_denial(g, now).is_none())
        .map(|g| ControllerGrant {
            id: g.id.clone(),
            action_class: g.action_class.as_str().to_owned(),
            scope_kind: g.scope.kind.as_str().to_owned(),
            scope_ref: g.scope.reference.clone(),
            expires_at: g.expires_at.clone(),
            budget_remaining: g.budget - g.consumed,
        })
        .collect();
    ControllerAuthority {
        launch_root_allowed: verdict.allow,
        launch_root_reason: verdict.reason,
        grants,
    }
}

/// `summary` を作る。`state` は controller の行（無ければ既定）、`last` は最後の判断。
pub fn summary(
    store: &SqliteStore,
    project_id: ProjectId,
    state: &ProjectControllerState,
    last: Option<&ProjectControllerDecision>,
    pending: &[ProjectWake],
    limits: &BudgetLimits,
    now: OffsetDateTime,
) -> Result<ProjectControllerSummary, OpsError> {
    let snapshot = ps::build(
        store,
        project_id,
        SnapshotContext {
            controller: SnapshotController {
                mode: state.mode.as_str().to_owned(),
                phase: state.phase.as_str().to_owned(),
                revision: u64::try_from(state.revision).unwrap_or(0),
                human_epoch: u64::try_from(state.human_epoch).unwrap_or(0),
                retry_count: state.retry_count,
                progress_epoch: u64::try_from(state.progress_epoch).unwrap_or(0),
            },
            decisions: Vec::new(),
            wakes: Vec::new(),
            limits: SnapshotLimits::default(),
            now,
        },
    )?;
    let recorded = recorded_outcomes(store, state.mode, &snapshot).map_err(OpsError::Validation)?;
    let outcomes = snapshot
        .outcomes
        .iter()
        .map(|o| {
            let evidence_root_ids: Vec<String> = snapshot
                .roots
                .iter()
                .filter(|r| pp::root_satisfies(r, o))
                .map(|r| r.id.clone())
                .collect();
            let is_recorded = recorded.contains(&o.id);
            ControllerOutcomeStatus {
                id: o.id.clone(),
                title: o.title.clone(),
                source: o.source.clone(),
                recorded: is_recorded,
                reached: is_recorded && !evidence_root_ids.is_empty(),
                evidence_root_ids,
            }
        })
        .collect();
    let waiting = match state.phase {
        ControllerPhase::Waiting
        | ControllerPhase::Blocked
        | ControllerPhase::Paused
        | ControllerPhase::Cancelled => Some(ControllerWaitReason {
            phase: state.phase,
            kind: wait_kind(state.phase, last).to_owned(),
            rule_id: last.map(|d| d.validation.clone()),
            details: last.map(wait_details).unwrap_or_default(),
            decision_id: last.map(|d| d.id.clone()),
            decided_at: last.map(|d| d.created_at.clone()),
        }),
        ControllerPhase::Active | ControllerPhase::Achieved => None,
    };
    let mut kinds: Vec<ProjectWakeKind> = ProjectWakeKind::ALL
        .into_iter()
        .filter(|k| pending.iter().any(|w| w.kind == *k))
        .collect();
    kinds.dedup();
    Ok(ProjectControllerSummary {
        outcomes,
        outcomes_invalid: snapshot.outcomes_invalid,
        waiting,
        authority: authority(store, &snapshot.project.id, now),
        next_wake: ControllerNextWake {
            at: state.next_wake_at.clone(),
            backoff_until: state.backoff_until.clone(),
            pending_kinds: kinds,
        },
        budget_limits: ControllerBudgetLimits::from(limits),
        trace: last.map(|d| ControllerTrace {
            decision_id: d.id.clone(),
            rule_id: d.validation.clone(),
            mode: d.mode,
            effect_state: d.effect_state,
            effect_ref: d.effect_ref.clone(),
            based_on_revision: d.based_on_revision,
            snapshot_sha256: d.snapshot_sha256.clone(),
            generation_id: d.generation_id.clone(),
            decided_at: d.created_at.clone(),
        }),
        retention: store.retention_project_counts(&snapshot.project.id)?,
    })
}
