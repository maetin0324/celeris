//! ADR 2026-10-27-unblocks-reopen-on-promotion: 修正 task を含む release の昇格で、その修正を待っている
//! task を 1 度だけ reopen（`failed`）するかコメントを残す（`blocked`）。
//!
//! - [`validate_unblocks`]: 修正 task の作成時（`POST /tasks` / CoS の `task.create`）の `unblocks` の検証。
//! - [`read_promoted_releases`]: `releases_dir` の各 `<sha12>/promoted.json` の `included.tasks` を読む。
//! - [`sweep`]: 未発火の結びのうち、修正 task（またはその祖先）が昇格した release に入ったものを発火させる。
//!
//! どれも決定的（JSON と DB だけ。git も LLM も使わない）。`releases_dir` は呼び出し側（daemon）が渡す。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use task_core::comment::{CommentAuthorKind, TaskComment};
use task_core::unblocks::{MAX_UNBLOCKS, UnblockAction, UnblockFire};
use task_core::{Status, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::error::OpsError;

/// 祖先を辿る上限（木の深さ max_depth 3 より十分広い）。
const ANCESTOR_DEPTH: usize = 16;

/// D1: `unblocks` の検証（読むだけ）。上限・重複・存在を確かめる。
pub fn validate_unblocks(store: &dyn TaskStore, unblocks: &[TaskId]) -> Result<(), OpsError> {
    if unblocks.len() > MAX_UNBLOCKS {
        return Err(OpsError::Validation(format!(
            "unblocks: at most {MAX_UNBLOCKS} tasks (got {})",
            unblocks.len()
        )));
    }
    for (i, id) in unblocks.iter().enumerate() {
        if unblocks[..i].contains(id) {
            return Err(OpsError::Validation(format!(
                "unblocks: task {id} is listed twice"
            )));
        }
        if store.get(*id)?.is_none() {
            return Err(OpsError::Validation(format!(
                "unblocks: task {id} does not exist"
            )));
        }
    }
    Ok(())
}

/// 昇格に成功した release 1 つ（`promoted.json` がある）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedRelease {
    /// release の置き場の名前（`<releases_dir>/<sha12>`）。
    pub sha12: String,
    pub promoted_at: String,
    /// `included.tasks[].task_id`（`included` が `null` なら空）。
    pub task_ids: Vec<String>,
}

/// D2: `releases_dir` の昇格済み release を古い順（`promoted_at`、同時刻は名前順）で返す。読めない・
/// 壊れた `promoted.json` は飛ばす（その release は修正を含むと判定しない）。
pub fn read_promoted_releases(releases_dir: &Path) -> Vec<PromotedRelease> {
    let mut out: Vec<PromotedRelease> = std::fs::read_dir(releases_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let sha12 = entry.file_name().to_str()?.to_owned();
            let raw = std::fs::read(entry.path().join("promoted.json")).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
            let promoted_at = value.get("promoted_at")?.as_str()?.to_owned();
            let task_ids = value
                .pointer("/included/tasks")
                .and_then(serde_json::Value::as_array)
                .map(|tasks| {
                    tasks
                        .iter()
                        .filter_map(|t| t.get("task_id")?.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            Some(PromotedRelease {
                sha12,
                promoted_at,
                task_ids,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.promoted_at
            .cmp(&b.promoted_at)
            .then_with(|| a.sha12.cmp(&b.sha12))
    });
    out
}

/// 発火した結び 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnblockOutcome {
    pub fix_task_id: TaskId,
    pub waiting_task_id: TaskId,
    pub release: String,
    pub action: UnblockAction,
}

/// D2〜D4: 未発火の結びを見て、修正 task（かその祖先）を含む最も古い昇格済み release があれば発火させる。
/// 発火した結びを返す（同じ結びは二度返らない）。
pub fn sweep(
    store: &dyn TaskStore,
    releases: &[PromotedRelease],
    now: OffsetDateTime,
) -> Result<Vec<UnblockOutcome>, OpsError> {
    let pending = store.unblock_pending()?;
    if pending.is_empty() || releases.is_empty() {
        return Ok(Vec::new());
    }
    let mut chains: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    let mut out = Vec::new();
    for link in pending {
        let chain = match chains.entry(link.fix_task_id) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(ancestor_chain(store, link.fix_task_id)?),
        };
        let Some(release) = releases.iter().find(|r| {
            chain
                .iter()
                .any(|id| r.task_ids.iter().any(|t| *t == id.to_string()))
        }) else {
            continue;
        };
        let Some(waiting) = store.get(link.waiting_task_id)? else {
            continue;
        };
        let fix_title = store
            .get(link.fix_task_id)?
            .map(|t| t.title)
            .unwrap_or_default();
        let action = match waiting.status {
            Status::Failed => UnblockAction::Reopened,
            Status::Blocked => UnblockAction::Commented,
            _ => UnblockAction::Skipped,
        };
        let comment = (action != UnblockAction::Skipped).then(|| {
            TaskComment::new(
                link.waiting_task_id,
                CommentAuthorKind::System,
                None,
                comment_body(link.fix_task_id, &fix_title, &release.sha12, action),
                None,
                now,
            )
        });
        let fired = store.unblock_fire(&UnblockFire {
            fix_task_id: link.fix_task_id,
            waiting_task_id: link.waiting_task_id,
            release: release.sha12.clone(),
            action,
            comment,
            now,
        })?;
        if !fired {
            continue;
        }
        if action == UnblockAction::Reopened {
            // `comment::finish_reopen` と同じ: 再開は run の履歴を区切る（冪等）。
            crate::routing_outcome::record_routing_outcomes(store, link.waiting_task_id)?;
        }
        out.push(UnblockOutcome {
            fix_task_id: link.fix_task_id,
            waiting_task_id: link.waiting_task_id,
            release: release.sha12.clone(),
            action,
        });
    }
    Ok(out)
}

/// 修正 task 自身と祖先の id（子 task は親の配送で release に入るため）。
fn ancestor_chain(store: &dyn TaskStore, fix: TaskId) -> Result<Vec<TaskId>, OpsError> {
    let mut chain = vec![fix];
    let mut cursor = fix;
    for _ in 0..ANCESTOR_DEPTH {
        let Some(parent) = store.get(cursor)?.and_then(|t| t.parent_id) else {
            break;
        };
        if chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        cursor = parent;
    }
    Ok(chain)
}

fn comment_body(fix: TaskId, fix_title: &str, release: &str, action: UnblockAction) -> String {
    let fix = if fix_title.is_empty() {
        format!("修正 task {fix}")
    } else {
        format!("修正 task {fix}（{fix_title}）")
    };
    match action {
        UnblockAction::Reopened => format!(
            "{fix} を含む release {release} が昇格したので、この task を再開した\
             （unblocks の結び。ADR 2026-10-27-unblocks-reopen-on-promotion）。"
        ),
        _ => format!(
            "{fix} を含む release {release} が昇格した。この task は blocked のままなので、待ちの理由を解いてから\
             進める（unblocks の結び。ADR 2026-10-27-unblocks-reopen-on-promotion）。"
        ),
    }
}

#[cfg(test)]
mod tests;
