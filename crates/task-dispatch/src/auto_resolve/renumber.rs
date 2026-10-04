//! ADR 2026-10-02-parallel-integration-auto-resolve D1b・付記（agent-docs 配置への追従）:
//! migration の番号衝突は取り込み側の file だけ空き番号へ振り直す。番号付き ADR の衝突は番号を
//! 振り直さず、取り込み側の file を日付名 `agent-docs/adr/YYYY-MM-DD-<slug>.md`（ADR-0128 D5）へ移す。
//! target（main）に既にある file は決して動かさない。参照は旧ファイル名・旧 stem だけを機械的に
//! 置換し、番号だけの参照（`ADR-0039`、`RESERVED_VERSIONS` の `39` など）は人に回す。

use super::classify::{
    adr_file_name, adr_number, allowed_adr_duplicates, dated_adr, in_tree, tree_paths,
};
use super::{ClassifiedPath, ConflictKind, ResolutionAction, ResolveAttempt, ResolveContext, git};
use std::collections::BTreeSet;
use std::path::Path;

/// 番号付き file の置き場所。
#[derive(Debug, Clone, Copy)]
struct Scheme {
    dir: &'static str,
    delimiter: char,
    extension: &'static str,
}

const MIGRATION: Scheme = Scheme {
    dir: "crates/task-core/migrations/",
    delimiter: '_',
    extension: ".sql",
};
/// 日付名 ADR の置き場所（ADR-0128 D5）。
const DATED_ADR_DIR: &str = "agent-docs/adr/";

impl Scheme {
    fn for_kind(kind: ConflictKind) -> Option<Self> {
        match kind {
            ConflictKind::Migration => Some(MIGRATION),
            _ => None,
        }
    }

    /// `dir/NNNN<delim>rest<ext>` を `(NNNN, <delim>rest<ext>)` に分ける。
    fn split<'a>(&self, path: &'a str) -> Option<(u32, &'a str)> {
        let name = path.strip_prefix(self.dir)?;
        if name.contains('/') || !name.ends_with(self.extension) {
            return None;
        }
        let (number, rest) = name.split_at_checked(4)?;
        if !number.bytes().all(|b| b.is_ascii_digit())
            || !rest.starts_with(self.delimiter)
            || rest.len() <= 1 + self.extension.len()
        {
            return None;
        }
        number.parse().ok().map(|n| (n, rest))
    }

    fn numbers(&self, paths: impl Iterator<Item = String>) -> BTreeSet<u32> {
        paths
            .filter_map(|p| self.split(&p).map(|(n, _)| n))
            .collect()
    }
}

/// 呼び出し側が ref を渡さないときの走査対象: target の SHA、`refs/heads/main`、全 `refs/heads/celeris/*`。
pub fn default_refs(repo: &Path, ctx: &ResolveContext) -> Result<Vec<String>, String> {
    let mut refs = vec![ctx.target_sha.clone()];
    for line in git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/main",
            "refs/heads/celeris/",
        ],
    )?
    .lines()
    .filter(|l| !l.is_empty())
    {
        refs.push(line.to_string());
    }
    Ok(refs)
}

pub fn resolve(
    repo: &Path,
    ctx: &ResolveContext,
    path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    let refs = default_refs(repo, ctx)?;
    resolve_with_refs(repo, ctx, path, &refs)
}

