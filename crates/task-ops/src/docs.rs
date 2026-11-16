//! 文書（ADR-0044 D7。Phase 57）。**正本は git**（案件の primary リポジトリの `[outputs].docs` の下の
//! `*.md`）で、DB には何も持たない。
//!
//! - [`page_path`] — API が受け取ったパスをリポジトリ相対に正規化する（境界。`..`・絶対パス・`.md` 以外を弾く）
//! - [`list`] / [`grep`] — `git ls-tree` と `git grep -il`（default_branch の内容だけを見る）
//! - [`read_page`] / [`blob_sha`] / [`history`] — ページ 1 枚（`git show` / `rev-parse` / `log`）
//! - [`front_matter`] / [`render`] — front matter（最小の YAML もどき）と Markdown の描画（`pulldown-cmark`、生 HTML は捨てる）
//! - [`commit_page`] — 一時 worktree でコミットし default_branch を fast-forward（ADR-0043 D5 と同じ規則）
//! - [`init_docs_repo`] — 案件に git のリポジトリが無いときの既定の文書リポジトリ（`~/workspace/<slug>/`）
//!
//! ここも `task_ops::changes` と同じく **`git` を起こすだけ**で、判断も LLM も無い（DESIGN 原則 1）。
//! 子プロセスには全部待ち時間の上限がある。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::changes::{GIT_TIMEOUT, GIT_WRITE_TIMEOUT, git};
use task_core::knowledge::yaml_scalar;
/// front matter（最小の YAML もどき）・題名・slug の規則は KB と同じ（ADR-0044 D7、ADR-0047 D1）。
pub use task_core::knowledge::{FrontMatter, front_matter, slugify, title_of};

/// ADR-0044 D7: 人の編集のコミットの author / committer。
pub const DOCS_AUTHOR_NAME: &str = "Celeris (human)";
pub const DOCS_AUTHOR_EMAIL: &str = "celeris@local";

/// ADR-0044 D7: ページの履歴は直近 20 件。
pub const HISTORY_LIMIT: usize = 20;
/// ツリーに出すページの上限（大きな案件で画面と git が詰まらないように）。
pub const MAX_TREE_ITEMS: usize = 2_000;
/// 1 ページの本文の上限（これを超えるページは `too_large`）。
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// 既定の文書リポジトリを作るときの最初のページ。
pub const INITIAL_PAGE: &str = "docs/README.md";

// ---------------------------------------------------------------------------
// パス（境界）
// ---------------------------------------------------------------------------

/// パスの検査の失敗（API は `Forbidden` を 403、`NotMarkdown` を 422 にする）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path must be relative and must not contain `..`")]
    Forbidden,
    #[error("a document page must be a Markdown file (*.md)")]
    NotMarkdown,
    #[error("path is required")]
    Empty,
}

/// 文書の根（`docs`）を `.`・`/` の揺れを取り除いた形にする。ルートそのものは `""`。
pub fn normalize_root(docs: &str) -> String {
    let trimmed = docs.trim().trim_start_matches("./").trim_matches('/');
    if trimmed == "." {
        String::new()
    } else {
        trimmed.to_string()
    }
}

/// API が受け取った `path` を**リポジトリ相対**に正規化する（ADR-0044 D7 の境界）。
///
/// - `..`・絶対パス・Windows の prefix は `Forbidden`（403）
/// - `.md` で終わらないものは `NotMarkdown`（422）
/// - 文書の根（`docs`）で始まっていなければ根の下だと解釈して前に付ける
///   （GUI の `?path=docs/x.md` も、ページの中の `[[x.md]]` も同じ 1 本の表記になる）
pub fn page_path(root: &str, raw: &str) -> Result<String, PathError> {
    use task_core::knowledge::PathError as Kb;
    let cleaned = task_core::knowledge::scope_path(raw).map_err(|e| match e {
        Kb::Forbidden => PathError::Forbidden,
        Kb::NotMarkdown => PathError::NotMarkdown,
        Kb::Empty => PathError::Empty,
    })?;
    if !cleaned.to_ascii_lowercase().ends_with(".md") {
        return Err(PathError::NotMarkdown);
    }
    let root = normalize_root(root);
    if root.is_empty() || cleaned == root || cleaned.starts_with(&format!("{root}/")) {
        Ok(cleaned)
    } else {
        Ok(format!("{root}/{cleaned}"))
    }
}

