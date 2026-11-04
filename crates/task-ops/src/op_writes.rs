//! 人の回答が書く行の書き込み口（ADR 2026-11-04-project-controller D12・付記 A1 の 9）。
//!
//! gate・phase/plan gate・rereview・統合の扱いは、検証は同じで、書き込みだけが呼び出し側で変わる。
//! [`PlainWrites`] は store の 1 transaction、[`WritesAlso`] はその transaction に呼び出し側の書き込み
//! （`human_epoch`）を足す。

use task_core::store::SqliteStore;
use task_core::transition::Outcome;
use task_core::{Event, StoreError, TaskId, TaskRepoIntegrationState, TaskStore, Trigger};

pub trait OpWrites {
    fn transition(
        &self,
        id: TaskId,
        trigger: Trigger,
        events: Vec<Event>,
    ) -> Result<Outcome, StoreError>;
    fn append_event(&self, id: TaskId, event: &Event) -> Result<(), StoreError>;
    fn integration_upsert(&self, state: &TaskRepoIntegrationState) -> Result<(), StoreError>;
}

/// 足す書き込みがない経路（MCP・daemon・試験）。
pub struct PlainWrites<'a>(pub &'a dyn TaskStore);

impl OpWrites for PlainWrites<'_> {
    fn transition(
        &self,
        id: TaskId,
        trigger: Trigger,
        events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        self.0.apply_transition_with_events(id, trigger, events)
    }
    fn append_event(&self, id: TaskId, event: &Event) -> Result<(), StoreError> {
        self.0.append_event(id, event).map(|_| ())
    }
    fn integration_upsert(&self, state: &TaskRepoIntegrationState) -> Result<(), StoreError> {
        self.0.task_integration_upsert(state)
    }
}

/// どの書き込みも `also(tx, task)` と同じ transaction で書く。
pub struct WritesAlso<'a, F> {
    pub store: &'a SqliteStore,
    pub also: F,
}

impl<F> OpWrites for WritesAlso<'_, F>
where
    F: Fn(&rusqlite::Connection, TaskId) -> Result<(), StoreError>,
{
    fn transition(
        &self,
        id: TaskId,
        trigger: Trigger,
        events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        self.store.write_tx(|tx| {
            let outcome = SqliteStore::apply_transition_tx(tx, id, trigger, events)?;
            (self.also)(tx, id)?;
            Ok(outcome)
        })
    }
    fn append_event(&self, id: TaskId, event: &Event) -> Result<(), StoreError> {
        self.store.write_tx(|tx| {
            SqliteStore::append_event_tx(tx, id, event)?;
            (self.also)(tx, id)
        })
    }
    fn integration_upsert(&self, state: &TaskRepoIntegrationState) -> Result<(), StoreError> {
        self.store.write_tx(|tx| {
            SqliteStore::task_integration_write_tx(tx, state)?;
            (self.also)(tx, state.task_id)
        })
    }
}
