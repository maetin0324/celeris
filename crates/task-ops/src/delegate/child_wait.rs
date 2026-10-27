//! ADR 2026-10-18-parent-waits-for-children 付記（2026-10-27）: 親が待つ子の判定。
//!
//! 終端でない子に加え、`failed` の子のうち「再実行を待て」の印があるもの（未解決の escalated 項目、
//! または子の最後の失敗より後の人・CoS・operator の回答・comment）を待つ。印の無い `failed` の子は
//! 従来どおり終端として扱い、親を起こす。読むのはストアだけ。LLM は呼ばない（DESIGN 原則 1）。

use task_core::comment::CommentAuthorKind;
use task_core::{Event, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;

/// 「待て」と読む語（小文字にした本文に含まれるか）。決定的な文字列照合で、意味の解釈はしない。
const WAIT_PHRASES: &[&str] = &[
    "再実行を待",
    "再実行まで待",
    "再実行待ち",
    "やり直しを待",
    "やり直しまで待",
    "再開を待",
    "完了を待",
    "終わるまで待",
    "直るまで待",
    "修正を待",
    "wait for rerun",
    "wait for the rerun",
    "wait for re-run",
    "wait for the re-run",
    "wait for retry",
    "wait for the retry",
    "wait for the child",
    "wait until the child",
];

/// 待たないと読む語。どれかを含めば「待て」とは読まない（「完了を待たずに進めて」など）。
const NO_WAIT_PHRASES: &[&str] = &[
    "待たず",
    "待たない",
    "待たなくて",
    "don't wait",
    "do not wait",
    "without waiting",
    "no need to wait",
];

/// 本文が「再実行を待て」の指示か。
pub fn is_wait_instruction(text: &str) -> bool {
    let lower = text.to_lowercase();
    WAIT_PHRASES.iter().any(|p| lower.contains(p))
        && !NO_WAIT_PHRASES.iter().any(|p| lower.contains(p))
}

/// 親が待つ子（id 順）。
///
/// - 終端でない子: `children_wait_started` が真（呼び出し側が子待ちに入った・入れる）か、親の最後の
///   run の開始より後に「待て」の指示がある間。
/// - `failed` の子: `escalated(child)` が真（未解決の escalated 項目）か、子の最後の失敗より後に
///   「待て」の指示がある間。子が reopen・retry で再び動けば上の規則に移り、終われば外れる。
///
/// 指示は、親への回答（`Event::Answered`）と、親・その子への人のコメント・worker の run に依らない
/// node のコメント（CoS・operator・lead）。
pub fn awaited_children(
    store: &dyn TaskStore,
    parent: TaskId,
    children_wait_started: bool,
    escalated: &mut dyn FnMut(TaskId) -> Result<bool, OpsError>,
) -> Result<Vec<TaskId>, OpsError> {
    let children: Vec<Task> = store
        .children(parent)?
        .into_iter()
        .filter(|c| !c.status.is_terminal() || c.status == Status::Failed)
        .collect();
    if children.is_empty() {
        return Ok(Vec::new());
    }
    let parent_instructions = instruction_times(store, parent, true)?;
    let mut parent_last_run: Option<Option<OffsetDateTime>> = None;
    let mut out = Vec::new();
    for child in children {
        let held = if child.status == Status::Failed {
            if escalated(child.id)? {
                true
            } else {
                match last_failed_at(store, child.id)? {
                    Some(failed_at) => {
                        parent_instructions.iter().any(|t| *t >= failed_at)
                            || instruction_times(store, child.id, false)?
                                .iter()
                                .any(|t| *t >= failed_at)
                    }
                    None => false,
                }
            }
        } else if children_wait_started {
            true
        } else {
            let since = match parent_last_run {
                Some(v) => v,
                None => {
                    let v = last_run_started_at(store, parent)?;
                    parent_last_run = Some(v);
                    v
                }
            };
            let after = |t: &OffsetDateTime| since.is_none_or(|s| *t >= s);
            parent_instructions.iter().any(after)
                || instruction_times(store, child.id, false)?.iter().any(after)
        };
        if held {
            out.push(child.id);
        }
    }
    out.sort();
    Ok(out)
}

fn parse_ts(ts: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(ts, &Rfc3339).ok()
}

/// 「待て」の指示の時刻。`answers` なら親への回答も含める。
fn instruction_times(
    store: &dyn TaskStore,
    task: TaskId,
    answers: bool,
) -> Result<Vec<OffsetDateTime>, OpsError> {
    let mut out: Vec<OffsetDateTime> = store
        .comments_for(task)?
        .into_iter()
        .filter(|c| match c.author_kind {
            CommentAuthorKind::Human => true,
            CommentAuthorKind::Node => c.run_id.is_none(),
            CommentAuthorKind::System => false,
        })
        .filter(|c| is_wait_instruction(&c.body))
        .map(|c| c.created_at)
        .collect();
    if answers {
        out.extend(
            store
                .event_rows_for(task, None, usize::MAX)?
                .into_iter()
                .filter(|r| matches!(&r.event, Event::Answered { answer, .. } if is_wait_instruction(answer)))
                .filter_map(|r| parse_ts(&r.ts)),
        );
    }
    Ok(out)
}

/// 子が最後に `failed` へ遷移した時刻。
fn last_failed_at(store: &dyn TaskStore, task: TaskId) -> Result<Option<OffsetDateTime>, OpsError> {
    Ok(store
        .event_rows_for(task, None, usize::MAX)?
        .into_iter()
        .rev()
        .find(|r| {
            matches!(
                &r.event,
                Event::Transitioned {
                    to: Status::Failed,
                    ..
                }
            )
        })
        .and_then(|r| parse_ts(&r.ts)))
}

/// 親の最後のワーカー run の開始時刻（reviewer の run は数えない）。
fn last_run_started_at(
    store: &dyn TaskStore,
    task: TaskId,
) -> Result<Option<OffsetDateTime>, OpsError> {
    Ok(store
        .event_rows_for(task, None, usize::MAX)?
        .into_iter()
        .rev()
        .find(|r| matches!(&r.event, Event::WorkerStarted { role: None, .. }))
        .and_then(|r| parse_ts(&r.ts)))
}