/// `a/b/c.md` と `../d.md` → `a/d.md`（ページの中の `[[…]]` の解決。根の外には出ない）。
pub fn resolve_relative(root: &str, from: &str, link: &str) -> Option<String> {
    let base: Vec<&str> = from.split('/').collect();
    let mut parts: Vec<String> = base[..base.len().saturating_sub(1)]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for part in link.trim().split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }
    let joined = parts.join("/");
    let root = normalize_root(root);
    if root.is_empty() || joined == root || joined.starts_with(&format!("{root}/")) {
        Some(joined)
    } else {
        None
    }
}

/// ADR-0044 D7: 既定の文書リポジトリのディレクトリ名（案件の題名の slug。作れなければ案件の id）。
pub fn project_slug(title: &str, project_id: &str) -> String {
    match slugify(title) {
        Some(slug) => slug,
        None => project_id.to_ascii_lowercase(),
    }
}

/// front matter を書き足す（昇格のとき。既にあれば `title` / `tasks` を混ぜる）。
pub fn merge_front_matter(raw: &str, title: Option<&str>, task_id: &str) -> String {
    let (mut front, body) = front_matter(raw);
    if front
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
        && let Some(title) = title.map(str::trim).filter(|t| !t.is_empty())
    {
        front.title = Some(title.to_string());
    }
    if !front.tasks.iter().any(|t| t == task_id) {
        front.tasks.push(task_id.to_string());
    }
    let mut out = String::from("---\n");
    if let Some(title) = &front.title {
        out.push_str(&format!("title: {}\n", yaml_scalar(title)));
    }
    if !front.tags.is_empty() {
        out.push_str(&format!(
            "tags: [{}]\n",
            front
                .tags
                .iter()
                .map(|t| yaml_scalar(t))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str(&format!(
        "tasks: [{}]\n",
        front
            .tasks
            .iter()
            .map(|t| yaml_scalar(t))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("---\n");
    let body = body.trim_start_matches(['\n', '\r']);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
    }
    out
}

// ---------------------------------------------------------------------------
// 描画（`pulldown-cmark`。生 HTML は捨てる）
// ---------------------------------------------------------------------------

/// Markdown を HTML にする（決定的。ADR-0044 D7: **生 HTML は捨てる**ので、結果はそのまま
/// GUI に渡してよい）。表・脚注・打ち消し線は有効。
///
/// リンクの書き換え（ADR-0044 D7）:
/// - `celeris:task/<ULID>` → `/tasks/<ULID>`
/// - `[[relative/path.md]]` → `<a href="<link_base>?path=…">…</a>`（`link_base` は GUI の文書タブの URL）
pub fn render(body: &str, root: &str, from: &str, link_base: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag};

    let expanded = expand_wiki_links(body, root, from, link_base);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&expanded, options).filter_map(|event| match event {
        // 生 HTML は捨てる（`dangerouslySetInnerHTML` で描く側の安全はここが全部）。
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Link {
            link_type,
            dest_url: rewrite_link(&dest_url).into(),
            title,
            id,
        })),
        other => Some(other),
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

/// `celeris:task/<ULID>` → `/tasks/<ULID>`。それ以外はそのまま。
fn rewrite_link(dest: &str) -> String {
    match dest.trim().strip_prefix("celeris:task/") {
        Some(id) if !id.is_empty() => format!("/tasks/{id}"),
        _ => dest.to_string(),
    }
}

/// `[[relative/path.md]]` と `[[relative/path.md|題名]]` を普通の Markdown のリンクに開く。
/// 根の外に出るものは開かない（文字のまま残す）。
fn expand_wiki_links(body: &str, root: &str, from: &str, link_base: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = after.find("]]") else {
            out.push_str(after);
            return out;
        };
        let inner = &after[2..end];
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) => (t.trim(), l.trim().to_string()),
            None => (inner.trim(), inner.trim().to_string()),
        };
        match resolve_relative(root, from, target)
            .filter(|_| !target.is_empty() && !target.contains(['\n', ']']))
        {
            Some(path) => out.push_str(&format!(
                "[{}]({}?path={})",
                escape_markdown(&label),
                link_base,
                urlencode(&path)
            )),
            None => out.push_str(&after[..end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn escape_markdown(text: &str) -> String {
    text.replace('[', "\\[").replace(']', "\\]")
}

/// クエリに載せるための最小の percent encoding（英数字と `-._~/` 以外を逃がす）。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | '/') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 読み取り（default_branch の中身だけを見る）
// ---------------------------------------------------------------------------

/// コミット 1 件（ツリーの「最終コミット」とページの履歴）。
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct DocCommit {
    pub sha: String,
    /// RFC 3339（`%aI`）。
    pub at: String,
    pub author: String,
    pub subject: String,
}

/// 区切り（`git log --format` の中で使う。パスにもコミットメッセージにも出ない）。
pub(crate) const UNIT: char = '\u{1f}';
pub(crate) const RECORD: char = '\u{1e}';

fn branch_ref(default_branch: &str) -> String {
    format!("refs/heads/{default_branch}")
}

/// その default_branch が存在するか。
pub fn branch_exists(repo: &Path, default_branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &branch_ref(default_branch),
        ],
        GIT_TIMEOUT,
    )
    .is_some_and(|o| o.ok)
}

/// 文書の根にある `*.md` のパス（リポジトリ相対。並びは git の順 = 名前順）。
pub fn list(repo: &Path, default_branch: &str, root: &str) -> Vec<String> {
    let reference = branch_ref(default_branch);
    let root = normalize_root(root);
    let mut args: Vec<&str> = vec!["ls-tree", "-r", "--name-only", "-z", &reference];
    if !root.is_empty() {
        args.push("--");
        args.push(&root);
    }
    let Some(out) = git(repo, &args, GIT_TIMEOUT).filter(|o| o.ok) else {
        return Vec::new();
    };
    out.stdout
        .split('\0')
        .map(str::trim)
        .filter(|p| !p.is_empty() && p.to_ascii_lowercase().ends_with(".md"))
        .take(MAX_TREE_ITEMS)
        .map(str::to_string)
        .collect()
}

/// ADR-0044 D7 の `?q=`: 文書の根の中を `git grep -il`（大文字小文字を区別しない固定文字列）。
pub fn grep(repo: &Path, default_branch: &str, root: &str, needle: &str) -> Vec<String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let reference = branch_ref(default_branch);
    let root = normalize_root(root);
    let mut args: Vec<&str> = vec!["grep", "-I", "-i", "-l", "-F", "-e", needle, &reference];
    if !root.is_empty() {
        args.push("--");
        args.push(&root);
    }
    // 見つからなければ exit 1（エラーではない）。
    let Some(out) = git(repo, &args, GIT_TIMEOUT) else {
        return Vec::new();
    };
    let prefix = format!("{reference}:");
    out.stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.strip_prefix(&prefix)
                .or_else(|| l.split_once(':').map(|(_, p)| p))
        })
        .filter(|p| p.to_ascii_lowercase().ends_with(".md"))
        .take(MAX_TREE_ITEMS)
        .map(str::to_string)
        .collect()
}

