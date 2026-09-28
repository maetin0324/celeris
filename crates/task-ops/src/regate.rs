//! ADR-0072「Phase F6 実装時の決定」: 起票済みの Task を、後から人が分解の経路（ExecutionPlan）に
//! 入れる / atomic に戻す（`POST /tasks/{id}/execution/decompose`、MCP `task_decompose`、
//! `POST /tasks/{id}/retry` の `execution`）。
//!
//! - 書くのは `Task.routing.execution_hint = {mode, explicit: true}`（人の明示）と、前の gate の判定
//!   （`Task.routing.execution`）の消去だけ。`Event::ExecutionHintSet` を同じトランザクションに積む。
//!   gate の判定そのものは**書かない**: 次の dispatch で daemon の `execution_gate_if_needed` が
//!   `human/explicit` として判定し直し、新しい `Event::ExecutionGated` を残す（新しく作った「人が
//!   `execution: compound` を明示した Task」と同じ経路。`gate = "shadow"` でも人の明示の compound は
//!   planner run に進む。ADR-0074「Phase F3（途中確認）」）。
//! - 受け付ける状態は `draft` / `ready` / `blocked`（`blocked` は回答などで `ready` に戻った次の
//!   dispatch から効く）。`running` / `reviewing` は 409（走っている run を止めない。止めたいなら
//!   中止してから `retry` の `execution`）。終端（`done` / `failed` / `cancelled`）も 409（`retry` の
//!   `execution` を使う）。
//! - 計画を既に持つ Task への `compound` は replan の依頼（`replan = true`。daemon は次の dispatch で
//!   `max_replans` の範囲で replan の planner run を起こす）。計画を持つ Task を `atomic` に戻すことは
//!   しない（採用済みの WU を宙に浮かせない。409）。
//! - gate の対象外（`kind != execute`・対話・support-task・`routing` の無い旧タスク・固定パイプラインの
//!   harness・`workspace_mode = shared`）は 422（gate が常に atomic にするので、書いても効かない）。
//!
//! I/O はストアの読み書きだけで、LLM は呼ばない（DESIGN 原則 1）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Event, ExecutionHintSpec, ExecutionMode, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::error::OpsError;

/// `note` の上限（文字数）。replan の planner run の「起こした理由」にそのまま渡る。
pub const NOTE_MAX_CHARS: usize = 2000;

/// `POST /tasks/{id}/execution/decompose` の要求本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecomposeRequest {
    /// `"compound"`（計画を作らせる）か `"atomic"`（1 つの run で直接実行する）。
    pub mode: ExecutionMode,
    /// 人の一言（任意、2,000 文字まで）。replan の依頼では planner run の「起こした理由」に渡る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /tasks/{id}/execution/decompose` の応答（200）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecomposeResult {
    pub task: Task,
    pub mode: ExecutionMode,
    /// `true` なら計画を既に持つ Task への replan の依頼（次の dispatch で replan の planner run）。
    pub replan: bool,
    /// 消した前の gate の判定（無ければ `null`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_decision: Option<task_core::ExecutionGateDecision>,
}

/// gate の対象外なら理由を返す（`execution_gate::out_of_scope_rule` と同じ 6 条件）。
fn out_of_scope_reason(task: &Task) -> Option<String> {
    task_core::execution_gate::out_of_scope_rule(task).map(|_| {
        "this task is out of scope of the Complexity Gate (not kind=execute, a conversation, a \
         support task, a task without routing, a fixed-pipeline harness or a shared workspace); it \
         always runs atomic"
            .to_string()
    })
}

