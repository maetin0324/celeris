//! 途中目標の判定（ADR-0038。Phase 41）の**読み取りだけ**を残したもの。
//!
//! ADR-0079 D13（Phase R5a）: 途中目標は凍結した。判定の run（`start_review`。celeris の
//! `milestone_review::schedule`）・結果ファイルの `milestone_proposal` の取り込み（`record_proposal`）・人の判定
//! （`decide`。`POST /milestones/{id}/decide` は 410）・`MilestoneReady` の通知は外した。既存の行とレビューの
//! 対話を `GET /projects/{id}?include_frozen=true` と Console が履歴として読むための関数だけが残る。
//!
//! I/O は `TaskStore` の読み取りだけ（DESIGN 原則 1）。

use task_core::{
    Message, MessageRole, Milestone, MilestoneId, MilestoneStatus, Project, ProjectId, Task,
    TaskStore,
};

use crate::error::OpsError;

/// 案件を跨いで途中目標を引くときに見る案件の数の上限（案件はそう多くない）。
const PROJECT_SCAN: usize = 500;
/// レビューの対話を探すときに 1 案件あたりに見るタスクの上限。
const REVIEW_TASK_SCAN: usize = 1_000;
/// レビューの返事を探すときに読む対話の件数。
const REVIEW_MESSAGE_SCAN: usize = 200;

/// 途中目標とその案件を id から引く（`milestones` は案件ごとにしか引けないので順に見る）。
pub fn find(
    store: &dyn TaskStore,
    id: MilestoneId,
) -> Result<Option<(Project, Milestone)>, OpsError> {
    for project in store.project_list()?.into_iter().take(PROJECT_SCAN) {
        if let Some(found) = store
            .milestone_list(project.id)?
            .into_iter()
            .find(|m| m.id == id)
        {
            return Ok(Some((project, found)));
        }
    }
    Ok(None)
}

/// その案件の最新の `proposed` の途中目標（`seq` の大きい方。`exclude` は除く）。
pub fn latest_proposal(
    store: &dyn TaskStore,
    project_id: ProjectId,
    exclude: Option<MilestoneId>,
) -> Result<Option<Milestone>, OpsError> {
    let mut proposed: Vec<Milestone> = store
        .milestone_list(project_id)?
        .into_iter()
        // ADR-0074 D3.8（Phase F4b）: 案件計画の提案中の途中目標（`plan_key` あり）は旧い意味の
        // 「次の途中目標の提案」ではない。
        .filter(|m| {
            m.status == MilestoneStatus::Proposed && Some(m.id) != exclude && m.plan_key.is_none()
        })
        .collect();
    proposed.sort_by_key(|m| m.seq);
    Ok(proposed.pop())
}

// ---- D1: レビューの対話 run ----

/// その途中目標のレビューの対話の状態（ADR-0038 D1 / D4）。GUI のカード（`GET /projects/{id}`）と
/// 通知（`milestone_ready`）とレビュー run の重複判定が、同じ 1 つの読み取りを使う。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReviewState {
    /// 一番新しいレビューの対話タスク（まだ無ければ `None`。run 中なら `reply` が `None`）。
    pub task: Option<Task>,
    /// そのタスクに紐づく秘書の返事（`messages` の `role = node`）。
    pub reply: Option<Message>,
}

impl ReviewState {
    /// 人に知らせてよいか（返事が付いているか。ADR-0038 D4: 通知は返事の後）。
    pub fn has_reply(&self) -> bool {
        self.reply.is_some()
    }
}

/// レビューの対話とその返事を引く（決定的。無ければ空の `ReviewState`）。
pub fn review_state(
    store: &dyn TaskStore,
    project_id: ProjectId,
    milestone_id: MilestoneId,
) -> Result<ReviewState, task_core::StoreError> {
    let filter = task_core::ListFilter {
        project_id: Some(project_id),
        ..task_core::ListFilter::default()
    };
    let page = store.list_page(
        &filter,
        task_core::ListOrder::CreatedDesc,
        None,
        REVIEW_TASK_SCAN,
    )?;
    let mut reviews: Vec<Task> = page
        .items
        .into_iter()
        .filter(|t| task_core::milestone_review_of(t) == Some(milestone_id))
        .collect();
    reviews.sort_by_key(|t| (t.created_at, t.id));
    let Some(task) = reviews.pop() else {
        return Ok(ReviewState::default());
    };
    let reply = match task.assignee.as_deref() {
        Some(node_id) => store
            .message_list(node_id, Some(project_id), REVIEW_MESSAGE_SCAN)?
            .into_iter()
            .rfind(|m| m.role == MessageRole::Node && m.task_id == Some(task.id)),
        None => None,
    };
    Ok(ReviewState {
        task: Some(task),
        reply,
    })
}