/// 文書の根の下のページごとの**最後のコミット**（`git log --name-only` を 1 回だけ起こす）。
pub fn last_commits(repo: &Path, default_branch: &str, root: &str) -> HashMap<String, DocCommit> {
    let reference = branch_ref(default_branch);
    let root = normalize_root(root);
    // `--format=<fmt>` は **1 つの引数**で渡す（`--format <fmt>` は git が revision と読む）。
    let format = format!("--format={RECORD}%H{UNIT}%aI{UNIT}%an{UNIT}%s");
    let mut args: Vec<&str> = vec!["log", "--no-merges", "--name-only", &format, &reference];
    if !root.is_empty() {
        args.push("--");
        args.push(&root);
    }
    let mut out = HashMap::new();
    let Some(result) = git(repo, &args, GIT_TIMEOUT).filter(|o| o.ok) else {
        return out;
    };
    for record in result.stdout.split(RECORD).skip(1) {
        let mut lines = record.lines();
        let Some(header) = lines.next() else { continue };
        let Some(commit) = parse_commit(header) else {
            continue;
        };
        for path in lines.map(str::trim).filter(|p| !p.is_empty()) {
            out.entry(path.to_string())
                .or_insert_with(|| commit.clone());
        }
    }
    out
}

pub(crate) fn parse_commit(line: &str) -> Option<DocCommit> {
    let mut parts = line.split(UNIT);
    let sha = parts.next()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    Some(DocCommit {
        sha,
        at: parts.next().unwrap_or_default().trim().to_string(),
        author: parts.next().unwrap_or_default().trim().to_string(),
        subject: parts.next().unwrap_or_default().trim().to_string(),
    })
}

