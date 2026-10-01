//! ADR-0074 D1.2 / D1.4（Phase F2）: WU ごとの worktree と工程末尾の統合（daemon 側の git 操作だけ）。
//!
//! ここは `git` を起こすだけで、判断（どの WU をいつ統合するか、衝突を誰に直させるか）は
//! `dispatcher.rs` と `execution_scheduler.rs` にある。**LLM は呼ばない**（DESIGN 原則 1）。
//!
//! - WU の worktree: `<task_dir>/wu/<key>/repos/<name>/`、ブランチ `celeris-wu/<task_id>/<key>`。
//!   基点は Task ブランチの HEAD（工程の最初の WU）か、同じ工程の依存先の WU ブランチの HEAD（積み上げ）。
//! - WU の完了時の commit: `git add -A && git commit -m "wu/<key>: <title>"`（作者は celeris の固定値。
//!   変更が無ければ commit しない）。
//! - 統合: Task の worktree で、工程の葉の WU のブランチを `seq` 順に `git merge --no-ff --no-edit`。
//!   既に入っている（`merge-base --is-ancestor`）ものは飛ばし、`MERGE_HEAD` が残っていれば
//!   `merge --abort` してからやり直す（再起動後のやり直しを冪等にする）。衝突したら `merge --abort`
//!   して止める（呼び出し側が repair WU を作る）。

use std::path::{Path, PathBuf};
use std::process::Command;

use task_worker::local_worktree::{BaseKind, BaseRef, LocalWorktree};

/// WU の worktree を置くディレクトリ名（`<task_dir>/wu/<key>`）。
pub const WU_DIR_NAME: &str = "wu";
/// 統合・WU の commit の作者（決定的な固定値）。
pub const CELERIS_GIT_NAME: &str = "celeris";
pub const CELERIS_GIT_EMAIL: &str = "celeris@localhost";

/// `celeris-wu/<task_id>/<key>`（ADR-0074 D1.2。`celeris/<task_id>/…` にしないのは ref の D/F 衝突のため）。
pub fn wu_branch(task_id: &str, key: &str) -> String {
    format!("celeris-wu/{task_id}/{key}")
}

/// `<task_dir>/wu/<key>`（その WU の `artifacts/` と `repos/` の親）。
pub fn wu_dir(task_dir: &Path, key: &str) -> PathBuf {
    task_dir.join(WU_DIR_NAME).join(key)
}

/// 統合の merge のメッセージ（固定）。
pub fn merge_message(key: &str, phase: &str) -> String {
    format!("integrate wu/{key} (phase {phase})")
}

/// WU の完了時の commit のメッセージ。
pub fn commit_message(key: &str, title: &str) -> String {
    format!("wu/{key}: {title}")
}

/// `repo`（元のリポジトリ）の上に WU の worktree を表す値を作る（純粋なデータ。作るのは `ensure`）。
/// `task_dir` は Task のディレクトリ（worktree の錠は `task_dir` の親 = workspace_root に置かれる）。
pub fn wu_worktree(
    task_dir: &Path,
    task_id: &str,
    key: &str,
    repo_name: &str,
    repo_source: &Path,
    base_sha: &str,
) -> LocalWorktree {
    LocalWorktree {
        repo: repo_source.to_path_buf(),
        task_dir: task_dir.to_path_buf(),
        dir: wu_dir(task_dir, key)
            .join(task_worker::task_repos::REPOS_DIR_NAME)
            .join(repo_name),
        branch: wu_branch(task_id, key),
        base: BaseRef {
            kind: BaseKind::Head,
            sha: base_sha.to_string(),
        },
    }
}

