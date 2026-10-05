//! ADR-0047 付記（2026-10-04）H4: 旧形の `sources: human` を KB の git 履歴から判別して書き換える移行。
//!
//! 判断も LLM も無い（`git log` の author と件名だけで決める）。既定は dry-run。`apply` は書き換えを
//! 1 commit（author [`kb::AGENT_AUTHOR_NAME`]）にまとめる。判別できないページは書き換えずに一覧に出す
//! （`human` のまま = validator は保護し続ける）。

use super::*;

/// 1 ページの判別結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanSourceClass {
    /// 人の編集の commit がある → `human:authored`。
    Authored,
    /// run の書き込みだけ → `human:instruction`。
    Instruction,
    /// 判別できない（雛形の commit がある・履歴が無い 等）→ 保護のまま残す。
    Undetermined,
}

/// 移行の 1 件。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HumanSourceEntry {
    pub path: String,
    pub class: HumanSourceClass,
    /// 判別の根拠（人が読む 1 行）。
    pub reason: String,
    /// 書き換え前の `sources`。
    pub before: Vec<String>,
    /// 書き換え後の `sources`（`Undetermined` は `None`）。
    pub after: Option<Vec<String>>,
}

/// 移行の結果（dry-run と apply で同じ形）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct HumanSourceMigration {
    pub applied: bool,
    /// apply で作った commit（書き換えが無ければ `None`）。
    pub sha: Option<String>,
    pub entries: Vec<HumanSourceEntry>,
}

impl HumanSourceMigration {
    pub fn count(&self, class: HumanSourceClass) -> usize {
        self.entries.iter().filter(|e| e.class == class).count()
    }
}

/// commit 1 件の分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitKind {
    HumanEdit,
    Agent,
    Seed,
    Other,
}

fn classify_commit(author: &str, subject: &str) -> CommitKind {
    if author == kb::AGENT_AUTHOR_NAME {
        return CommitKind::Agent;
    }
    if author != kb::HUMAN_AUTHOR_NAME {
        // Celeris を通さない人の直接の git 編集。
        return CommitKind::HumanEdit;
    }
    if subject.contains("知識ベースを作る") || subject.contains("雛形を追加") {
        CommitKind::Seed
    } else if subject.contains("を取り込む") {
        // 人の accept だが、中身は run の候補。
        CommitKind::Agent
    } else if subject.contains("を捨てる") {
        CommitKind::Other
    } else {
        // `PUT /knowledge/page`（GUI の KB 編集）。
        CommitKind::HumanEdit
    }
}

/// 件名の `(task <id>)` / `（task <id>）` から task id を拾う。
fn task_id_of(subject: &str) -> Option<String> {
    let (_, rest) = subject.split_once("task ")?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (id.len() == 26).then_some(id)
}

/// `.git`・`skills/`・`_retired/` を除く `*.md`（`_inbox/` は含む）。
fn all_markdown(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if rel.starts_with('.') || kb::is_skills(&rel) || is_retired(&rel) || meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            all_markdown(root, &path, out);
        } else if rel.ends_with(".md") {
            out.push(rel);
        }
    }
}

