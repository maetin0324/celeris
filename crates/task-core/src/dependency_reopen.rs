//! ADR 2026-11-08-health-enforcement-loop D7（P-9・決定 p9-scope）: 依存先が終端から戻ったら（reopen・rereview・
//! 後続の復帰）、その失敗で `dependency_failed` の cancel になった後続を戻す。呼ぶのは依存の cascade
//! （`SqliteStore::cascade_after_transition_tx`）だけ。判定の述語は retry の付け替え（`SqliteStore::retry_task_tx`）と
//! 共有する（D7.2）。approval・browser wait・cluster job の `*_tx` と同じく、遷移の transaction の中で呼ばれる。

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Event, Status, Task, TaskId};
use crate::store::{SqliteStore, StoreError};
use crate::transition::Trigger;

/// `task_id` の最後の `Event::Transitioned` の `(from, reason)`。無ければ `None`。
pub(crate) fn last_transitioned_tx(
    tx: &Connection,
    task_id: TaskId,
) -> Result<Option<(Status, String)>, StoreError> {
    let json: Option<String> = tx
        .query_row(
            "SELECT json FROM events WHERE task_id = ?1 AND json LIKE '%\"type\":\"transitioned\"%' ORDER BY seq DESC LIMIT 1",
            params![task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(json) = json else {
        return Ok(None);
    };
    match serde_json::from_str(&json)? {
        Event::Transitioned { from, reason, .. } => Ok(Some((from, reason))),
        _ => Ok(None),
    }
}

/// `task` が依存先の失敗で cancel されたままか（`cancelled` で、最後の遷移が `dependency_failed`）。そうなら cancel の
/// 前の状態を返す。`cancelled` から出る遷移は retry の付け替えと `DependencyReopened` だけで、どちらも最後の遷移の
/// 理由を変えるので、「cancel の後に人・CoS の遷移が無い」もこの判定に含まれる。人・CoS の cancel（`cancel`）と連鎖の
/// 中止（`parent_cancelled`・`project_cancelled`・`milestone_cancelled`）はここで外れる。
pub(crate) fn cancelled_by_dependency_from_tx(
    tx: &Connection,
    task: &Task,
) -> Result<Option<Status>, StoreError> {
    if task.status != Status::Cancelled {
        return Ok(None);
    }
    Ok(last_transitioned_tx(tx, task.id)?
        .filter(|(_, reason)| reason == Trigger::DependencyFailed.name())
        .map(|(from, _)| from))
}

/// `from → to` が終端から終端でない状態への遷移なら、`reopened` の後続のうち `dependency_failed` で cancel されたままで、
/// ほかの依存先に failed・cancelled（または見つからない task）が無いものを `Trigger::DependencyReopened` で戻す（cancel の
/// 前が draft なら draft、それ以外は ready）。戻すのは新しい `Transitioned` の追記だけで、cancel の event は変えない。
/// 戻した後続の遷移がまた cascade を通るので、その後続も同じ条件で推移的に戻る。ほかの依存先がまだ failed の後続は、
/// その依存先が戻ったときに戻る。それ以外の遷移では何もしない。
pub(crate) fn restore_dependents_tx(
    tx: &Connection,
    reopened: TaskId,
    from: Status,
    to: Status,
) -> Result<(), StoreError> {
    if !from.is_terminal() || to.is_terminal() {
        return Ok(());
    }
    for dependent in SqliteStore::dependents_of_tx(tx, reopened)? {
        let Some(task) = SqliteStore::get_locked(tx, dependent)? else {
            continue;
        };
        let Some(before) = cancelled_by_dependency_from_tx(tx, &task)? else {
            continue;
        };
        if has_unsuccessful_dependency_tx(tx, &task, reopened)? {
            continue;
        }
        let draft = before == Status::Draft;
        SqliteStore::apply_transition_tx(
            tx,
            dependent,
            Trigger::DependencyReopened { draft },
            vec![],
        )?;
    }
    Ok(())
}

/// `task` の依存先のうち `except` 以外に failed・cancelled（または見つからない）ものがあるか。
fn has_unsuccessful_dependency_tx(
    tx: &Connection,
    task: &Task,
    except: TaskId,
) -> Result<bool, StoreError> {
    for dep in task.depends_on.iter().filter(|d| **d != except) {
        match SqliteStore::task_status_tx(tx, *dep)? {
            Some(Status::Failed | Status::Cancelled) | None => return Ok(true),
            Some(_) => {}
        }
    }
    Ok(false)
}
