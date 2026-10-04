//! `approve` / `reject` / `answer` / `cancel` の判断と検証 — DESIGN.md §5.9 / ADR-0002 D4 /
//! ADR-0004 D1-D3 / ADR-0010 D3/D4（ADR-0013 D7）。
//!
//! 元は `celerisctl` の `commands/gate.rs` と `commands/cancel.rs` にあったロジックをそのまま移した。
//! 状態変更は `TaskStore::apply_transition` だけで行う。`expected` が `Some` で現在の `status` と
//! 違えば、遷移を試みずに `OpsError::Conflict` を返す。

use task_core::approval::Decision;
use task_core::{Event, Status, TaskId, TaskKind, TaskStore, Trigger};
use time::OffsetDateTime;

use crate::derive::latest_question;
use crate::error::OpsError;
use crate::view::{TaskRef, task_ref};

/// `events_since` を読むときの「大きめの limit」（`docs/api/v1/gui-api.md` §5.7）。伝播は同一トランザクション
/// 内で完結するので、通常はこの上限にかからない。
const CASCADE_EVENTS_LIMIT: usize = 100_000;

/// 状態変更の結果（承認・却下・回答・取り消し共通）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct TransitionResult {
    pub id: TaskId,
    pub from: Status,
    pub to: Status,
    pub reason: String,
    /// この遷移の伝播で `cancelled` になった、対象タスク以外のタスク（`docs/api/v1/gui-api.md` §5.7）。
    #[serde(default)]
    pub cascaded: Vec<TaskRef>,
}

/// `since_id`（遷移前の `latest_event_id()`）より後に追記されたイベントのうち、`subject` 以外の
/// タスクに付いた `Transitioned{to: Cancelled, reason: "cancel" | "dependency_failed" | "parent_cancelled"}` を
/// `TaskRef` にして返す（`docs/api/v1/gui-api.md` §5.7）。
fn collect_cascaded(
    store: &dyn TaskStore,
    subject: TaskId,
    since_id: u64,
) -> Result<Vec<TaskRef>, OpsError> {
    let rows = store.events_since(since_id, CASCADE_EVENTS_LIMIT)?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        if row.task_id == subject {
            continue;
        }
        let Event::Transitioned { to, reason, .. } = &row.event else {
            continue;
        };
        if *to != Status::Cancelled
            || (reason != "cancel" && reason != "dependency_failed" && reason != "parent_cancelled")
        {
            continue;
        }
        if !seen.insert(row.task_id) {
            continue;
        }
        if let Some(t) = store.get(row.task_id)? {
            out.push(task_ref(&t));
        }
    }
    Ok(out)
}

fn check_expected(actual: Status, expected: Option<Status>) -> Result<(), OpsError> {
    match expected {
        Some(exp) if exp != actual => Err(OpsError::Conflict {
            expected: exp,
            actual,
        }),
        _ => Ok(()),
    }
}

/// `status == Draft`（kind 不問）は `Trigger::Accept`、`kind == Approval && status == Ready` は
/// `Trigger::Approve` で `Event::ApprovalDecided` を同一トランザクションに追記する。
pub fn approve(
    store: &dyn TaskStore,
    id: TaskId,
    note: Option<String>,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    approve_as(store, id, "human", note, expected)
}

/// ADR-0056 Phase 101: `approve` と同じ判断・遷移だが、`Event::ApprovalDecided.by` を渡せる。
/// MCP の `task_approve` は `mcp:<client_id>` を渡す。`approve`（GUI/HTTP の `POST
/// /tasks/{id}/approve`）は `"human"` のまま（挙動を変えない）。
pub fn approve_as(
    store: &dyn TaskStore,
    id: TaskId,
    by: &str,
    note: Option<String>,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    check_expected(task.status, expected)?;

    let (trigger, extra_event) = if task.status == Status::Draft {
        (Trigger::Accept, None)
    } else if task.kind == TaskKind::Approval && task.status == Status::Ready {
        (
            Trigger::Approve,
            Some(Event::ApprovalDecided {
                by: by.to_string(),
                approved: true,
                note,
            }),
        )
    } else {
        return Err(OpsError::InvalidState {
            id,
            context: format!("kind={:?}, status={:?}", task.kind, task.status),
            action: "approved".to_string(),
        });
    };

    let from = task.status;
    let since_id = store.latest_event_id()?;
    let outcome = store.apply_transition(id, trigger, extra_event)?;
    let cascaded = collect_cascaded(store, id, since_id)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded,
    })
}

