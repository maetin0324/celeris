//! ローカルの作業場所をタスクごとの `git worktree` にする（ADR-0041 D1）。
//!
//! ADR-0019 がリモート（`sync = "worktree"`）に対して決めたことを、ローカルにも同じ形で持ち込む。
//! 実機の問題: 案件の作業場所 `local: ~/workspace/agent-platform` を全タスクが同じ作業ツリーで共有すると、
//! 並列の実装者が別のブランチを `checkout` して互いの未コミット変更を壊す（人も同じチェックアウトで作業している）。
//!
//! ここがやること（`git` を起こすだけ。LLM も判断も無い）:
//!
//! - `path` が git リポジトリか（`git -C <path> rev-parse --git-dir`）
//! - base を決める（`main` / 本番の `current` / `HEAD`。ADR-0041 D1 の決定的な規則）
//! - `git -C <path> worktree add -b <branch_prefix><task_id> <dir> <base>`（再試行では作り直さず使い回す）
//! - 終端で `git status --porcelain` が空なら `git -C <path> worktree remove <dir>`
//!
//! **celeris はコミットしない**（ADR-0019 D2）。ブランチも消さない。元のリポジトリの作業ツリーには触らない
//! （ADR-0019 D3）。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::workspace::WorkspaceError;

/// worktree の親（`<workspace_root>/<task_id>`）の下に切る作業ツリーの名前。
pub const WORKTREE_DIR_NAME: &str = "tree";

/// ローカルの worktree のブランチ接頭辞の既定（ADR-0042 D3 でこの基盤の名前 Celeris に合わせた。
/// Phase 49 までは ADR-0019 のクラスタ側と同じ旧名だった）。`[workspace] worktree_branch_prefix` で変えられる。
/// **クラスタ側**（ADR-0019 の `WorktreeSettings::branch_prefix`）も ADR-0045 D1 の全面改名で `celeris/` に
/// 揃えた。クラスタに残っている旧名のブランチと worktree ディレクトリは使われなくなるだけで、celeris は消さない。
pub const DEFAULT_BRANCH_PREFIX: &str = "celeris/";

/// `git worktree add` の直列化に使う錠（`workspace_root` 直下。元のリポジトリには置かない）。
const LOCK_FILE: &str = ".celeris-worktree.lock";

/// base をどこから取ったか（前置きに書く。ADR-0041 D1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    /// リポジトリの `main`。
    Main,
    /// 本番の `current` リリースの sha（`main` がまだ本番に追いついていない）。
    Current,
    /// `main` が無いリポジトリの `HEAD`。
    Head,
    /// ADR-0079 D6（Phase R1c）: 木の子 task の基点（親の段階の基点、または同じ段階の依存先の HEAD。
    /// `Task.tree.base_commit`）。子の成果は親のブランチに取り込まれ、main とは比べない。
    Parent,
}

impl BaseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BaseKind::Main => "main",
            BaseKind::Current => "current",
            BaseKind::Head => "head",
            BaseKind::Parent => "parent",
        }
    }
}

/// 切り出す base（全長の sha と、その出どころ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRef {
    pub kind: BaseKind,
    pub sha: String,
}

impl BaseRef {
    /// 前置きに出す短縮 sha（12 桁）。
    pub fn sha12(&self) -> String {
        self.sha.chars().take(12).collect()
    }
}

/// 1 タスク分の worktree（ディスパッチャが dispatch のたびに組み立てる。純粋なデータ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorktree {
    /// 元のリポジトリ（`WorkspaceSpec::Local.path`）。ここには書かない。
    pub repo: PathBuf,
    /// celeris が持つタスクのディレクトリ（`<workspace_root>/<task_id>`）。`runs/` `inputs/` `artifacts/` はここ。
    pub task_dir: PathBuf,
    /// 作業ツリー（`<task_dir>/tree`）。ワーカーのカレントディレクトリになる。
    pub dir: PathBuf,
    /// ブランチ（`<branch_prefix><task_id>`）。celeris は作るだけで、消さない。
    pub branch: String,
    /// 切り出した base。
    pub base: BaseRef,
}

