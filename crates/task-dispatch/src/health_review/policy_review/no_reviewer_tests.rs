//! ADR 2026-11-07-health-review-integration I8・D6.4: `Check::Reviewer` の無い task（reviewer run の無い final
//! review）にも policy の関門を掛ける試験（prefix `health_review_policy_no_reviewer_`）。
//!
//! 一時 git repo の main に stub の `repo_health.py` を commit し（[`super::unknown_tests`] の fixture）、command の
//! 条件だけの task で材料作りを `policy_only`（reviewer には何も渡さず、policy の状態だけを記録する）で本当に起こす。
//! 残った event を [`Dispatcher::policy_review_gate`] が読む。外部ネットワークに出ず、LLM を呼ばない。

use super::unknown_tests::{Case, POLICY_FILE, Policy, Report, case_for};
use super::*;
use crate::health_review::input::NO_REVIEWER;

fn no_reviewer(checker: Option<(Policy, Report)>, changed: &str) -> Case {
    case_for(checker, changed, false)
}

/// 材料作りが残した event のうち、policy の状態を持つ `HealthReviewInputSkipped` の `(review_run, reason)`。
/// reviewer に渡す材料（`HealthReviewInputBuilt`）が無いことも確かめる。
fn policy_records(c: &Case) -> Vec<(String, String)> {
    let events = c.store.events_for(c.task.id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::HealthReviewInputBuilt { .. })),
        "a review without a reviewer run builds no reviewer input"
    );
    events
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::HealthReviewInputSkipped {
                review_run,
                reason,
                target_sha,
                reviewed_sha,
                policy: Some(_),
                ..
            } => {
                assert_eq!((&target_sha, &reviewed_sha), (&c.target, &c.reviewed));
                Some((review_run, reason))
            }
            _ => None,
        })
        .collect()
}

/// 受け入れ条件 0: command の条件だけの task で、policy の file だけを変えた candidate と policy の状態を決められない
/// candidate は、command の条件が合格しても承認の子を作って待つ（人の承認なしには合格しない）。review 方針
/// （`[review]`）の有無・読めないことに依らない（reviewer の材料にしか使わないので読まない）。
#[test]
fn health_review_policy_no_reviewer_policy_change_and_unknown_wait_for_a_human() {
    let files = format!("policy files: {POLICY_FILE}");
    let cases = [
        (
            Policy::Present,
            Report::Valid(true),
            POLICY_FILE,
            files.as_str(),
        ),
        (Policy::Absent, Report::Valid(true), POLICY_FILE, &files),
        (Policy::Fails, Report::Valid(true), POLICY_FILE, &files),
        (
            Policy::Present,
            Report::Missing,
            "crates/x/src/lib.rs",
            "policy state unknown: ",
        ),
        (
            Policy::Present,
            Report::Broken,
            "crates/x/src/lib.rs",
            "report を読めない",
        ),
    ];
    for (policy, report, changed, text) in cases {
        let c = no_reviewer(Some((policy, report)), changed);
        let records = policy_records(&c);
        assert_eq!(records.len(), 1, "{text}");
        assert_eq!(records[0].0, "worker");
        assert!(records[0].1.starts_with(NO_REVIEWER), "{}", records[0].1);
        let child = c.assert_waits();
        assert!(
            child.objective.contains(text) && child.objective.contains(&c.reviewed),
            "{text}: {}",
            child.objective
        );
        // 同じ candidate の再判定で子を作り直さない。
        c.gate();
        assert_eq!(c.approvals().len(), 1);
    }
}

/// 受け入れ条件 0: 人の承認で合格する（verdict は acceptance の外の criterion。command の条件には付けない）。却下は
/// `try_review_repair` が修復にしない（command の局所修復に化けず、ReviewFail → replan）。
#[test]
fn health_review_policy_no_reviewer_approval_passes_and_rejection_replans() {
    let approved = no_reviewer(Some((Policy::Present, Report::Valid(true))), POLICY_FILE);
    let child = approved.assert_waits();
    approved.decide(&child, true);
    assert!(approved.d.policy_rereview(&approved.task).unwrap());
    let outcome = approved.gate();
    assert!(outcome.all_pass(), "{:?}", outcome.verdicts);
    let last = outcome.verdicts.last().unwrap();
    assert_eq!(last.criterion_idx, approved.task.acceptance.len());
    assert_eq!(
        last.reason,
        format!(
            "human(approval task {}): health:policy approved by a human: {POLICY_FILE}",
            child.id
        )
    );

    let rejected = no_reviewer(Some((Policy::Present, Report::Valid(true))), POLICY_FILE);
    let child = rejected.assert_waits();
    rejected.decide(&child, false);
    let outcome = rejected.gate();
    assert!(!outcome.all_pass());
    let failed: Vec<&Verdict> = outcome.verdicts.iter().filter(|v| !v.pass).collect();
    assert_eq!(failed.len(), 1, "{:?}", outcome.verdicts);
    assert!(failed[0].criterion_idx >= rejected.task.acceptance.len());
    assert!(
        failed[0]
            .reason
            .contains("health:policy: rejected by a human")
    );
    let mut events = Vec::new();
    let repair = rejected
        .d
        .try_review_repair(&rejected.task, &outcome.verdicts, &mut events)
        .unwrap();
    assert!(repair.is_none() && events.is_empty());
}

