//! ADR-0131 D10: worker の提案を検証し、KB の差分と反映を作る。
//! 判断は worker 側で行い、この module はファイル操作だけを行う。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use task_core::knowledge::front_matter;

use crate::knowledge;

/// 日次整理の harness id（`[[cron.seed.template]] harness`。cron 由来の task の `genre` になる）。
pub const CURATION_HARNESS: &str = "knowledge-curation";

/// cron が作った日次整理 task か（ADR-0131 付記 D10 (6): この task の報告は daemon が 1 件だけ作り、
/// 報告のまとめにも入れない）。
pub fn is_curation_task(task: &task_core::Task) -> bool {
    task.genre.as_deref() == Some(CURATION_HARNESS)
        && task
            .labels
            .iter()
            .any(|l| l == crate::cron_jobs::CRON_TASK_LABEL)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CurationPlan {
    pub version: u32,
    pub kb: Vec<KbAction>,
    pub inbox: Vec<InboxProposal>,
    pub human_decisions: Vec<HumanDecision>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Merge,
    New,
    Delete,
    Keep,
    Fix,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KbAction {
    pub path: String,
    pub action: Action,
    pub target: Option<String>,
    pub reason: String,
    /// New/fix はこの本文、merge は統合先の完成した本文。delete/keep は null。
    pub content: Option<String>,
    /// 変更前の path の SHA-256（new は null）。
    pub expected_hash: Option<String>,
    /// merge の統合先の変更前 SHA-256。それ以外は null。
    pub target_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxProposal {
    pub task_id: String,
    pub proposal: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HumanDecision {
    pub subject: String,
    pub proposal: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPlan {
    pub kb: Vec<KbAction>,
    pub inbox: Vec<InboxProposal>,
    pub human_decisions: Vec<HumanDecision>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub merged: usize,
    pub new: usize,
    pub deleted: usize,
    pub fixed: usize,
    pub kept: usize,
    pub skipped_human: usize,
}

pub fn content_hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn safe_path(root: &Path, name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if path.components().count() < 2
        || path.extension().is_none_or(|ext| ext != "md")
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || name.starts_with("_curation/")
        || name.starts_with(".git/")
        || name == "README.md"
    {
        return Err(format!("不正な KB path: {name}"));
    }
    // 既存の symlink をすべて拒否する。未作成の末端についても親を走査する。
    let mut current = root.to_path_buf();
    for component in path.components() {
        current.push(component);
        if let Ok(meta) = std::fs::symlink_metadata(&current)
            && meta.file_type().is_symlink()
        {
            return Err(format!("symlink を通る KB path: {name}"));
        }
    }
    Ok(())
}

fn read_page(root: &Path, name: &str) -> Result<Option<String>, String> {
    safe_path(root, name)?;
    let path = root.join(name);
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(&path)
        .map(Some)
        .map_err(|e| format!("{name} を読めません: {e}"))
}

fn check_hash(name: &str, raw: &str, expected: Option<&str>) -> Result<(), String> {
    if expected != Some(content_hash(raw).as_str()) {
        return Err(format!("{name} の content hash が一致しません"));
    }
    Ok(())
}

fn human_page(name: &str, raw: &str) -> bool {
    if name.starts_with("user/") {
        return true;
    }
    let (front, _) = front_matter(raw);
    if front.sources.iter().any(|source| source == "human") {
        return true;
    }
    // 古いページの単数形も保護する。
    raw.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .is_some_and(|(head, _)| head.lines().any(|line| line.trim() == "source: human"))
}

fn major_rewrite(before: &str, after: &str) -> bool {
    let old: Vec<&str> = before
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if old.is_empty() {
        return false;
    }
    let new: BTreeSet<&str> = after.lines().collect();
    old.iter().filter(|line| !new.contains(**line)).count() * 2 > old.len()
}

/// 全操作を事前に検証する。保護ページの操作は返値の human_decisions に移す。
pub fn validate(root: &Path, plan: &CurationPlan) -> Result<ValidatedPlan, String> {
    validate_with_inbox(root, plan, None)
}

/// 入力 manifest の task id を渡した場合、提案先が実際の入力にあることも確かめる。
pub fn validate_with_inbox(
    root: &Path,
    plan: &CurationPlan,
    known_task_ids: Option<&BTreeSet<String>>,
) -> Result<ValidatedPlan, String> {
    if plan.version != 1 {
        return Err(format!("未対応の curation plan version: {}", plan.version));
    }
    if !root.is_dir() {
        return Err(format!("KB root がありません: {}", root.display()));
    }
    if !root.join("README.md").is_file() {
        return Err("KB の README.md がありません".into());
    }
    for generated in ["README.md", "index.json", "_curation"] {
        if let Ok(meta) = std::fs::symlink_metadata(root.join(generated))
            && meta.file_type().is_symlink()
        {
            return Err(format!("派生ファイルが symlink です: {generated}"));
        }
    }
    let mut used = BTreeSet::new();
    let mut kb = Vec::new();
    let mut human_decisions = plan.human_decisions.clone();
    for item in &plan.kb {
        if item.reason.trim().is_empty() {
            return Err(format!("{} の理由がありません", item.path));
        }
        if item.content.as_deref().is_some_and(str::is_empty) {
            return Err(format!("{} の新しい本文が空です", item.path));
        }
        safe_path(root, &item.path)?;
        if !used.insert(item.path.clone()) {
            return Err(format!("重複した KB 操作: {}", item.path));
        }
        let before = read_page(root, &item.path)?;
        let target_before = match &item.target {
            Some(target) => {
                safe_path(root, target)?;
                if target == &item.path || !used.insert(target.clone()) {
                    return Err(format!("重複した KB 操作先: {target}"));
                }
                read_page(root, target)?
            }
            None => None,
        };
        match item.action {
            Action::New
                if before.is_none()
                    && item.target.is_none()
                    && item.content.is_some()
                    && item.expected_hash.is_none()
                    && item.target_hash.is_none() => {}
            Action::Merge
                if before.is_some() && target_before.is_some() && item.content.is_some() => {}
            Action::Delete | Action::Keep | Action::Fix
                if before.is_some()
                    && item.target.is_none()
                    && item.target_hash.is_none()
                    && (item.action == Action::Fix) == item.content.is_some() => {}
            _ => {
                return Err(format!(
                    "{} の action と path/content/target が一致しません",
                    item.path
                ));
            }
        }
        if let Some(raw) = before.as_deref() {
            check_hash(&item.path, raw, item.expected_hash.as_deref())?;
        }
        if let Some(raw) = target_before.as_deref() {
            check_hash(
                item.target.as_deref().unwrap_or_default(),
                raw,
                item.target_hash.as_deref(),
            )?;
        }
        let protected = before
            .as_deref()
            .is_some_and(|raw| human_page(&item.path, raw))
            && (item.action == Action::Delete
                || item.action == Action::Merge
                || (item.action == Action::Fix
                    && major_rewrite(
                        before.as_deref().unwrap_or_default(),
                        item.content.as_deref().unwrap_or_default(),
                    )));
        let target_protected = target_before.as_deref().is_some_and(|raw| {
            human_page(item.target.as_deref().unwrap_or_default(), raw)
                && major_rewrite(raw, item.content.as_deref().unwrap_or_default())
        });
        if protected || target_protected {
            human_decisions.push(HumanDecision {
                subject: item.path.clone(),
                proposal: format!(
                    "{:?}{}",
                    item.action,
                    item.target
                        .as_ref()
                        .map(|t| format!(" -> {t}"))
                        .unwrap_or_default()
                ),
                reason: format!("人が書いたページの保護: {}", item.reason),
            });
        } else {
            kb.push(item.clone());
        }
    }
    let mut task_ids = BTreeSet::new();
    for item in &plan.inbox {
        if item.task_id.trim().is_empty()
            || item.proposal.trim().is_empty()
            || item.reason.trim().is_empty()
            || !task_ids.insert(&item.task_id)
            || known_task_ids.is_some_and(|known| !known.contains(&item.task_id))
        {
            return Err("不正または重複した inbox 提案".into());
        }
    }
    Ok(ValidatedPlan {
        kb,
        inbox: plan.inbox.clone(),
        human_decisions,
    })
}

type PageChanges = BTreeMap<String, (Option<String>, Option<String>)>;

fn planned_pages(root: &Path, plan: &ValidatedPlan) -> Result<PageChanges, String> {
    let mut changes = BTreeMap::new();
    for item in &plan.kb {
        let before = read_page(root, &item.path)?;
        let after = match item.action {
            Action::New | Action::Fix => item.content.clone(),
            Action::Keep => before.clone(),
            Action::Merge | Action::Delete => None,
        };
        changes.insert(item.path.clone(), (before, after));
        if let Some(target) = &item.target {
            changes.insert(
                target.clone(),
                (read_page(root, target)?, item.content.clone()),
            );
        }
    }
    Ok(changes)
}

/// Worker が返した unified diff の KB path 集合を、検証済み計画の変更 path 集合と照合する。
/// `inputs/kb/` は worker の入力コピーを指す接頭辞なので比較前に取り除く。
pub fn check_diff_matches(validated: &ValidatedPlan, worker_diff: &str) -> Result<(), String> {
    let expected: BTreeSet<String> = validated
        .kb
        .iter()
        .filter(|item| item.action != Action::Keep)
        .flat_map(|item| std::iter::once(item.path.as_str()).chain(item.target.as_deref()))
        .map(str::to_owned)
        .collect();
    let mut actual = BTreeSet::new();
    for line in worker_diff.lines() {
        let header = line
            .strip_prefix("--- ")
            .or_else(|| line.strip_prefix("+++ "));
        let Some(header) = header else { continue };
        // Unified diff の timestamp は path の後ろに TAB 区切りで付く。
        let raw_path = header.split_once('\t').map_or(header, |(path, _)| path);
        if raw_path == "/dev/null" {
            continue;
        }
        let path = raw_path
            .strip_prefix("a/")
            .or_else(|| raw_path.strip_prefix("b/"))
            .unwrap_or(raw_path);
        let path = path.strip_prefix("inputs/kb/").unwrap_or(path);
        if !path.is_empty() {
            actual.insert(path.to_owned());
        }
    }
    if actual == expected {
        return Ok(());
    }
    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
    let extra: Vec<_> = actual.difference(&expected).cloned().collect();
    Err(format!(
        "curation.diff の path が計画と一致しません (不足: [{}]; 余分: [{}])",
        missing.join(", "),
        extra.join(", ")
    ))
}

fn unified(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let old = before.unwrap_or_default();
    let new = after.unwrap_or_default();
    if before == after {
        return String::new();
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = format!(
        "--- {}\n+++ {}\n@@ -{},{} +{},{} @@\n",
        if before.is_some() {
            format!("a/{path}")
        } else {
            "/dev/null".into()
        },
        if after.is_some() {
            format!("b/{path}")
        } else {
            "/dev/null".into()
        },
        usize::from(!old_lines.is_empty()),
        old_lines.len(),
        usize::from(!new_lines.is_empty()),
        new_lines.len()
    );
    for line in old_lines {
        out.push_str(&format!("-{line}\n"));
    }
    if !old.is_empty() && !old.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    for line in new_lines {
        out.push_str(&format!("+{line}\n"));
    }
    if !new.is_empty() && !new.ends_with('\n') {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

/// 本番 KB は一切書かず、メモリ上の写しに適用した unified diff を返す。
pub fn dry_run(root: &Path, plan: &CurationPlan) -> Result<String, String> {
    let validated = validate(root, plan)?;
    let mut diff = String::new();
    for (path, (before, after)) in planned_pages(root, &validated)? {
        diff.push_str(&unified(&path, before.as_deref(), after.as_deref()));
    }
    Ok(diff)
}

/// 検証済み計画を反映する。承認と diff hash の照合は呼び出し側の責務。
pub fn apply(root: &Path, plan: &CurationPlan, date: &str) -> Result<ApplyOutcome, String> {
    if date.len() != 10
        || !date.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        return Err("date は YYYY-MM-DD で指定してください".into());
    }
    let validated = validate(root, plan)?;
    let changes = planned_pages(root, &validated)?;
    let mut outcome = ApplyOutcome {
        skipped_human: validated
            .human_decisions
            .len()
            .saturating_sub(plan.human_decisions.len()),
        ..Default::default()
    };
    let mut deleted = Vec::new();
    for item in &validated.kb {
        match item.action {
            Action::Merge => {
                outcome.merged += 1;
                deleted.push((&item.path, &item.reason));
            }
            Action::New => outcome.new += 1,
            Action::Delete => {
                outcome.deleted += 1;
                deleted.push((&item.path, &item.reason));
            }
            Action::Fix => outcome.fixed += 1,
            Action::Keep => outcome.kept += 1,
        }
    }
    for (path, (_, after)) in changes {
        let full = root.join(&path);
        match after {
            Some(body) => {
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&full, body).map_err(|e| format!("{path}: {e}"))?;
            }
            None => std::fs::remove_file(&full).map_err(|e| format!("{path}: {e}"))?,
        }
    }
    if !deleted.is_empty() {
        let dir = root.join("_curation");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let log = dir.join(format!("{date}.md"));
        let mut body = if log.exists() {
            std::fs::read_to_string(&log).map_err(|e| e.to_string())?
        } else {
            format!("# 知識整理の削除記録 ({date})\n\n")
        };
        for (path, reason) in deleted {
            body.push_str(&format!("- `{path}`: {}\n", reason.replace('\n', " ")));
        }
        std::fs::write(log, body).map_err(|e| e.to_string())?;
    }
    let index = knowledge::reindex(root)?;
    let readme = root.join("README.md");
    let original = std::fs::read_to_string(&readme).map_err(|e| e.to_string())?;
    const START: &str = "<!-- curation:index:start -->";
    const END: &str = "<!-- curation:index:end -->";
    let prefix = original.split(START).next().unwrap_or_default().trim_end();
    let suffix = original
        .split_once(END)
        .map(|(_, tail)| tail)
        .unwrap_or_default();
    let mut body = format!("{prefix}\n\n{START}\n## ページ一覧\n\n");
    for item in index
        .items
        .iter()
        .filter(|item| item.path != "README.md" && !item.path.starts_with("_curation/"))
    {
        body.push_str(&format!(
            "- [{}]({})\n",
            item.title.replace('[', "\\[").replace(']', "\\]"),
            item.path
        ));
    }
    body.push_str(&format!("{END}{suffix}"));
    if !body.ends_with('\n') {
        body.push('\n');
    }
    std::fs::write(readme, body).map_err(|e| e.to_string())?;
    Ok(outcome)
}

#[cfg(test)]
mod tests;
