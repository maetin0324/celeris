//! ADR 2026-10-07-build-tmp-hygiene 付記 A3（2026-10-09）: 作業場所（`<workspace_root>/<task>/repos/<repo>`）の中に
//! できた cargo の target を見つけ、build 中でなければ消す。
//!
//! - **cargo の target** = `CACHEDIR.TAG` を持つか、`<profile>/.cargo-lock`（`<target>/<profile>` または
//!   `<target>/<triple>/<profile>`）を持つ実 dir。symlink は辿らない。
//! - **build 中** = どれかの profile の `.cargo-lock` が `flock(LOCK_EX | LOCK_NB)` で取れない（cargo は build の間
//!   この lock を持つ）。
//! - 消すのは終端（`done`・`cancelled`・`failed`）になってから一定時間経った task だけ（実行中の task は見ない）。
//!
//! 消し方は target sweep（D1.2 規則 4）と同じ: lock を持ったまま `<repo>/.deleting-<name>` へ rename し、lock を
//! 放してから `remove_dir_all`。**LLM は呼ばない。**

use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};
use task_core::{Status, StoreError, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::scratch::DELETING_PREFIX;

/// cargo が target の根に置く印。
pub const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
/// cargo が profile dir に置き、build の間 flock する file。
pub const CARGO_LOCK: &str = ".cargo-lock";
/// Only this cleaner owns these tombstones.
pub const TARGET_DELETING_PREFIX: &str = ".deleting-cargo-target-";
/// 作業場所の中で探す target の相対 path（各 worktree の直下）。
pub const WORKSPACE_TARGET_NAME: &str = "target";

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_file())
        .unwrap_or(false)
}

fn sorted_children(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    out.sort();
    out
}

/// `<target>/<profile>` と `<target>/<triple>/<profile>` のうち `.cargo-lock` を持つ実 dir（名前順）。
pub fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (name, p) in sorted_children(target) {
        if name.starts_with(DELETING_PREFIX) || !is_real_dir(&p) {
            continue;
        }
        if is_real_file(&p.join(CARGO_LOCK)) {
            out.push(p);
            continue;
        }
        for (sub, q) in sorted_children(&p) {
            if !sub.starts_with(DELETING_PREFIX)
                && is_real_dir(&q)
                && is_real_file(&q.join(CARGO_LOCK))
            {
                out.push(q);
            }
        }
    }
    out
}

/// `dir` が cargo の target か（`CACHEDIR.TAG` か、`.cargo-lock` を持つ profile がある実 dir）。
pub fn is_cargo_target(dir: &Path) -> bool {
    is_real_dir(dir) && (is_real_file(&dir.join(CACHEDIR_TAG)) || !profile_dirs(dir).is_empty())
}

/// target の全 profile の lock。drop で解放する。
pub struct TargetLock {
    _locks: Vec<Flock<File>>,
}

/// target のすべての profile の `.cargo-lock` を非 blocking の排他で取る。1 つでも取れなければ `None`
/// （build 中。取れた分はすぐ放す）。profile が無ければ空の lock（消してよい）。読み取りで開くので file を作らない。
pub fn try_lock_target(target: &Path) -> Option<TargetLock> {
    fn collect(dir: &Path, depth: u8, locks: &mut Vec<Flock<File>>) -> Option<()> {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let lock_path = path.join(CARGO_LOCK);
            match std::fs::symlink_metadata(&lock_path) {
                Ok(meta) if meta.file_type().is_file() => {
                    let file = File::open(lock_path).ok()?;
                    locks.push(Flock::lock(file, FlockArg::LockExclusiveNonblock).ok()?);
                }
                // Unknown or redirected lock files cannot prove the target idle.
                Ok(_) => return None,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if depth > 1 {
                        collect(&path, depth - 1, locks)?;
                    }
                }
                Err(_) => return None,
            }
        }
        Some(())
    }
    if !is_real_dir(target) {
        return None;
    }
    let mut locks = Vec::new();
    collect(target, 2, &mut locks)?;
    Some(TargetLock { _locks: locks })
}

/// 木の実使用量（`st_blocks × 512`。symlink は辿らない）。
pub fn tree_bytes(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    let mut bytes = meta.blocks().saturating_mul(512);
    if meta.file_type().is_dir() {
        for (_, child) in sorted_children(path) {
            bytes = bytes.saturating_add(tree_bytes(&child));
        }
    }
    bytes
}

/// 作業場所 1 つ（`<workspace_root>/<task>`）の worktree の直下にある cargo の target と、前回の残り
/// （`<repo>/.deleting-*`）。
pub fn task_targets(task_dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut targets = Vec::new();
    let mut leftovers = Vec::new();
    let mut repos = crate::workspace_prune::worktree_dirs(task_dir);
    let units = task_dir.join("wu");
    if is_real_dir(task_dir) && is_real_dir(&units) {
        for (_, unit) in sorted_children(&units) {
            repos.extend(crate::workspace_prune::worktree_dirs(&unit));
        }
    }
    for repo in repos {
        let target = repo.join(WORKSPACE_TARGET_NAME);
        if is_cargo_target(&target) {
            targets.push(target);
        }
        for (name, p) in sorted_children(&repo) {
            if name.starts_with(TARGET_DELETING_PREFIX) && is_cargo_target(&p) {
                leftovers.push(p);
            }
        }
    }
    (targets, leftovers)
}

