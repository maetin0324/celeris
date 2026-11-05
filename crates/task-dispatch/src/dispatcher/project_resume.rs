//! P1-G（ADR 2026-11-05-phase3-p1g）: `blocked` の案件を、止めた原因が解消したときだけ根拠つきで再開する。LLM は呼ばない。
//!
//! 原因は最後の判断の行から決める（読めない値は再開しない）。再開は 1 つの transaction の CAS
//! （[`SqliteStore::project_controller_resume`]）で、判断の行に根拠を残す。再評価は tick の次の段が 1 回行い、
//! 判定・許可・予算の門は通常どおり掛かる（ここで効果は起こさない）。
//!
//! | 原因 | 解消 |
//! |---|---|
//! | 人への問い（TS の `escalate`。`human_required` の分類） | 人の回答: 問いの key の許可・提案の却下。`objective_change` は依頼文の変更も |
//! | 同じ失敗（`guard_same-failure`） | 人の操作（`human_changed`）か、失敗した root 自身の完了 |
//! | TTL（`guard_ttl`） | 外からの入力の wake |
//! | 日次予算（`guard_budget`） | 期限（次の UTC 00:00）を過ぎ、その日の使用量が危険でなく、自動再開の回数が残り、起票の予算は有効な許可がある |
//! | 検証落ちの連続・判定／効果の失敗・分類できない止まり | 自動では解けない（人が mode を置き直す） |
//!
//! 人の pause・cancel は phase が `paused`・`cancelled` になるのでここを通らない。

use serde_json::{Value, json};
use task_core::project_wake::{
    ControllerMode, ControllerPhase, EffectState, ProjectControllerDecision,
    ProjectControllerState, ProjectWake, ProjectWakeKind, is_external_input,
};
use task_core::store::{ProjectResume, SqliteStore};
use task_core::{StoreError, authority::ActionClass};
use task_ops::project_budget::{self as budget, BudgetLimits, BudgetSeverity};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::project_controller::ProjectEffect;

/// 再開の判断の行の schema（`proposal_json`）。
pub(crate) const RESUME_RECORD_SCHEMA: &str = "celeris.project-resume/1";
pub(crate) const SAME_FAILURE_RESUME_RULE: &str = "pc/resume/same_failure";
const BUDGET_RESUME_RULE: &str = "pc/resume/budget";
const TTL_RESUME_RULE: &str = "pc/resume/ttl";
const QUESTION_RESUME_RULE: &str = "pc/resume/question";
/// 自動再開の回数を数える期間（日）。
const AUTO_RESUME_WINDOW_DAYS: i64 = 7;

