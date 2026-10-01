//! ADR-0098（Phase R7-10）: worker の run が宣言した後続 task（`<artifacts_dir>/followups.json`）を作る。
//!
//! - 出自（元の task X と run R）は呼び出し側（daemon）が**自分で R に割り当てた成果物ディレクトリ**から決める。
//!   ファイルの中身の task id / project id は信じない（D2）。
//! - 案件は X のもの。違う案件を書いた 1 件は拒否する。`repos` の省略は X の repos → 案件の primary（D3）。
//! - `parent` / `assignee` / `adapter` / `workspace` / `cluster` / `workspace_mode` は使わず、`status` は常に `draft`（D3）。
//! - 同じ案件に終端でない同じ題名の task があれば作らない（D4）。
//! - 作った task の `Event::Created.origin` は `worker_run{task_id, run_id}`、X には `WorkerProgress` を残す（D5）。
//!
//! LLM は使わない（DESIGN 原則 1）。検証は `add::build_task_with_roles`（`POST /tasks` と同じ規則）を通す。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use task_core::{CreatedOrigin, Event, GenreSpec, RoleSpec, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::add::{NewTaskSpec, SpecOrigin, SpecProvenance, build_task_with_roles};
use crate::error::OpsError;

/// 後続の宣言ファイルの名前（成果物ディレクトリの中。`delegate.json` / `result.json` と同じ置き場）。
pub const FOLLOWUPS_FILE_NAME: &str = "followups.json";
/// ADR-0098 D6: daemon が worker の run に渡す env。`celerisctl add` は `ENV_RUN_DB` の DB に向けたとき、DB を開かずにここへ追記する。
pub const ENV_FOLLOWUPS_FILE: &str = "CELERIS_FOLLOWUPS_FILE";
/// ADR-0098 D6: run から見た daemon の DB（ADR-0095 の読み取り専用の DB）。`celerisctl` の `--db` の既定の最後の候補で、
/// `add` はこの DB に向けたときだけ後続の宣言になる（試験が一時 DB に書く `add` を巻き込まない）。
pub const ENV_RUN_DB: &str = "CELERIS_RUN_DB";
/// ADR-0098 D6: 情報用（daemon は読まない）。
pub const ENV_TASK_ID: &str = "CELERIS_TASK_ID";
/// ADR-0098 D6: 情報用（daemon は読まない）。
pub const ENV_RUN_ID: &str = "CELERIS_RUN_ID";
/// 1 回の run が宣言できる後続の上限（超えた分は作らずに進行へ 1 行）。
pub const MAX_FOLLOWUPS_PER_RUN: usize = 20;

/// 適用済みのファイルの名前（`followups.<run_id>.applied.json`）。記録として残し、二度目の適用を防ぐ。
pub fn applied_file_name(run_id: &str) -> String {
    format!("followups.{run_id}.applied.json")
}

/// `followups.json` の形。要素は 1 件ずつ `NewTaskSpec` として読む（1 件の綴り間違いで他を落とさない）。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowupsFile {
    pub tasks: Vec<serde_json::Value>,
}

