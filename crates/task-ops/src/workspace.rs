//! ローカルの作業場所の置き場（ADR-0041 D1）。
//!
//! `WorkspaceSpec::Local` の `mode = worktree`（既定）で、`path` が git リポジトリのときだけ、
//! celeris はタスクごとに `git worktree` を切る。そのとき **run の足回り**（`runs/`, `inputs/`,
//! `artifacts/`）は worktree の外、`<workspace_root>/<task_id>/` に置き、作業ツリーそのものは
//! `<workspace_root>/<task_id>/tree` になる。作業ツリーの中に `runs/` を作ると
//! `git status --porcelain` が常に汚れ、終端で worktree を消せなくなるため。
//!
//! ここは「どこを見ればよいか」を 1 か所に決める純粋な関数（+ 目印ファイルの有無だけを見る）。
//! ディスパッチャは dispatch の時点で `git rev-parse` まで見て決め、その結果を目印
//! （`<task_dir>/worktree.json`）として残す。API・`celerisctl` はその目印だけを見る
//! （git を起こさない。worktree を消した後も `runs/` と `artifacts/` が引けるように、目印は消さない）。

use std::path::{Path, PathBuf};

use task_core::{Task, WorkspaceMode, WorkspaceSpec};

/// worktree を切ったタスクの目印（`<workspace_root>/<task_id>/worktree.json`）。
pub const WORKTREE_MARKER: &str = "worktree.json";

/// 作業ツリーのディレクトリ名（`<workspace_root>/<task_id>/tree`）。
pub const WORKTREE_DIR_NAME: &str = "tree";

/// そのタスクの**足回りのディレクトリ**（`runs/`, `inputs/`, `artifacts/` があるところ）。
///
/// - `Local` で worktree を切った（目印がある）→ `<workspace_root>/<task_id>`
/// - `Local`（従来）→ `path`（相対なら `workspace_root` 基準）
/// - `Remote` → 手元の写し `<workspace_root>/<task_id>`（ADR-0018 D1。従来どおり）
pub fn local_dir(task: &Task, workspace_root: &Path) -> PathBuf {
    match &task.workspace {
        WorkspaceSpec::Local { path, .. } => {
            let per_task = workspace_root.join(task.id.to_string());
            if !per_task.join(WORKTREE_MARKER).is_file() {
                return workspace_root.join(path);
            }
            // ADR-0043 D2（Phase 52）: 複数リポジトリのタスクは、リポジトリごとの `mode` に関わらず
            // 足回りが `<workspace_root>/<task_id>/` にある（目印の `repos` が空でないのが印）。
            let multi = read_marker(&per_task).is_some_and(|m| !m.repos.is_empty());
            if multi || task.workspace.local_mode() == WorkspaceMode::Worktree {
                per_task
            } else {
                workspace_root.join(path)
            }
        }
        WorkspaceSpec::Remote { .. } => workspace_root.join(task.id.to_string()),
    }
}

/// 目印に書く内容（ADR-0041 D1 / ADR-0043 D2。人が読む・API が読む）。
///
/// 先頭の 5 つは Phase 49 からある「1 リポジトリのときの姿」で、複数リポジトリのタスクでは
/// **`repos[0]`（カレントディレクトリになるリポジトリ）の写し**が入る（既存の `TaskDetail.worktree` と
/// GUI がそのまま読める）。`repos` は ADR-0043 D2 で足したリポジトリ全部の一覧。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeMarker {
    /// 元のリポジトリ（`WorkspaceSpec::Local.path`、または `repos[0]` の実体）。
    pub repo: String,
    /// 作業ツリー（`<workspace_root>/<task_id>/tree` か `.../repos/<name>`）。
    pub dir: String,
    /// ブランチ（`<branch_prefix><task_id>`）。git でなければ空。
    pub branch: String,
    /// 切り出した base の sha（全長）。git でなければ空。
    pub base: String,
    /// base をどこから取ったか（`main` / `current` / `head`）。git でなければ空。
    pub base_kind: String,
    /// ADR-0043 D2: このタスクが使うリポジトリ全部（順番は cwd が先頭）。
    /// Phase 49 までの目印には無いので既定は空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<WorktreeMarkerRepo>,
}

/// 目印の `repos[]` の 1 件（ADR-0043 D2）。ファイル閲覧 API（D6）と GUI がこれを見る。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeMarkerRepo {
    /// 案件の中での名前（`project_repos.name`）。
    pub name: String,
    /// `git`（worktree）か `dir`（シンボリックリンク）。
    pub kind: String,
    /// 実体（案件のリポジトリの場所）。
    pub source: String,
    /// タスクの中での場所（`<workspace_root>/<task_id>/repos/<name>`）。
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_kind: Option<String>,
}

/// 目印を書く（worktree を用意したディスパッチャが 1 回だけ。上書きしてよい）。
pub fn write_marker(task_dir: &Path, marker: &WorktreeMarker) -> std::io::Result<()> {
    std::fs::create_dir_all(task_dir)?;
    let text = serde_json::to_string_pretty(marker).map_err(std::io::Error::other)?;
    std::fs::write(task_dir.join(WORKTREE_MARKER), format!("{text}\n"))
}

/// 目印を読む（無い・壊れていれば `None`）。
pub fn read_marker(task_dir: &Path) -> Option<WorktreeMarker> {
    let text = std::fs::read_to_string(task_dir.join(WORKTREE_MARKER)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests;
