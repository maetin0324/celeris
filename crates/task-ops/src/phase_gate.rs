//! ADR-0074 D2.2/D2.4（Phase F3 途中確認）: 工程の後で止まった Task（`blocked(awaiting_human)`）の
//! 判定と、人の 3 つの操作（`continue` / `replan` / `withdraw`）。
//!
//! 状態変更は `TaskStore::apply_transition` だけで行う（`gate.rs` と同じ規律）。LLM は使わない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Event, PhaseReport, PhaseResumeMode, Status, Task, TaskId, TaskStore, Trigger};

use crate::error::OpsError;
use crate::gate::TransitionResult;

/// `Trigger::PhaseGate` の `Transitioned.reason`。
pub const AWAITING_HUMAN: &str = "awaiting_human";

/// 途中確認で人が書いた指示を `Event::Answered.question` に残すときの接頭辞（`answers` の節と同じ形で
/// 次の run のプロンプトに渡るように、既存の `Answered` をそのまま使う）。
pub const PHASE_GATE_QUESTION_PREFIX: &str = "途中確認: 工程";

/// replan の planner run に渡す「起こした理由」の接頭辞（D2.4「人の指示: <note>」）。
pub const HUMAN_INSTRUCTION_PREFIX: &str = "人の指示: ";

/// 直前の `Transitioned.reason`（無ければ `None`）。
pub fn last_transition_reason(events: &[(u64, Event)]) -> Option<&str> {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::Transitioned { reason, .. } => Some(reason.as_str()),
        _ => None,
    })
}

/// D2.2: Task が工程の後の途中確認で止まっているか（`Blocked` で、直前の遷移の reason が
/// `awaiting_human`）。
pub fn is_awaiting_human(task: &Task, events: &[(u64, Event)]) -> bool {
    task.status == Status::Blocked && last_transition_reason(events) == Some(AWAITING_HUMAN)
}

/// 途中確認の 1 件分（受信箱・通知・GUI が読む材料）。
#[derive(Debug, Clone, PartialEq)]
pub struct PhaseCheckpointInfo {
    /// `Transitioned{reason:"awaiting_human"}` の event の seq（通知の重複排除の key に使う）。
    pub transition_seq: u64,
    pub report: PhaseReport,
    /// `GET /tasks/{id}/artifacts/{idx}` の添字（`phase-reports/<n>-<phase>.md`。書けなければ `None`）。
    pub report_idx: Option<usize>,
}

/// D2.3/D2.4: 直近の途中報告（`PhaseReported`）と、その Markdown の成果物の添字。
/// 途中確認で止まっていなければ `None`。
pub fn latest_phase_checkpoint(
    task: &Task,
    events: &[(u64, Event)],
) -> Option<PhaseCheckpointInfo> {
    if !is_awaiting_human(task, events) {
        return None;
    }
    let transition_seq = events.iter().rev().find_map(|(seq, e)| match e {
        Event::Transitioned { reason, .. } if reason == AWAITING_HUMAN => Some(*seq),
        _ => None,
    })?;
    let report = events.iter().rev().find_map(|(_, e)| match e {
        Event::PhaseReported { report, .. } => Some(report.as_ref().clone()),
        _ => None,
    })?;
    let artifact_run = format!("daemon:phase-gate:{}", report.phase);
    let report_idx = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ArtifactProduced { run_id, artifact } => Some((run_id, artifact)),
            _ => None,
        })
        .enumerate()
        .filter(|(_, (run_id, _))| **run_id == artifact_run)
        .map(|(idx, _)| idx)
        .last();
    Some(PhaseCheckpointInfo {
        transition_seq,
        report,
        report_idx,
    })
}

/// D2.4: `POST /tasks/{id}/execution/phase-gate` の `action`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PhaseGateAction {
    /// 次の工程へ進める（`PhaseResume{Continue}`）。
    Continue,
    /// replan の planner run を起こす（`PhaseResume{Replan}`。`note` 必須）。
    Replan,
    /// 取り下げる（既存の `Cancel`）。
    Withdraw,
}