/// 人（`source = "human"`）または MCP クライアント（`"mcp:<id>"`）が実行の形を決める。
pub fn set_execution_mode(
    store: &dyn TaskStore,
    id: TaskId,
    mode: ExecutionMode,
    source: &str,
    note: Option<String>,
    now: OffsetDateTime,
) -> Result<DecomposeResult, OpsError> {
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if let Some(n) = &note
        && n.chars().count() > NOTE_MAX_CHARS
    {
        return Err(OpsError::Validation(format!(
            "note must be at most {NOTE_MAX_CHARS} characters"
        )));
    }
    let mut task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if let Some(reason) = out_of_scope_reason(&task) {
        return Err(OpsError::Validation(reason));
    }
    match task.status {
        Status::Draft | Status::Ready | Status::Blocked => {}
        Status::Running | Status::Reviewing => {
            return Err(OpsError::InvalidState {
                id,
                context: format!("status={:?}", task.status),
                action: "re-gated while a run is in flight; wait for it to finish, or cancel \
                         the task and retry it with `execution`"
                    .to_string(),
            });
        }
        Status::Done | Status::Failed | Status::Cancelled => {
            return Err(OpsError::InvalidState {
                id,
                context: format!("status={:?}", task.status),
                action: "re-gated; it is terminal (use POST /tasks/{id}/retry with `execution` \
                         to run a copy through the chosen path)"
                    .to_string(),
            });
        }
    }
    let has_plan = store.execution_plan_active(id)?.is_some();
    if has_plan && mode == ExecutionMode::Atomic {
        return Err(OpsError::InvalidState {
            id,
            context: "it has an adopted execution plan".to_string(),
            action: "switched back to atomic (cancel it and retry with execution=atomic)"
                .to_string(),
        });
    }
    let mut routing = task.routing.clone().unwrap_or_default();
    let previous = routing.execution_hint;
    // 計画を持つ Task（replan の依頼）は gate の判定を消さない（計画の由来の監査のため）。
    let previous_decision = if has_plan {
        None
    } else {
        routing.execution.take()
    };
    routing.execution_hint = Some(ExecutionHintSpec {
        mode,
        explicit: true,
    });
    task.routing = Some(routing);
    task.updated_at = now;
    let task = store.update_task(
        &task,
        Event::ExecutionHintSet {
            mode,
            previous,
            previous_decision: previous_decision.clone().map(Box::new),
            source: source.to_string(),
            note,
            replan: has_plan,
        },
    )?;
    Ok(DecomposeResult {
        task,
        mode,
        replan: has_plan,
        previous_decision,
    })
}

