//! ADR 2026-10-02-parallel-integration-auto-resolve D1b: migration・ADR の番号衝突を、取り込み側の file だけ空き番号へ振り直す。
//! target（main）に既にある file は決して動かさない。参照は旧ファイル名・旧 stem だけを機械的に
//! 置換し、番号だけの参照（`ADR-0039`、`RESERVED_VERSIONS` の `39` など）は人に回す。

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
const ADR: Scheme = Scheme {
    dir: "docs/adr/",
    delimiter: '-',
    extension: ".md",
};

impl Scheme {
    fn for_kind(kind: ConflictKind) -> Option<Self> {
        match kind {
            ConflictKind::Migration => Some(MIGRATION),
            ConflictKind::Adr => Some(ADR),
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

    let leftovers = bare_number_refs(&added, path, old_stem, number, scheme);
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
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(scheme.extension).unwrap_or(name)
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
    scheme: Scheme,
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
        let plain_hit = scheme.dir == MIGRATION.dir
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
