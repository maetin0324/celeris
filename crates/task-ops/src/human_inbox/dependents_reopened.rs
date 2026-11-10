//! ADR 2026-11-08-health-enforcement-loop D7.3・決定 p9-scope: 受信箱の `dependents_reopened` の項目。
//!
//! 依存先 A（`task`）ごとに 1 件。本文に、戻した後続のうちまだ動いていないものの task id と依存先、戻さなかった後続
//! （`kept`）の理由を書く。選択肢は戻した後続ごとの cancel（key `cancel:<後続の task id>`）で、答えは
//! `POST /tasks/{id}/cancel` に写す（[`cancel_target`]。項目の戻した後続に無い id は写さない）。
use task_core::{KeptDependent, KeptReason, TaskId};

use super::{
    AttentionItem, Builder, Draft, InboxBlocking, InboxItem, InboxKind, clip, link, native, opt,
};

/// 戻した後続 1 件を cancel する選択肢の key の頭。続きは後続の task id。
pub const CANCEL_OPTION_PREFIX: &str = "cancel:";

/// `dependents_reopened` の項目 `item` で選択肢 `option` が cancel する後続。項目がまだ動いていないとして挙げた
/// 戻した後続（`blocking.tasks`）に無い id・ほかの種類の項目は `None`。
pub fn cancel_target(item: &InboxItem, option: &str) -> Option<TaskId> {
    if item.kind != InboxKind::DependentsReopened {
        return None;
    }
    let id: TaskId = option.strip_prefix(CANCEL_OPTION_PREFIX)?.parse().ok()?;
    item.blocking.tasks.iter().any(|t| t.id == id).then_some(id)
}

fn ids(list: &[TaskId]) -> String {
    list.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("・")
}

fn kept_line(k: &KeptDependent) -> String {
    match &k.why {
        KeptReason::Retried { successor } => {
            format!(
                "{}（retry の後継 {successor} があるので戻さない）",
                k.task_id
            )
        }
        KeptReason::OtherDependency { unresolved } => format!(
            "{}（ほかの依存先 {} が failed・cancelled のまま。それが戻ると戻る）",
            k.task_id,
            ids(unresolved)
        ),
    }
}

impl Builder<'_> {
    pub(super) fn dependents_reopened(&self, a: &AttentionItem) -> Option<InboxItem> {
        let AttentionItem::DependentsReopened {
            task,
            pending,
            kept,
            seq,
            at,
        } = a
        else {
            return None;
        };
        let listed: Vec<String> = pending
            .iter()
            .map(|p| {
                format!(
                    "{}（{}、依存先 {}）",
                    p.task.title,
                    p.task.id,
                    ids(&p.depends_on)
                )
            })
            .collect();
        let mut detail = format!(
            "依存先 {}（{}）が終端から戻ったので、その失敗で cancel されていた後続を戻した。まだ動いていない後続: {}。後継を手で起票していて仕事が重なる後続は cancel する。戻した後続がすべて動き出すか終端になると、この項目は消える",
            task.title,
            task.id,
            listed.join("、")
        );
        if !kept.is_empty() {
            let kept: Vec<String> = kept.iter().map(kept_line).collect();
            detail.push_str(&format!("。戻さなかった後続: {}", kept.join("、")));
        }
        let options = pending
            .iter()
            .map(|p| {
                opt(
                    &format!("{CANCEL_OPTION_PREFIX}{}", p.task.id),
                    &format!("{} を取り消す", p.task.title),
                    false,
                    &format!("POST /tasks/{}/cancel（重なった後続を取り消す）", p.task.id),
                )
            })
            .collect();
        let mut links: Vec<_> = pending
            .iter()
            .map(|p| link("戻した後続", format!("/tasks/{}", p.task.id)))
            .collect();
        links.push(link("依存先の task", format!("/tasks/{}", task.id)));
        let single = (pending.len() == 1).then(|| pending[0].task.id);
        Some(self.finish(Draft {
            id: format!("dependents_reopened-{}-{seq}", task.id),
            kind: InboxKind::DependentsReopened,
            title: format!("依存先の復帰で後続を戻した: {}", task.title),
            detail: clip(&detail),
            options,
            recommended: None,
            due_at: None,
            blocking: InboxBlocking {
                tasks: pending.iter().map(|p| p.task.clone()).collect(),
                units: Vec::new(),
                root: self.root_of(task.id).filter(|r| r.id != task.id),
                summary: format!("戻した後続 {} 件", pending.len()),
            },
            blocked_by: Vec::new(),
            native: single.and_then(|id| native("POST", format!("/api/v1/tasks/{id}/cancel"))),
            project_id: self.project_of(task.id),
            task: Some(task.clone()),
            created_at: at.clone(),
            links,
        }))
    }
}