/// `git -C <dir> <args>` の結果。
#[derive(Debug, Clone)]
struct GitOut {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn git(dir: &Path, args: &[&str]) -> Result<GitOut, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            &format!("user.name={CELERIS_GIT_NAME}"),
            "-c",
            &format!("user.email={CELERIS_GIT_EMAIL}"),
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", CELERIS_GIT_NAME)
        .env("GIT_AUTHOR_EMAIL", CELERIS_GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", CELERIS_GIT_NAME)
        .env("GIT_COMMITTER_EMAIL", CELERIS_GIT_EMAIL)
        .output()
        .map_err(|e| format!("cannot run git in {}: {e}", dir.display()))?;
    Ok(GitOut {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<GitOut, String> {
    let out = git(dir, args)?;
    if out.ok {
        Ok(out)
    } else {
        Err(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            out.stderr.trim()
        ))
    }
}

/// `rev` の全長 sha（無ければ `None`）。
pub fn rev_parse(dir: &Path, rev: &str) -> Option<String> {
    let out = git(dir, &["rev-parse", "--verify", "--quiet", rev]).ok()?;
    if !out.ok {
        return None;
    }
    let sha = out.stdout.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// `sha` がこのリポジトリの commit として解決できれば、その全長 sha。
pub fn commit_in(dir: &Path, sha: &str) -> Option<String> {
    if sha.is_empty() {
        return None;
    }
    rev_parse(dir, &format!("{sha}^{{commit}}"))
}

/// ADR-0074「Phase F5-fix7 実装時の明確化」: 同じ工程の依存先 `dep` の成果の上に WU を積むときの基点 sha
/// （`repo` は登録元のリポジトリ、`task_branch` は Task ブランチ）。決定的で、ref も行も書き換えない。
///
/// 1. `celeris-wu/<task_id>/<dep>` があればその HEAD（従来どおり）。
/// 2. 依存先が WU の worktree を持たなかった（`dep.branch == None`。Task の worktree で走った repair WU・
///    統合 WU・並列 1 の WU）: その成果は Task ブランチにあるので、`head_commit` → `integrated_commit` →
///    `base_commit`（このリポジトリで解決できるもの）→ Task ブランチの HEAD の順。commit しなかった
///    （`head_commit` も `base_commit` も無い）依存先は Task ブランチの HEAD になる。
/// 3. 依存先が WU のブランチを持っていたのに ref が消えている: `head_commit` → `base_commit`
///    （commit の無い done は `head_commit == base_commit`）。どちらも解決できなければ `Err`
///    （時間では直らない。呼び出し側は WU を blocked にして人に聞く）。
///
/// ADR-0079 D6（Phase R1c）: 依存先が kind task の unit（子 task）なら、子のブランチ
/// `<branch_prefix><child_task_id>`（子の成果の置き場）の HEAD → 記録した `head_commit`（子の done の時点の
/// HEAD）→ Task ブランチの HEAD（子がブランチを持たなかった: shared / remote）の順。子 task の基点
/// （`Task.tree.base_commit`）にも同じ関数を使う（葉と同じ規則で決まる）。
pub fn dependency_base(
    repo: &Path,
    task_id: &str,
    dep: &task_core::WorkUnitRow,
    task_branch: &str,
    branch_prefix: &str,
) -> Result<String, String> {
    let recorded = |c: &Option<String>| c.as_deref().and_then(|sha| commit_in(repo, sha));
    if dep.kind == task_core::WorkUnitKind::Task {
        return dep
            .child_task_id
            .as_deref()
            .and_then(|child| rev_parse(repo, &format!("refs/heads/{branch_prefix}{child}")))
            .or_else(|| recorded(&dep.head_commit))
            .or_else(|| rev_parse(repo, &format!("refs/heads/{task_branch}")))
            .ok_or_else(|| {
                format!(
                    "dependency {} is a child task unit but neither its branch, its recorded head_commit \
                     nor the task branch {task_branch} exist in {}",
                    dep.key,
                    repo.display()
                )
            });
    }
    if let Some(sha) = rev_parse(
        repo,
        &format!("refs/heads/{}", wu_branch(task_id, &dep.key)),
    ) {
        return Ok(sha);
    }
    if dep.branch.is_none() {
        return recorded(&dep.head_commit)
            .or_else(|| recorded(&dep.integrated_commit))
            .or_else(|| recorded(&dep.base_commit))
            .or_else(|| rev_parse(repo, &format!("refs/heads/{task_branch}")))
            .ok_or_else(|| {
                format!(
                    "dependency {} ran in the task worktree but neither its recorded commits nor the \
                     task branch {task_branch} exist in {}",
                    dep.key,
                    repo.display()
                )
            });
    }
    recorded(&dep.head_commit)
        .or_else(|| recorded(&dep.base_commit))
        .ok_or_else(|| {
            format!(
                "dependency branch of {} does not exist in {} and its recorded head_commit/base_commit \
                 ({}/{}) cannot be resolved",
                dep.key,
                repo.display(),
                dep.head_commit.as_deref().unwrap_or("-"),
                dep.base_commit.as_deref().unwrap_or("-"),
            )
        })
}

/// `ancestor` が `descendant` の祖先（または同じ）か。
pub fn is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", ancestor, descendant]).is_ok_and(|o| o.ok)
}

/// WU の worktree を切る（冪等。既にあれば使い回す。`LocalWorktree::ensure_blocking` と同じ規則）。
pub fn ensure_wu_worktree(worktree: &LocalWorktree) -> Result<(), String> {
    worktree
        .ensure_blocking()
        .map_err(|e| format!("cannot create the work unit worktree: {e}"))
}

/// WU の完了時の commit（`git add -A && git commit`）。変更が無ければ commit しない。
/// 戻り値は `(HEAD の sha, commit したか)`。
pub fn commit_all(dir: &Path, message: &str) -> Result<(String, bool), String> {
    git_ok(dir, &["add", "-A"])?;
    let staged = git(dir, &["diff", "--cached", "--quiet"])?;
    let committed = if staged.ok {
        false
    } else {
        git_ok(dir, &["commit", "--no-verify", "-q", "-m", message])?;
        true
    };
    let head = rev_parse(dir, "HEAD").ok_or_else(|| format!("no HEAD in {}", dir.display()))?;
    Ok((head, committed))
}

/// 工程の統合で merge する 1 件（葉の WU、または ADR-0079 D5 / D6 の kind task の unit の子 task）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeItem {
    pub key: String,
    pub branch: String,
    /// ADR-0079 D6（Phase R1c）: このリポジトリにブランチが無ければ飛ばす（`merged` に出さない）。
    /// 子 task のブランチは子の repos（親の部分集合）にだけあるので `true`。WU のブランチは `false`
    /// （無ければ `Err`。従来どおり）。
    pub optional: bool,
}

impl MergeItem {
    /// WU のブランチ（無ければ `Err`）。
    pub fn work_unit(key: impl Into<String>, branch: impl Into<String>) -> Self {
        MergeItem {
            key: key.into(),
            branch: branch.into(),
            optional: false,
        }
    }

    /// ADR-0079 D6: 子 task のブランチ `celeris/<child_id>`（このリポジトリに無ければ飛ばす）。
    pub fn child_task(key: impl Into<String>, branch: impl Into<String>) -> Self {
        MergeItem {
            key: key.into(),
            branch: branch.into(),
            optional: true,
        }
    }
}

/// 1 件の merge の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub key: String,
    /// merge した WU ブランチの HEAD（`PhaseIntegrated.merged[].commit`）。
    pub commit: String,
    /// 既に Task ブランチに入っていたので飛ばした（冪等なやり直し）。
    pub skipped: bool,
}

/// 衝突（`merge --abort` 済み）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: String,
    pub branch: String,
    /// 衝突したファイル（`git diff --name-only --diff-filter=U`）。
    pub files: Vec<String>,
}