/// ADR-0072「Phase F6 実装時の決定」: 計画を持つ Task に、まだ消費されていない人の replan の依頼
/// （`ExecutionHintSet{replan: true}`）があれば、その `note`（無ければ空文字列）を返す。
/// 依頼の後に計画が採用された（`ExecutionPlanned`）か、run が始まった（`Transitioned{to: running}`）
/// なら消費済み（`None`）。daemon の `wu_dispatch_gate` が決定的に読む。
pub fn pending_replan_request(events: &[(u64, Event)]) -> Option<String> {
    for (_, event) in events.iter().rev() {
        match event {
            Event::ExecutionHintSet {
                replan: true, note, ..
            } => return Some(note.clone().unwrap_or_default()),
            Event::ExecutionPlanned { .. } => return None,
            Event::Transitioned {
                to: Status::Running,
                ..
            } => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{SqliteStore, TaskKind, Trigger};

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("open")
    }

    fn spec(title: &str) -> crate::add::NewTaskSpec {
        crate::add::NewTaskSpec {
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            title: title.to_string(),
            objective: "do it".to_string(),
            acceptance: vec![crate::add::CriterionSpec::ArtifactExists {
                name: "result.md".to_string(),
            }],
            kind: TaskKind::Execute,
            tier: None,
            priority: Some(crate::add::PriorityInput::Number(0)),
            parent: None,
            depends_on: vec![],
            max_turns: None,
            max_wall_secs: None,
            max_retries: 2,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            workspace: None,
            cluster: None,
            workspace_mode: None,
            adapter: None,
            labels: Vec::new(),
            category: None,
            status: None,
            features: None,
            execution: None,
            pause_after: None,
            provenance: crate::add::SpecProvenance::default(),
        }
    }

    fn gated_atomic(store: &SqliteStore) -> Task {
        let task =
            crate::add::create_task(store, spec("t"), OffsetDateTime::now_utc()).expect("create");
        store
            .apply_transition(task.id, Trigger::Accept, None)
            .expect("accept");
        let mut t = store.get(task.id).expect("get").expect("some");
        let mut routing = t.routing.clone().unwrap_or_default();
        routing.execution_hint = Some(ExecutionHintSpec {
            mode: ExecutionMode::Compound,
            explicit: false,
        });
        routing.execution = Some(task_core::ExecutionGateDecision {
            mode: ExecutionMode::Compound,
            source: task_core::GateSource::Hint,
            score: 9,
            threshold: 5,
            rule_id: "compound/score".to_string(),
            signals: Vec::new(),
            policy_version: "exec-gate/1".to_string(),
            shadow: true,
        });
        t.routing = Some(routing.clone());
        store
            .update_task(
                &t,
                Event::ExecutionGated {
                    decision: Box::new(routing.execution.clone().expect("decision")),
                },
            )
            .expect("gate")
    }

    #[test]
    fn compound_sets_an_explicit_hint_clears_the_decision_and_records_the_source() {
        let store = store();
        let task = gated_atomic(&store);
        let r = set_execution_mode(
            &store,
            task.id,
            ExecutionMode::Compound,
            "mcp:chatgpt",
            Some("  分けて進めて  ".to_string()),
            OffsetDateTime::now_utc(),
        )
        .expect("set");
        assert!(!r.replan);
        let routing = r.task.routing.expect("routing");
        assert_eq!(
            routing.execution_hint,
            Some(ExecutionHintSpec {
                mode: ExecutionMode::Compound,
                explicit: true
            })
        );
        assert!(routing.execution.is_none(), "the old decision is cleared");
        assert_eq!(
            r.previous_decision.map(|d| d.rule_id),
            Some("compound/score".to_string())
        );
        let events = store.events_for(task.id).expect("events");
        let last = events.last().map(|(_, e)| e.clone());
        match last {
            Some(Event::ExecutionHintSet {
                mode,
                previous,
                source,
                note,
                replan,
                previous_decision,
            }) => {
                assert_eq!(mode, ExecutionMode::Compound);
                assert_eq!(previous.map(|p| p.explicit), Some(false));
                assert_eq!(source, "mcp:chatgpt");
                assert_eq!(note.as_deref(), Some("分けて進めて"));
                assert!(!replan);
                assert!(previous_decision.is_some_and(|d| d.shadow));
            }
            other => panic!("unexpected last event {other:?}"),
        }
        assert_eq!(pending_replan_request(&events), None);
    }

    #[test]
    fn running_and_terminal_tasks_are_refused() {
        let store = store();
        let task = gated_atomic(&store);
        store
            .apply_transition(task.id, Trigger::Dispatch, None)
            .expect("dispatch");
        let err = set_execution_mode(
            &store,
            task.id,
            ExecutionMode::Compound,
            "human",
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
        store
            .apply_transition(task.id, Trigger::WorkerError { retryable: false }, None)
            .expect("fail");
        let err = set_execution_mode(
            &store,
            task.id,
            ExecutionMode::Compound,
            "human",
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("retry"),
            "a terminal task points at retry: {err}"
        );
    }

    #[test]
    fn out_of_scope_tasks_and_long_notes_are_validation_errors() {
        let store = store();
        // `routing` の無い旧タスク（Phase 114 より前）は gate の対象外。
        let legacy = crate::add::create_task(&store, spec("legacy"), OffsetDateTime::now_utc())
            .expect("create");
        let mut legacy_task = store.get(legacy.id).expect("get").expect("some");
        legacy_task.routing = None;
        store
            .update_task(
                &legacy_task,
                Event::Edited {
                    fields: vec!["routing".into()],
                    by: "test".into(),
                },
            )
            .expect("update");
        let err = set_execution_mode(
            &store,
            legacy.id,
            ExecutionMode::Compound,
            "human",
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
        let task = gated_atomic(&store);
        let err = set_execution_mode(
            &store,
            task.id,
            ExecutionMode::Compound,
            "human",
            Some("x".repeat(NOTE_MAX_CHARS + 1)),
            OffsetDateTime::now_utc(),
        )
        .unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    }

    #[test]
    fn a_replan_request_is_pending_until_a_plan_or_a_run_follows() {
        let hint = |replan| Event::ExecutionHintSet {
            mode: ExecutionMode::Compound,
            previous: None,
            previous_decision: None,
            source: "human".to_string(),
            note: Some("migration を先に".to_string()),
            replan,
        };
        let running = Event::Transitioned {
            from: Status::Ready,
            to: Status::Running,
            reason: "dispatch".to_string(),
        };
        assert_eq!(
            pending_replan_request(&[(1, hint(true))]).as_deref(),
            Some("migration を先に")
        );
        assert_eq!(pending_replan_request(&[(1, hint(false))]), None);
        assert_eq!(
            pending_replan_request(&[(1, hint(true)), (2, running)]),
            None
        );
    }
}
