//! Task routing audit projection. Historical shadow events remain readable for existing runs.
use crate::error::OpsError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::routing_shadow_history::{ShadowKind, ShadowReason, ShadowRecord, ShadowStatus};
use task_core::{Event, RoutingAudit, SqliteStore, TaskId, TaskStore};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingShadowAudit {
    pub shadow_id: String,
    pub primary_decision_id: String,
    pub kind: ShadowKind,
    pub status: ShadowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ShadowReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub differs_from_primary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunRoutingAudit {
    #[serde(flatten)]
    pub audit: RoutingAudit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_shadow: Option<Vec<RoutingShadowAudit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestRoutingAudit {
    pub request_id: Option<String>,
    pub decision_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRoutingAudit {
    pub runs: Vec<RunRoutingAudit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unbound_requests: Vec<RequestRoutingAudit>,
}

pub trait RequestLog {}
impl RequestLog for SqliteStore {}

pub fn task_routing_audit(
    store: &dyn TaskStore,
    task_id: TaskId,
) -> Result<Vec<RoutingAudit>, OpsError> {
    let task = store.get(task_id)?.ok_or(OpsError::NotFound(task_id))?;
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    Ok(task_core::routing_audit(&task, &events))
}

pub fn task_routing_audit_with_requests(
    store: &dyn TaskStore,
    _log: &dyn RequestLog,
    task_id: TaskId,
) -> Result<TaskRoutingAudit, OpsError> {
    let task = store.get(task_id)?.ok_or(OpsError::NotFound(task_id))?;
    let events: Vec<Event> = store
        .events_for(task_id)?
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    Ok(routing_audit_with_requests(&task, &events, _log))
}

pub fn routing_audit_with_requests(
    task: &task_core::Task,
    events: &[Event],
    _log: &dyn RequestLog,
) -> TaskRoutingAudit {
    let mut runs: Vec<_> = task_core::routing_audit(task, events)
        .into_iter()
        .map(|audit| RunRoutingAudit {
            decision_id: audit
                .routing_features
                .as_ref()
                .map(|record| record.decision_id.clone()),
            audit,
            routing_shadow: None,
        })
        .collect();
    for event in events {
        let Event::RoutingShadowRecorded { record } = event else {
            continue;
        };
        let index = record
            .run_id
            .as_deref()
            .and_then(|run| runs.iter().position(|r| r.audit.run_id == run))
            .or_else(|| {
                runs.iter().position(|r| {
                    r.decision_id.as_deref() == Some(record.primary_decision_id.as_str())
                })
            });
        if let Some(index) = index {
            runs[index]
                .routing_shadow
                .get_or_insert_with(Vec::new)
                .push(shadow_audit(record));
        }
    }
    TaskRoutingAudit {
        runs,
        unbound_requests: Vec::new(),
    }
}

fn shadow_audit(record: &ShadowRecord) -> RoutingShadowAudit {
    RoutingShadowAudit {
        shadow_id: record.shadow_id.clone(),
        primary_decision_id: record.primary_decision_id.clone(),
        kind: record.kind,
        status: record.status,
        reason: record.reason,
        candidate_model: record.candidate_model.clone(),
        candidate_source: record.candidate_source.clone(),
        differs_from_primary: None,
        input_tokens: record.input_tokens,
        output_tokens: record.output_tokens,
    }
}

#[cfg(test)]
#[path = "routing_audit_tests.rs"]
mod tests;