/// ADR-0070 D2 追記（Phase 116。本番で確認: `POST /tasks/{id}/retry` の `accept` を明示しないまま
/// 「やり直す」だけ押すと `draft` のまま止まり、`draft` を `ready` にする専用の道具が `approve`
/// （`Approval` タスクの承認と兼用で分かりにくい）しか無かった）: `status == Draft`（kind 不問）だけを
/// 許す、`accept` と同じ意味だが承認の記録は残さない専用の道具。`approve` は `Draft` でも
/// `Trigger::Accept` を選ぶので挙動はまったく同じ（こちらは名前で意図を明確にするための薄い別名）。
pub fn accept(
    store: &dyn TaskStore,
    id: TaskId,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    check_expected(task.status, expected)?;
    if task.status != Status::Draft {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", task.status),
            action: "accepted".to_string(),
        });
    }
    let from = task.status;
    let since_id = store.latest_event_id()?;
    let outcome = store.apply_transition(id, Trigger::Accept, None)?;
    let cascaded = collect_cascaded(store, id, since_id)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded,
    })
}

/// `kind == Approval && status == Ready` のみ許可する（`draft` への `reject` は
/// 成功させない。ADR-0004 D2: P-5 は不採用。draft の取り消しは `cancel` を使う）。
pub fn reject(
    store: &dyn TaskStore,
    id: TaskId,
    note: Option<String>,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    reject_as(store, id, "human", note, expected)
}

/// ADR-0056 Phase 101: `reject` と同じ判断・遷移だが、`Event::ApprovalDecided.by` を渡せる。
/// MCP の `task_reject` は `mcp:<client_id>` を渡す。`reject`（GUI/HTTP の `POST
/// /tasks/{id}/reject`）は `"human"` のまま（挙動を変えない）。
pub fn reject_as(
    store: &dyn TaskStore,
    id: TaskId,
    by: &str,
    note: Option<String>,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    check_expected(task.status, expected)?;

    if task.kind == TaskKind::Approval && task.status == Status::Ready {
        let from = task.status;
        let since_id = store.latest_event_id()?;
        let outcome = store.apply_transition(
            id,
            Trigger::Reject,
            Some(Event::ApprovalDecided {
                by: by.to_string(),
                approved: false,
                note,
            }),
        )?;
        let cascaded = collect_cascaded(store, id, since_id)?;
        Ok(TransitionResult {
            id,
            from,
            to: outcome.next,
            reason: outcome.reason.to_string(),
            cascaded,
        })
    } else {
        Err(OpsError::InvalidState {
            id,
            context: format!("kind={:?}, status={:?}", task.kind, task.status),
            action: "rejected".to_string(),
        })
    }
}