/// ADR-0098 D6: `celerisctl add` が run の中で呼ばれたとき、spec を `path` に追記する。戻り値は追記後の件数。
/// 既存のファイルが読めない（壊れている）ときは上書きせずにエラーにする（先に宣言した後続を消さない）。
pub fn append_to_file(path: &Path, spec: &NewTaskSpec) -> Result<usize, OpsError> {
    let mut file = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<FollowupsFile>(&text).map_err(|e| {
            OpsError::Validation(format!(
                "{} exists but is not a followups file ({e}); fix or remove it first",
                path.display()
            ))
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FollowupsFile::default(),
        Err(e) => {
            return Err(OpsError::Validation(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let value = serde_json::to_value(spec)
        .map_err(|e| OpsError::Validation(format!("cannot serialise the task spec: {e}")))?;
    file.tasks.push(value);
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)
            .map_err(|e| OpsError::Validation(format!("cannot create {}: {e}", dir.display())))?;
    }
    let text = serde_json::to_string_pretty(&file)
        .map_err(|e| OpsError::Validation(format!("cannot serialise {}: {e}", path.display())))?;
    std::fs::write(path, text + "\n")
        .map_err(|e| OpsError::Validation(format!("cannot write {}: {e}", path.display())))?;
    Ok(file.tasks.len())
}

/// ADR-0098 D3: 元の task `origin` に結び付けた spec と、落とした欄の説明（進行に残す）を返す。
/// 違う案件を指した spec は `OpsError::Validation`。
pub fn bind_to_origin(
    store: &dyn TaskStore,
    origin: &Task,
    mut spec: NewTaskSpec,
) -> Result<(NewTaskSpec, Vec<String>), OpsError> {
    let mut notes = Vec::new();
    match (spec.project_id, origin.project_id) {
        (Some(asked), Some(own)) if asked != own => {
            return Err(OpsError::Validation(format!(
                "project_id {asked} is not the originating task's project {own}; a project's worker \
                 can only create tasks in its own project (ADR-0098 D3)"
            )));
        }
        (Some(asked), None) => {
            return Err(OpsError::Validation(format!(
                "project_id {asked} given, but the originating task belongs to no project; a worker \
                 cannot choose a project (a human can attach one with PATCH /tasks/{{id}}; ADR-0098 D3/D7)"
            )));
        }
        _ => {}
    }
    spec.project_id = origin.project_id;
    // repos の省略 → X の repos（今の名前で。消えたものは落とす）→ 空なら `build_task` が案件の primary を選ぶ。
    if spec.repos.is_empty()
        && let Some(project_id) = origin.project_id
        && !origin.repos.is_empty()
    {
        let available = store.repo_list(project_id)?;
        spec.repos = origin
            .repos
            .iter()
            .filter_map(|r| {
                available
                    .iter()
                    .find(|a| a.id == r.repo_id)
                    .map(|a| a.name.clone())
            })
            .collect();
    }
    if let Some(parent) = spec.parent.take() {
        notes.push(format!(
            "parent {parent} ignored (a follow-up is an independent task; use delegate.json for children)"
        ));
    }
    if let Some(assignee) = spec.assignee.take().filter(|a| !a.trim().is_empty()) {
        notes.push(format!(
            "assignee {assignee} ignored (celeris assigns the owner deterministically; ADR-0069 D1)"
        ));
    }
    if let Some(adapter) = spec.adapter.take() {
        notes.push(format!("adapter {adapter} ignored (routing is celeris's)"));
    }
    let dropped_workspace = spec.workspace.take().is_some();
    let dropped_cluster = spec.cluster.take().is_some();
    let dropped_mode = spec.workspace_mode.take().is_some();
    if dropped_workspace || dropped_cluster || dropped_mode {
        notes.push(
            "workspace/cluster/workspace_mode ignored (the workspace follows the project's repos)"
                .to_string(),
        );
    }
    if let Some(status) = spec.status
        && status != Status::Draft
    {
        notes.push(format!(
            "status {status:?} ignored; follow-ups start as draft and wait for a human Go (ADR-0098 D3)"
        ));
    }
    spec.status = Some(Status::Draft);
    spec.provenance = SpecProvenance {
        origin: SpecOrigin::Agent,
        ..SpecProvenance::default()
    };
    Ok((spec, notes))
}

/// ADR-0098 D4: 同じ案件（`project_id` が同じ。無ければ案件無し同士）に終端でない同じ題名の task があればその id。
fn live_duplicate(
    store: &dyn TaskStore,
    project_id: Option<task_core::ProjectId>,
    title: &str,
) -> Result<Option<TaskId>, OpsError> {
    let title = title.trim();
    Ok(store
        .list(None)?
        .into_iter()
        .find(|t| !t.status.is_terminal() && t.project_id == project_id && t.title.trim() == title)
        .map(|t| t.id))
}

/// 1 件の結果。
#[derive(Debug, Clone, PartialEq)]
pub enum FollowupResult {
    Created(Box<Task>),
    /// 同じ案件に終端でない同じ題名の task がある（D4）。
    Duplicate(TaskId),
}

/// ADR-0098 D3〜D5: run `run_id`（task `origin` の run）が宣言した 1 件を作る。
pub fn create_followup(
    store: &dyn TaskStore,
    origin: &Task,
    run_id: &str,
    spec: NewTaskSpec,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<(FollowupResult, Vec<String>), OpsError> {
    let (spec, notes) = bind_to_origin(store, origin, spec)?;
    if let Some(existing) = live_duplicate(store, spec.project_id, &spec.title)? {
        return Ok((FollowupResult::Duplicate(existing), notes));
    }
    let task = build_task_with_roles(store, spec, roles, genres, now)?;
    store.create_task_with_origin(
        &task,
        Some(CreatedOrigin::WorkerRun {
            task_id: origin.id,
            run_id: run_id.to_string(),
        }),
        vec![],
    )?;
    Ok((FollowupResult::Created(Box::new(task)), notes))
}

/// `absorb_followups_file` の結果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AbsorbOutcome {
    pub created: Vec<TaskId>,
    /// 元の task の `WorkerProgress` に残した行（同じ文面）。
    pub notes: Vec<String>,
    /// 改名した先（適用済みの記録）。改名できなければ `None`。
    pub applied_path: Option<PathBuf>,
}

/// ADR-0098 D1: `<artifacts_dir>/followups.json` があれば読んで、task `origin_id` の run `run_id` の後続として
/// 1 件ずつ作る。ファイルは先に `followups.<run_id>.applied.json` へ改名する（二度目の適用を防ぐ）。
/// **lease の確認は呼び出し側**（daemon の `run_holds_lease` と同じ規則）。ファイルが無ければ `Ok(None)`。
/// 1 件の失敗は他を止めず、理由は `origin_id` の `WorkerProgress` に残す。
pub fn absorb_followups_file(
    store: &dyn TaskStore,
    origin_id: TaskId,
    run_id: &str,
    artifacts_dir: &Path,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    now: OffsetDateTime,
) -> Result<Option<AbsorbOutcome>, OpsError> {
    let path = artifacts_dir.join(FOLLOWUPS_FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(OpsError::Validation(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let mut outcome = AbsorbOutcome::default();
    let applied = artifacts_dir.join(applied_file_name(run_id));
    match std::fs::rename(&path, &applied) {
        Ok(()) => outcome.applied_path = Some(applied),
        Err(e) => outcome.notes.push(format!(
            "follow-ups: could not rename {} ({e}); duplicates are still suppressed by title",
            path.display()
        )),
    }
    let Some(origin) = store.get(origin_id)? else {
        return Err(OpsError::NotFound(origin_id));
    };
    match serde_json::from_str::<FollowupsFile>(&text) {
        Err(e) => outcome.notes.push(format!(
            "follow-ups: {FOLLOWUPS_FILE_NAME} ignored ({e}); expected {{\"tasks\": [<POST /tasks body>, …]}}"
        )),
        Ok(file) => {
            for (i, value) in file.tasks.into_iter().enumerate() {
                if i >= MAX_FOLLOWUPS_PER_RUN {
                    outcome.notes.push(format!(
                        "follow-ups: only the first {MAX_FOLLOWUPS_PER_RUN} are created per run; the rest were dropped"
                    ));
                    break;
                }
                let spec = match serde_json::from_value::<NewTaskSpec>(value) {
                    Ok(spec) => spec,
                    Err(e) => {
                        outcome
                            .notes
                            .push(format!("follow-up [{i}] rejected: not a task spec ({e})"));
                        continue;
                    }
                };
                let title = spec.title.clone();
                match create_followup(store, &origin, run_id, spec, roles, genres, now) {
                    Ok((result, notes)) => {
                        for note in notes {
                            outcome.notes.push(format!("follow-up [{i}] {title:?}: {note}"));
                        }
                        match result {
                            FollowupResult::Created(task) => {
                                outcome.notes.push(format!(
                                    "follow-up created: {} {:?} (project {}, repos [{}], draft; ADR-0098)",
                                    task.id,
                                    task.title,
                                    task.project_id
                                        .map(|p| p.to_string())
                                        .unwrap_or_else(|| "none".to_string()),
                                    task.repos
                                        .iter()
                                        .map(|r| r.name.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
                                outcome.created.push(task.id);
                            }
                            FollowupResult::Duplicate(existing) => outcome.notes.push(format!(
                                "follow-up [{i}] {title:?} not created: live task {existing} has the same title in this project"
                            )),
                        }
                    }
                    Err(e) => outcome
                        .notes
                        .push(format!("follow-up [{i}] {title:?} rejected: {e}")),
                }
            }
        }
    }
    for note in &outcome.notes {
        store.append_event(origin_id, &Event::worker_progress(run_id, note.clone()))?;
    }
    Ok(Some(outcome))
}

#[cfg(test)]
mod tests;