impl LocalWorktree {
    /// `git worktree add`（既にあれば使い回す。ADR-0041 D1 の「再試行では作り直さない」）。
    ///
    /// - 作業ツリーがある（`<dir>/.git` がある）→ 何もしない
    /// - ブランチだけある（前の run で作って worktree を消した）→ `worktree add <dir> <branch>`
    /// - どちらも無い → `worktree add -b <branch> <dir> <base>`
    pub async fn ensure(&self) -> Result<(), WorkspaceError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.ensure_blocking())
            .await
            .map_err(|e| WorkspaceError::Io(std::io::Error::other(format!("worktree task: {e}"))))?
    }

    /// `ensure` の同期版（ADR-0074 D1.2: daemon が WU の worktree を dispatch の時点で切るのに使う）。
    ///
    /// Phase R6-3: 作業ツリーを作った後（使い回すときも）、`.gitmodules` があり未初期化の submodule が
    /// あれば `git submodule update --init --recursive` する（`init_submodules`）。
    pub fn ensure_blocking(&self) -> Result<(), WorkspaceError> {
        self.ensure_tree_blocking()?;
        init_submodules(&self.dir)?;
        Ok(())
    }

    fn ensure_tree_blocking(&self) -> Result<(), WorkspaceError> {
        std::fs::create_dir_all(&self.task_dir)?;
        if self.dir.join(".git").exists() {
            return Ok(());
        }
        // 同じリポジトリに複数のタスクが同時に worktree を作ることがある（`max_concurrency > 1`）。
        // git の worktree 管理はリポジトリで共有なので、celeris 側の錠で直列化する。
        let _lock = WorktreeLock::acquire(self.task_dir.parent().unwrap_or(&self.task_dir));
        // 手で消された作業ツリーの登録が残っていると `worktree add` が「既に登録済み」で失敗する。
        let _ = git(&self.repo, &["worktree", "prune"]);
        if self.dir.join(".git").exists() {
            return Ok(());
        }
        let dir = self.dir.to_string_lossy().into_owned();
        let out = if branch_exists(&self.repo, &self.branch) {
            git(&self.repo, &["worktree", "add", &dir, &self.branch])
        } else {
            git(
                &self.repo,
                &["worktree", "add", "-b", &self.branch, &dir, &self.base.sha],
            )
        };
        match out {
            Some(o) if o.ok => Ok(()),
            Some(o) => Err(WorkspaceError::Remote(format!(
                "cannot create the git worktree {} of {} (exit {:?}): {}",
                dir,
                self.repo.display(),
                o.code,
                o.stderr.trim()
            ))),
            None => Err(WorkspaceError::Remote(format!(
                "cannot run git for the worktree of {}",
                self.repo.display()
            ))),
        }
    }

    /// 終端での後片付け（ADR-0041 D1）。`git status --porcelain` が空なら worktree を消す
    /// （**ブランチは消さない**。コミットはリポジトリに残る）。空でなければ残して `Err(dir)` を返す
    /// （呼び出し側が `WorkerProgress` を 1 行積む）。
    pub fn remove_if_clean(&self) -> CleanupOutcome {
        if !self.dir.exists() {
            return CleanupOutcome::AlreadyGone;
        }
        match status_is_clean(&self.dir) {
            None => CleanupOutcome::Unknown,
            Some(false) => CleanupOutcome::Dirty,
            Some(true) => {
                let dir = self.dir.to_string_lossy().into_owned();
                match git(&self.repo, &["worktree", "remove", &dir]) {
                    Some(o) if o.ok => CleanupOutcome::Removed,
                    _ => CleanupOutcome::Unknown,
                }
            }
        }
    }
    /// ADR-0043 D2 の**中止（cancel）**の後片付け: 未コミットの変更があっても worktree を消し、
    /// **ブランチも消す**（`git branch -D`）。人が「このタスクは中止」と決めたときだけ呼ぶ。
    /// 終端（`done` / `failed`）では呼ばない（差分を見るために残す）。
    pub fn remove_with_branch(&self) -> CleanupOutcome {
        let existed = self.dir.exists();
        let dir = self.dir.to_string_lossy().into_owned();
        if existed {
            // `--force` は未コミットの変更ごと消す（cancel は人の指示）。
            let removed =
                git(&self.repo, &["worktree", "remove", "--force", &dir]).is_some_and(|o| o.ok);
            if !removed {
                // 登録が壊れている（人が手で消した等）なら prune してからディレクトリを落とす。
                let _ = git(&self.repo, &["worktree", "prune"]);
                if self.dir.exists() && std::fs::remove_dir_all(&self.dir).is_err() {
                    return CleanupOutcome::Unknown;
                }
                let _ = git(&self.repo, &["worktree", "prune"]);
            }
        }
        // ブランチは worktree が無くなってからでないと消せない。
        let had_branch = branch_exists(&self.repo, &self.branch);
        if had_branch && !git(&self.repo, &["branch", "-D", &self.branch]).is_some_and(|o| o.ok) {
            return CleanupOutcome::Unknown;
        }
        if existed || had_branch {
            CleanupOutcome::Removed
        } else {
            CleanupOutcome::AlreadyGone
        }
    }
}

