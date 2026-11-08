//! 純粋な部分（repo ごとの write path・level の合成・events だけの待ち）の試験。

use super::*;
use task_core::design_review::{PathClass, ReviewPolicy};

fn policy(class: &str, pattern: &str, digest: &str) -> ReviewPolicyState {
    ReviewPolicyState::Present(ReviewPolicy {
        owner_map: None,
        high_risk: vec![PathClass {
            class: class.into(),
            paths: vec![pattern.into()],
        }],
        invariants: Default::default(),
        definition_sha256: digest.into(),
    })
}

fn spec(units: serde_json::Value, brief: Option<serde_json::Value>) -> ExecutionPlanSpec {
    let mut plan = serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "r",
        "stages": [{"key": "s", "kind": "implement", "title": "S"}],
        "units": units,
    });
    if let Some(brief) = brief {
        plan["design_brief"] = brief;
    }
    serde_json::from_value(plan).unwrap()
}

fn leaf(key: &str, paths: &[&str], repo: Option<&str>) -> serde_json::Value {
    let mut unit = serde_json::json!({
        "key": key, "stage": "s", "kind": "implement", "title": key,
        "objective": "do it", "done_when": ["done"],
        "context": {"paths": paths},
    });
    if let Some(repo) = repo {
        unit["context"]["repo"] = serde_json::json!(repo);
    }
    unit
}

fn brief_with_new_state() -> serde_json::Value {
    serde_json::json!({
        "root_cause": "c", "owner": "o",
        "options": [{"name": "a", "chosen": true, "reason": "x"}, {"name": "b", "chosen": false, "reason": "y"}],
        "new_state": ["a column"],
    })
}

/// unit の `context.repo` ごとに write path を分け、repo の無い unit は先頭の repo に入れる。先頭の repo は path が
/// 無くても入る。
#[test]
fn health_review_design_gate_paths_are_grouped_by_unit_repo() {
    let plan = spec(
        serde_json::json!([
            leaf("a", &["crates/x/src/a.rs"], None),
            leaf("b", &["web/b.ts"], Some("web"))
        ]),
        None,
    );
    let by_repo = write_paths_by_repo(&plan, "core");
    assert_eq!(by_repo["core"], vec!["crates/x/src/a.rs".to_string()]);
    assert_eq!(by_repo["web"], vec!["web/b.ts".to_string()]);
    let empty = spec(serde_json::json!([leaf("c", &[], Some("web"))]), None);
    assert!(write_paths_by_repo(&empty, "core")["core"].is_empty());
}

/// 複数 repo: repo ごとにその repo の方針で判定し、最も高い level を採る。理由には repo 名が付き、
/// `policy_sha256` は最も高い level の repo の方針。
#[test]
fn health_review_design_gate_takes_the_highest_level_across_repos() {
    let plan = spec(
        serde_json::json!([
            leaf("a", &["notes/a.md"], None),
            leaf("b", &["store/b.rs"], Some("web"))
        ]),
        Some(brief_with_new_state()),
    );
    let by_repo = write_paths_by_repo(&plan, "core");
    let policies = vec![
        (
            "core".to_string(),
            policy("store", "^store/", &"a".repeat(64)),
        ),
        (
            "web".to_string(),
            policy("store", "^store/", &"b".repeat(64)),
        ),
    ];
    let (risk, digest) = merge_risk(&policies, &by_repo, &plan);
    assert_eq!(risk.level, RiskLevel::High);
    assert_eq!(
        risk.reasons,
        vec!["web:path_class:store+new_state".to_string()]
    );
    assert_eq!(digest, Some("b".repeat(64)));
    // 片方の repo の方針を読めなければ unknown。high の repo があれば high が勝つ。
    let unreadable = vec![
        (
            "core".to_string(),
            ReviewPolicyState::Unavailable("boom".into()),
        ),
        (
            "web".to_string(),
            policy("store", "^nothing/", &"b".repeat(64)),
        ),
    ];
    let (risk, digest) = merge_risk(&unreadable, &by_repo, &plan);
    assert_eq!(risk.level, RiskLevel::Unknown);
    assert_eq!(digest, None);
    assert!(risk.level.needs_design_review());
    // brief を必須にする class は Present の repo の和。
    assert_eq!(
        brief_required_classes(&policies, &by_repo),
        vec!["store".to_string()]
    );
    assert!(brief_required_classes(&unreadable, &by_repo).is_empty());
}