/// 止まった原因（最後の判断の行から決める）。
#[derive(Debug, PartialEq, Eq)]
enum BlockCause {
    Question {
        key: String,
        category: String,
    },
    SameFailure {
        failed_roots: Vec<String>,
    },
    Ttl,
    Budget {
        kind: String,
        day: String,
    },
    /// 自動では解けない（理由つき）。
    HumanOnly(&'static str),
}

/// 判断の結果。
enum Verdict {
    Stay,
    Resume {
        rule: &'static str,
        basis: Value,
    },
    /// 日次予算の期限が来たが再開しない（人に 1 回知らせて、期限を消す）。
    Hold {
        reason: &'static str,
        detail: String,
    },
}

fn classify(
    state: &ProjectControllerState,
    last: Option<&ProjectControllerDecision>,
) -> BlockCause {
    // 判定の失敗で止まると判断の行は増えず、`retry_count` だけが残る（tx1 が 0 に戻す）。
    if state.retry_count > 0 {
        return BlockCause::HumanOnly("decide_failed");
    }
    let Some(last) = last else {
        return BlockCause::HumanOnly("no_decision");
    };
    if last.effect_state == EffectState::Failed {
        return BlockCause::HumanOnly("effect_failed");
    }
    let Ok(ProjectEffect::OpenEscalation {
        question_key,
        category,
        ..
    }) = serde_json::from_str::<ProjectEffect>(&last.effect_json)
    else {
        return BlockCause::HumanOnly("not_an_escalation");
    };
    let detail = serde_json::from_str::<Value>(&last.proposal_json)
        .ok()
        .and_then(|rec| rec.get("detail").cloned())
        .unwrap_or(Value::Null);
    match category.as_str() {
        "guard_ttl" => BlockCause::Ttl,
        "guard_same-failure" => BlockCause::SameFailure {
            failed_roots: detail["failed_roots"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        },
        "guard_budget" => match (detail["kind"].as_str(), detail["usage"]["day"].as_str()) {
            (Some(kind), Some(day)) => BlockCause::Budget {
                kind: kind.to_owned(),
                day: day.to_owned(),
            },
            _ => BlockCause::HumanOnly("budget_record_unreadable"),
        },
        other if other.starts_with("guard_") => BlockCause::HumanOnly("guard"),
        _ => BlockCause::Question {
            key: question_key,
            category,
        },
    }
}

/// 止まった案件の notice の末尾: 何で再開するか。
pub(crate) fn resume_note(category: &str) -> &'static str {
    match category {
        "guard_budget" => "",
        "guard_ttl" => {
            "外からの応答が来ると自動で再評価します。人が controller の mode を置き直しても再開します。"
        }
        "guard_same-failure" => {
            "人が案件を操作するか、失敗した root が完了すると自動で再評価します。"
        }
        _ if category.starts_with("guard_") => {
            "人が controller の mode を置き直すまで判断しません。"
        }
        _ => "人が問いに答える（許可・却下）か controller の mode を置き直すまで判断しません。",
    }
}

fn wake_keys(wakes: &[&ProjectWake]) -> Value {
    json!(
        wakes
            .iter()
            .map(|w| w.wake_key.as_str())
            .collect::<Vec<_>>()
    )
}

fn time_reached(value: Option<&str>, now: OffsetDateTime) -> bool {
    value
        .and_then(|v| OffsetDateTime::parse(v, &Rfc3339).ok())
        .is_some_and(|t| t <= now)
}

/// 直近の期間（最後の人の変更より後）の自動再開の回数。
fn auto_budget_resumes(
    store: &SqliteStore,
    project: &str,
    now: OffsetDateTime,
) -> Result<usize, StoreError> {
    let from = (now - time::Duration::days(AUTO_RESUME_WINDOW_DAYS))
        .format(&Rfc3339)
        .unwrap_or_default();
    let human = store
        .project_wakes_all(project)?
        .into_iter()
        .filter(|w| w.kind == ProjectWakeKind::HumanChanged)
        .map(|w| w.created_at)
        .max()
        .unwrap_or_default();
    Ok(store
        .project_controller_decisions(project, None, 256)?
        .iter()
        .filter(|d| {
            d.validation == BUDGET_RESUME_RULE && d.created_at >= from && d.created_at >= human
        })
        .count())
}

/// 有効な `launch_root` の許可があるか（範囲は問わない。実際の起票は root の範囲で tx の中でもう一度照合する）。
fn has_live_launch_grant(
    store: &SqliteStore,
    project: &str,
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    let class = ActionClass::LaunchRoot;
    Ok(store
        .authority_grants(project)?
        .iter()
        .filter(|g| g.action_class == class)
        .any(|g| {
            store
                .authority_check(project, class.as_str(), &g.scope, now)
                .allow
        }))
}

fn decide_budget(
    store: &SqliteStore,
    limits: &BudgetLimits,
    state: &ProjectControllerState,
    (kind, day): (&str, &str),
    now: OffsetDateTime,
) -> Result<Verdict, StoreError> {
    if !time_reached(state.next_wake_at.as_deref(), now) {
        return Ok(Verdict::Stay);
    }
    let project = state.project_id.as_str();
    // 止めた日の使用量を数え直す（止めた後に走り切った run の分を含む）。
    let used = budget::usage_on_day(store, project, day, now)?;
    if budget::severity(limits, &used) == BudgetSeverity::Dangerous {
        return Ok(Verdict::Hold {
            reason: "dangerous_overrun",
            detail: format!(
                "{day} の使用量（token {}、CoS 入力 {}、起票 {}、CoS 起動 {}）が危険な超過でした",
                used.tokens, used.cos_inputs, used.roots, used.chat_starts
            ),
        });
    }
    let resumes = auto_budget_resumes(store, project, now)?;
    if resumes >= budget::MAX_AUTO_DAILY_RESUMES {
        return Ok(Verdict::Hold {
            reason: "auto_resume_limit",
            detail: format!(
                "予算の止まりからの自動の再評価が直近 {AUTO_RESUME_WINDOW_DAYS} 日で {resumes} 回に達しました"
            ),
        });
    }
    // 永続の許可なしに翌日の起票枠で root を増やさない。
    if kind == budget::BudgetKind::Roots.as_str()
        && state.mode == ControllerMode::Enabled
        && !has_live_launch_grant(store, project, now)?
    {
        return Ok(Verdict::Hold {
            reason: "no_live_grant",
            detail: "起票の予算で止まりましたが、有効な起票の許可がありません".to_owned(),
        });
    }
    Ok(Verdict::Resume {
        rule: BUDGET_RESUME_RULE,
        basis: json!({"reason": "next_day", "kind": kind, "day": day, "usage": used,
                       "auto_resumes_before": resumes}),
    })
}

fn decide(
    store: &SqliteStore,
    limits: &BudgetLimits,
    state: &ProjectControllerState,
    last: Option<&ProjectControllerDecision>,
    wakes: &[ProjectWake],
    now: OffsetDateTime,
) -> Result<Verdict, StoreError> {
    let project = state.project_id.as_str();
    match classify(state, last) {
        BlockCause::HumanOnly(_) => Ok(Verdict::Stay),
        BlockCause::Ttl => {
            let inputs: Vec<&ProjectWake> = wakes
                .iter()
                .filter(|w| is_external_input(w.kind, &w.wake_key))
                .collect();
            Ok(if inputs.is_empty() {
                Verdict::Stay
            } else {
                Verdict::Resume {
                    rule: TTL_RESUME_RULE,
                    basis: json!({"reason": "external_input", "wakes": wake_keys(&inputs)}),
                }
            })
        }
        BlockCause::SameFailure { failed_roots } => {
            let solved: Vec<&ProjectWake> = wakes
                .iter()
                .filter(|w| {
                    w.kind == ProjectWakeKind::HumanChanged
                        || (w.kind == ProjectWakeKind::RootDone
                            && w.source_ref
                                .as_ref()
                                .is_some_and(|r| failed_roots.contains(r)))
                })
                .collect();
            Ok(if solved.is_empty() {
                Verdict::Stay
            } else {
                Verdict::Resume {
                    rule: SAME_FAILURE_RESUME_RULE,
                    basis: json!({"reason": "cause_resolved", "wakes": wake_keys(&solved)}),
                }
            })
        }
        BlockCause::Budget { kind, day } => decide_budget(store, limits, state, (&kind, &day), now),
        BlockCause::Question { key, category } => {
            let resume = |basis: Value| Verdict::Resume {
                rule: QUESTION_RESUME_RULE,
                basis,
            };
            if let Some(g) = store.authority_answered(project, &key, now)? {
                return Ok(resume(
                    json!({"reason": "authority_answered", "question_key": key,
                    "grant_id": g.id, "source": g.source, "granted_by": g.granted_by}),
                ));
            }
            if let Some(p) = store.authority_question_dismissed(project, &key)? {
                return Ok(resume(
                    json!({"reason": "question_dismissed", "question_key": key,
                    "proposal_id": p.id, "resolved_by": p.resolved_by}),
                ));
            }
            // 依頼文を変えられるのは人だけ（CoS は `project_request_human_only`）。
            if category == "objective_change"
                && let Some(last) = last
            {
                let current = super::project_wake::project_request_sha256(store, project);
                if current != last.request_sha256 {
                    return Ok(resume(
                        json!({"reason": "request_changed", "question_key": key,
                        "from": last.request_sha256, "to": current}),
                    ));
                }
            }
            Ok(Verdict::Stay)
        }
    }
}

/// `blocked` の案件を見て、原因が解消したものを再開する。再開した数を返す。読めない値は再開しない（fail closed）。
pub(crate) fn resume_blocked(
    store: &SqliteStore,
    limits: &BudgetLimits,
    now: OffsetDateTime,
) -> usize {
    let states = match store.project_controller_active() {
        Ok(states) => states,
        Err(error) => {
            tracing::warn!(%error, "project controller: states unreadable for resume");
            return 0;
        }
    };
    let mut resumed = 0;
    for state in states
        .into_iter()
        .filter(|s| s.phase == ControllerPhase::Blocked && s.mode.wakes())
    {
        match resume_one(store, limits, &state, now) {
            Ok(true) => resumed += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, project = %state.project_id, "project controller: resume check failed");
            }
        }
    }
    resumed
}