/// [`integrate`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOutcome {
    pub merged: Vec<Merged>,
    /// 統合後の Task ブランチの HEAD。
    pub head: String,
    /// 衝突で止まったら `Some`（その WU より後は試していない）。
    pub conflict: Option<Conflict>,
}

/// ADR-0074 D1.4 1〜3: Task の worktree（`task_tree`）で、葉の WU のブランチを順に merge する。
///
/// - `MERGE_HEAD` が残っていれば `merge --abort` してから始める（途中で止まったやり直し）。
/// - 既に入っている WU（`merge-base --is-ancestor <branch> HEAD`）は飛ばす（冪等）。
/// - 衝突したら `merge --abort` して止める（`conflict` に入れて返す。Task ブランチは衝突の前のまま）。
pub fn integrate(
    task_tree: &Path,
    items: &[MergeItem],
    phase: &str,
) -> Result<IntegrationOutcome, String> {
    if rev_parse(task_tree, "MERGE_HEAD").is_some() {
        let _ = git(task_tree, &["merge", "--abort"]);
    }
    let mut merged = Vec::new();
    for item in items {
        let Some(commit) = rev_parse(task_tree, &format!("refs/heads/{}", item.branch)) else {
            if item.optional {
                continue;
            }
            return Err(format!(
                "work unit branch {} does not exist in {}",
                item.branch,
                task_tree.display()
            ));
        };
        if is_ancestor(task_tree, &commit, "HEAD") {
            merged.push(Merged {
                key: item.key.clone(),
                commit,
                skipped: true,
            });
            continue;
        }
        let out = git(
            task_tree,
            &[
                "merge",
                "--no-ff",
                "--no-edit",
                "-m",
                &merge_message(&item.key, phase),
                &item.branch,
            ],
        )?;
        if out.ok {
            merged.push(Merged {
                key: item.key.clone(),
                commit,
                skipped: false,
            });
            continue;
        }
        let files = git(task_tree, &["diff", "--name-only", "--diff-filter=U"])
            .map(|o| {
                o.stdout
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let _ = git(task_tree, &["merge", "--abort"]);
        let head = rev_parse(task_tree, "HEAD")
            .ok_or_else(|| "no HEAD after merge --abort".to_string())?;
        return Ok(IntegrationOutcome {
            merged,
            head,
            conflict: Some(Conflict {
                key: item.key.clone(),
                branch: item.branch.clone(),
                files,
            }),
        });
    }
    let head = rev_parse(task_tree, "HEAD")
        .ok_or_else(|| format!("no HEAD in {}", task_tree.display()))?;
    Ok(IntegrationOutcome {
        merged,
        head,
        conflict: None,
    })
}

/// `git diff --stat <base>..<branch>`（repair WU の最小 context 用。失敗は空文字）。
pub fn diff_stat(dir: &Path, base: &str, branch: &str) -> String {
    git(dir, &["diff", "--stat", &format!("{base}...{branch}")])
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default()
}

/// 統合が済んだ WU の worktree を消す（ADR-0074 D1.2。ブランチは Task の終端まで残す）。
pub fn remove_wu_worktree(worktree: &LocalWorktree) -> Result<(), String> {
    if !worktree.dir.exists() {
        return Ok(());
    }
    let dir = worktree.dir.to_string_lossy().into_owned();
    let out = git(&worktree.repo, &["worktree", "remove", "--force", &dir])?;
    if !out.ok {
        let _ = git(&worktree.repo, &["worktree", "prune"]);
        if worktree.dir.exists() {
            std::fs::remove_dir_all(&worktree.dir)
                .map_err(|e| format!("cannot remove {}: {e}", worktree.dir.display()))?;
        }
        let _ = git(&worktree.repo, &["worktree", "prune"]);
    }
    // 空になった `<task_dir>/wu/<key>/repos` も片付ける（`artifacts/` は残す）。
    if let Some(parent) = worktree.dir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
