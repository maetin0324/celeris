//! ADR-0067 付記 2026-10-07 D3-d: 既存 task の未申告成果物の取りこぼしを一度だけ補完する
//! （`celerisctl workspace backfill-artifacts`）。
//!
//! remote（cluster）の task は付記より前は run 後の走査が一度も走らなかったので、手元の写しの
//! `artifacts/` にある成果物が `Event::ArtifactProduced` に無い。ここでは run を起こさず、DB の task と
//! その作業場所（`task_ops::workspace::local_dir`）から D3-b / D3-c と同じ走査で未登録のものを拾い、
//! `declared: false` で登録する。何度流しても同じ結果（`(path, sha256)` の重複規則）。LLM も daemon も使わない。

use std::path::Path;

use task_core::{ArtifactRef, Event, StoreError, Task, TaskId, TaskStore, WorkspaceSpec};

use super::{registered_keys, scan_undeclared_artifacts_in_dir};

/// 1 task の補完の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReport {
    pub task_id: TaskId,
    /// 登録に使った `run_id`（その task の最後の run。無ければ空文字）。
    pub run_id: String,
    /// 見つかった（`dry_run` でなければ登録した）成果物。
    pub artifacts: Vec<ArtifactRef>,
}

/// 補完の対象になる task（`--task` 無しのとき）: remote（cluster）の task 全部。
pub fn default_targets(store: &dyn TaskStore) -> Result<Vec<Task>, StoreError> {
    let mut tasks: Vec<Task> = store
        .list(None)?
        .into_iter()
        .filter(|t| matches!(t.workspace, WorkspaceSpec::Remote { .. }))
        .collect();
    tasks.sort_by_key(|a| a.id.to_string());
    Ok(tasks)
}

/// その task の最後の run の id（`runs` 索引の `started_at` 最新 → `WorkerStarted` の最後 → 空文字）。
pub fn latest_run_id(store: &dyn TaskStore, task_id: TaskId, events: &[Event]) -> String {
    if let Ok(rows) = store.runs_for_task(task_id)
        && let Some(last) = rows.iter().max_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.run_id.cmp(&b.run_id))
        })
    {
        return last.run_id.clone();
    }
    events
        .iter()
        .rev()
        .find_map(|ev| match ev {
            Event::WorkerStarted { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// 1 task を補完する。`workspace_root` は `[workspace] root`（daemon と同じ）。`dry_run` なら登録しない。
/// 作業場所の写しが無ければ空（エラーにしない。写しが消えた task は補完できない）。
pub fn backfill_task(
    store: &dyn TaskStore,
    task: &Task,
    workspace_root: &Path,
    dry_run: bool,
) -> Result<BackfillReport, StoreError> {
    let workspace = task_ops::workspace::local_dir(task, workspace_root);
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let artifacts_dir = task_core::artifacts::artifacts_dir_for(task, &workspace);
    let events: Vec<Event> = store
        .events_for(task.id)?
        .into_iter()
        .map(|(_, ev)| ev)
        .collect();
    let run_id = latest_run_id(store, task.id, &events);
    let artifacts = if artifacts_dir.is_dir() {
        scan_undeclared_artifacts_in_dir(&workspace, &artifacts_dir, &registered_keys(&events))
    } else {
        Vec::new()
    };
    if !dry_run {
        for artifact in &artifacts {
            store.append_event(
                task.id,
                &Event::ArtifactProduced {
                    run_id: run_id.clone(),
                    artifact: artifact.clone(),
                },
            )?;
        }
    }
    Ok(BackfillReport {
        task_id: task.id,
        run_id,
        artifacts,
    })
}