/// I9: 待ちは最後の `DesignReviewRequested` と同じ `plan_sha256` の `DesignReviewed` の有無だけで決まり、reviewer run
/// を起こしたかはその後の `WorkerStarted { role: reviewer }` で決まる。
#[test]
fn health_review_design_gate_pending_review_is_read_from_events() {
    let plan = spec(serde_json::json!([leaf("a", &["x"], None)]), None);
    let requested = Event::DesignReviewRequested {
        plan_sha256: "p1".into(),
        plan: Box::new(plan),
        risk: assess_risk(&ReviewPolicyState::Unavailable("x".into()), &[], None),
        policy_sha256: None,
    };
    let started = Event::WorkerStarted {
        run_id: "r1".into(),
        adapter: "a".into(),
        model: "m".into(),
        provider: None,
        account: None,
        role: Some(task_core::RunRole::Reviewer),
        task_role: None,
    };
    let reviewed = |sha: &str| Event::DesignReviewed {
        plan_sha256: sha.into(),
        review_run: "r1".into(),
        pass: true,
        findings: Vec::new(),
    };
    let mut events = vec![(1, requested)];
    let (seq, _) = pending_design_review(&events).expect("pending");
    assert_eq!(seq, 1);
    assert!(design_review_in_flight(&events, seq).is_none());
    events.push((2, started));
    assert_eq!(design_review_in_flight(&events, seq), Some("r1"));
    events.push((3, reviewed("other")));
    assert!(
        pending_design_review(&events).is_some(),
        "another plan's verdict"
    );
    events.push((4, reviewed("p1")));
    assert!(pending_design_review(&events).is_none());
}

/// D3.5 4.・5.: 続いた不合格は最後の採用か合格より後の `DesignReviewed { pass: false }` の数。replan の理由は findings の
/// 種別・severity・path・理由を 1 行にする。
#[test]
fn health_review_design_verdict_consecutive_failures_and_reason_line() {
    let reviewed = |pass: bool| Event::DesignReviewed {
        plan_sha256: "p".into(),
        review_run: "r".into(),
        pass,
        findings: vec![serde_json::json!({
            "kind": "dual_source_of_truth",
            "severity": "blocking",
            "path": "crates/task-store/src/rows.rs",
            "reason": "rows and events both hold the revision",
        })],
    };
    let planned = Event::ExecutionPlanned {
        plan_id: "x".into(),
        version: 1,
        origin: task_core::PlanOrigin::Planner,
        supersedes: None,
        reason: None,
        plan: Box::new(spec(serde_json::json!([leaf("a", &["x"], None)]), None)),
    };
    let mut events = vec![(1, reviewed(false)), (2, reviewed(false))];
    assert_eq!(consecutive_design_review_failures(&events), 2);
    events.push((3, planned));
    assert_eq!(consecutive_design_review_failures(&events), 0);
    events.push((4, reviewed(false)));
    events.push((5, reviewed(true)));
    events.push((6, reviewed(false)));
    assert_eq!(consecutive_design_review_failures(&events), 1);
    let Event::DesignReviewed { findings, .. } = reviewed(false) else {
        unreachable!()
    };
    assert_eq!(
        design_review_failed_reason(&findings),
        "design review failed: dual_source_of_truth(blocking) crates/task-store/src/rows.rs: rows and events both hold the revision"
    );
    assert_eq!(
        design_review_failed_reason(&[]),
        "design review failed: reviewer の判定が不合格（findings なし）"
    );
}