/// ページ 1 枚の履歴（新しい順、直近 [`HISTORY_LIMIT`] 件）。
pub fn history(repo: &Path, default_branch: &str, path: &str) -> Vec<DocCommit> {
    let reference = branch_ref(default_branch);
    let format = format!("--format=%H{UNIT}%aI{UNIT}%an{UNIT}%s");
    let limit = format!("-{HISTORY_LIMIT}");
    let Some(out) = git(
        repo,
        &["log", &limit, &format, &reference, "--", path],
        GIT_TIMEOUT,
    )
    .filter(|o| o.ok) else {
        return Vec::new();
    };
    out.stdout.lines().filter_map(parse_commit).collect()
}

/// ページの中身（default_branch の blob）。無ければ `None`。
pub fn read_page(repo: &Path, default_branch: &str, path: &str) -> Option<String> {
    let target = format!("{}:{path}", branch_ref(default_branch));
    let out = git(repo, &["show", &target], GIT_TIMEOUT)?;
    if !out.ok {
        return None;
    }
    Some(out.stdout)
}

/// ページの blob sha（`etag`）。無ければ `None`。
pub fn blob_sha(repo: &Path, default_branch: &str, path: &str) -> Option<String> {
    let target = format!("{}:{path}", branch_ref(default_branch));
    let out = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &target],
        GIT_TIMEOUT,
    )?;
    if !out.ok {
        return None;
    }
    let sha = out.stdout.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

// ---------------------------------------------------------------------------
// 書き込み（一時 worktree でコミットし default_branch を fast-forward）
// ---------------------------------------------------------------------------

/// 書き込みの結果（ADR-0044 D7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// コミットした（`sha` は新しいコミット、`etag` は書いた後の blob sha。削除なら `None`）。
    Written {
        sha: String,
        etag: Option<String>,
        /// 人のチェックアウトも早送りした。
        fast_forwarded: bool,
        /// 中身が同じだったのでコミットしなかった。
        unchanged: bool,
    },
    /// `etag` が現在の blob と違う（409）。`etag` はいまの値。
    EtagMismatch { etag: Option<String> },
    /// 人のチェックアウトが default_branch を編集中（409。ADR-0043 D5 と同じ規則）。
    Busy { detail: String },
    /// 消そうとしたページが無い（404）。
    Missing,
    /// git が失敗した。
    Failed { detail: String },
}

/// 1 回の書き込み（作成・更新・削除）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEdit {
    /// リポジトリ相対のパス（[`page_path`] を通したもの）。
    pub path: String,
    /// 本文。`None` なら削除。
    pub body: Option<String>,
    /// 期待する blob sha。新規作成のときだけ `None` でよい。
    pub etag: Option<String>,
    /// コミットメッセージ。
    pub message: String,
    /// `etag` の照合をしない（昇格の `overwrite: true`。宛先があってよいかは呼び出し側が決める）。
    pub overwrite: bool,
}