/// Phase R7-4: `init_submodules` の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmoduleInit {
    /// 初期化後に初期化済みの submodule の数（`submodule status --recursive` の行頭が `-` でない行）。
    pub initialised: usize,
    /// 初期化できなかった submodule（`.gitmodules` の path と、stderr の要点の 1 行）。準備は止めない。
    pub failed: Vec<(String, String)>,
}

/// Phase R6-3 / R7-4（ADR-0019 付記）: 作業ツリー `dir` に `.gitmodules` があり、未初期化の submodule
/// （`git submodule status --recursive` の行頭が `-`）があれば、`.gitmodules` の path ごとに
/// `git submodule update --init --recursive -- <path>` する（**1 つずつ・best-effort**。1 つの失敗で
/// 残りを止めない）。初期化したら `Some(SubmoduleInit)` を返す（成功の数を tracing の info、失敗を
/// 1 つずつ tracing の warn に出す）。`.gitmodules` が無い・全部初期化済みなら `None`（冪等）。
/// `Err` は `git submodule status` 自体が動かない（git が無い・worktree が使えない）ときだけ。
pub fn init_submodules(dir: &Path) -> Result<Option<SubmoduleInit>, WorkspaceError> {
    init_submodules_with(dir, &[])
}

/// `init_submodules` の本体。`git_config` は `git -c` の前置き（テストでローカルパスの submodule を許すのに使う）。
fn init_submodules_with(
    dir: &Path,
    git_config: &[&str],
) -> Result<Option<SubmoduleInit>, WorkspaceError> {
    if !dir.join(".gitmodules").is_file() {
        return Ok(None);
    }
    let status = match git(dir, &["submodule", "status", "--recursive"]) {
        Some(o) if o.ok => o,
        other => {
            let detail = match other {
                Some(o) => format!("exit {:?}: {}", o.code, o.stderr.trim()),
                None => "cannot run git".to_string(),
            };
            return Err(WorkspaceError::Remote(format!(
                "cannot run the git submodule step in the worktree {} (git submodule status, {detail})",
                dir.display()
            )));
        }
    };
    if !status.stdout.lines().any(|l| l.starts_with('-')) {
        return Ok(None);
    }
    let paths: Vec<String> = git(
        dir,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    )
    .filter(|o| o.ok)
    .map(|o| {
        o.stdout
            .lines()
            .filter_map(|l| l.split_once(' ').map(|(_, p)| p.to_string()))
            .filter(|p| !p.is_empty())
            .collect()
    })
    .unwrap_or_default();
    let mut failed = Vec::new();
    for path in &paths {
        let mut update: Vec<&str> = git_config.to_vec();
        update.extend(["submodule", "update", "--init", "--recursive", "--", path]);
        match git(dir, &update) {
            Some(o) if o.ok => {}
            other => {
                let why = match other {
                    Some(o) => first_error_line(&o.stderr),
                    None => "cannot run git".to_string(),
                };
                tracing::warn!(worktree = %dir.display(), submodule = %path, error = %why, "a git submodule could not be initialised (R7-4, best-effort)");
                failed.push((path.clone(), why));
            }
        }
    }
    let initialised = git(dir, &["submodule", "status", "--recursive"])
        .filter(|o| o.ok)
        .map(|o| {
            o.stdout
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('-'))
                // 失敗した submodule は clone だけ済んで `-` でない行になることがあるので数えない。
                .filter(|l| {
                    let p = l
                        .get(1..)
                        .and_then(|r| r.split_once(' '))
                        .map_or("", |(_, p)| p);
                    !failed.iter().any(|(f, _)| {
                        p == f || p.starts_with(&format!("{f} ")) || p.starts_with(&format!("{f}/"))
                    })
                })
                .count()
        })
        .unwrap_or(0);
    tracing::info!(worktree = %dir.display(), submodules = initialised, "initialised {initialised} submodules in {} (R6-3)", dir.display());
    Ok(Some(SubmoduleInit {
        initialised,
        failed,
    }))
}