impl PhaseGateAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PhaseGateAction::Continue => "continue",
            PhaseGateAction::Replan => "replan",
            PhaseGateAction::Withdraw => "withdraw",
        }
    }
}

/// D2.4: `POST /tasks/{id}/execution/phase-gate` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseGateRequest {
    pub action: PhaseGateAction,
    /// `continue` では任意（次の工程の WU の run に「人の指示」として渡す）。`replan` では必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// D2.4: 人の途中確認への応答を適用する。
///
/// - Task が `awaiting_human` でなければ `InvalidState`（API は 409）。
/// - `replan` で `note` が空なら `Validation`（API は 422）。状態は変えない。
/// - `continue` / `replan` の `note` は `Event::Answered{question:"途中確認: 工程『<phase>』の後", answer}`
///   として同じトランザクションで残す（次の run のプロンプトの `answers` の節に出る）。
/// - `withdraw` は既存の `Trigger::Cancel`（後片付けは ADR-0043 D2 の中止のまま）。
pub fn phase_gate(
    store: &dyn TaskStore,
    id: TaskId,
    action: PhaseGateAction,
    note: Option<String>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    let events = store.events_for(id)?;
    if !is_awaiting_human(&task, &events) {
        return Err(OpsError::InvalidState {
            id,
            context: format!(
                "status={:?}, reason={}",
                task.status,
                last_transition_reason(&events).unwrap_or("-")
            ),
            action: format!(
                "resumed from a phase gate ({}); only tasks awaiting_human accept it",
                action.as_str()
            ),
        });
    }
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let phase = events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::PhaseReported { phase, .. } => Some(phase.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let (trigger, extra) = match action {
        PhaseGateAction::Withdraw => return crate::gate::cancel(store, id, Some(Status::Blocked)),
        PhaseGateAction::Continue => (
            Trigger::PhaseResume {
                mode: PhaseResumeMode::Continue,
            },
            note,
        ),
        PhaseGateAction::Replan => {
            let Some(note) = note else {
                return Err(OpsError::Validation(
                    "note: replan requires a non-empty note (the human instruction for the planner)"
                        .to_string(),
                ));
            };
            (
                Trigger::PhaseResume {
                    mode: PhaseResumeMode::Replan,
                },
                Some(note),
            )
        }
    };
    let extra_event = extra.map(|answer| Event::Answered {
        question: format!("{PHASE_GATE_QUESTION_PREFIX}『{phase}』の後"),
        answer,
    });
    let from = task.status;
    let outcome = store.apply_transition(id, trigger, extra_event)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded: Vec::new(),
    })
}