/// `refs` は番号の使用状況を見る ref（main と `refs/heads/celeris/*`）。同じ入力での再実行は何もしない。
pub fn resolve_with_refs(
    repo: &Path,
    ctx: &ResolveContext,
    item: &ClassifiedPath,
    refs: &[String],
) -> Result<ResolveAttempt, String> {
    let not_handled = |reason: String| Ok(ResolveAttempt::NotHandled { reason });
    if item.kind == ConflictKind::Adr {
        return resolve_adr(repo, ctx, item);
    }
    let Some(scheme) = Scheme::for_kind(item.kind) else {
        return not_handled("番号付き file ではない".into());
    };
    let path = item.path.as_str();
    let Some((number, rest)) = scheme.split(path) else {
        return not_handled("番号付き file の名前ではない".into());
    };
    let unmerged = unmerged_paths(repo)?.contains(path);
    let in_target = exists_at(repo, &ctx.target_sha, path);
    let present = worktree_paths(repo)?;

    if !unmerged {
        if in_target || !present.contains(path) {
            // target の file は動かさない。消えていれば既に振り直し済み。
            return Ok(ResolveAttempt::Handled {
                actions: Vec::new(),
            });
        }
        let duplicates = present
            .iter()
            .filter(|p| *p != path && scheme.split(p).is_some_and(|(n, _)| n == number))
            .collect::<Vec<_>>();
        if duplicates.is_empty() {
            return Ok(ResolveAttempt::Handled {
                actions: Vec::new(),
            });
        }
        if !duplicates
            .iter()
            .any(|p| exists_at(repo, &ctx.target_sha, p))
        {
            return not_handled(format!(
                "取り込み側同士の番号重複でどちらを動かすか決まらない: {}",
                duplicates
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else if !in_target {
        return not_handled("target に無い番号付き file が衝突している".into());
    }

    let base = merge_base(repo, ctx)?;
    if unmerged && exists_at(repo, &base, path) {
        return not_handled("既存の番号付き file の内容衝突（番号は同じ file を指す）".into());
    }
    let old_stem = stem(path, scheme);
    let added = added_lines(repo, &base, &ctx.source_sha)?;
    if unmerged
        && added
            .iter()
            .any(|(file, line)| file != path && contains_token(line, old_stem, is_name_char))
    {
        // 両側が同じ名前を足した: 取り込み側の参照がどちらを指すか決まらない。
        return not_handled(format!(
            "両側が同じ名前 {old_stem} を追加し、取り込み側の参照先が曖昧"
        ));
    }

    let next = next_number(repo, scheme, refs)?;
    let new_path = format!("{}{next:04}{rest}", scheme.dir);
    let mut actions = Vec::new();
    if unmerged {
        // 両側が同じ名前を追加した: target 版を残し、取り込み側（stage 3）を新しい番号へ。
        let theirs = git_bytes(repo, &["show", &format!(":3:{path}")])?;
        std::fs::write(repo.join(&new_path), theirs).map_err(|e| format!("{new_path}: {e}"))?;
        git(repo, &["checkout", "--ours", "--", path])?;
        git(repo, &["add", "--", path, &new_path])?;
        actions.push(action(
            &new_path,
            item.kind,
            format!("取り込み側の {path} を {new_path} として追加（target 版は残す）"),
        ));
    } else {
        git(repo, &["mv", "--", path, &new_path])?;
        actions.push(action(
            &new_path,
            item.kind,
            format!("git mv {path} {new_path}"),
        ));
    }

    let new_stem = stem(&new_path, scheme).to_string();
    let unmerged_now = unmerged_paths(repo)?;
    let new_title_number = format!("{next:04}");
    let old_number = format!("{number:04}");
    for file in worktree_paths(repo)? {
        if unmerged_now.contains(&file) {
            continue;
        }
        let Ok(bytes) = std::fs::read(repo.join(&file)) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let (mut updated, mut count) = if unmerged {
            (text.clone(), 0)
        } else {
            replace_token(&text, old_stem, &new_stem, is_name_char)
        };
        if file == new_path {
            // 動かした file 自身の題名（1 行目）の番号は自分を指す。
            let (first, tail) = updated.split_once('\n').unwrap_or((&updated, ""));
            let (title, n) = replace_token(first, &old_number, &new_title_number, is_digit);
            if n > 0 {
                count += n;
                updated = if updated.contains('\n') {
                    format!("{title}\n{tail}")
                } else {
                    title
                };
            }
        }
        if count == 0 {
            continue;
        }
        std::fs::write(repo.join(&file), &updated).map_err(|e| format!("{file}: {e}"))?;
        git(repo, &["add", "--", &file])?;
        actions.push(action(
            &file,
            item.kind,
            format!("参照更新 {old_stem} → {new_stem}（{count} 箇所）"),
        ));
    }

    let leftovers = bare_number_refs(&added, path, old_stem, number, true);
    if !leftovers.is_empty() {
        let done = actions
            .iter()
            .map(|a| a.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return not_handled(format!(
            "{done}。番号だけの参照 {old_number} は参照先を機械的に決められない: {}",
            leftovers.join(" | ")
        ));
    }
    Ok(ResolveAttempt::Handled { actions })
}

fn action(path: &str, kind: ConflictKind, detail: String) -> ResolutionAction {
    ResolutionAction {
        path: path.to_string(),
        kind,
        detail,
    }
}

fn stem(path: &str, scheme: Scheme) -> &str {
    file_stem(path, scheme.extension)
}

fn file_stem<'a>(path: &'a str, extension: &str) -> &'a str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(extension).unwrap_or(name)
}

/// 番号付き ADR の衝突: 取り込み側だけにある file を、取り込み側でその file を足した commit の日付名へ移す。
/// main（target）に入った ADR は動かさない。日付名同士の衝突はコード衝突と同じく人に回す。
fn resolve_adr(
    repo: &Path,
    ctx: &ResolveContext,
    item: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    let not_handled = |reason: String| Ok(ResolveAttempt::NotHandled { reason });
    let path = item.path.as_str();
    if dated_adr(path) {
        return not_handled("日付名 ADR の衝突（どちらの内容を採るかは人が決める）".into());
    }
    let Some(number) = adr_number(path) else {
        return not_handled("番号付き ADR の名前ではない".into());
    };
    let unmerged = unmerged_paths(repo)?.contains(path);
    let in_target = exists_at(repo, &ctx.target_sha, path);
    let present = worktree_paths(repo)?;

    if !unmerged {
        // 動かさない ADR: target に既にある（旧 docs/adr/ から移しただけのものを含む）か、許可リストにある。
        let target_tree = tree_paths(repo, &ctx.target_sha)?;
        let allowed = allowed_adr_duplicates(repo);
        let anchored = |p: &str| {
            in_tree(&target_tree, p) || adr_file_name(p).is_some_and(|name| allowed.contains(name))
        };
        if anchored(path) || !present.contains(path) {
            // target・許可リストの ADR は動かさない。消えていれば既に移動済み。
            return Ok(ResolveAttempt::Handled {
                actions: Vec::new(),
            });
        }
        let duplicates = present
            .iter()
            .filter(|p| *p != path && adr_number(p).as_deref() == Some(number.as_str()))
            .collect::<Vec<_>>();
        if duplicates.is_empty() {
            return Ok(ResolveAttempt::Handled {
                actions: Vec::new(),
            });
        }
        if !duplicates.iter().any(|p| anchored(p)) {
            return not_handled(format!(
                "取り込み側同士の ADR 番号重複でどちらを動かすか決まらない: {}",
                duplicates
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else if !in_target {
        return not_handled("target に無い番号付き ADR が衝突している".into());
    }

    let base = merge_base(repo, ctx)?;
    if unmerged && exists_at(repo, &base, path) {
        return not_handled("既存の ADR の内容衝突（番号は同じ file を指す）".into());
    }
    let old_stem = file_stem(path, ".md");
    let added = added_lines(repo, &base, &ctx.source_sha)?;
    if unmerged
        && added
            .iter()
            .any(|(file, line)| file != path && contains_token(line, old_stem, is_name_char))
    {
        return not_handled(format!(
            "両側が同じ名前 {old_stem} を追加し、取り込み側の参照先が曖昧"
        ));
    }
    let Some(date) = added_date(repo, &base, &ctx.source_sha, path)? else {
        return not_handled(format!(
            "取り込み側で {path} を足した commit が見つからない"
        ));
    };
    let slug = &old_stem[number.len() + 1..];
    let new_path = format!("{DATED_ADR_DIR}{date}-{slug}.md");
    if present.contains(&new_path) || exists_at(repo, &ctx.target_sha, &new_path) {
        return not_handled(format!("日付名 {new_path} が既にある（日付名同士の衝突）"));
    }

    let mut actions = Vec::new();
    if unmerged {
        // 両側が同じ名前を追加した: target 版を残し、取り込み側（stage 3）を日付名へ。
        let theirs = git_bytes(repo, &["show", &format!(":3:{path}")])?;
        let full = repo.join(&new_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{new_path}: {e}"))?;
        }
        std::fs::write(&full, theirs).map_err(|e| format!("{new_path}: {e}"))?;
        git(repo, &["checkout", "--ours", "--", path])?;
        git(repo, &["add", "--", path, &new_path])?;
        actions.push(action(
            &new_path,
            item.kind,
            format!("取り込み側の {path} を {new_path} として追加（target 版は残す）"),
        ));
    } else {
        if let Some(parent) = repo.join(&new_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{new_path}: {e}"))?;
        }
        git(repo, &["mv", "--", path, &new_path])?;
        actions.push(action(
            &new_path,
            item.kind,
            format!("git mv {path} {new_path}"),
        ));
    }

    // 参照の追従は取り込み側が足した・変えた file に限る（main 側の file は旧 stem を知らない）。
    let new_stem = file_stem(&new_path, ".md").to_string();
    let unmerged_now = unmerged_paths(repo)?;
    let mut followers = git(
        repo,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            &base,
            &ctx.source_sha,
        ],
    )?
    .lines()
    .filter(|l| !l.is_empty())
    .map(|l| {
        if l == path {
            new_path.clone()
        } else {
            l.to_string()
        }
    })
    .collect::<BTreeSet<_>>();
    followers.insert(new_path.clone());
    let old_title = format!("ADR-{number}");
    let new_title = format!("ADR {new_stem}");
    for file in followers {
        if unmerged_now.contains(&file) {
            continue;
        }
        let Ok(bytes) = std::fs::read(repo.join(&file)) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let (mut updated, mut count) = if unmerged {
            (text.clone(), 0)
        } else {
            replace_token(&text, old_stem, &new_stem, is_name_char)
        };
        if file == new_path {
            // 動かした file 自身の題名（1 行目）の `ADR-NNNN` は自分を指す。
            let (first, tail) = updated.split_once('\n').unwrap_or((&updated, ""));
            let (title, n) = replace_token(first, &old_title, &new_title, is_name_char);
            if n > 0 {
                count += n;
                updated = if updated.contains('\n') {
                    format!("{title}\n{tail}")
                } else {
                    title
                };
            }
        }
        if count == 0 {
            continue;
        }
        std::fs::write(repo.join(&file), &updated).map_err(|e| format!("{file}: {e}"))?;
        git(repo, &["add", "--", &file])?;
        actions.push(action(
            &file,
            item.kind,
            format!("参照更新 {old_stem} → {new_stem}（{count} 箇所）"),
        ));
    }

    let parsed = number.parse::<u32>().map_err(|e| format!("{path}: {e}"))?;
    let leftovers = bare_number_refs(&added, path, old_stem, parsed, false);
    if !leftovers.is_empty() {
        let done = actions
            .iter()
            .map(|a| a.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return not_handled(format!(
            "{done}。番号だけの参照 {number} は参照先を機械的に決められない: {}",
            leftovers.join(" | ")
        ));
    }
    Ok(ResolveAttempt::Handled { actions })
}

/// 取り込み側（`base..source`）で `path` を足した最初の commit の日付（`YYYY-MM-DD`）。
fn added_date(repo: &Path, base: &str, source: &str, path: &str) -> Result<Option<String>, String> {
    let range = format!("{base}..{source}");
    let log = git(
        repo,
        &[
            "log",
            "--diff-filter=A",
            "--no-renames",
            "--format=%cs",
            &range,
            "--",
            path,
        ],
    )?;
    Ok(log.lines().rfind(|l| !l.is_empty()).map(str::to_string))
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// `needle` の前後が `word` の文字でない出現だけを数える。
fn token_positions(haystack: &str, needle: &str, word: fn(char) -> bool) -> Vec<usize> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        if !before.is_some_and(word) && !after.is_some_and(word) {
            found.push(start);
        }
        from = end;
    }
    found
}

fn contains_token(haystack: &str, needle: &str, word: fn(char) -> bool) -> bool {
    !token_positions(haystack, needle, word).is_empty()
}

fn replace_token(
    haystack: &str,
    needle: &str,
    with: &str,
    word: fn(char) -> bool,
) -> (String, usize) {
    let positions = token_positions(haystack, needle, word);
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for start in &positions {
        out.push_str(&haystack[last..*start]);
        out.push_str(with);
        last = start + needle.len();
    }
    out.push_str(&haystack[last..]);
    (out, positions.len())
}

/// 取り込み側が足した行に残る、番号だけの参照（旧 stem を除いた後）。
fn bare_number_refs(
    added: &[(String, String)],
    moved: &str,
    old_stem: &str,
    number: u32,
    plain_in_rust: bool,
) -> Vec<String> {
    let padded = format!("{number:04}");
    let plain = number.to_string();
    let mut seen_title = false;
    let mut hits = Vec::new();
    for (file, line) in added {
        if file == moved && !seen_title {
            // 動かした file の 1 行目（題名）は自分を指すので更新済み。
            seen_title = true;
            continue;
        }
        let (stripped, _) = replace_token(line, old_stem, "", is_name_char);
        let padded_hit = contains_token(&stripped, &padded, is_digit);
        let plain_hit = plain_in_rust
            && file.ends_with(".rs")
            && contains_token(&stripped, &plain, is_number_char);
        if padded_hit || plain_hit {
            hits.push(format!("{file}: {}", line.trim()));
        }
    }
    hits
}

/// `base..source` で取り込み側が足した行（`(path, line)`、diff の順）。
fn added_lines(repo: &Path, base: &str, source: &str) -> Result<Vec<(String, String)>, String> {
    let diff = git(
        repo,
        &[
            "diff",
            "-U0",
            "--no-color",
            "--no-renames",
            "--no-ext-diff",
            base,
            source,
        ],
    )?;
    let mut current = None::<String>;
    let mut lines = Vec::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ ") {
            current = path.strip_prefix("b/").map(str::to_string);
        } else if let Some(text) = line.strip_prefix('+')
            && let Some(path) = &current
        {
            lines.push((path.clone(), text.to_string()));
        }
    }
    Ok(lines)
}

fn merge_base(repo: &Path, ctx: &ResolveContext) -> Result<String, String> {
    match &ctx.merge_base {
        Some(base) => Ok(base.clone()),
        None => Ok(
            git(repo, &["merge-base", &ctx.target_sha, &ctx.source_sha])?
                .trim()
                .to_string(),
        ),
    }
}

/// 走査対象の ref と作業 tree（index・未追跡）で使われている最大番号 + 1。
fn next_number(repo: &Path, scheme: Scheme, refs: &[String]) -> Result<u32, String> {
    let mut used = scheme.numbers(worktree_paths(repo)?.into_iter());
    for r in refs {
        let listed = git(
            repo,
            &["ls-tree", "-r", "--name-only", "-z", r, "--", scheme.dir],
        )?;
        used.extend(scheme.numbers(listed.split('\0').map(str::to_string)));
    }
    let next = used.last().copied().unwrap_or(0) + 1;
    if next > 9999 {
        return Err(format!("{} の番号が 4 桁を超える", scheme.dir));
    }
    Ok(next)
}

fn exists_at(repo: &Path, rev: &str, path: &str) -> bool {
    git(repo, &["cat-file", "-e", &format!("{rev}:{path}")]).is_ok()
}

fn unmerged_paths(repo: &Path) -> Result<BTreeSet<String>, String> {
    Ok(git(repo, &["diff", "--name-only", "--diff-filter=U"])?
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

fn worktree_paths(repo: &Path) -> Result<BTreeSet<String>, String> {
    Ok(git(
        repo,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?
    .split('\0')
    .filter(|s| !s.is_empty() && repo.join(s).exists())
    .map(str::to_string)
    .collect())
}

fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests;
