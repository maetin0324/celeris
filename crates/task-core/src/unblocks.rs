//! ADR 2026-10-27-unblocks-reopen-on-promotion: 修正 task と、その修正を待っている task の結び。
//!
//! 結びは修正 task の作成時（`POST /tasks` の `unblocks`）に `task_unblocks` へ書く。修正 task を含む
//! release が昇格したら、daemon が待っている task を 1 度だけ reopen（`failed`）するかコメントを残す
//! （`blocked`）。判定は `task_ops::unblocks`、ここは型だけ。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::comment::TaskComment;
use crate::model::TaskId;

/// 1 つの task が `unblocks` に書ける上限。
pub const MAX_UNBLOCKS: usize = 8;

/// 昇格を見つけたときに行ったこと（`task_unblocks.fired_action`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnblockAction {
    /// 待っている task は `failed` だった。system コメントと `Trigger::Reopen` を同じ transaction で書いた。
    Reopened,
    /// 待っている task は `blocked` だった。状態は変えず system コメントだけを残した。
    Commented,
    /// 待っている task は待っていなかった（`ready`・`running`・`done` など）。何もしていない。
    Skipped,
}

impl UnblockAction {
    pub fn as_str(self) -> &'static str {
        match self {
            UnblockAction::Reopened => "reopened",
            UnblockAction::Commented => "commented",
            UnblockAction::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reopened" => Some(UnblockAction::Reopened),
            "commented" => Some(UnblockAction::Commented),
            "skipped" => Some(UnblockAction::Skipped),
            _ => None,
        }
    }
}

/// `task_unblocks` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnblockLink {
    pub fix_task_id: TaskId,
    pub waiting_task_id: TaskId,
    pub created_at: OffsetDateTime,
    /// 昇格を見つけた release の sha12（未発火なら `None`）。
    pub fired_release: Option<String>,
    pub fired_action: Option<UnblockAction>,
    pub fired_at: Option<OffsetDateTime>,
}

/// 結びを 1 度だけ発火させる書き込み。`TaskStore::unblock_fire` が 1 つの transaction で行う:
/// `fired_at IS NULL` の行だけを更新し、更新できたときだけ `comment`（あれば）と `reopen`（真なら
/// `Trigger::Reopen`）を書く。
#[derive(Debug, Clone)]
pub struct UnblockFire {
    pub fix_task_id: TaskId,
    pub waiting_task_id: TaskId,
    pub release: String,
    pub action: UnblockAction,
    pub comment: Option<TaskComment>,
    pub now: OffsetDateTime,
}
