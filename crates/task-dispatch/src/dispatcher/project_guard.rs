//! ADR 2026-11-04-project-controller A6: 判定（[`crate::project_controller::ProjectDecider`]）の前に Rust が
//! 掛ける guard。LLM は呼ばない（I3）。guard が効いたら decider（TS の isolate）を呼ばずに、判断の記録を
//! 同じ tx1（CAS）で書く。効果は `OpenEscalation`（問いの key で 1 回だけ。人が mode を置き直すまで `blocked`）
//! か効果なし。
//!
//! 順（最初に当たったものだけ）:
//! 1. `achieved` の静けさ: phase `achieved` の案件は人の変更（`human_changed`）の wake でだけ判定する。
//!    それ以外の wake は効果なしの判断で畳む（起票 0・CoS 入力 0）。
//! 2. 同じ失敗の繰り返し: root の失敗の wake の理由の署名が、最後の人の変更の後に
//!    `max_same_failure_repeats` 回に達したら止めて人に聞く。
//! 3. TS の品質: 直前の判断が `max_invalid_decisions` 回続けて検証に落ちた（`pc/invalid:*`）ら止める。
//! 4. 予算: 今日の CoS 入力・起票・token が上限に達したら止める（`task_ops::project_budget`）。`Daily` の超過は
//!    次の UTC の 00:00 を期限にし（`next_wake_at`）、期限の後の再開は `project_resume` が判定する。
//! 5. TTL: phase `waiting` で外からの入力（cron 以外の wake）が `ttl_secs` 来ないまま timer か cron で
//!    起きたら止める（外部の応答の欠落）。
//!
//! 停滞（進捗なしの判断の連続）は TS の判定の後の `runaway_guard`（`project_decider_ts`）が持つ。

use serde_json::json;
use task_core::project_wake::{
    ControllerPhase, ProjectControllerState, ProjectWake, ProjectWakeKind,
};
use task_core::store::SqliteStore;
use task_core::{Event, Status, TaskStore};
use task_ops::project_budget::{self as budget, BudgetLimits};
use time::OffsetDateTime;

use crate::project_controller::{ProjectDecision, ProjectEffect};

/// guard の判断の記録（`proposal_json`）の schema。
pub(crate) const GUARD_RECORD_SCHEMA: &str = "celeris.project-guard/1";

/// guard の結果。
pub(crate) enum PreGuard {
    /// 判定（decider）を呼ぶ。
    Decide,
    /// decider を呼ばずにこの判断を書く。`record` は判断の記録。
    Guarded {
        decision: ProjectDecision,
        record: serde_json::Value,
    },
}

fn guarded(
    name: &str,
    phase: ControllerPhase,
    effect: ProjectEffect,
    detail: serde_json::Value,
) -> PreGuard {
    PreGuard::Guarded {
        decision: ProjectDecision {
            effect,
            next_wake_after_secs: None,
            phase,
            rule_id: format!("pc/guard/{name}"),
        },
        record: json!({ "schema": GUARD_RECORD_SCHEMA, "guard": name, "detail": detail }),
    }
}

/// 止めて人に聞く（phase は tx1 で `waiting`、効果の tx で `blocked`。付記 A1 の 10 の形）。
fn halt(name: &str, question_key: String, summary: String, detail: serde_json::Value) -> PreGuard {
    guarded(
        name,
        ControllerPhase::Waiting,
        ProjectEffect::OpenEscalation {
            question_key,
            category: format!("guard_{name}"),
            summary,
        },
        detail,
    )
}

/// root の失敗の wake の（種類, 理由）。失敗でない・event が読めなければ `None`。
fn failure_of(store: &SqliteStore, w: &ProjectWake) -> Option<(String, String)> {
    if !matches!(
        w.kind,
        ProjectWakeKind::RootFailed | ProjectWakeKind::ReviewFailed
    ) {
        return None;
    }
    let id = u64::try_from(w.source_event_id?).ok()?;
    let rows = store.events_since(id.checked_sub(1)?, 1).ok()?;
    let row = rows.into_iter().find(|r| r.id == id)?;
    match row.event {
        Event::Transitioned {
            to: Status::Failed,
            reason,
            ..
        } => Some((w.kind.as_str().to_owned(), reason)),
        _ => None,
    }
}

