//! ADR 2026-11-07-health-review-integration D3.3・D3.5・D3.6: 計画の採用前の risk 判定と、高 risk（`high`・`unknown`）の
//! 計画だけに既存の reviewer run で設計 review を起こす処理のうち、Dispatcher の状態を使わない純粋な部分。
//!
//! - risk の判定そのものは `task_core::design_review::assess_risk`（純粋関数。I4）。ここでは計画の write path を
//!   unit の `context.repo` ごとに分け（[`write_paths_by_repo`]）、repo ごとの判定から最も高い level を採る
//!   （[`merge_risk`]）。
//! - 設計 review の待ちは events だけで表す（I9）: [`pending_design_review`] は最後の `DesignReviewRequested` と同じ
//!   `plan_sha256` の `DesignReviewed` が無い間を「待ち」とし、[`design_review_in_flight`] はその後に起こした reviewer run
//!   がまだ終わっていないかを events から読む（結果の扱いは `design_verdict.rs`）。
//!
//! Dispatcher の状態（実行中の run・完了の channel）を使う配線は `design_gate.rs`（`dispatcher` の子 module）にある。
//! review 方針は [`super::policy::load_review_policy`] が base 側から読んだものを使う。LLM は呼ばない。

use std::collections::{BTreeMap, BTreeSet};

use task_core::Event;
use task_core::design_review::{
    ReviewPolicyState, RiskAssessment, RiskLevel, assess_risk, high_risk_classes,
};
use task_core::execution_plan::{ExecutionPlanSpec, RepoSelector};

/// 計画の write path を repo ごとに分ける。/3 の unit は `context.repo`（無ければ `primary`）、/1・/2 の WorkUnit は
/// `primary`。集める path は `task_core::execution_plan::plan_write_paths` と同じ（unit の `expected_write_paths` と
/// leaf の `context.paths`）。`primary` は path が無くても必ず入る（write path の無い計画も判定する）。
pub(crate) fn write_paths_by_repo(
    spec: &ExecutionPlanSpec,
    primary: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut by_repo: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    by_repo.entry(primary.to_string()).or_default();
    for unit in &spec.units {
        let mut paths: Vec<String> = unit
            .expected_write_paths
            .iter()
            .flatten()
            .cloned()
            .collect();
        if !unit.is_task() {
            paths.extend(unit.context.paths.iter().cloned());
        }
        let repos = match &unit.context.repo {
            Some(RepoSelector::One(name)) => vec![name.clone()],
            Some(RepoSelector::Many(names)) if !names.is_empty() => names.clone(),
            _ => vec![primary.to_string()],
        };
        for repo in repos {
            by_repo
                .entry(repo)
                .or_default()
                .extend(paths.iter().cloned());
        }
    }
    let primary_paths = by_repo.entry(primary.to_string()).or_default();
    for wu in &spec.work_units {
        primary_paths.extend(wu.context.paths.iter().cloned());
    }
    by_repo
        .into_iter()
        .map(|(repo, paths)| (repo, paths.into_iter().collect()))
        .collect()
}

/// D3.3: planner の計画で brief を必須にする高 risk の class（repo ごとの方針と write path から求めた和。重複なし）。
/// 方針が `Present` でない repo・正規表現を読めない repo は数えない（後の [`merge_risk`] で `none`・`unknown` になる）。
pub(crate) fn brief_required_classes(
    policies: &[(String, ReviewPolicyState)],
    by_repo: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut classes = BTreeSet::new();
    for (repo, state) in policies {
        let (ReviewPolicyState::Present(policy), Some(paths)) = (state, by_repo.get(repo)) else {
            continue;
        };
        if let Ok(found) = high_risk_classes(policy, paths) {
            classes.extend(found);
        }
    }
    classes.into_iter().collect()
}

/// level の順位（高いほど設計 review の根拠が強い）。`unknown`（方針を読めない）は `low` より上、`high`（宣言と
/// path から決まった）より下に置く。どちらも設計 review を起こす（D3.5）。
fn rank(level: RiskLevel) -> u8 {
    match level {
        RiskLevel::None => 0,
        RiskLevel::Low => 1,
        RiskLevel::Unknown => 2,
        RiskLevel::High => 3,
    }
}

/// D3.5: repo ごとに `assess_risk` を呼び、最も高い level を採る。理由はその level の repo の理由（repo が 2 つ以上
/// なら `<repo>:` を前に付ける）。返す 2 つ目は判定に使った方針の `definition_sha256`（最も高い level の repo の
/// うち最初の `Present`）。
pub(crate) fn merge_risk(
    policies: &[(String, ReviewPolicyState)],
    by_repo: &BTreeMap<String, Vec<String>>,
    spec: &ExecutionPlanSpec,
) -> (RiskAssessment, Option<String>) {
    let empty = Vec::new();
    let assessed: Vec<(&String, &ReviewPolicyState, RiskAssessment)> = policies
        .iter()
        .map(|(repo, state)| {
            let paths = by_repo.get(repo).unwrap_or(&empty);
            (
                repo,
                state,
                assess_risk(state, paths, spec.design_brief.as_ref()),
            )
        })
        .collect();
    let Some(top) = assessed
        .iter()
        .map(|(_, _, r)| r.level)
        .max_by_key(|l| rank(*l))
    else {
        return (assess_risk(&ReviewPolicyState::Absent, &[], None), None);
    };
    let multi = assessed.len() > 1;
    let mut reasons = Vec::new();
    let mut policy_sha256 = None;
    for (repo, state, risk) in assessed.iter().filter(|(_, _, r)| r.level == top) {
        reasons.extend(risk.reasons.iter().map(|why| {
            if multi {
                format!("{repo}:{why}")
            } else {
                why.clone()
            }
        }));
        if let (None, ReviewPolicyState::Present(policy)) = (&policy_sha256, state) {
            policy_sha256 = Some(policy.definition_sha256.clone());
        }
    }
    (
        RiskAssessment {
            level: top,
            reasons,
        },
        policy_sha256,
    )
}