fn resume_one(
    store: &SqliteStore,
    limits: &BudgetLimits,
    state: &ProjectControllerState,
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    let project = state.project_id.as_str();
    let Some(last_id) = state.last_decision_id.as_deref() else {
        return Ok(false);
    };
    let last = store.project_controller_decision(last_id)?;
    let wakes = store.project_wakes_pending(project)?;
    match decide(store, limits, state, last.as_ref(), &wakes, now)? {
        Verdict::Stay => Ok(false),
        Verdict::Hold { reason, detail } => {
            let notice = hold_notice(project, last_id, reason, &detail, now);
            store.project_controller_budget_hold(project, last_id, &notice, now)?;
            Ok(false)
        }
        Verdict::Resume { rule, basis } => {
            let record = json!({
                "schema": RESUME_RECORD_SCHEMA,
                "rule_id": rule,
                "blocked_decision_id": last_id,
                "basis": basis,
            });
            store.project_controller_resume(
                &ProjectResume {
                    project_id: project.to_owned(),
                    revision: state.revision,
                    human_epoch: state.human_epoch,
                    blocked_decision_id: last_id.to_owned(),
                    decision_id: ulid::Ulid::new().to_string(),
                    request_sha256: super::project_wake::project_request_sha256(store, project),
                    rule_id: rule.to_owned(),
                    record_json: record.to_string(),
                    effect_json: serde_json::to_string(&ProjectEffect::None)
                        .map_err(|e| StoreError::Invalid(e.to_string()))?,
                },
                now,
            )
        }
    }
}