/// git の stderr の要点の 1 行（`fatal:` / `error:` の最初の行。無ければ最初の空でない行）。
/// `Cloning into ...` は要点ではないので、`fatal:` を優先する。
fn first_error_line(stderr: &str) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .or_else(|| lines().next())
        .unwrap_or("unknown error")
        .to_string()
}

/// `remove_if_clean` の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// 消した（クリーンだった）。
    Removed,
    /// 未コミットの変更が残っているので残した。
    Dirty,
    /// もう無い（人が消した・作られなかった）。
    AlreadyGone,
    /// git が動かせなかった。残す。
    Unknown,
}

/// `path` が git リポジトリか（ADR-0019 D3 と同じ判定）。
pub fn is_git_repo(path: &Path) -> bool {
    path.is_dir() && git(path, &["rev-parse", "--git-dir"]).is_some_and(|o| o.ok)
}

/// ADR-0074 Phase F5-fix4: `dir` が git の作業ツリーの**最上位**なら、そこで `git commit` / `git merge`
/// するのに書き込みが要る git の管理領域を返す（`git rev-parse --absolute-git-dir` と
/// `--git-common-dir`。重複は 1 つにまとめる。通常のリポジトリなら `<dir>/.git` 1 つ、worktree なら
/// `<登録元>/.git/worktrees/<name>` と `<登録元>/.git` の 2 つ）。
///
/// sandbox 付きのアダプタ（codex の `workspace-write`）は cwd の外に書けないので、worktree の
/// `ORIG_HEAD`・index・refs・objects が read-only になる（本番障害 01M3JXB3DHVBWKWKPW04DTG6SJ）。
/// その書き込み先を明示的に足すための材料。決定的（`git` を起こすだけ。LLM は使わない）。
///
/// 次のときは空（何も足さない）: `dir` が無い・git でない・git が起動できない・`dir` が作業ツリーの
/// 最上位でない（上位のディレクトリのリポジトリ ― 例えばホームのドットファイル ― の `.git` を
/// たまたま拾って広げない）。呼び出し元の環境の `GIT_DIR` などには引きずられない。
pub fn git_admin_dirs(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ])
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(toplevel), Some(git_dir), Some(common_dir)) =
        (lines.next(), lines.next(), lines.next())
    else {
        return Vec::new();
    };
    let is_toplevel = match (std::fs::canonicalize(toplevel), std::fs::canonicalize(dir)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !is_toplevel {
        return Vec::new();
    }
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(2);
    for candidate in [git_dir, common_dir] {
        let path = PathBuf::from(candidate);
        if !path.is_absolute() || !path.is_dir() {
            continue;
        }
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    dirs
}

/// ADR-0041 D1 の base の規則（決定的）:
///
/// 1. `main` があればその sha。無ければ `HEAD`。
/// 2. ただし本番の `current` リリースの sha が分かり、それが `main` の**子孫**（`main` が本番に
///    追いついていない）なら、`current` の sha。本番より古いコードから分岐させない。
///
/// commit が 1 つも無いリポジトリでは `None`（worktree を切れない → 従来どおりの共有に倒す）。
pub fn resolve_base(repo: &Path, current_sha: Option<&str>) -> Option<BaseRef> {
    let main = rev_parse(repo, "refs/heads/main");
    let Some(main_sha) = main else {
        let head = rev_parse(repo, "HEAD")?;
        return Some(BaseRef {
            kind: BaseKind::Head,
            sha: head,
        });
    };
    if let Some(current) = current_sha
        && let Some(current_full) = rev_parse(repo, &format!("{current}^{{commit}}"))
        && current_full != main_sha
        && git(
            repo,
            &["merge-base", "--is-ancestor", &main_sha, &current_full],
        )
        .is_some_and(|o| o.ok)
    {
        return Some(BaseRef {
            kind: BaseKind::Current,
            sha: current_full,
        });
    }
    Some(BaseRef {
        kind: BaseKind::Main,
        sha: main_sha,
    })
}

/// 本番の `current` リリースの sha（ADR-0040 D6: `current` は `releases_dir` の**親**にある）。
/// 読めない・壊れている・`sha` が無いときは `None`（base は `main` に倒れる）。
pub fn current_release_sha(releases_dir: &Path) -> Option<String> {
    let manifest = releases_dir.parent()?.join("current").join("manifest.json");
    let text = std::fs::read_to_string(manifest).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let sha = value
        .get("sha")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("sha12").and_then(|v| v.as_str()))?;
    let sha = sha.trim();
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(sha.to_string())
}