/// 消す候補の作業場所 1 つ分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedTaskTargets {
    pub task_id: TaskId,
    pub targets: Vec<PathBuf>,
    pub leftovers: Vec<PathBuf>,
}

/// 終端でも消さずに残した target（[`scan_finished_task_targets`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldTaskTargets {
    pub task_id: TaskId,
    pub targets: Vec<PathBuf>,
    /// `running_run`（木のどれかに running の run）・`active_descendant`（終端でない子孫）・`grace`（木の最後の
    /// 終端から猶予内）。
    pub reason: &'static str,
}

/// [`scan_finished_task_targets`] の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FinishedScan {
    pub found: Vec<FinishedTaskTargets>,
    pub held: Vec<HeldTaskTargets>,
}

/// ADR 2026-10-10-local-disk-growth-paths D2: checkout を持つ task とその子孫（`parent_id` の鎖）を見て、
/// 消してはいけない理由を返す（`None` = 全員が終端・running の run なし・最後の終端から `after_secs` 経過）。
/// `repos=[]` の子 task は親の checkout で cargo を走らせるので、子が 1 つでも動いていれば親の target は残す。
pub fn tree_hold(
    store: &dyn TaskStore,
    root: &task_core::Task,
    running: &std::collections::HashSet<String>,
    now: OffsetDateTime,
    after_secs: u64,
) -> Result<Option<&'static str>, StoreError> {
    let grace = i64::try_from(after_secs).unwrap_or(i64::MAX);
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![root.clone()];
    let mut grace_hold = false;
    while let Some(task) = stack.pop() {
        if !seen.insert(task.id) {
            continue;
        }
        if running.contains(&task.id.to_string()) {
            return Ok(Some("running_run"));
        }
        if !task.status.is_terminal() {
            return Ok(Some("active_descendant"));
        }
        if (now - task.updated_at).whole_seconds() < grace {
            grace_hold = true;
        }
        stack.extend(store.children(task.id)?);
    }
    Ok(grace_hold.then_some("grace"))
}

/// 終端（done・cancelled・failed）の task の作業場所に残る cargo の target を、消してよいもの（`found`）と
/// 残すもの（`held`。[`tree_hold`] の理由付き）に分ける（task id の順）。終端でない task は見ない。
pub fn scan_finished_task_targets(
    store: &dyn TaskStore,
    workspace_root: &Path,
    now: OffsetDateTime,
    after_secs: u64,
) -> Result<FinishedScan, StoreError> {
    let mut out = FinishedScan::default();
    if after_secs == 0 {
        return Ok(out);
    }
    let running: std::collections::HashSet<_> = store
        .runs_running()?
        .into_iter()
        .map(|r| r.task_id)
        .collect();
    let mut terminal = Vec::new();
    for status in [Status::Done, Status::Cancelled, Status::Failed] {
        terminal.extend(store.list(Some(status))?);
    }
    terminal.sort_by_key(|t| t.id.to_string());
    for task in terminal {
        let (targets, leftovers) = task_targets(&workspace_root.join(task.id.to_string()));
        if targets.is_empty() && leftovers.is_empty() {
            continue;
        }
        if let Some(reason) = tree_hold(store, &task, &running, now, after_secs)? {
            if !targets.is_empty() {
                out.held.push(HeldTaskTargets {
                    task_id: task.id,
                    targets,
                    reason,
                });
            }
            continue;
        }
        out.found.push(FinishedTaskTargets {
            task_id: task.id,
            targets,
            leftovers,
        });
    }
    Ok(out)
}

/// 終端（done・cancelled・failed）の task とその子孫が全て終端になり、最後の終端から `after_secs` 以上経った
/// task の作業場所に残る cargo の target（task id の順）。終端でない task は見ない。
pub fn finished_task_targets(
    store: &dyn TaskStore,
    workspace_root: &Path,
    now: OffsetDateTime,
    after_secs: u64,
) -> Result<Vec<FinishedTaskTargets>, StoreError> {
    Ok(scan_finished_task_targets(store, workspace_root, now, after_secs)?.found)
}

/// [`remove_target`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveTarget {
    /// rename まで済んだ（`moved` は消す前の置き場。`bytes` は rename 前に測った量）。
    Moved { moved: PathBuf, bytes: u64 },
    /// build 中（lock が取れない）なので残した。
    Locked,
}

/// `target` の全 profile の lock を取り、持ったまま同じ親の `<DELETING_PREFIX><suffix>` へ rename する
/// （削除は呼び出し側が lock を放した後に `remove_dir_all`）。`apply = false` は量を測って lock を確かめるだけ。
pub fn remove_target(target: &Path, suffix: &str, apply: bool) -> std::io::Result<RemoveTarget> {
    let Some(lock) = try_lock_target(target) else {
        return Ok(RemoveTarget::Locked);
    };
    let bytes = tree_bytes(target);
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::other("target has no parent"))?;
    let moved = parent.join(format!("{TARGET_DELETING_PREFIX}{suffix}"));
    if apply {
        std::fs::rename(target, &moved)?;
    }
    drop(lock);
    Ok(RemoveTarget::Moved { moved, bytes })
}

#[cfg(test)]
mod tests;
