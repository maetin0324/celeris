//! Human correction of an applied CoS operation. All source, audit and pause writes share one
//! immediate transaction; the source event stream remains append-only.
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;
use ulid::Ulid;

use super::store::{ChatError, chat_ts};
use crate::decision::{DecisionOrigin, DecisionRequest, DecisionStatus};
use crate::execution_plan::WorkUnitStatus;
use crate::model::{
    Budget, Check, Criterion, Event, Status, Task, TaskId, TaskKind, Tier, WorkerHint,
    WorkspaceSpec,
};
use crate::store::SqliteStore;
use crate::transition::Trigger;

const APPROVAL_HUMAN_PREFIX: &str = "[CoS override: human answer required] ";
const DECISION_HUMAN_PREFIX: &str = "[CoS override: human answer required] ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideAction {
    Revoke,
    Return,
}

#[derive(Debug, Clone)]
pub struct OverrideResult {
    pub operation_id: String,
    pub state: String,
    pub action: OverrideAction,
    pub new_revision: Option<String>,
    pub new_wait_id: Option<String>,
    pub remediation_task_id: Option<String>,
    pub paused_task_ids: Vec<String>,
}

impl SqliteStore {
    /// The newly opened source wait of a human override cannot be answered by CoS, even through
    /// the generic `/cos/operations` endpoint. Provenance is stored with the source itself so a
    /// triage item being marked escalated cannot remove this guard.
    pub fn cos_override_wait_requires_human(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<bool, ChatError> {
        let conn = self.lock()?;
        match kind {
            "decision" => {
                let id = match key.strip_prefix("decision-") {
                    Some(id) => id,
                    None => key,
                };
                let raw: Option<String> = conn
                    .query_row("SELECT json FROM decisions WHERE id=?1", [id], |r| r.get(0))
                    .optional()?;
                Ok(raw
                    .map(|raw| serde_json::from_str::<DecisionRequest>(&raw))
                    .transpose()?
                    .is_some_and(|request| {
                        request.raised_by.origin == DecisionOrigin::Human
                            && request.question.starts_with(DECISION_HUMAN_PREFIX)
                    }))
            }
            "authorization" | "approval" => {
                let id = match key.strip_prefix("authorization-") {
                    Some(id) => id,
                    None => key,
                };
                let question: Option<String> = conn
                    .query_row("SELECT question FROM approvals WHERE id=?1", [id], |r| {
                        r.get(0)
                    })
                    .optional()?;
                Ok(question.is_some_and(|question| question.starts_with(APPROVAL_HUMAN_PREFIX)))
            }
            _ => Ok(false),
        }
    }
    /// Correct one operation once. The applied-state check prevents two human requests racing. The
    /// operation's original payload and event are retained; its current state is a projection.
    pub fn cos_operation_override_at(
        &self,
        id: &str,
        action: OverrideAction,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<OverrideResult, ChatError> {
        if reason.trim().is_empty() {
            return Err(ChatError::Invalid("reason must not be empty".into()));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source = tx.query_row(
            "SELECT target_kind,target_id,action,state,(SELECT id FROM cos_inbox_items WHERE operation_id=cos_operations.id LIMIT 1),updated_at,thread_id,result_json FROM cos_operations WHERE id=?1",
            [id], |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, Option<String>>(4)?,r.get::<_, String>(5)?,r.get::<_, String>(6)?,r.get::<_, Option<String>>(7)?)),
        ).optional()?.ok_or_else(|| ChatError::not_found("operation", id))?;
        let (kind, target, op_action, state, item_id, applied_at, thread_id, result_json) = source;
        if state != "applied" {
            return Err(ChatError::Conflict(format!("operation {id} is {state}")));
        }
        // A CoS operation that created an external effect cannot be undone by a database edit.
        let reversible = matches!(op_action.as_str(), "decision.answer" | "approval.decide")
            && matches!(kind.as_str(), "decision" | "approval");
        let task_id: Option<TaskId> = if kind == "decision" {
            tx.query_row(
                "SELECT task_id FROM decisions WHERE id=?1",
                [&target],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|s| {
                s.parse()
                    .map_err(|_| ChatError::Invalid("invalid decision task id".into()))
            })
            .transpose()?
        } else if kind == "approval" {
            tx.query_row(
                "SELECT task_id FROM approvals WHERE id=?1",
                [&target],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .map(|s| {
                s.parse()
                    .map_err(|_| ChatError::Invalid("invalid approval task id".into()))
            })
            .transpose()?
        } else if kind == "task" {
            target.parse().ok()
        } else {
            None
        };
        let at = chat_ts(now);
        let mut result = OverrideResult {
            operation_id: id.into(),
            state: String::new(),
            action,
            new_revision: None,
            new_wait_id: None,
            remediation_task_id: None,
            paused_task_ids: Vec::new(),
        };
        let consumed: bool = if let Some(task) = task_id {
            tx.query_row("WITH RECURSIVE subtree(id) AS (SELECT ?1 UNION SELECT t.id FROM tasks t JOIN subtree s ON t.parent_id=s.id OR json_extract(t.json,'$.tree.parent_unit.task_id')=s.id) \
                SELECT EXISTS(SELECT 1 FROM runs r JOIN subtree s ON s.id=r.task_id WHERE r.started_at>?2)",
                params![task.to_string(),applied_at], |r| r.get(0))?
        } else {
            false
        };
        if consumed && let Some(task) = task_id {
            // The ancestor pause gates descendants, including children created after this point.
            // Interrupt moves live runs out of running; dispatcher abort_stale_runs stops processes.
            let mut stmt = tx.prepare("WITH RECURSIVE subtree(id) AS (SELECT ?1 UNION SELECT t.id FROM tasks t JOIN subtree s ON t.parent_id=s.id OR json_extract(t.json,'$.tree.parent_unit.task_id')=s.id) SELECT id FROM subtree")?;
            let ids = stmt
                .query_map([task.to_string()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            for raw in ids {
                let child: TaskId = raw
                    .parse()
                    .map_err(|_| ChatError::Invalid("invalid child task id".into()))?;
                if let Some(mut row) = Self::get_locked(&tx, child)? {
                    if child == task
                        && matches!(row.status, Status::Done | Status::Failed)
                        && row.kind == TaskKind::Execute
                    {
                        Self::apply_transition_tx(&tx, child, Trigger::Reopen, vec![])?;
                        row = Self::get_locked(&tx, child)?
                            .ok_or_else(|| ChatError::not_found("task", &raw))?;
                    }
                    if row.status.is_terminal() {
                        continue;
                    }
                    if row.paused_at.is_none() {
                        row.paused_at = Some(now);
                        row.updated_at = now;
                        tx.execute(
                            "UPDATE tasks SET json=?2,updated_at=?3 WHERE id=?1",
                            params![raw, serde_json::to_string(&row)?, at],
                        )?;
                        Self::append_event_tx(
                            &tx,
                            child,
                            &Event::Edited {
                                fields: vec!["paused_at".into()],
                                by: "human".into(),
                            },
                        )?;
                    }
                    if matches!(row.status, Status::Running | Status::Reviewing) {
                        Self::apply_transition_tx(&tx, child, Trigger::Interrupt, vec![])?;
                    }
                    result.paused_task_ids.push(raw);
                }
            }
        }
        let new_state = if reversible {
            "superseded"
        } else {
            "needs_remediation"
        };
        if reversible && kind == "decision" {
            let raw: String = tx
                .query_row(
                    "SELECT json FROM decisions WHERE id=?1 AND status='answered'",
                    [&target],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| {
                    ChatError::Conflict("source decision is no longer answered".into())
                })?;
            let mut request: DecisionRequest = serde_json::from_str(&raw)?;
            if request
                .answer
                .as_ref()
                .is_none_or(|answer| answer.by != "cos")
            {
                return Err(ChatError::Conflict(
                    "a human already changed this decision".into(),
                ));
            }
            let owner =
                task_id.ok_or_else(|| ChatError::Conflict("decision has no task".into()))?;
            Self::append_event_tx(
                &tx,
                owner,
                &Event::DecisionWithdrawn {
                    id: target.clone(),
                    reason: format!("overridden {id}: {reason}"),
                },
            )?;
            request.id = Ulid::new().to_string();
            request.status = DecisionStatus::Open;
            request.answer = None;
            request.withdrawn_reason = None;
            request.raised_by.origin = DecisionOrigin::Human;
            request.raised_by.run_id = None;
            request.question = format!("{DECISION_HUMAN_PREFIX}{}", request.question);
            Self::append_event_tx(
                &tx,
                owner,
                &Event::DecisionRequested {
                    decision: Box::new(request.clone()),
                },
            )?;
            // Answering may have released units before any run consumed the answer. Restore
            // their decision gate so the new human wait is effective immediately.
            let mut stmt=tx.prepare("SELECT id,key,status,phase,needs_decisions_json FROM work_units WHERE task_id=?1 AND status IN ('pending','ready')")?;
            let rows = stmt
                .query_map([owner.to_string()], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            for (unit_id, key, status, phase, needs_json) in rows {
                let needs: Vec<String> = serde_json::from_str(&needs_json)?;
                let named = request.needed_before.iter().any(|name| {
                    name == "self"
                        || name == &key
                        || phase
                            .as_ref()
                            .is_some_and(|stage| name == &format!("stage:{stage}"))
                });
                if !named && !needs.contains(&request.key) {
                    continue;
                }
                let from = WorkUnitStatus::parse(&status)
                    .ok_or_else(|| ChatError::Invalid("invalid work unit status".into()))?;
                tx.execute("UPDATE work_units SET status='blocked',blocked_reason='decision',updated_at=?2 WHERE id=?1",params![unit_id,at])?;
                Self::append_event_tx(
                    &tx,
                    owner,
                    &Event::WorkUnitTransitioned {
                        work_unit_id: unit_id,
                        key,
                        from,
                        to: WorkUnitStatus::Blocked,
                        reason: format!("human_override:{id}"),
                        run_id: None,
                    },
                )?;
            }
            result.new_wait_id = Some(request.id.clone());
            result.new_revision = Some(tx.query_row(
                "SELECT created_at FROM decisions WHERE id=?1",
                [&request.id],
                |r| r.get(0),
            )?);
        } else if reversible && kind == "approval" {
            let new_id = Ulid::new().to_string();
            let changed=tx.execute("INSERT INTO approvals(id,project_id,node_id,task_id,question,decision,answer,created_at,decided_at) \
                SELECT ?2,project_id,node_id,task_id,?4 || question,NULL,NULL,?3,NULL FROM approvals WHERE id=?1 AND decision IS NOT NULL", params![target,new_id,at,APPROVAL_HUMAN_PREFIX])?;
            if changed != 1 {
                return Err(ChatError::Conflict(
                    "source approval is no longer decided".into(),
                ));
            }
            if let Some(raw) = result_json.as_deref() {
                let receipt: serde_json::Value = serde_json::from_str(raw)?;
                if let Some(rule_id) = receipt
                    .get("standing_rule_id")
                    .and_then(|value| value.as_str())
                {
                    rule_id.parse::<Ulid>().map_err(|_| {
                        ChatError::Invalid("invalid standing rule id in operation receipt".into())
                    })?;
                    tx.execute("DELETE FROM standing_rules WHERE id=?1", [rule_id])?;
                }
            }
            result.new_wait_id = Some(new_id);
            result.new_revision = Some(at.clone());
            if let Some(owner) = task_id {
                // `approval.decide` may have moved its blocked task to ready. Restore the
                // source wait as a blocked task before another dispatcher tick can claim it.
                let mut current = Self::get_locked(&tx, owner)?
                    .ok_or_else(|| ChatError::not_found("task", &owner.to_string()))?;
                if matches!(current.status, Status::Running | Status::Reviewing) {
                    Self::apply_transition_tx(&tx, owner, Trigger::Interrupt, vec![])?;
                    current = Self::get_locked(&tx, owner)?
                        .ok_or_else(|| ChatError::not_found("task", &owner.to_string()))?;
                }
                if current.status == Status::Ready {
                    current.status = Status::Blocked;
                    current.lease = None;
                    current.updated_at = now;
                    tx.execute("UPDATE tasks SET status='blocked',json=?2,updated_at=?3,lease_worker_run_id=NULL,lease_expires_at=NULL WHERE id=?1",params![owner.to_string(),serde_json::to_string(&current)?,at])?;
                    Self::append_event_tx(
                        &tx,
                        owner,
                        &Event::Transitioned {
                            from: Status::Ready,
                            to: Status::Blocked,
                            reason: "human_override".into(),
                        },
                    )?;
                }
                Self::append_event_tx(&tx, owner, &Event::ApprovalRequested)?;
            }
        } else {
            let owner = task_id
                .map(|task| Self::get_locked(&tx, task))
                .transpose()?
                .flatten();
            let remediation_id = TaskId::new();
            let mut task = if let Some(original) = owner {
                original
            } else {
                Task {
                    id: remediation_id,
                    requirements: Default::default(),
                    parent_id: None,
                    kind: TaskKind::Execute,
                    title: String::new(),
                    objective: String::new(),
                    acceptance: vec![Criterion {
                        text: "Human confirms the remediation".into(),
                        check: Check::Human,
                    }],
                    inputs: vec![],
                    depends_on: vec![],
                    status: Status::Draft,
                    priority: 0,
                    worker_hint: WorkerHint {
                        tier: Tier::Standard,
                        adapter: None,
                    },
                    workspace: WorkspaceSpec::Local {
                        path: std::path::PathBuf::from(remediation_id.to_string()),
                        mode: None,
                    },
                    repos: vec![],
                    budget: Budget {
                        max_turns: 10,
                        max_wall_secs: 600,
                        max_retries: 1,
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
                    labels: vec![],
                    category: Default::default(),
                    skills: vec![],
                    mode: Default::default(),
                    routing: None,
                    tree: None,
                    paused_at: None,
                }
            };
            let original = task_id;
            task.id = remediation_id;
            // A remediation task must remain runnable while the affected subtree is paused.
            task.parent_id = if consumed { None } else { original };
            task.tree = None;
            task.kind = TaskKind::Execute;
            task.acceptance = vec![Criterion {
                text: "Human confirms the remediation".into(),
                check: Check::Human,
            }];
            task.depends_on.clear();
            task.conversation = None;
            task.routing = None;
            task.aggregate = false;
            task.workspace = WorkspaceSpec::Local {
                path: std::path::PathBuf::from(remediation_id.to_string()),
                mode: None,
            };
            task.status = Status::Draft;
            task.title = format!("Remediate CoS operation {id}");
            task.objective =
                format!("Review and compensate operation {id}. Human reason: {reason}");
            task.attempts = 0;
            task.lease = None;
            task.paused_at = None;
            task.created_at = now;
            task.updated_at = now;
            Self::create_task_tx(&tx, &task, None, vec![])?;
            Self::apply_transition_tx(&tx, task.id, Trigger::Accept, vec![])?;
            result.remediation_task_id = Some(task.id.to_string());
        }
        if let (Some(item_id), Some(revision)) = (item_id.as_ref(), result.new_revision.as_ref()) {
            let row = tx.query_row("SELECT source_kind,source_key,thread_id,policy_version FROM cos_inbox_items WHERE id=?1", [item_id],
                |r| Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?))).optional()?;
            if let Some((source_kind, source_key, inbox_thread, policy)) = row {
                let new_key = match kind.as_str() {
                    "decision" | "approval" => {
                        let wait_id = result
                            .new_wait_id
                            .as_deref()
                            .ok_or_else(|| ChatError::Invalid("override has no new wait".into()))?;
                        format!("{}-{wait_id}", source_kind)
                    }
                    _ => source_key,
                };
                tx.execute("INSERT INTO cos_inbox_items(id,source_kind,source_key,source_revision,thread_id,message_id,state,policy_version,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'','pending',?6,?7,?7)",
                    params![Ulid::new().to_string(),source_kind,new_key,revision,inbox_thread,policy,at])?;
            }
        }
        tx.execute(
            "UPDATE cos_operations SET state=?2,updated_at=?3 WHERE id=?1 AND state='applied'",
            params![id, new_state, at],
        )?;
        let stream = match task_id {
            Some(task) => task,
            None => id
                .parse()
                .map_err(|_| ChatError::Invalid("operation id is not a task id".into()))?,
        };
        Self::append_event_tx(
            &tx,
            stream,
            &Event::CosOperation {
                actor: "human".into(),
                thread_id,
                run_id: String::new(),
                operation_id: id.into(),
                reason: reason.into(),
                policy_version: "override-1".into(),
                state: new_state.into(),
                target_kind: kind,
                target_id: target,
            },
        )?;
        result.state = new_state.into();
        tx.commit()?;
        Ok(result)
    }
}