/// D2.4: 直近の `phase_replan` の人の指示（replan の planner run の「起こした理由」）。
/// 直前の遷移が `phase_replan` でなければ `None`。
pub fn phase_replan_instruction(events: &[(u64, Event)]) -> Option<String> {
    let idx = events.iter().rposition(|(_, e)| {
        matches!(e, Event::Transitioned { reason, .. } if reason == PhaseResumeMode::Replan.name())
    })?;
    // 同じトランザクションの `Answered`（遷移の直前か直後）を探す。
    let lo = idx.saturating_sub(1);
    let hi = (idx + 2).min(events.len());
    events[lo..hi].iter().find_map(|(_, e)| match e {
        Event::Answered { question, answer }
            if question.starts_with(PHASE_GATE_QUESTION_PREFIX) =>
        {
            Some(format!("{HUMAN_INSTRUCTION_PREFIX}{answer}"))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{
        Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };
    use time::OffsetDateTime;

    fn blocked_task(store: &SqliteStore, awaiting: bool) -> TaskId {
        let now = OffsetDateTime::now_utc();
        let task = Task {
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "t".to_string(),
            objective: "o".to_string(),
            acceptance: vec![Criterion {
                text: "ok".to_string(),
                check: Check::Command {
                    cmd: "true".to_string(),
                    expect_exit: 0,
                },
            }],
            inputs: Vec::new(),
            depends_on: vec![],
            status: Status::Ready,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "/tmp/workspace".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 10,
                max_wall_secs: 600,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        };
        store.insert(&task).unwrap();
        store
            .apply_transition(task.id, Trigger::Dispatch, None)
            .unwrap();
        if awaiting {
            store
                .apply_transition_with_events(
                    task.id,
                    Trigger::PhaseGate {
                        phase: "design".to_string(),
                    },
                    vec![Event::PhaseReported {
                        phase: "design".to_string(),
                        report: Box::new(PhaseReport {
                            phase: "design".to_string(),
                            phase_title: "Design".to_string(),
                            next_phase: Some("build".to_string()),
                            ..Default::default()
                        }),
                    }],
                )
                .unwrap();
        } else {
            store
                .apply_transition(task.id, Trigger::WorkerQuestion, None)
                .unwrap();
        }
        task.id
    }

    #[test]
    fn continue_resumes_and_keeps_the_note_as_an_answer() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, true);
        let r = phase_gate(
            &store,
            id,
            PhaseGateAction::Continue,
            Some("  keep it small ".to_string()),
        )
        .unwrap();
        assert_eq!(r.to, Status::Ready);
        assert_eq!(r.reason, "phase_continue");
        let events = store.events_for(id).unwrap();
        assert!(events.iter().any(|(_, e)| matches!(e,
            Event::Answered { question, answer }
                if question == "途中確認: 工程『design』の後" && answer == "keep it small")));
    }

    #[test]
    fn replan_requires_a_note_and_does_not_change_the_state() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, true);
        for note in [None, Some("   ".to_string())] {
            let err = phase_gate(&store, id, PhaseGateAction::Replan, note).unwrap_err();
            assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
        }
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
        let r = phase_gate(
            &store,
            id,
            PhaseGateAction::Replan,
            Some("split build".to_string()),
        )
        .unwrap();
        assert_eq!(r.reason, "phase_replan");
        let events = store.events_for(id).unwrap();
        assert_eq!(
            phase_replan_instruction(&events).as_deref(),
            Some("人の指示: split build")
        );
    }

    /// D2.2: `Trigger::Answer` は awaiting_human の Task には 409（`InvalidState`）。
    #[test]
    fn answer_is_rejected_while_awaiting_human() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, true);
        let err = crate::gate::answer(&store, id, "go".to_string(), None).unwrap_err();
        assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
    }

    #[test]
    fn withdraw_cancels() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, true);
        let r = phase_gate(&store, id, PhaseGateAction::Withdraw, None).unwrap();
        assert_eq!(r.to, Status::Cancelled);
    }

    #[test]
    fn a_question_block_is_not_a_phase_gate() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, false);
        for action in [
            PhaseGateAction::Continue,
            PhaseGateAction::Replan,
            PhaseGateAction::Withdraw,
        ] {
            let err = phase_gate(&store, id, action, Some("x".to_string())).unwrap_err();
            assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
        }
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
    }

    #[test]
    fn latest_checkpoint_reads_the_report_only_while_awaiting_human() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = blocked_task(&store, true);
        let task = store.get(id).unwrap().unwrap();
        let events = store.events_for(id).unwrap();
        let info = latest_phase_checkpoint(&task, &events).expect("checkpoint");
        assert_eq!(info.report.phase, "design");
        assert_eq!(info.report_idx, None);
        phase_gate(&store, id, PhaseGateAction::Continue, None).unwrap();
        let task = store.get(id).unwrap().unwrap();
        let events = store.events_for(id).unwrap();
        assert!(latest_phase_checkpoint(&task, &events).is_none());
    }
}