/// 判定の前の guard（上の順）。読めない値は guard を外さず `Err`（呼び出し側は decide しない＝fail closed）。
pub(crate) fn pre_decide(
    store: &SqliteStore,
    limits: &BudgetLimits,
    state: &ProjectControllerState,
    wakes: &[ProjectWake],
    now: OffsetDateTime,
) -> Result<PreGuard, String> {
    let project = state.project_id.as_str();
    let human = wakes
        .iter()
        .any(|w| w.kind == ProjectWakeKind::HumanChanged);
    // 1. achieved
    if state.phase == ControllerPhase::Achieved && !human {
        return Ok(guarded(
            "achieved-quiet",
            ControllerPhase::Achieved,
            ProjectEffect::None,
            json!({ "wakes": wakes.len() }),
        ));
    }
    // 2. 同じ失敗（人が今変えたなら数え直す: この評価では掛けない）
    let since = if human {
        None
    } else {
        same_failure_since(store, project)?
    };
    for w in wakes.iter().filter(|_| !human) {
        let Some((kind, reason)) = failure_of(store, w) else {
            continue;
        };
        let sig = budget::failure_signature(&kind, &reason);
        let mut count = 0u32;
        let mut failed_roots = Vec::new();
        for past in store
            .project_wakes_all(project)
            .map_err(|e| e.to_string())?
        {
            if since
                .as_deref()
                .is_some_and(|s| past.created_at.as_str() < s)
            {
                continue;
            }
            if failure_of(store, &past)
                .is_some_and(|(k, r)| budget::failure_signature(&k, &r) == sig)
            {
                count += 1;
                failed_roots.extend(past.source_ref.clone());
            }
        }
        if limits.max_same_failure_repeats > 0 && count >= limits.max_same_failure_repeats {
            // 解消（人の操作・再開）の時刻を key に入れる: 解消の後にまた止まれば、新しい問いとして 1 回知らせる。
            let marker = since.as_deref().unwrap_or("0");
            return Ok(halt(
                "same-failure",
                format!("same_failure:{sig}:{marker}"),
                format!(
                    "案件の root が同じ理由で {count} 回失敗したので、controller は次の root を作らずに止まりました"
                ),
                json!({ "signature": sig, "count": count, "limit": limits.max_same_failure_repeats,
                        "failed_roots": failed_roots }),
            ));
        }
    }
    // 3. TS の品質
    if limits.max_invalid_decisions > 0 {
        let recent = store
            .project_controller_decisions(project, None, limits.max_invalid_decisions as usize)
            .map_err(|e| e.to_string())?;
        if recent.len() == limits.max_invalid_decisions as usize
            && recent
                .iter()
                .all(|d| d.validation.starts_with("pc/invalid"))
        {
            return Ok(halt(
                "invalid-proposals",
                format!("invalid_proposals:{}", recent[0].id),
                format!(
                    "controller の判定（TS）の案が {} 回続けて検証に落ちたので止まりました",
                    recent.len()
                ),
                json!({ "count": recent.len(), "last": recent[0].validation }),
            ));
        }
    }
    // 4. 予算
    let usage = budget::usage(store, project, now).map_err(|e| e.to_string())?;
    if let Some(kind) = budget::exceeded(limits, &usage) {
        let severity = budget::severity(limits, &usage);
        let expiry =
            (severity == budget::BudgetSeverity::Daily).then(|| budget::secs_until_next_day(now));
        let summary = if expiry.is_some() {
            format!(
                "controller の 1 日の予算（{}）を使い切ったので止まりました。次の UTC 00:00 に使用量を確かめ、危険な超過でなければ自動で再評価します",
                kind.as_str()
            )
        } else {
            format!(
                "controller の 1 日の予算（{}）を大きく超えたので止まりました。日付が変わっても自動では再開しません",
                kind.as_str()
            )
        };
        let mut pre = halt(
            "budget",
            format!("budget:{}:{}", kind.as_str(), usage.day),
            summary,
            json!({ "kind": kind, "severity": severity, "usage": usage, "limits": limits }),
        );
        if let PreGuard::Guarded { decision, .. } = &mut pre {
            decision.next_wake_after_secs = expiry;
        }
        return Ok(pre);
    }
    // 5. TTL
    let external = wakes
        .iter()
        .any(|w| task_core::project_wake::is_external_input(w.kind, &w.wake_key));
    if state.phase == ControllerPhase::Waiting
        && !external
        && let Some(last) = store
            .project_controller_last_input_at(project)
            .map_err(|e| e.to_string())?
        && budget::ttl_expired(&last, limits.ttl_secs, now)
    {
        return Ok(halt(
            "ttl",
            format!("ttl:{last}"),
            format!(
                "controller は {} 秒のあいだ外からの応答を待ちましたが来なかったので止まりました",
                limits.ttl_secs
            ),
            json!({ "last_input_at": last, "ttl_secs": limits.ttl_secs }),
        ));
    }
    Ok(PreGuard::Decide)
}

/// 同じ失敗を数え始める時刻: 最後の人の変更（`human_changed` の wake）か、同じ失敗の止まりからの再開の遅い方。
fn same_failure_since(store: &SqliteStore, project: &str) -> Result<Option<String>, String> {
    let human = store
        .project_wakes_all(project)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|w| w.kind == ProjectWakeKind::HumanChanged)
        .map(|w| w.created_at)
        .max();
    let resumed = store
        .project_controller_decisions(project, None, 256)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.validation == super::project_resume::SAME_FAILURE_RESUME_RULE)
        .map(|d| d.created_at);
    Ok(human.max(resumed))
}
