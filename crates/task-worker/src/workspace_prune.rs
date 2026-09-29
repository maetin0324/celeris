//! ADR-0066 D2: 終端タスクの作業場所から、ビルド生成物だけを自動で刈る。
//!
//! ADR-0043 D2 は「中止（cancel）のときだけ worktree ごと消す」と決めている。ここではそれを変えず
//! （done / failed のまま残った worktree は差分を見るために必要）、終端になってから
//! `[workspace] prune_after_secs` 経った作業場所から、`target/` 等のビルド生成物だけを削る。
//! ソースツリー（`.git` を含む）と `artifacts/`、`.celeris/` は残す。
//!
//! シンボリックリンク（ADR-0043 D2 の `dir` リポジトリ）は対象外にする: リンク先は人の実体なので、
//! 生成物であっても celeris が消してよいものではない。

use std::path::{Path, PathBuf};

use task_core::{Status, StoreError, TaskId, TaskStore};
use time::OffsetDateTime;

/// 刈る対象の候補（各 worktree のディレクトリからの相対パス）。ADR-0066 D2。
pub const PRUNABLE_SUBPATHS: &[&str] = &[
    "target",
    "node_modules",
    "build",
    ".venv",
    "gui/node_modules",
    "gui/build",
];

/// 1 タスクの作業場所で見つかった、刈れる生成物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneCandidate {
    pub task_id: TaskId,
    pub task_dir: PathBuf,
    /// 存在が確認できた、消してよい絶対パス（1 件以上）。
    pub paths: Vec<PathBuf>,
}

/// シンボリックリンクではない実在のディレクトリか。
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

/// `task_dir` の下にある worktree（git worktree。シンボリックリンクは含まない）を 1 段だけ列挙する。
/// ADR-0043 D2 の複数リポジトリ（`repos/<name>/`）と、Phase 49 の 1 リポジトリだけの旧い形
/// （`tree/`）の両方を見る。
fn worktree_dirs(task_dir: &Path) -> Vec<PathBuf> {
    let repos_dir = task_dir.join(crate::task_repos::REPOS_DIR_NAME);
    if repos_dir.is_dir() {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&repos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_real_dir(&path) {
                    out.push(path);
                }
            }
        }
        out.sort();
        return out;
    }
    let legacy = task_dir.join(crate::local_worktree::WORKTREE_DIR_NAME);
    if is_real_dir(&legacy) {
        vec![legacy]
    } else {
        Vec::new()
    }
}

/// この作業場所に残っている、刈れる生成物の絶対パスを集める（存在するものだけ。無ければ空）。
pub fn prunable_paths(task_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for repo_dir in worktree_dirs(task_dir) {
        for rel in PRUNABLE_SUBPATHS {
            let candidate = repo_dir.join(rel);
            if is_real_dir(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

/// 終端（done / failed / cancelled）になってから `after_secs` 以上経ち、まだ刈れる生成物が残っている
/// 作業場所を、決定的な順（task id の文字列順）ですべて集める。`after_secs == 0` は「無効」（常に空）。
pub fn find_prune_candidates(
    store: &dyn TaskStore,
    workspace_root: &Path,
    now: OffsetDateTime,
    after_secs: u64,
) -> Result<Vec<PruneCandidate>, StoreError> {
    if after_secs == 0 {
        return Ok(Vec::new());
    }
    let mut terminal = Vec::new();
    for status in [Status::Done, Status::Failed, Status::Cancelled] {
        terminal.extend(store.list(Some(status))?);
    }
    terminal.sort_by_key(|a| a.id.to_string());
    let mut out = Vec::new();
    for task in terminal {
        let age = now - task.updated_at;
        if age.whole_seconds() < after_secs as i64 {
            continue;
        }
        let task_dir = workspace_root.join(task.id.to_string());
        let paths = prunable_paths(&task_dir);
        if !paths.is_empty() {
            out.push(PruneCandidate {
                task_id: task.id,
                task_dir,
                paths,
            });
        }
    }
    Ok(out)
}

/// `find_prune_candidates` の最初の 1 件（dispatcher の tick が「1 tick に最大 1 か所」で使う）。
pub fn find_prune_candidate(
    store: &dyn TaskStore,
    workspace_root: &Path,
    now: OffsetDateTime,
    after_secs: u64,
) -> Result<Option<PruneCandidate>, StoreError> {
    Ok(
        find_prune_candidates(store, workspace_root, now, after_secs)?
            .into_iter()
            .next(),
    )
}

/// 実際に消す。消せなかったパスは無視して残りを続ける（途中で 1 つ失敗しても他は消す）。
/// 戻り値は実際に消せたパス（呼び出し側が `workspace_pruned` イベントの `removed` に使う）。
pub fn prune(candidate: &PruneCandidate) -> Vec<PathBuf> {
    candidate
        .paths
        .iter()
        .filter(|path| std::fs::remove_dir_all(path).is_ok())
        .cloned()
        .collect()
}

/// `workspace_pruned` イベントの `removed` に載せる、作業場所からの相対パス（表示用）。
pub fn relative_removed(task_dir: &Path, removed: &[PathBuf]) -> Vec<String> {
    removed
        .iter()
        .map(|p| {
            p.strip_prefix(task_dir)
                .map(|rel| rel.display().to_string())
                .unwrap_or_else(|_| p.display().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests;
