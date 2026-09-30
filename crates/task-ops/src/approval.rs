//! 認可の決定（ADR-0033 D5。Phase 26）。SPEC §3.6「あなたはそれに対して『今回だけ』か『同じようなことは
//! 今後ずっと』のどちらかの認可を出す。永続の認可は文字で記録してエージェントに注入する」。
//!
//! 人が `approvals` の 1 件に `once` / `standing` / `denied` のどれで答えても、**タスクの再開は既存の
//! 「質問に答える」経路（`gate::answer`）に相乗りする**（二重実装しない）。ここで足すのは:
//! - `denied` は「認めない: <answer>」を答えにする（ワーカーが自分で判断できるように）。
//! - `standing` は上と同じ再開に加えて `standing_rules` に 1 行足す。
//!
//! `Approval` の取得と「無い id は 404」の判定は呼び出し側（`task-api`）が行う（`report.rs` / `reports.rs`
//! と同じ役割分担）。ここは判断と、決まった後の書き込みだけ。

use task_core::approval::{Approval, ApprovalId, Decision, StandingRule, StandingRuleId};
use task_core::{Status, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::error::OpsError;
use crate::gate::{self, TransitionResult};

/// `standing` のときの適用範囲（SPEC §3.6 の「今後ずっと」を誰に適用するか）。要求本文の `scope` に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// そのノードだけ（既定）。
    #[default]
    Node,
    /// 全員（`standing_rules.node_id = NULL`）。
    All,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "node" => Some(Scope::Node),
            "all" => Some(Scope::All),
            _ => None,
        }
    }
}

/// `decide` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct DecideOutcome {
    pub approval: Approval,
    /// `decision == Standing` のときだけ `Some`。
    pub standing_rule: Option<StandingRule>,
    /// `approval.task_id` があるときだけ `Some`（`gate::answer` がタスクを再開した結果）。
    pub transition: Option<TransitionResult>,
    /// Phase F7: 認可元のタスクが既に終端（または無い）で、決定だけ記録してタスクには答えなかったとき
    /// の説明（例: `task … is already cancelled; decision recorded without resuming the task`）。
    pub note: Option<String>,
}

/// 人が答える（ADR-0033 D5）。`approval` は呼び出し側が `approval_get` で引いた既存の行
/// （404 の判定は呼び出し側の責任）。
pub fn decide(
    store: &dyn TaskStore,
    approval: Approval,
    decision: Decision,
    answer: String,
    scope: Scope,
    now: OffsetDateTime,
) -> Result<DecideOutcome, OpsError> {
    if answer.trim().is_empty() {
        return Err(OpsError::Validation("answer must not be blank".to_string()));
    }
    if decision == Decision::Withdrawn {
        return Err(OpsError::Validation(
            "decision `withdrawn` is set by celeris only (use once / standing / denied)"
                .to_string(),
        ));
    }

    // Phase F7: **書く前に**タスクへ答えを渡せるかを決める（以前は決定を書いてから `gate::answer` が
    // 409 を返し、決定だけが残る半端な状態になった）。
    // - タスクが無い / 終端: 答える相手がいない。決定だけ記録して 200（`note` で知らせる）。
    // - `blocked`（途中確認〈awaiting_human〉ではない）: 従来どおり答えてタスクを再開する。
    // - それ以外（ready / running / reviewing / draft、途中確認）: 409。何も書かない。
    let target = match approval.task_id {
        None => Target::Nothing,
        Some(task_id) => match store.get(task_id)? {
            None => Target::Closed {
                note: format!("task {task_id} no longer exists; decision recorded only"),
            },
            Some(task) if task.status.is_terminal() => Target::Closed {
                note: format!(
                    "task {task_id} is already {}; decision recorded without resuming the task",
                    status_word(task.status)
                ),
            },
            Some(task) if task.status == Status::Blocked => {
                let events = store.events_for(task_id)?;
                if crate::phase_gate::is_awaiting_human(&task, &events) {
                    return Err(OpsError::InvalidState {
                        id: task_id,
                        context: format!("status={:?}, reason=awaiting_human", task.status),
                        action: "answered; a phase checkpoint is resumed via execution/phase-gate"
                            .to_string(),
                    });
                }
                if crate::plan_gate::is_awaiting_plan_approval(&task, &events) {
                    return Err(OpsError::InvalidState {
                        id: task_id,
                        context: format!("status={:?}, reason=awaiting_plan_approval", task.status),
                        action: "answered; a plan approval is resumed via execution/plan-gate"
                            .to_string(),
                    });
                }
                Target::Answer(task_id)
            }
            Some(task) => {
                return Err(OpsError::InvalidState {
                    id: task_id,
                    context: format!("status={:?}", task.status),
                    action: "answered; only blocked tasks accept an answer".to_string(),
                });
            }
        },
    };

    let id: ApprovalId = approval.id;
    let decided = store
        .approval_decide(id, decision, Some(answer.clone()), now)?
        .unwrap_or(approval);

    // ADR-0033 D5: `denied` は「認めない: …」を答えにして、ワーカーが自分で判断できるようにする。
    // `once` / `standing` はそのまま答えを渡す。
    let effective_answer = match decision {
        Decision::Denied => format!("認めない: {answer}"),
        Decision::Once | Decision::Standing | Decision::Withdrawn => answer.clone(),
    };

    // 既存の「質問に答える」経路にそのまま乗せる（`task_id` が無い approval は再開するタスクが無い）。
    let (transition, note) = match target {
        Target::Answer(task_id) => (
            Some(gate::answer(store, task_id, effective_answer, None)?),
            None,
        ),
        Target::Closed { note } => (None, Some(note)),
        Target::Nothing => (None, None),
    };

    let standing_rule = if decision == Decision::Standing {
        // Phase 27（監査 H-1）: 部をまたぐ委譲の質問だけは、答えの文ではなく**質問の鍵**を規則にする
        // （`delegate` の照合が前方一致でできるように）。それ以外の質問は従来どおり答えの文。
        let rule = crate::conversation::cross_department_key(&decided.question)
            .unwrap_or_else(|| answer.clone());
        let rule = StandingRule {
            id: StandingRuleId::new(),
            node_id: match scope {
                Scope::Node => Some(decided.node_id.clone()),
                Scope::All => None,
            },
            rule,
            created_at: now,
        };
        store.standing_rule_append(&rule)?;
        Some(rule)
    } else {
        None
    };

    Ok(DecideOutcome {
        approval: decided,
        standing_rule,
        transition,
        note,
    })
}

/// `decide` が決定を書いた後にすること（Phase F7）。
enum Target {
    /// 紐づくタスクが無い approval。決定だけ。
    Nothing,
    /// 紐づくタスクが無くなった・終端。決定だけ記録し、`note` を返す。
    Closed { note: String },
    /// `blocked` のタスクに答えて再開する。
    Answer(TaskId),
}

fn status_word(status: Status) -> &'static str {
    match status {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Blocked => "blocked",
        Status::Reviewing => "reviewing",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests;
