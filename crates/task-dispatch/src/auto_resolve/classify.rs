use super::git;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictKind {
    Record,
    Migration,
    Adr,
    Generated,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedPath {
    pub path: String,
    pub kind: ConflictKind,
    /// 同一 directory 内に同じ番号の別名 file がある場合の相手。
    pub duplicate_with: Vec<String>,
}

pub fn kind(path: &str) -> ConflictKind {
    if path == "docs/PROGRESS.md"
        || path == "agent-docs/PROGRESS.md"
        || (path.starts_with("docs/progress/") || path.starts_with("agent-docs/progress/"))
            && path.ends_with(".md")
    {
        ConflictKind::Record
    } else if numbered(path, "crates/task-core/migrations/", '_', ".sql").is_some() {
        ConflictKind::Migration
    } else if numbered(path, "docs/adr/", '-', ".md").is_some() {
        ConflictKind::Adr
    } else if (path.starts_with("docs/protocol/") || path.starts_with("docs/api/v1/"))
        && path.ends_with(".schema.json")
    {
        ConflictKind::Generated
    } else {
        ConflictKind::Code
    }
}

fn numbered(path: &str, dir: &str, delimiter: char, extension: &str) -> Option<String> {
    let name = path.strip_prefix(dir)?;
    if name.contains('/') || !name.ends_with(extension) {
        return None;
    }
    let (number, rest) = name.split_once(delimiter)?;
    (number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty())
        .then(|| number.to_string())
}

/// 現在の unmerged path と、index/working tree の番号重複を安定した順序で返す。
pub fn classify(repo: &Path, target_sha: &str) -> Result<Vec<ClassifiedPath>, String> {
    let mut paths = BTreeMap::<String, ClassifiedPath>::new();
    for path in git(repo, &["diff", "--name-only", "--diff-filter=U"])?
        .lines()
        .filter(|s| !s.is_empty())
    {
        paths.insert(
            path.to_string(),
            ClassifiedPath {
                path: path.to_string(),
                kind: kind(path),
                duplicate_with: Vec::new(),
            },
        );
    }
    let listed = git(
        repo,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut groups = BTreeMap::<(ConflictKindKey, String), BTreeSet<String>>::new();
    for path in listed.split('\0').filter(|s| !s.is_empty()) {
        let key = if let Some(number) = numbered(path, "crates/task-core/migrations/", '_', ".sql")
        {
            Some((ConflictKindKey::Migration, number))
        } else {
            numbered(path, "docs/adr/", '-', ".md").map(|n| (ConflictKindKey::Adr, n))
        };
        if let Some(key) = key {
            groups.entry(key).or_default().insert(path.to_string());
        }
    }
    // 既に target にあった重複は今回の取り込みの問題ではない。
    let changed = git(repo, &["diff", "--name-only", target_sha, "--"])?
        .lines()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for members in groups
        .values()
        .filter(|members| members.len() > 1 && !members.is_disjoint(&changed))
    {
        for path in members {
            let others = members
                .iter()
                .filter(|other| *other != path)
                .cloned()
                .collect();
            match paths.entry(path.clone()) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().duplicate_with = others;
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(ClassifiedPath {
                        path: path.clone(),
                        kind: kind(path),
                        duplicate_with: others,
                    });
                }
            }
        }
    }
    Ok(paths.into_values().collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ConflictKindKey {
    Migration,
    Adr,
}
