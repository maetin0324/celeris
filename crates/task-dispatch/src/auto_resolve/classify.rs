use super::git;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use task_core::integration_request::ConflictKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedPath {
    pub path: String,
    pub kind: ConflictKind,
    /// 同一 directory 内に同じ番号の別名 file がある場合の相手。
    pub duplicate_with: Vec<String>,
}

/// 番号付き ADR と日付名 ADR の置き場所。両方を同じ名前空間として見る（scripts/dev/check-adr-numbers.sh と同じ）。
pub const ADR_DIRS: [&str; 2] = ["agent-docs/adr/", "docs/adr/"];

/// 追記だけの記録: 凍結済みの PROGRESS.md と task ごとの進捗ファイル（入れ子を含む）。
fn is_record(path: &str) -> bool {
    path == "docs/PROGRESS.md"
        || path == "agent-docs/PROGRESS.md"
        || (path.starts_with("docs/progress/") || path.starts_with("agent-docs/progress/"))
            && path.ends_with(".md")
}

pub fn kind(path: &str) -> ConflictKind {
    if is_record(path) {
        ConflictKind::Record
    } else if numbered(path, "crates/task-core/migrations/", '_', ".sql").is_some() {
        ConflictKind::Migration
    } else if adr_number(path).is_some() || dated_adr(path) {
        ConflictKind::Adr
    } else if (path.starts_with("docs/protocol/") || path.starts_with("docs/api/v1/"))
        && path.ends_with(".schema.json")
    {
        ConflictKind::Generated
    } else {
        ConflictKind::Code
    }
}

/// `agent-docs/adr/NNNN-*.md` か `docs/adr/NNNN-*.md` の番号。日付名 `YYYY-MM-DD-<slug>.md`（ADR-0128 D5）は
/// 番号を持たない（先頭 4 桁の年を番号と見なさない。2026-10-04 本番の誤検出の原因）。
pub fn adr_number(path: &str) -> Option<String> {
    if dated_adr(path) {
        return None;
    }
    ADR_DIRS
        .iter()
        .find_map(|dir| numbered(path, dir, '-', ".md"))
}

/// ADR-0128 D5 の日付名 `YYYY-MM-DD-<slug>.md`。
pub fn dated_adr(path: &str) -> bool {
    ADR_DIRS.iter().any(|dir| {
        path.strip_prefix(dir).is_some_and(|name| {
            let b = name.as_bytes();
            !name.contains('/')
                && name.ends_with(".md")
                && b.len() > 14
                && b[..10].iter().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        *c == b'-'
                    } else {
                        c.is_ascii_digit()
                    }
                })
                && b[10] == b'-'
        })
    })
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
            adr_number(path).map(|n| (ConflictKindKey::Adr, n))
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