fn hold_notice(
    project: &str,
    decision: &str,
    reason: &str,
    detail: &str,
    now: OffsetDateTime,
) -> task_core::feed::NoticeEvent {
    let title = format!("案件 {project} の controller は予算の止まりから自動で再開しません");
    task_core::feed::NoticeEvent {
        source_key: format!("project_controller:budget_hold:{project}:{decision}"),
        kind: task_core::feed::NoticeKind::BadNews,
        group_key: format!("bad_news:project_controller:{project}"),
        summary: format!(
            "{title}。{detail}（{reason}）。使用量と許可を確かめてから controller の mode を置き直すと再開します。"
        ),
        title,
        project_id: Some(project.to_owned()),
        task_id: None,
        target: None,
        links: Vec::new(),
        at: now,
    }
}

/// 最後の判断が「人だけを待つ」待ちか（`proposal` の `wait` の `waiting_on` が全部 human、または答えのある問いの待ち）。
pub(crate) fn is_human_wait(decision: &ProjectControllerDecision) -> bool {
    let Ok(rec) = serde_json::from_str::<Value>(&decision.proposal_json) else {
        return false;
    };
    let proposal = &rec["proposal"];
    let action = &proposal["action"];
    if action["kind"] != "wait" || action["phase"] != "waiting" {
        return false;
    }
    if proposal["explanation"]["rule_id"] == "pc/authority-answered" {
        return true;
    }
    action["waiting_on"]
        .as_array()
        .is_some_and(|w| !w.is_empty() && w.iter().all(|x| x["kind"] == "human"))
}