/// I9: 待っている設計 review（最後の `DesignReviewRequested` の seq と event。同じ `plan_sha256` の `DesignReviewed`
/// が後にあれば待っていない）。
pub(crate) fn pending_design_review(events: &[(u64, Event)]) -> Option<(u64, &Event)> {
    let (seq, request) = events
        .iter()
        .rev()
        .find(|(_, e)| matches!(e, Event::DesignReviewRequested { .. }))?;
    let Event::DesignReviewRequested { plan_sha256, .. } = request else {
        return None;
    };
    let reviewed = events.iter().any(|(s, e)| {
        *s > *seq
            && matches!(e, Event::DesignReviewed { plan_sha256: done, .. } if done == plan_sha256)
    });
    (!reviewed).then_some((*seq, request))
}

/// I9: `seq` の `DesignReviewRequested` の後に起こした reviewer run（`WorkerStarted { role: reviewer }`）の run id を
/// 順に返す。最後の run にまだ `WorkerFinished` が無ければ、その run の結果（design-verdict）を待つ（もう 1 本は
/// 起こさない）。デーモンの再起動で失われた run は孤児の回収（`ownerless_runs`）が `WorkerFinished{interrupted}` で
/// 閉じるので、その後の dispatch が起こし直す。
pub(crate) fn design_review_runs(events: &[(u64, Event)], seq: u64) -> Vec<&str> {
    events
        .iter()
        .filter(|(s, _)| *s > seq)
        .filter_map(|(_, e)| match e {
            Event::WorkerStarted {
                run_id,
                role: Some(task_core::RunRole::Reviewer),
                ..
            } => Some(run_id.as_str()),
            _ => None,
        })
        .collect()
}

/// I9: `seq` の後に起こした設計 review の run のうち、まだ終わっていない（`WorkerFinished` の無い）run の id。
pub(crate) fn design_review_in_flight(events: &[(u64, Event)], seq: u64) -> Option<&str> {
    design_review_runs(events, seq).into_iter().find(|run_id| {
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: done, .. } if done == run_id))
    })
}

/// D3.5 5.: 同じ task で続いた設計 review の不合格の数。最後の計画の採用（`ExecutionPlanned`）か合格
/// （`DesignReviewed { pass: true }`）より後の `DesignReviewed { pass: false }` を数える。
pub(crate) fn consecutive_design_review_failures(events: &[(u64, Event)]) -> usize {
    events
        .iter()
        .rev()
        .take_while(|(_, e)| {
            !matches!(
                e,
                Event::ExecutionPlanned { .. } | Event::DesignReviewed { pass: true, .. }
            )
        })
        .filter(|(_, e)| matches!(e, Event::DesignReviewed { pass: false, .. }))
        .count()
}

/// `seq` の `DesignReviewRequested` を出した planner run の id（その前の最後の `WorkerStarted { role: planner }`）。
pub(crate) fn planner_run_before(events: &[(u64, Event)], seq: u64) -> Option<&str> {
    events.iter().rev().find_map(|(s, e)| match e {
        Event::WorkerStarted {
            run_id,
            role: Some(task_core::RunRole::Planner),
            ..
        } if *s < seq => Some(run_id.as_str()),
        _ => None,
    })
}

/// D3.5 4.: 不合格の findings を planner の prior（再試行の理由）に渡す 1 行。`<kind>(<severity>)[ <path>]: <reason>` を
/// `; ` で結ぶ。findings が無ければ `why`（review.json が無い・壊れている等）。
pub(crate) fn design_findings_line(
    findings: &[task_worker::protocol::HealthFindingOut],
    why: &str,
) -> String {
    if findings.is_empty() {
        return why.to_string();
    }
    findings
        .iter()
        .map(|f| {
            let path = f
                .path
                .as_deref()
                .map(|p| format!(" {p}"))
                .unwrap_or_default();
            format!(
                "{}({}){path}: {}",
                serde_json::to_value(f.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                serde_json::to_value(f.severity)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                f.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// D3.5 4.: `DesignReviewed { pass: false }` からの replan の理由（`ExecutionPlannerContext.replan_reason`）。`findings` は
/// event の JSON の写し（`HealthFindingOut`）。読めない要素は数に入れない。
pub(crate) fn design_review_failed_reason(findings: &[serde_json::Value]) -> String {
    let parsed: Vec<task_worker::protocol::HealthFindingOut> = findings
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    format!(
        "design review failed: {}",
        design_findings_line(&parsed, "reviewer の判定が不合格（findings なし）")
    )
}

/// `DesignReviewRequest.plan_summary`: 計画の rationale、unit（/3）と WorkUnit（/1・/2）の key・題・write path。
pub(crate) fn plan_summary(spec: &ExecutionPlanSpec) -> String {
    let mut lines = vec![format!("rationale: {}", spec.rationale)];
    for unit in &spec.units {
        let mut paths: Vec<&String> = unit.expected_write_paths.iter().flatten().collect();
        paths.extend(unit.context.paths.iter());
        lines.push(format!(
            "- {} ({}): {} [paths: {}]",
            unit.key,
            unit.stage,
            unit.title,
            paths
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for wu in &spec.work_units {
        lines.push(format!(
            "- {}: {} [paths: {}]",
            wu.key,
            wu.title,
            wu.context.paths.join(", ")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "design/tests.rs"]
mod tests;
