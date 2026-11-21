//! 純粋な部分（repo ごとの write path・level の合成・events だけの待ち）の試験。

use super::*;
use task_core::design_review::{PathClass, ReviewPolicy};

fn policy(class: &str, pattern: &str, digest: &str) -> ReviewPolicyState {
    ReviewPolicyState::Present(ReviewPolicy {
        owner_map: None,
        high_risk: vec![PathClass {
            class: class.into(),
            paths: vec![pattern.into()],
            invariants: vec![],
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
        vec!["web:path_class:store:store/b.rs", "web:declared:new_state"]
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

/// 不変条件（ADR 2026-11-19-planner-reviewer-prompt-coverage D2）: 採用前の計画の設計 review の reviewer の prompt
/// （`run_design_reviewer` と同じく `crate::review::synthetic_review_task` で対象 task の objective と acceptance を写した
/// `Review` task を、`ReviewRequest { design: Some(..) }` の `RunContext` で `claude_code::build_prompt` に渡したもの）に、
/// 3,000 文字の objective の末尾の印と全受入条件の text が切れずに載り、設計 review の節も出る。設計 review は計画が
/// 依頼の末尾の条件（禁止事項・不変条件）を満たすかを判定するので、末尾が落ちると判定の根拠が欠ける。
/// 既存の試験で検出できない理由: `prompt_boundary_design_review_shows_reason_owner_row_and_invariants`（task-worker）は
/// 短い fixture の objective で risk の理由の行しか見ず、`synthetic_review_task` を通らない。
/// `prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule` は末尾の印を worker の prompt に
/// しか当てない。この file の既存の試験は prompt を組まない。
#[test]
fn prompt_full_objective_design_review_keeps_objective_tail() {
    use crate::review::tests::{assert_review_prompt_keeps_subject, long_objective_subject};
    use task_worker::protocol::DesignReviewRequest;
    use task_worker::{ReviewRequest, RunContext};

    let dir = tempfile::tempdir().unwrap();
    let subject = long_objective_subject(dir.path());
    let plan = spec(
        serde_json::json!([leaf("a", &["store/a.rs"], None)]),
        Some(brief_with_new_state()),
    );
    let policies = vec![(
        "core".to_string(),
        policy("store", "^store/", &"a".repeat(64)),
    )];
    let (risk, _) = merge_risk(&policies, &write_paths_by_repo(&plan, "core"), &plan);
    assert!(risk.level.needs_design_review());
    let request = DesignReviewRequest {
        brief: plan.design_brief.clone(),
        plan_summary: plan_summary(&plan),
        risk,
        review_policy: None,
        owner_rows: Vec::new(),
    };
    let review = crate::review::synthetic_review_task(
        &subject,
        "design-tail",
        &crate::review::reviewer_hint(),
    );
    let context = RunContext {
        review: Some(ReviewRequest {
            design: Some(Box::new(request)),
            ..Default::default()
        }),
        ..RunContext::default()
    };
    let prompt =
        task_worker::claude_code::build_prompt(&review, &context, "design-tail", "artifacts");
    assert!(
        prompt.contains(task_worker::change_rules::review::DESIGN_HEADING),
        "the design review section is missing"
    );
    assert_review_prompt_keeps_subject(&prompt, &subject, "design review");
}

/// 不変条件（ADR 2026-11-21-ancestor-objective-in-prompts D3）: 子 task の計画の設計 review の reviewer の prompt
/// （`run_design_reviewer` と同じく `ReviewRequest { design: Some(..) }` と祖先を `lineage.ancestors` に写した `RunContext`）
/// に、`start_design_review` と同じく store から `prompt_ancestors::chain_to_root` で組んだ親の 3,000 文字の objective の末尾の印と受入条件の末尾の
/// 印がちょうど 1 回ずつ載る。設計 review は計画が人の元の目的（親の禁止事項・不変条件）を満たすかを判定する。
/// 既存の試験で検出できない理由: `prompt_full_objective_design_review_keeps_objective_tail` は対象 task 自身の objective と
/// 受入条件だけを見て、祖先を持つ task を使わない。
#[test]
fn prompt_ancestor_objective_design_review_carries_the_parent() {
    use task_core::{Check, Criterion, Status};
    use task_worker::protocol::DesignReviewRequest;
    use task_worker::{ReviewRequest, RunContext};

    let dir = tempfile::tempdir().unwrap();
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    let mut parent = crate::review::tests::long_objective_subject(dir.path());
    parent.status = Status::Draft;
    parent.objective = format!("{}PARENT-DESIGN-OBJECTIVE-TAIL", "親".repeat(3000));
    parent.acceptance = vec![Criterion {
        text: format!("{}PARENT-DESIGN-ACCEPTANCE-TAIL", "条".repeat(3000)),
        check: Check::Human,
    }];
    store.insert(&parent).unwrap();
    let mut subject = crate::review::tests::long_objective_subject(dir.path());
    subject.id = task_core::TaskId::new();
    subject.parent_id = Some(parent.id);
    store.insert(&subject).unwrap();
    let plan = spec(
        serde_json::json!([leaf("a", &["store/a.rs"], None)]),
        Some(brief_with_new_state()),
    );
    let policies = vec![(
        "core".to_string(),
        policy("store", "^store/", &"a".repeat(64)),
    )];
    let (risk, _) = merge_risk(&policies, &write_paths_by_repo(&plan, "core"), &plan);
    let request = DesignReviewRequest {
        brief: plan.design_brief.clone(),
        plan_summary: plan_summary(&plan),
        risk,
        review_policy: None,
        owner_rows: Vec::new(),
    };
    let review = crate::review::synthetic_review_task(
        &subject,
        "design-ancestor",
        &crate::review::reviewer_hint(),
    );
    let context = RunContext {
        review: Some(ReviewRequest {
            design: Some(Box::new(request)),
            ..Default::default()
        }),
        lineage: task_worker::protocol::TaskLineageContext {
            ancestors: crate::prompt_ancestors::ancestor_contexts(
                &crate::prompt_ancestors::chain_to_root(&store, &subject).unwrap(),
            ),
            ..Default::default()
        },
        ..RunContext::default()
    };
    let prompt =
        task_worker::claude_code::build_prompt(&review, &context, "design-ancestor", "artifacts");
    assert!(prompt.contains(task_worker::change_rules::review::DESIGN_HEADING));
    for tail in [
        "PARENT-DESIGN-OBJECTIVE-TAIL",
        "PARENT-DESIGN-ACCEPTANCE-TAIL",
    ] {
        assert_eq!(
            prompt.matches(tail).count(),
            1,
            "`{tail}` must appear exactly once:\n{prompt}"
        );
    }
}
