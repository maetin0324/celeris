use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};
use crate::model::{Event, TaskId};
use crate::repos::RepoId;
use crate::{TaskRepoIntegrationState, TaskRepoIntegrationStatus};
use rusqlite::{OptionalExtension, params};

type IntegrationStateRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
);

impl SqliteStore {
    pub(super) fn task_integration_upsert_impl(
        &self,
        value: &TaskRepoIntegrationState,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let old: Option<String> = tx
            .query_row(
                "SELECT state FROM task_integrations WHERE task_id=?1 AND repo_id=?2",
                params![value.task_id.to_string(), value.repo_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute("INSERT INTO task_integrations(task_id,repo_id,branch,head_sha,state,reason,actor,checked_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(task_id,repo_id) DO UPDATE SET branch=excluded.branch,head_sha=excluded.head_sha,state=excluded.state,reason=excluded.reason,actor=excluded.actor,checked_at=excluded.checked_at,updated_at=excluded.updated_at", params![value.task_id.to_string(), value.repo_id.to_string(), value.branch, value.head_sha, value.state.as_str(), value.reason, value.actor, format_rfc3339(value.checked_at)?, format_rfc3339(value.updated_at)?])?;
        if old.as_deref() != Some(value.state.as_str()) {
            let event = match (old.as_deref(), value.state) {
                (Some("held" | "discarded"), TaskRepoIntegrationStatus::Unintegrated) => {
                    Event::IntegrationReopened {
                        repo_id: value.repo_id,
                        branch: value.branch.clone(),
                        head_sha: value.head_sha.clone(),
                        reason: value.reason.clone(),
                        actor: value.actor.clone(),
                    }
                }
                (_, TaskRepoIntegrationStatus::Unintegrated) => Event::IntegrationDetected {
                    repo_id: value.repo_id,
                    branch: value.branch.clone(),
                    head_sha: value.head_sha.clone(),
                    reason: value.reason.clone(),
                    actor: value.actor.clone(),
                },
                (_, TaskRepoIntegrationStatus::Integrated) => Event::IntegrationMerged {
                    repo_id: value.repo_id,
                    branch: value.branch.clone(),
                    head_sha: value.head_sha.clone(),
                    actor: value.actor.clone(),
                },
                (_, TaskRepoIntegrationStatus::Discarded) => Event::IntegrationDiscarded {
                    repo_id: value.repo_id,
                    branch: value.branch.clone(),
                    head_sha: value.head_sha.clone(),
                    reason: value.reason.clone().unwrap_or_default(),
                    actor: value.actor.clone(),
                },
                (_, TaskRepoIntegrationStatus::Held) => Event::IntegrationHeld {
                    repo_id: value.repo_id,
                    branch: value.branch.clone(),
                    head_sha: value.head_sha.clone(),
                    reason: value.reason.clone().unwrap_or_default(),
                    actor: value.actor.clone(),
                },
            };
            Self::append_event_tx(&tx, value.task_id, &event)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(super) fn task_integration_get_impl(
        &self,
        task_id: TaskId,
        repo_id: RepoId,
    ) -> Result<Option<TaskRepoIntegrationState>, StoreError> {
        self.with_read_conn(|c| { let raw: Option<IntegrationStateRow> = c.query_row("SELECT branch,head_sha,state,reason,actor,checked_at,updated_at FROM task_integrations WHERE task_id=?1 AND repo_id=?2", params![task_id.to_string(),repo_id.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
            raw.map(|(branch,head_sha,state,reason,actor,checked_at,updated_at)| { let state=TaskRepoIntegrationStatus::parse(&state).ok_or_else(|| StoreError::Invalid("invalid task integration state".into()))?; Ok(TaskRepoIntegrationState{task_id,repo_id,branch,head_sha,state,reason,actor,checked_at:parse_rfc3339(&checked_at)?,updated_at:parse_rfc3339(&updated_at)?}) }).transpose() })
    }
    pub(super) fn task_integration_list_impl(
        &self,
        filter: Option<TaskRepoIntegrationStatus>,
    ) -> Result<Vec<TaskRepoIntegrationState>, StoreError> {
        let ids: Vec<(String,String)> = self.with_read_conn(|c| { let mut s=c.prepare("SELECT task_id,repo_id FROM task_integrations WHERE (?1 IS NULL OR state=?1) ORDER BY updated_at DESC")?; let rows=s.query_map(params![filter.map(TaskRepoIntegrationStatus::as_str)],|r|Ok((r.get(0)?,r.get(1)?)))?; rows.collect::<rusqlite::Result<Vec<_>>>().map_err(StoreError::from) })?;
        ids.into_iter()
            .map(|(t, r)| {
                let task_id = t
                    .parse()
                    .map_err(|e| StoreError::Invalid(format!("invalid task id: {e}")))?;
                let repo_id = r
                    .parse()
                    .map_err(|e| StoreError::Invalid(format!("invalid repo id: {e}")))?;
                self.task_integration_get_impl(task_id, repo_id)?
                    .ok_or_else(|| StoreError::Invalid("task integration disappeared".into()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskId;
    use crate::{RepoId, TaskStore};
    #[test]
    fn task_integration_upsert_emits_only_on_state_change_and_lists_by_state() {
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = time::OffsetDateTime::now_utc();
        let value = TaskRepoIntegrationState {
            task_id,
            repo_id,
            branch: "topic".into(),
            head_sha: "abc".into(),
            state: TaskRepoIntegrationStatus::Unintegrated,
            reason: None,
            actor: "sweep".into(),
            checked_at: now,
            updated_at: now,
        };
        store.task_integration_upsert(&value).expect("first upsert");
        assert_eq!(store.events_for(task_id).expect("events").len(), 1);
        store
            .task_integration_upsert(&value)
            .expect("repeat upsert");
        assert_eq!(store.events_for(task_id).expect("events").len(), 1);
        assert_eq!(
            store
                .task_integration_get(task_id, repo_id)
                .expect("get")
                .unwrap()
                .state,
            TaskRepoIntegrationStatus::Unintegrated
        );
        assert_eq!(
            store
                .task_integration_list(Some(TaskRepoIntegrationStatus::Unintegrated))
                .expect("list")
                .len(),
            1
        );
        let mut held = value;
        held.state = TaskRepoIntegrationStatus::Held;
        held.reason = Some("awaiting decision".into());
        store.task_integration_upsert(&held).expect("transition");
        assert_eq!(store.events_for(task_id).expect("events").len(), 2);
    }
}