/// cron の照合の wake が要らない理由。`blocked` は再開の段が見るので要らない。人だけを待つ案件は、人の回答が wake で
/// 届き、取りこぼしの照合は `next_wake_at`（既定 1 日）が持つので、その時刻までは要らない。
pub(crate) fn reconcile_not_needed(
    store: &SqliteStore,
    project: &str,
    now: OffsetDateTime,
) -> Result<Option<&'static str>, StoreError> {
    let Some(state) = store.project_controller_state(project)? else {
        return Ok(None);
    };
    match state.phase {
        ControllerPhase::Blocked => return Ok(Some("project controller is blocked")),
        ControllerPhase::Waiting => {}
        _ => return Ok(None),
    }
    let Some(next) = state.next_wake_at.as_deref() else {
        return Ok(None);
    };
    if time_reached(Some(next), now) {
        return Ok(None);
    }
    let Some(last) = state.last_decision_id.as_deref() else {
        return Ok(None);
    };
    Ok(store
        .project_controller_decision(last)?
        .is_some_and(|d| is_human_wait(&d))
        .then_some("project controller waits for a person until next_wake_at"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ProjectControllerState {
        task_core::store::project_controller_default_state("p", "2026-11-05T00:00:00Z")
    }

    fn decision(effect: ProjectEffect, record: Value) -> ProjectControllerDecision {
        ProjectControllerDecision {
            id: "d".into(),
            project_id: "p".into(),
            based_on_revision: 1,
            human_epoch: 0,
            snapshot_sha256: String::new(),
            request_sha256: String::new(),
            generation_id: None,
            mode: ControllerMode::Enabled,
            proposal_json: record.to_string(),
            validation: "x".into(),
            effect_json: serde_json::to_string(&effect).unwrap(),
            effect_state: EffectState::Applied,
            effect_ref: None,
            progress_epoch: 0,
            created_at: "2026-11-05T00:00:00Z".into(),
            applied_at: None,
        }
    }

    fn ask(category: &str) -> ProjectEffect {
        ProjectEffect::OpenEscalation {
            question_key: "q".into(),
            category: category.into(),
            summary: "s".into(),
        }
    }

    #[test]
    fn p1g_block_cause_is_read_from_the_last_decision_and_fails_closed() {
        let s = state();
        assert_eq!(classify(&s, None), BlockCause::HumanOnly("no_decision"));
        assert_eq!(
            classify(&s, Some(&decision(ProjectEffect::None, json!({})))),
            BlockCause::HumanOnly("not_an_escalation")
        );
        assert_eq!(
            classify(
                &s,
                Some(&decision(ask("guard_invalid-proposals"), json!({})))
            ),
            BlockCause::HumanOnly("guard")
        );
        assert_eq!(
            classify(&s, Some(&decision(ask("guard_budget"), json!({})))),
            BlockCause::HumanOnly("budget_record_unreadable")
        );
        assert_eq!(
            classify(&s, Some(&decision(ask("explicit_human"), json!({})))),
            BlockCause::Question {
                key: "q".into(),
                category: "explicit_human".into()
            }
        );
        let mut failed = decision(ask("guard_ttl"), json!({}));
        failed.effect_state = EffectState::Failed;
        assert_eq!(
            classify(&s, Some(&failed)),
            BlockCause::HumanOnly("effect_failed")
        );
        let retried = ProjectControllerState {
            retry_count: 4,
            ..state()
        };
        assert_eq!(
            classify(&retried, Some(&decision(ask("guard_ttl"), json!({})))),
            BlockCause::HumanOnly("decide_failed")
        );
    }

    #[test]
    fn p1g_human_wait_needs_only_human_waits() {
        let wait = |rule: &str, on: Value| {
            decision(
                ProjectEffect::None,
                json!({"proposal": {"action": {"kind": "wait", "phase": "waiting", "waiting_on": on},
                                    "explanation": {"rule_id": rule}}}),
            )
        };
        assert!(is_human_wait(&wait(
            "pc/human-wait",
            json!([{"kind": "human"}])
        )));
        assert!(is_human_wait(&wait("pc/authority-answered", json!([]))));
        assert!(!is_human_wait(&wait("pc/x", json!([]))));
        assert!(!is_human_wait(&wait(
            "pc/x",
            json!([{"kind": "human"}, {"kind": "external"}])
        )));
        assert!(!is_human_wait(&decision(ProjectEffect::None, json!({}))));
    }
}
