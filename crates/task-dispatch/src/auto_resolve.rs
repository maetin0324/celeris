//! ADR-0137: merge 中の衝突を分類し、定型 resolver に渡す。
//! 呼び出し側が `NeedsHuman` の後に merge を abort する。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

pub mod classify;
pub mod generated;
pub mod records;
pub mod renumber;

pub use classify::{ClassifiedPath, ConflictKind};

/// 呼び出し側が固定した merge の両端。SHA は branch の現在値から再取得しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveContext {
    pub target_branch: String,
    pub target_sha: String,
    pub source_branch: String,
    pub source_sha: String,
    pub merge_base: Option<String>,
    /// 生成物の再作成に使う argv。後続の resolver が解釈する。
    pub generated_command: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionAction {
    pub path: String,
    pub kind: ConflictKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Resolved { actions: Vec<ResolutionAction> },
    NeedsHuman { request: Box<IntegrationRequest> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveAttempt {
    Handled { actions: Vec<ResolutionAction> },
    NotHandled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffStat {
    pub added: u64,
    pub deleted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitIntent {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideIntent {
    pub branch: String,
    pub path: String,
    pub commits: Vec<CommitIntent>,
    pub diffstat: Option<FileDiffStat>,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIntent {
    pub path: String,
    pub target: SideIntent,
    pub source: SideIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationRequest {
    pub target_branch: String,
    pub target_sha: String,
    pub source_branch: String,
    pub source_sha: String,
    pub merge_base: Option<String>,
    pub conflict_files: Vec<String>,
    pub intent: Vec<FileIntent>,
    pub reason: String,
    pub recommendation: String,
    pub actions: Vec<ResolutionAction>,
    pub candidate_sha: Option<String>,
}

impl IntegrationRequest {
    /// 表示用。件名は git の事実だけを使用し、推測した意図を加えない。
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# 統合の依頼\n\n- target: `{}` (`{}`)\n- source: `{}` (`{}`)\n- merge base: `{}`\n- 理由: {}\n- 推奨: {}\n",
            self.target_branch,
            self.target_sha,
            self.source_branch,
            self.source_sha,
            self.merge_base.as_deref().unwrap_or("取得不可"),
            self.reason,
            self.recommendation
        );
        for file in &self.intent {
            out.push_str(&format!("\n## `{}`\n", file.path));
            for side in [&file.target, &file.source] {
                out.push_str(&format!("\n### `{}`\n", side.branch));
                if let Some(reason) = &side.unavailable {
                    out.push_str(&format!("- 履歴: 取得不可 ({reason})\n"));
                }
                for commit in &side.commits {
                    out.push_str(&format!("- `{}` {}\n", commit.sha, commit.subject));
                }
                if let Some(stat) = &side.diffstat {
                    out.push_str(&format!("- 差分: +{} -{}\n", stat.added, stat.deleted));
                }
            }
        }
        out
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
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
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn side_intent(repo: &Path, base: Option<&str>, sha: &str, branch: &str, path: &str) -> SideIntent {
    let mut side = SideIntent {
        branch: branch.to_string(),
        path: path.to_string(),
        commits: Vec::new(),
        diffstat: None,
        unavailable: None,
    };
    let Some(base) = base else {
        side.unavailable = Some("merge base が取得できない".into());
        return side;
    };
    let range = format!("{base}..{sha}");
    match git(repo, &["log", "--format=%H%x09%s", &range, "--", path]) {
        Ok(output) => {
            side.commits = output
                .lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(sha, subject)| CommitIntent {
                    sha: sha.into(),
                    subject: subject.into(),
                })
                .collect();
        }
        Err(e) => side.unavailable = Some(e),
    }
    match git(repo, &["diff", "--numstat", base, sha, "--", path]) {
        Ok(output) => {
            let (mut added, mut deleted) = (0, 0);
            for line in output.lines() {
                let mut fields = line.split('\t');
                let (Some(a), Some(d)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let (Ok(a), Ok(d)) = (a.parse::<u64>(), d.parse::<u64>()) else {
                    side.unavailable = Some("binary diffstat".into());
                    return side;
                };
                added += a;
                deleted += d;
            }
            side.diffstat = Some(FileDiffStat { added, deleted });
        }
        Err(e) => side.unavailable = Some(e),
    }
    side
}

fn recommendation(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::Code => "両側の意図を比較し、採る実装または統合案を指定する",
        ConflictKind::Record | ConflictKind::Migration | ConflictKind::Adr => {
            "記録・参照の正しい内容を指定してから再統合する"
        }
        ConflictKind::Generated => "生成コマンドと対象範囲を確認し、再生成と gate を行う",
    }
}

/// merge は中断しない。未解消がある場合も、試行済み action を依頼に残す。
pub fn resolve(repo: &Path, ctx: &ResolveContext) -> Result<Resolution, String> {
    let classified = classify::classify(repo, &ctx.target_sha)?;
    let mut actions = Vec::new();
    let mut unresolved = Vec::new();
    for item in &classified {
        let attempt = match item.kind {
            ConflictKind::Record => records::resolve(repo, ctx, item)?,
            ConflictKind::Migration | ConflictKind::Adr => renumber::resolve(repo, ctx, item)?,
            ConflictKind::Generated => generated::resolve(repo, ctx, item)?,
            ConflictKind::Code => ResolveAttempt::NotHandled {
                reason: "コードの内容衝突".into(),
            },
        };
        match attempt {
            ResolveAttempt::Handled { actions: completed } => actions.extend(completed),
            ResolveAttempt::NotHandled { reason } => unresolved.push((item, reason)),
        }
    }
    if unresolved.is_empty() {
        return Ok(Resolution::Resolved { actions });
    }
    let conflict_files = classified.iter().map(|item| item.path.clone()).collect();
    let intent = classified
        .iter()
        .map(|item| FileIntent {
            path: item.path.clone(),
            target: side_intent(
                repo,
                ctx.merge_base.as_deref(),
                &ctx.target_sha,
                &ctx.target_branch,
                &item.path,
            ),
            source: side_intent(
                repo,
                ctx.merge_base.as_deref(),
                &ctx.source_sha,
                &ctx.source_branch,
                &item.path,
            ),
        })
        .collect();
    let reason = unresolved
        .iter()
        .map(|(item, reason)| format!("{}: {reason}", item.path))
        .collect::<Vec<_>>()
        .join("; ");
    let recommendation = unresolved
        .iter()
        .map(|(item, _)| recommendation(item.kind))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Resolution::NeedsHuman {
        request: Box::new(IntegrationRequest {
            target_branch: ctx.target_branch.clone(),
            target_sha: ctx.target_sha.clone(),
            source_branch: ctx.source_branch.clone(),
            source_sha: ctx.source_sha.clone(),
            merge_base: ctx.merge_base.clone(),
            conflict_files,
            intent,
            reason,
            recommendation,
            actions,
            candidate_sha: None,
        }),
    })
}

#[cfg(test)]
mod tests;