/// ADR-0044 D7: 人の編集を default_branch に直接コミットする。
///
/// 1. `etag` を照合（違えば 409。ページがあるのに `etag` が無いのも 409）
/// 2. 人のチェックアウトが default_branch を出していて dirty なら 409（ADR-0043 D5 と同じ規則）
/// 3. 一時 worktree（detach）で書き、`Celeris (human)` としてコミット
/// 4. default_branch を進める（人が出していて綺麗なら `merge --ff-only`、そうでなければ `update-ref`）
pub fn commit_page(
    repo: &Path,
    default_branch: &str,
    temp_dir: &Path,
    edit: &PageEdit,
) -> WriteOutcome {
    if !branch_exists(repo, default_branch) {
        return WriteOutcome::Failed {
            detail: format!("既定のブランチ {default_branch} がありません"),
        };
    }
    let current = blob_sha(repo, default_branch, &edit.path);
    if edit.body.is_none() && current.is_none() {
        return WriteOutcome::Missing;
    }
    // `overwrite`（昇格）は照合しない。それ以外は「ページがあるのに `etag` が無い」も 409。
    if !edit.overwrite && edit.etag.as_deref() != current.as_deref() {
        return WriteOutcome::EtagMismatch { etag: current };
    }

    let reference = branch_ref(default_branch);
    let Some(default_sha) = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &reference],
        GIT_TIMEOUT,
    )
    .filter(|o| o.ok)
    .map(|o| o.stdout.trim().to_string())
    .filter(|s| !s.is_empty()) else {
        return WriteOutcome::Failed {
            detail: format!("既定のブランチ {default_branch} を読めませんでした"),
        };
    };
    let checked_out = crate::changes::current_branch(repo).is_some_and(|b| b == default_branch);
    if checked_out && crate::changes::is_dirty(repo) != Some(false) {
        return WriteOutcome::Busy {
            detail: format!("{default_branch} が編集中"),
        };
    }

    if let Some(parent) = temp_dir.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return WriteOutcome::Failed {
            detail: "一時 worktree を作れませんでした".to_string(),
        };
    }
    let temp = temp_dir.to_string_lossy().into_owned();
    match git(
        repo,
        &["worktree", "add", "--detach", &temp, &reference],
        GIT_WRITE_TIMEOUT,
    ) {
        Some(o) if o.ok => {}
        Some(o) => {
            return WriteOutcome::Failed {
                detail: format!("一時 worktree を作れませんでした: {}", o.why()),
            };
        }
        None => {
            return WriteOutcome::Failed {
                detail: "git を起動できませんでした".to_string(),
            };
        }
    }
    let outcome = write_and_advance(
        repo,
        temp_dir,
        default_branch,
        &default_sha,
        checked_out,
        edit,
    );
    let _ = git(
        repo,
        &["worktree", "remove", "--force", &temp],
        GIT_WRITE_TIMEOUT,
    );
    let _ = git(repo, &["worktree", "prune"], GIT_TIMEOUT);
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(temp_dir);
    }
    outcome
}