/// `Blocked` タスクのみ `Trigger::Answer` を適用し、回答テキストを
/// `Event::Answered{question, answer}` として同一トランザクションで永続化する（ADR-0010 D3, P-10）。
/// `question` は直近の `WorkerFinished{outcome:"question: ..."}` から取る（無ければ空文字列）。
pub fn answer(
    store: &dyn TaskStore,
    id: TaskId,
    answer: String,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    check_expected(task.status, expected)?;

    if task.status != Status::Blocked {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", task.status),
            action: "answered; only blocked tasks accept an answer".to_string(),
        });
    }

    let events = store.events_for(id)?;
    // ADR-0074 D2.2（Phase F3 途中確認）: 工程の後の途中確認（`awaiting_human`）は質問ではない。
    // 「続ける」と「質問への回答」を混ぜない（`POST /tasks/{id}/execution/phase-gate` を使う）。
    if crate::phase_gate::is_awaiting_human(&task, &events) {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}, reason=awaiting_human", task.status),
            action: "answered; a phase checkpoint is resumed via execution/phase-gate".to_string(),
        });
    }
    // ADR-0079 D8（Phase R3b）: root の計画の承認待ちも質問ではない（`execution/plan-gate` を使う）。
    if crate::plan_gate::is_awaiting_plan_approval(&task, &events) {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}, reason=awaiting_plan_approval", task.status),
            action: "answered; a plan approval is resumed via execution/plan-gate".to_string(),
        });
    }
    let question = latest_question(&events);
    let from = task.status;
    // ADR parallel integration D4 付記: 段の統合依頼で blocked(question) になった統合 WU を、受信箱の
    // 専用経路でなく汎用の回答で再開したときも、その依頼は回答済み（受信箱から消す）。遷移が WU の
    // blocked を解く前に対象を決める。再開後の統合が衝突なしで通っても dispatcher は閉じた依頼に重ねない。
    close_phase_integration_requests(store, id, &answer)?;
    let since_id = store.latest_event_id()?;
    let outcome = store.apply_transition(
        id,
        Trigger::Answer,
        Some(Event::Answered {
            question,
            answer: answer.clone(),
        }),
    )?;
    let cascaded = collect_cascaded(store, id, since_id)?;
    // GUI 監査 H2: `POST /tasks/{id}/answer` で答えたときも、そのタスクの未決の `approvals` を
    // `once` + 同じ答えで決定済みにする（決定的。無ければ何もしない）。`POST /approvals/{id}/decide`
    // は既にこの経路（`gate::answer`）に相乗りしているので、これで両方向が揃う（`task_ops::approval`）。
    settle_pending_approvals(store, id, &answer)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded,
    })
}

/// 汎用の回答で再開される統合 WU（`blocked(question)` の `Integrate`）が出した未回答の統合依頼を
/// `answered` で閉じる。依頼の無い task では何もしない。
fn close_phase_integration_requests(
    store: &dyn TaskStore,
    task_id: TaskId,
    answer: &str,
) -> Result<(), OpsError> {
    let blocked_integrations: Vec<String> = store
        .work_units_for(task_id)?
        .into_iter()
        .filter(|unit| {
            unit.kind == task_core::WorkUnitKind::Integrate
                && unit.blocked_reason == Some(task_core::WorkUnitBlockedReason::Question)
        })
        .map(|unit| format!("phase:{}", unit.key))
        .collect();
    for origin in blocked_integrations {
        store.integration_requests_close(task_id, &origin, "answered", Some(answer))?;
    }
    Ok(())
}

/// `task_id` に紐づく未決の `approvals` を、渡された `answer` で `once` に決定する。
/// `task_ops::approval::decide` は呼ばない（そちらは決定のたびに `gate::answer` を呼び直すため、
/// ここから呼ぶと循環する。ストアへの書き込みだけをここで完結させる）。
pub(crate) fn settle_pending_approvals(
    store: &dyn TaskStore,
    task_id: TaskId,
    answer: &str,
) -> Result<(), OpsError> {
    let now = OffsetDateTime::now_utc();
    let pending = store.approval_list(Some(true), None, None)?;
    for approval in pending.into_iter().filter(|a| a.task_id == Some(task_id)) {
        store.approval_decide(approval.id, Decision::Once, Some(answer.to_string()), now)?;
    }
    Ok(())
}

/// 非終端と `failed` のタスクを `Trigger::Cancel` で `cancelled` にする。
/// `done` / `cancelled` はエラーにする（ADR-0131 D7）。子・後続への
/// 取り消し伝播は `TaskStore::apply_transition` がストア側の同一トランザクションで行う。
pub fn cancel(
    store: &dyn TaskStore,
    id: TaskId,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    check_expected(task.status, expected)?;

    if matches!(task.status, Status::Done | Status::Cancelled) {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", task.status),
            action: "cancelled".to_string(),
        });
    }

    let from = task.status;
    let since_id = store.latest_event_id()?;
    let outcome = store.apply_transition(id, Trigger::Cancel, None)?;
    let cascaded = collect_cascaded(store, id, since_id)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded,
    })
}

#[cfg(test)]
mod tests;