/// 受け入れ条件 1: 検査器の無い repo は何も記録せず、policy を変えない通常変更は状態（変更なし）を記録して、どちらも
/// 承認の子を作らず verdict も変えない。
#[test]
fn health_review_policy_no_reviewer_no_checker_and_normal_change_pass_without_approval() {
    for (checker, recorded) in [
        (None, 0),
        (Some((Policy::Present, Report::Valid(false))), 1),
        (Some((Policy::Fails, Report::Valid(false))), 1),
    ] {
        let c = no_reviewer(checker, "crates/x/src/lib.rs");
        assert_eq!(policy_records(&c).len(), recorded);
        let outcome = c.gate();
        assert_eq!(outcome.verdicts, c.outcome().verdicts);
        assert!(outcome.all_pass());
        assert!(c.approvals().is_empty());
        assert!(!c.d.policy_rereview(&c.task).unwrap());
    }
}

/// 受け入れ条件 2: `Check::Reviewer` のある task は従来どおり reviewer の条件に verdict を付ける。無い task は verdict の
/// 最大の `criterion_idx` の次（acceptance の外）。
#[test]
fn health_review_policy_no_reviewer_criterion_choice() {
    let verdict = |criterion_idx| Verdict {
        criterion_idx,
        pass: true,
        reason: "ok".into(),
        repair_hint: None,
    };
    let reviewer = case_for(None, "crates/x/src/lib.rs", true);
    let mut outcome = reviewer.outcome();
    outcome.verdicts.push(verdict(5));
    assert_eq!(policy_criterion(&reviewer.task, &outcome), 0);

    let c = no_reviewer(None, "crates/x/src/lib.rs");
    let mut outcome = c.outcome();
    assert_eq!(policy_criterion(&c.task, &outcome), 1);
    outcome.verdicts.push(verdict(3));
    assert_eq!(policy_criterion(&c.task, &outcome), 4);
    outcome.verdicts.clear();
    assert_eq!(policy_criterion(&c.task, &outcome), c.task.acceptance.len());
}

/// reviewer の合格を再利用した candidate は `ReviewerPassReused` の元の candidate の記録を見て（従来どおり）、再利用で
/// ない reviewer の無い review はその candidate の記録を見る。
#[test]
fn health_review_policy_no_reviewer_reuse_reads_the_original_candidate() {
    let c = no_reviewer(Some((Policy::Present, Report::Valid(true))), POLICY_FILE);
    let own = RequiredCheckBinding {
        repo_id: c.repo,
        target_sha: c.target.clone(),
        reviewed_sha: c.reviewed.clone(),
    };
    let later = RequiredCheckBinding {
        repo_id: c.repo,
        target_sha: "1".repeat(40),
        reviewed_sha: "2".repeat(40),
    };
    let events = c.store.events_for(c.task.id).unwrap();
    assert_eq!(policy_changes(&events, false, [&own]).len(), 1);
    assert!(policy_changes(&events, false, [&later]).is_empty());
    c.store
        .append_event(
            c.task.id,
            &Event::ReviewerPassReused {
                review_run: "rv".into(),
                repo_id: c.repo,
                target_sha: later.target_sha.clone(),
                reviewed_sha: later.reviewed_sha.clone(),
                from_target_sha: c.target.clone(),
                from_reviewed_sha: c.reviewed.clone(),
                target_files: vec![],
            },
        )
        .unwrap();
    let events = c.store.events_for(c.task.id).unwrap();
    let changes = policy_changes(&events, false, [&later]);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (&changes[0].target_sha, &changes[0].reviewed_sha),
        (&c.target, &c.reviewed)
    );
}