fn write_and_advance(
    repo: &Path,
    temp_dir: &Path,
    default_branch: &str,
    default_sha: &str,
    checked_out: bool,
    edit: &PageEdit,
) -> WriteOutcome {
    let target = temp_dir.join(&edit.path);
    match &edit.body {
        Some(body) => {
            if let Some(parent) = target.parent()
                && std::fs::create_dir_all(parent).is_err()
            {
                return WriteOutcome::Failed {
                    detail: format!("{} を作れませんでした", edit.path),
                };
            }
            let mut text = body.replace("\r\n", "\n");
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            if std::fs::write(&target, text.as_bytes()).is_err() {
                return WriteOutcome::Failed {
                    detail: format!("{} を書けませんでした", edit.path),
                };
            }
        }
        None => {
            if std::fs::remove_file(&target).is_err() {
                return WriteOutcome::Failed {
                    detail: format!("{} を消せませんでした", edit.path),
                };
            }
        }
    }
    if !git(
        temp_dir,
        &["add", "-A", "--", &edit.path],
        GIT_WRITE_TIMEOUT,
    )
    .is_some_and(|o| o.ok)
    {
        return WriteOutcome::Failed {
            detail: format!("{} を git に載せられませんでした", edit.path),
        };
    }
    // 中身が同じなら何もコミットしない（`git commit` は「何も変わっていない」で失敗する）。
    let nothing =
        git(temp_dir, &["diff", "--cached", "--quiet"], GIT_TIMEOUT).is_some_and(|o| o.ok);
    if nothing {
        return WriteOutcome::Written {
            sha: default_sha.to_string(),
            etag: blob_sha(repo, default_branch, &edit.path),
            fast_forwarded: false,
            unchanged: true,
        };
    }
    let author = format!("user.name={DOCS_AUTHOR_NAME}");
    let email = format!("user.email={DOCS_AUTHOR_EMAIL}");
    match git(
        temp_dir,
        &[
            "-c",
            &author,
            "-c",
            &email,
            "commit",
            "-q",
            "-m",
            &edit.message,
        ],
        GIT_WRITE_TIMEOUT,
    ) {
        Some(o) if o.ok => {}
        Some(o) => {
            return WriteOutcome::Failed {
                detail: format!("コミットできませんでした: {}", o.why()),
            };
        }
        None => {
            return WriteOutcome::Failed {
                detail: "git を起動できませんでした".to_string(),
            };
        }
    }
    let Some(new_sha) = git(temp_dir, &["rev-parse", "HEAD"], GIT_TIMEOUT)
        .filter(|o| o.ok)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return WriteOutcome::Failed {
            detail: "コミットの結果を読めませんでした".to_string(),
        };
    };
    let advanced = if checked_out {
        git(repo, &["merge", "--ff-only", &new_sha], GIT_WRITE_TIMEOUT)
    } else {
        let reference = branch_ref(default_branch);
        git(
            repo,
            &["update-ref", &reference, &new_sha, default_sha],
            GIT_WRITE_TIMEOUT,
        )
    };
    match advanced {
        Some(o) if o.ok => WriteOutcome::Written {
            etag: blob_sha(repo, default_branch, &edit.path),
            sha: new_sha,
            fast_forwarded: checked_out,
            unchanged: false,
        },
        Some(o) => WriteOutcome::Failed {
            detail: format!("{default_branch} を進められませんでした: {}", o.why()),
        },
        None => WriteOutcome::Failed {
            detail: "git を起動できませんでした".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// 既定の文書リポジトリ（ADR-0044 D7）
// ---------------------------------------------------------------------------

/// `~/workspace/<slug>/` に文書リポジトリを作る（`git init -b main` + `docs/README.md` の最初のコミット）。
/// 既に git のリポジトリならそのまま使う（**中身は触らない**）。
pub fn init_docs_repo(dir: &Path, project_title: &str) -> Result<(), String> {
    if dir.join(".git").exists() {
        return Ok(());
    }
    if dir.exists()
        && std::fs::read_dir(dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        return Err(format!("{} は空ではありません", dir.display()));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("{} を作れませんでした: {e}", dir.display()))?;
    match git(dir, &["init", "-q", "-b", "main"], GIT_WRITE_TIMEOUT) {
        Some(o) if o.ok => {}
        Some(o) => return Err(format!("git init に失敗しました: {}", o.why())),
        None => return Err("git を起動できませんでした".to_string()),
    }
    let readme = dir.join(INITIAL_PAGE);
    if let Some(parent) = readme.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("docs/ を作れませんでした: {e}"))?;
    }
    let title = project_title.trim();
    let title = if title.is_empty() { "文書" } else { title };
    let body = format!(
        "# {title}\n\nこの案件の文書はここ（`docs/`）に Markdown で置く。\
         題名は 1 行目の `# `、タスクとの紐付けは front matter の `tasks: [<タスク id>]`。\n"
    );
    std::fs::write(&readme, body.as_bytes())
        .map_err(|e| format!("{INITIAL_PAGE} を書けませんでした: {e}"))?;
    if !git(dir, &["add", "-A"], GIT_WRITE_TIMEOUT).is_some_and(|o| o.ok) {
        return Err("git add に失敗しました".to_string());
    }
    let author = format!("user.name={DOCS_AUTHOR_NAME}");
    let email = format!("user.email={DOCS_AUTHOR_EMAIL}");
    match git(
        dir,
        &[
            "-c",
            &author,
            "-c",
            &email,
            "commit",
            "-q",
            "-m",
            "docs: 文書リポジトリを作る",
        ],
        GIT_WRITE_TIMEOUT,
    ) {
        Some(o) if o.ok => Ok(()),
        Some(o) => Err(format!("最初のコミットに失敗しました: {}", o.why())),
        None => Err("git を起動できませんでした".to_string()),
    }
}

/// 既定の文書リポジトリの置き場（`<base>/<slug>`。既にあって git でなければ `<slug>-<id の末尾 8>`）。
pub fn docs_repo_dir(base: &Path, title: &str, project_id: &str) -> PathBuf {
    let slug = project_slug(title, project_id);
    let first = base.join(&slug);
    if !first.exists() || first.join(".git").exists() {
        return first;
    }
    let tail: String = project_id
        .to_ascii_lowercase()
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    base.join(format!("{slug}-{tail}"))
}

#[cfg(test)]
mod tests;