/// `git -C <dir> status --porcelain` が空か。git が動かせなければ `None`。
pub fn status_is_clean(dir: &Path) -> Option<bool> {
    let out = git(dir, &["status", "--porcelain"])?;
    if !out.ok {
        return None;
    }
    Some(out.stdout.trim().is_empty())
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_some_and(|o| o.ok)
}

fn rev_parse(repo: &Path, rev: &str) -> Option<String> {
    let out = git(repo, &["rev-parse", "--verify", "--quiet", rev])?;
    if !out.ok {
        return None;
    }
    let sha = out.stdout.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

struct GitOutput {
    ok: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// `git -C <dir> <args...>`。git が無い・起動できないときは `None`。
fn git(dir: &Path, args: &[&str]) -> Option<GitOutput> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // 人の設定（`core.hooksPath`、対話的な認証）に引きずられないようにする。
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    Some(GitOutput {
        ok: out.status.success(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// `workspace_root/.celeris-worktree.lock` の flock（取れなければ錠なしで進む。止めない）。
struct WorktreeLock(#[allow(dead_code)] Option<nix::fcntl::Flock<std::fs::File>>);

impl WorktreeLock {
    fn acquire(root: &Path) -> WorktreeLock {
        let _ = std::fs::create_dir_all(root);
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join(LOCK_FILE))
        {
            Ok(f) => f,
            Err(_) => return WorktreeLock(None),
        };
        match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusive) {
            Ok(lock) => WorktreeLock(Some(lock)),
            Err(_) => WorktreeLock(None),
        }
    }
}

#[cfg(test)]
mod tests;
