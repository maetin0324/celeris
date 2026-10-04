//! 統合の依頼を追記事象に保存するための共有型（ADR parallel integration D4）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 新しい依頼に置き換わった古い依頼を閉じる `IntegrationAnswered.answer`（人の回答 `integrated` /
/// `declined` / `retry` と区別する。同じ発生元で両端の head が動いた依頼が記録されたとき）。
pub const SUPERSEDED_ANSWER: &str = "superseded";
/// 統合側（段の統合・配送）が、target が source を祖先に含むようになった（統合済み）ことを記録して閉じる answer。
/// 人が受信箱で「統合した」と答えたときの値と同じ。
pub const INTEGRATED_ANSWER: &str = "integrated";
/// task が終端になったため依頼を閉じる。統合の成功や人の回答を意味しない。
pub const TASK_TERMINAL_ANSWER: &str = "task_terminal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ConflictKind {
    Record,
    Migration,
    Adr,
    Generated,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolutionAction {
    pub path: String,
    pub kind: ConflictKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileDiffStat {
    pub added: u64,
    pub deleted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitIntent {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SideIntent {
    pub branch: String,
    pub path: String,
    pub commits: Vec<CommitIntent>,
    pub diffstat: Option<FileDiffStat>,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileIntent {
    pub path: String,
    pub target: SideIntent,
    pub source: SideIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// ADR D4: 同じ task と両側の head を一つの受信箱項目にする識別子。
    pub fn id_for(&self, task_id: crate::model::TaskId) -> String {
        format!("{task_id}:{}:{}", self.target_sha, self.source_sha)
    }

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
