//! ADR 2026-11-12-cos-triage-scope D4: the deterministic match the daemon attaches to each inbox
//! item it hands a CoS run (`task_ops::triage_match`, the same function the resolve API uses).
//! Reads the store only; no LLM.

use std::collections::HashMap;

use task_core::chat::triage::CosTriageItemReport;
use task_core::store::SqliteStore;
use task_ops::triage_match::{MatchInput, match_item};
use time::OffsetDateTime;

/// One prompt line per item (`None` when nothing matched). The project of an item is its thread's;
/// a store error is the launch's failure (CoS does not judge without the ledger it is told about).
pub(crate) fn triage_hints(
    store: &SqliteStore,
    items: &[CosTriageItemReport],
    now: OffsetDateTime,
) -> Result<Vec<Option<String>>, String> {
    let mut projects: HashMap<String, Option<String>> = HashMap::new();
    items
        .iter()
        .map(|item| {
            if !projects.contains_key(&item.thread_id) {
                let project = store
                    .chat_thread_get(&item.thread_id)
                    .map_err(|e| format!("CoS inbox thread unavailable: {e}"))?
                    .and_then(|t| t.project_id);
                projects.insert(item.thread_id.clone(), project);
            }
            let project = projects.get(&item.thread_id).cloned().flatten();
            let input = MatchInput {
                source_kind: &item.source_kind,
                title: &item.summary,
                body: &item.detail,
                project_id: project.as_deref(),
            };
            match_item(store, input, now)
                .map(|m| m.prompt_line())
                .map_err(|e| format!("CoS triage match unavailable (authority ledger): {e}"))
        })
        .collect()
}

#[cfg(test)]
#[path = "triage_hint_tests.rs"]
mod tests;
