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
/// 配送（task が done になった後に main へ取り込む段）が記録する依頼の origin。配送は done の task の上で
/// 人の判断を待つので、`TASK_TERMINAL_ANSWER` の回収対象から外す（配送は統合できたら自分で閉じる）。
pub const DELIVERY_ORIGIN: &str = "delivery";

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

/// ADR 2026-10-28-cos-integrate-task D4: 受信箱の統合の依頼で「integrate task に解かせる」を選ぶ option の key。
pub const INTEGRATE_TASK_OPTION: &str = "integrate_task";
/// 同 ADR D3: `IntegrateTaskSettled.outcome`。元の依頼を閉じた。
pub const INTEGRATE_TASK_INTEGRATED: &str = "integrated";
/// 同 ADR D3: `IntegrateTaskSettled.outcome`。integrate task が失敗し、依頼を人に回した。
pub const INTEGRATE_TASK_ESCALATED: &str = "escalated";

/// 同 ADR D2・D3: 統合の依頼 1 件と integrate task の結びの今の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrateTaskLink {
    /// 結んだ integrate task がまだ決着していない。
    Active { task_id: crate::model::TaskId },
    /// integrate task が失敗し、依頼を人に回した。
    Escalated {
        task_id: crate::model::TaskId,
        detail: String,
    },
    /// integrate task の取り込みで依頼を閉じた。
    Integrated { task_id: crate::model::TaskId },
}

impl IntegrateTaskLink {
    pub fn task_id(&self) -> crate::model::TaskId {
        match self {
            IntegrateTaskLink::Active { task_id }
            | IntegrateTaskLink::Escalated { task_id, .. }
            | IntegrateTaskLink::Integrated { task_id } => *task_id,
        }
    }
}

/// 元の task の events から、依頼 `request_id` の最新の結びを畳む（無ければ `None`）。
pub fn integrate_task_link<'a>(
    events: impl IntoIterator<Item = &'a crate::model::Event>,
    request_id: &str,
) -> Option<IntegrateTaskLink> {
    let mut link = None;
    for event in events {
        match event {
            crate::model::Event::IntegrateTaskRequested {
                request_id: id,
                integrate_task_id,
                ..
            } if id == request_id => {
                link = Some(IntegrateTaskLink::Active {
                    task_id: *integrate_task_id,
                });
            }
            crate::model::Event::IntegrateTaskSettled {
                request_id: id,
                integrate_task_id,
                outcome,
                detail,
            } if id == request_id
                && link.as_ref().map(IntegrateTaskLink::task_id) == Some(*integrate_task_id) =>
            {
                link = Some(if outcome == INTEGRATE_TASK_ESCALATED {
                    IntegrateTaskLink::Escalated {
                        task_id: *integrate_task_id,
                        detail: detail.clone(),
                    }
                } else {
                    IntegrateTaskLink::Integrated {
                        task_id: *integrate_task_id,
                    }
                });
            }
            _ => {}
        }
    }
    link
}
