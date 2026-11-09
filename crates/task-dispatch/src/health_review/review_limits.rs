//! ADR 2026-11-08-health-enforcement-loop D3.4・D4.3・D4.4: task の review（`Dispatcher::try_review_repair`）で health の
//! 不合格を止める。repair にも `ReviewFail`（attempts+1 の再実行、compound の最終レビューの後の replan、retry 切れの
//! Failed → CoS の review_rework）にもせず、`Trigger::ReviewBlocked`（`Reviewing → Blocked`、attempts 不変）で止めて
//! 受信箱に質問を 1 件出す（`QuestionRaised` と承認待ちの行。`escalate_failed_children` と同じ形）。人は既存の
//! comment・answer（`Trigger::Answer`）で再開する。
//!
//! - [`stop_on_error`]（D3.4）: health の Error・Unavailable（`health_check_error: <理由>`）。task の種類を問わず、他の
//!   不合格より先に見る（測れない health は合格にも修復にもしない。J4）。
//! - [`stop_on_repeat`]（D4.3）: 不合格がすべて health の Fail で、[`limits::judge`]（範囲 review）が往復と判定した。
//!   木の root に `HealthFindingsRepeated` を追記する（root の受信箱の `health_repeat` 1 件。escalation の
//!   `task_ops::inbox::health_repeat::record_findings_repeated`）。
//! - [`stop_on_cap`]（D4.4）: 不合格がすべて health の Fail で、`max_repairs`・`max_repairs_per_class`
//!   （`health:<rule_id>`）を使い切った。health でない class は `None`（呼び出し側が今どおり `ReviewFail` へ進む）。
//!
//! この file は `dispatcher::review_verdict` の子 module として `#[path]` で読む（`check_item.rs` と同じ。dispatcher の
//! size の制約と、`dispatcher` → `health_review` の依存を足さないため）。LLM は呼ばない。

use super::super::*;
use crate::health_review::limits::{self, HealthScope, RepeatVerdict};

/// 止めたときは `Some`（`ReviewBlocked` の遷移の結果）、止めないときは `None`。
type Stopped = Result<Option<task_core::Outcome>, DispatchError>;

/// D3.4: acceptance の外の不合格に health の Error・Unavailable があれば、その理由を `; ` でつないだ質問で止める。
pub(super) fn stop_on_error(
    store: &dyn TaskStore,
    task: &Task,
    verdicts: &[Verdict],
    events: &mut Vec<Event>,
) -> Stopped {
    let errors: Vec<&str> = (verdicts.iter())
        .filter(|v| v.criterion_idx >= task.acceptance.len())
        .filter_map(Verdict::health_check_error)
        .collect();
    if errors.is_empty() {
        return Ok(None);
    }
    block(store, task, errors.join("; "), events).map(Some)
}

/// D4.3: 今の review の health の Fail が修復の前と同じ指摘の往復なら止め、木の root に 1 件だけ上げる。
pub(super) fn stop_on_repeat(
    store: &dyn TaskStore,
    task: &Task,
    events: &mut Vec<Event>,
) -> Stopped {
    let RepeatVerdict::Repeat { fingerprints, .. } =
        limits::judge(&store.events_for(task.id)?, HealthScope::Review)
    else {
        return Ok(None);
    };
    let question = format!(
        "health_findings_repeated: review の修復の後も同じ health の指摘が残った（{}）。repair をもう起こさず、木の root の受信箱に 1 件にまとめた。直し方を comment で書くと再開する。",
        fingerprints.join(", ")
    );
    let outcome = block(store, task, question, events)?;
    task_ops::inbox::health_repeat::record_findings_repeated(
        store,
        task,
        task_core::HealthRepeatSite::Review,
        &fingerprints,
    )
    .map_err(ops_to_store)?;
    Ok(Some(outcome))
}

/// D4.4: 修復の上限を使い切った。`health_bucket` は health の不合格の bucket（`health:<rule_id>` を `,` で並べたもの。
/// health でなければ `None` で止めない）。`used` は（修復の数, `max_repairs`, bucket の修復の数, `max_repairs_per_class`）。
pub(super) fn stop_on_cap(
    store: &dyn TaskStore,
    task: &Task,
    health_bucket: Option<&str>,
    used: (usize, u32, usize, u32),
    failing: &[String],
    events: &mut Vec<Event>,
) -> Stopped {
    let Some(bucket) = health_bucket else {
        return Ok(None);
    };
    let (repairs, max_repairs, same_class, per_class) = used;
    let question = format!(
        "health_repair_cap: health の修復が上限に達した（bucket {bucket}: {same_class} 回 / max_repairs_per_class {per_class}、修復の合計 {repairs} 回 / max_repairs {max_repairs}）。repair・再実行・replan をもう起こさない。直し方を comment で書くと再開する。指摘: {}",
        failing.join("; ")
    );
    block(store, task, question, events).map(Some)
}

/// review を `Trigger::ReviewBlocked` で止め、受信箱に `question` を 1 件出す。`events`（review の判定の記録）は同じ
/// transaction に入れる。質問のきっかけの run は判定した run（`ReviewVerdict.run_id`）。
fn block(
    store: &dyn TaskStore,
    task: &Task,
    question: String,
    events: &mut Vec<Event>,
) -> Result<task_core::Outcome, DispatchError> {
    let run_id = (events.iter())
        .find_map(|e| match e {
            Event::ReviewVerdict { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let now = OffsetDateTime::now_utc();
    if let Err(e) = crate::approvals::record_question_approval(store, task, &question, now) {
        tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the blocked review");
    }
    let mut all = std::mem::take(events);
    all.push(Event::QuestionRaised {
        run_id,
        text: question,
    });
    Ok(store.apply_transition_with_events(task.id, Trigger::ReviewBlocked, all)?)
}