fn replace_legacy(sources: &[String], mark: &str, task: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sources {
        let s = if s == kb::SOURCE_HUMAN_LEGACY {
            mark
        } else {
            s.as_str()
        };
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    if let Some(task) = task
        && !out.iter().any(|s| s.starts_with("task:"))
    {
        out.push(format!("task:{task}"));
    }
    out
}

/// 単数形 `source: human` の行を `author: human` に置き換える。
fn singular_to_author(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_front = false;
    for (i, line) in raw.split_inclusive('\n').enumerate() {
        let text = line.trim_end_matches(['\n', '\r']);
        if i == 0 && text == "---" {
            in_front = true;
        } else if in_front && (text == "---" || text == "...") {
            in_front = false;
        } else if in_front
            && text.split_once(':').is_some_and(|(k, v)| {
                k.trim().eq_ignore_ascii_case("source")
                    && v.trim().trim_matches(['"', '\'']) == "human"
            })
        {
            out.push_str("author: human");
            out.push_str(&line[text.len()..]);
            continue;
        }
        out.push_str(line);
    }
    out
}

fn singular_human(raw: &str) -> bool {
    singular_to_author(raw) != raw
}

/// 1 ページを判別し、書き換え後の原文を返す（`Undetermined` は `None`）。
fn plan_page(root: &Path, path: &str, raw: &str) -> (HumanSourceEntry, Option<String>) {
    let before = kb::front_matter(raw).0.sources;
    let log = git(
        root,
        &["log", "--format=%an%x1f%s", "--", path],
        GIT_TIMEOUT,
    )
    .filter(|o| o.ok)
    .map(|o| o.stdout)
    .unwrap_or_default();
    let commits: Vec<(CommitKind, &str)> = log
        .lines()
        .filter_map(|line| line.split_once('\u{1f}'))
        .map(|(author, subject)| (classify_commit(author, subject), subject))
        .collect();
    let undetermined = |reason: String| {
        (
            HumanSourceEntry {
                path: path.to_string(),
                class: HumanSourceClass::Undetermined,
                reason,
                before: before.clone(),
                after: None,
            },
            None,
        )
    };
    let has = |kind: CommitKind| commits.iter().any(|(k, _)| *k == kind);
    let singular = singular_human(raw);
    if has(CommitKind::HumanEdit) {
        let mut next = singular_to_author(raw);
        let sources = replace_legacy(&before, kb::SOURCE_HUMAN_AUTHORED, None);
        if before.iter().any(|s| s == kb::SOURCE_HUMAN_LEGACY) {
            match kb::replace_sources(&next, &sources) {
                Some(n) => next = n,
                None => return undetermined("front matter を書き換えられない".into()),
            }
        }
        let n = commits
            .iter()
            .filter(|(k, _)| *k == CommitKind::HumanEdit)
            .count();
        return (
            HumanSourceEntry {
                path: path.to_string(),
                class: HumanSourceClass::Authored,
                reason: format!("人の編集の commit が {n} 件ある"),
                before: before.clone(),
                after: Some(kb::front_matter(&next).0.sources),
            },
            Some(next),
        );
    }
    if commits.is_empty() {
        return undetermined("git の履歴が無い（未コミット）".into());
    }
    if has(CommitKind::Seed) {
        return undetermined("init の雛形の commit がある（human は init が付けた。人が埋めたかは履歴から分からない）".into());
    }
    if !has(CommitKind::Agent) {
        return undetermined("run の書き込みの commit が無い".into());
    }
    if singular {
        return undetermined("単数形 source: human（人の指示由来へ書き換える形が無い）".into());
    }
    // 最初の（いちばん古い）run の commit の task id を添える。
    let task = commits
        .iter()
        .rev()
        .filter(|(k, _)| *k == CommitKind::Agent)
        .find_map(|(_, s)| task_id_of(s));
    let sources = replace_legacy(&before, kb::SOURCE_HUMAN_INSTRUCTION, task.as_deref());
    let Some(next) = kb::replace_sources(raw, &sources) else {
        return undetermined("front matter を書き換えられない".into());
    };
    let n = commits
        .iter()
        .filter(|(k, _)| *k == CommitKind::Agent)
        .count();
    (
        HumanSourceEntry {
            path: path.to_string(),
            class: HumanSourceClass::Instruction,
            reason: format!("run の書き込みの commit だけ（{n} 件）"),
            before: before.clone(),
            after: Some(sources),
        },
        Some(next),
    )
}

/// ADR-0047 付記 H4: 旧形の `human` を持つページを判別する（`apply = false` は dry-run で何も書かない）。
pub fn migrate_human_sources(root: &Path, apply: bool) -> Result<HumanSourceMigration, String> {
    if !exists(root) {
        return Err(format!("{} は知識ベースではありません", root.display()));
    }
    let mut paths = Vec::new();
    all_markdown(root, root, &mut paths);
    paths.sort();
    let mut out = HumanSourceMigration {
        applied: apply,
        ..Default::default()
    };
    let mut writes: Vec<(String, String)> = Vec::new();
    for path in paths {
        let Ok(raw) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        if !kb::legacy_human(&raw) {
            continue;
        }
        let (entry, next) = plan_page(root, &path, &raw);
        if let Some(next) = next
            && next != raw
        {
            writes.push((path, next));
        }
        out.entries.push(entry);
    }
    if !apply || writes.is_empty() {
        return Ok(out);
    }
    for (path, next) in &writes {
        std::fs::write(root.join(path), next.as_bytes())
            .map_err(|e| format!("{path} を書けませんでした: {e}"))?;
    }
    let refs: Vec<&str> = writes.iter().map(|(p, _)| p.as_str()).collect();
    let sha = commit_paths(
        root,
        &format!(
            "knowledge: sources の human を移行（人が書いた {}・人の指示由来 {}・未判別 {}。ADR-0047 付記 2026-10-04）",
            out.count(HumanSourceClass::Authored),
            out.count(HumanSourceClass::Instruction),
            out.count(HumanSourceClass::Undetermined),
        ),
        (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
        &refs,
    )?;
    out.sha = Some(sha);
    let _ = reindex(root);
    Ok(out)
}

#[cfg(test)]
#[path = "human_sources_tests.rs"]
mod tests;
