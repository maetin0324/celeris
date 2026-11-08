//! ADR 2026-11-07-health-review-integration D3.3・D3.5・D3.6: 計画の採用前の risk 判定と、高 risk（`high`・`unknown`）の
//! 計画だけに既存の reviewer run で設計 review を起こす処理のうち、Dispatcher の状態を使わない純粋な部分。
//!
//! - risk の判定そのものは `task_core::design_review::assess_risk`（純粋関数。I4）。ここでは計画の write path を
//!   unit の `context.repo` ごとに分け（[`write_paths_by_repo`]）、repo ごとの判定から最も高い level を採る
//!   （[`merge_risk`]）。
//! - 設計 review の待ちは events だけで表す（I9）: [`pending_design_review`] は最後の `DesignReviewRequested` と同じ
//!   `plan_sha256` の `DesignReviewed` が無い間を「待ち」とし、[`design_review_started`] はその後に reviewer run を
//!   起こしたかを events から読む。
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

/// I9: `seq` の `DesignReviewRequested` の後に reviewer run を起こしたか（`WorkerStarted { role: reviewer }`）。
/// 起こしていれば、その run の結果（design-verdict）を待つ。もう 1 本は起こさない。
pub(crate) fn design_review_started(events: &[(u64, Event)], seq: u64) -> bool {
    events.iter().any(|(s, e)| {
        *s > seq
            && matches!(
                e,
                Event::WorkerStarted {
                    role: Some(task_core::RunRole::Reviewer),
                    ..
                }
            )
    })
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
