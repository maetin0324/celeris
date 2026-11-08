//! ADR 2026-11-07-health-review-integration D3.3・D3.5 の試験（prefix `health_review_risk_`）。

use serde_json::json;

use super::*;
use crate::execution_plan::{ExecutionPlanSpec, assess_plan_risk, plan_sha256, plan_write_paths};

/// celeris.toml の `[review]` と同じ形の方針（events・store の 2 class）。
fn policy() -> ReviewPolicyState {
    ReviewPolicyState::Present(ReviewPolicy {
        owner_map: Some("docs/architecture-map.md".into()),
        high_risk: vec![
            PathClass {
                class: "store".into(),
                paths: vec![
                    "^crates/task-core/src/store/".into(),
                    "^crates/task-core/migrations/".into(),
                ],
            },
            PathClass {
                class: "events".into(),
                paths: vec!["^crates/task-core/src/model/events\\.rs$".into()],
            },
        ],
        invariants: BTreeMap::from([("events-append-only".into(), "CLAUDE.md".into())]),
        definition_sha256: "d".repeat(64),
    })
}

fn brief() -> DesignBrief {
    DesignBrief {
        root_cause: "c".into(),
        owner: "o".into(),
        options: vec![
            DesignOption {
                name: "a".into(),
                chosen: true,
                reason: "r".into(),
            },
            DesignOption {
                name: "b".into(),
                chosen: false,
                reason: "r".into(),
            },
        ],
        ..DesignBrief::default()
    }
}

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// /3 の計画: 子 task（`expected_write_paths`）と leaf（`context.paths`）を 1 つずつ持つ。
fn plan() -> ExecutionPlanSpec {
    serde_json::from_value(json!({
        "schema": "celeris.execution-plan/3",
        "rationale": "r",
        "stages": [{"key": "s", "kind": "implement", "title": "t"}],
        "units": [
            {"key": "child", "stage": "s", "kind": "task", "title": "t", "objective": "o",
             "expected_write_paths": ["crates/task-core/src/store/tasks.rs", "docs/"],
             "acceptance": [{"text": "x", "check": {"type": "reviewer"}}]},
            {"key": "leaf", "stage": "s", "kind": "implement", "title": "t", "objective": "o",
             "checks": [{"cmd": "true", "expect_exit": 0}],
             "context": {"paths": ["web/src/app.ts", "docs/"]}}
        ]
    }))
    .unwrap()
}

/// 方針の無い repo は `None`（理由 `review_policy_absent`）で、設計 review に回さない。
#[test]
fn health_review_risk_absent_policy_is_none() {
    let risk = assess_risk(
        &ReviewPolicyState::Absent,
        &paths(&["crates/task-core/src/model/events.rs"]),
        Some(&DesignBrief {
            new_fallback: vec!["f".into()],
            ..brief()
        }),
    );
    assert_eq!(risk.level, RiskLevel::None);
    assert_eq!(risk.reasons, vec!["review_policy_absent"]);
    assert!(!risk.level.needs_design_review());
}

/// 方針を読めない・正規表現を読めないときは fail closed の `Unknown`（理由に要約を付ける）。
#[test]
fn health_review_risk_unavailable_policy_is_unknown() {
    let risk = assess_risk(
        &ReviewPolicyState::Unavailable("repo_health.py exited 2\nTraceback ...".into()),
        &[],
        None,
    );
    assert_eq!(risk.level, RiskLevel::Unknown);
    assert_eq!(
        risk.reasons,
        vec!["review_policy_unavailable:repo_health.py exited 2"]
    );
    assert!(risk.level.needs_design_review());

    // Python の re では読めても Rust の regex で読めない式（後読み）は方針を読めないものとして扱う。
    let bad = ReviewPolicyState::Present(ReviewPolicy {
        high_risk: vec![PathClass {
            class: "odd".into(),
            paths: vec!["(?<=crates/)x".into()],
        }],
        ..ReviewPolicy::default()
    });
    let risk = assess_risk(&bad, &paths(&["crates/x"]), Some(&brief()));
    assert_eq!(risk.level, RiskLevel::Unknown);
    assert_eq!(risk.reasons.len(), 1);
    assert!(
        risk.reasons[0].starts_with("review_policy_unavailable:invalid path pattern"),
        "{:?}",
        risk.reasons
    );
    // 要約は 1 行・200 文字まで。
    let long = assess_risk(&ReviewPolicyState::Unavailable("x".repeat(500)), &[], None);
    assert_eq!(
        long.reasons[0].len(),
        "review_policy_unavailable:".len() + 200
    );
}

/// 高 risk の class に合う path と `new_state` などの宣言で `High`（class と欄の組ごとの理由）。
#[test]
fn health_review_risk_high_risk_path_with_new_state_is_high() {
    let risk = assess_risk(
        &policy(),
        &paths(&[
            "crates/task-core/src/store/tasks.rs",
            "crates/task-core/src/model/events.rs",
            "web/src/app.ts",
        ]),
        Some(&DesignBrief {
            new_state: vec!["design review の待ち".into()],
            new_port: vec!["Event::DesignReviewRequested".into()],
            ..brief()
        }),
    );
    assert_eq!(risk.level, RiskLevel::High);
    assert_eq!(
        risk.reasons,
        vec![
            "path_class:store+new_state",
            "path_class:store+new_port",
            "path_class:events+new_state",
            "path_class:events+new_port",
        ]
    );
    // `re.search` の意味: `^` が無い式は path のどこに合ってもよく、`$` は末尾に固定する。
    let unanchored = ReviewPolicyState::Present(ReviewPolicy {
        high_risk: vec![PathClass {
            class: "lease".into(),
            paths: vec!["leases\\.rs$".into()],
        }],
        ..ReviewPolicy::default()
    });
    let declared = DesignBrief {
        new_branch: vec!["b".into()],
        ..brief()
    };
    let hit = assess_risk(
        &unanchored,
        &paths(&["crates/task-dispatch/src/dispatcher/leases.rs"]),
        Some(&declared),
    );
    assert_eq!(hit.reasons, vec!["path_class:lease+new_branch"]);
    let miss = assess_risk(
        &unanchored,
        &paths(&["crates/task-dispatch/src/dispatcher/leases.rs.bak"]),
        Some(&declared),
    );
    assert_eq!(miss.level, RiskLevel::Low);
}

/// `new_fallback` の宣言は高 risk の path に触れなくても `High`。
#[test]
fn health_review_risk_new_fallback_is_high_on_any_path() {
    let risk = assess_risk(
        &policy(),
        &paths(&["web/src/app.ts"]),
        Some(&DesignBrief {
            new_fallback: vec!["旧経路への切替".into()],
            ..brief()
        }),
    );
    assert_eq!(risk.level, RiskLevel::High);
    assert_eq!(risk.reasons, vec!["new_fallback"]);
    // 高 risk の path にも触れれば、class の理由と `new_fallback` の両方を残す。
    let both = assess_risk(
        &policy(),
        &paths(&["crates/task-core/migrations/0100_x.sql"]),
        Some(&DesignBrief {
            new_fallback: vec!["f".into()],
            ..brief()
        }),
    );
    assert_eq!(
        both.reasons,
        vec!["path_class:store+new_fallback", "new_fallback"]
    );
}

/// 高 risk の path に触れる人の計画に brief が無ければ `Low`（理由 `brief_absent_human_plan`）。
#[test]
fn health_review_risk_human_plan_without_brief_is_low_with_reason() {
    let risk = assess_risk(
        &policy(),
        &paths(&["crates/task-core/src/model/events.rs"]),
        None,
    );
    assert_eq!(risk.level, RiskLevel::Low);
    assert_eq!(risk.reasons, vec!["brief_absent_human_plan"]);
    assert!(!risk.level.needs_design_review());
}

/// それ以外は理由なしの `Low`: 高 risk の path に触れない計画、触れても新しい仕組みを宣言しない brief。
#[test]
fn health_review_risk_other_plans_are_low() {
    let plain = assess_risk(&policy(), &paths(&["web/src/app.ts"]), None);
    assert_eq!(plain, RiskAssessment::new(RiskLevel::Low, Vec::new()));
    let no_new = assess_risk(
        &policy(),
        &paths(&["crates/task-core/src/store/tasks.rs"]),
        Some(&DesignBrief {
            removals: vec!["旧い列".into()],
            ..brief()
        }),
    );
    assert_eq!(no_new, RiskAssessment::new(RiskLevel::Low, Vec::new()));
    // 方針はあるが高 risk の class が 1 つも無い repo も `Low`。
    let empty = ReviewPolicyState::Present(ReviewPolicy::default());
    let risk = assess_risk(
        &empty,
        &paths(&["crates/task-core/src/store/tasks.rs"]),
        None,
    );
    assert_eq!(risk.level, RiskLevel::Low);
    assert!(risk.reasons.is_empty());
}

/// 計画の write path は全 unit の `expected_write_paths` と leaf の `context.paths` の和（重複なし・昇順）。
/// /1・/2 の `work_units` の `context.paths` も数える。
#[test]
fn health_review_risk_plan_write_paths_and_classes() {
    let spec = plan();
    let write_paths = plan_write_paths(&spec);
    assert_eq!(
        write_paths,
        paths(&[
            "crates/task-core/src/store/tasks.rs",
            "docs/",
            "web/src/app.ts"
        ])
    );
    let ReviewPolicyState::Present(p) = policy() else {
        unreachable!()
    };
    assert_eq!(high_risk_classes(&p, &write_paths).unwrap(), vec!["store"]);

    let mut with_brief = spec.clone();
    with_brief.design_brief = Some(DesignBrief {
        new_state: vec!["s".into()],
        ..brief()
    });
    let risk = assess_plan_risk(&policy(), &with_brief);
    assert_eq!(risk.level, RiskLevel::High);
    assert_eq!(risk.reasons, vec!["path_class:store+new_state"]);
    assert_eq!(
        assess_plan_risk(&policy(), &spec).reasons,
        vec!["brief_absent_human_plan"]
    );

    let v1: ExecutionPlanSpec = serde_json::from_value(json!({
        "schema": "celeris.execution-plan/1",
        "rationale": "r",
        "work_units": [{"key": "a", "kind": "implement", "title": "t", "objective": "o",
                        "context": {"paths": ["crates/task-core/src/model/events.rs"]}}]
    }))
    .unwrap();
    assert_eq!(
        plan_write_paths(&v1),
        paths(&["crates/task-core/src/model/events.rs"])
    );
}

/// risk の JSON（event と wire の欄）は snake_case で往復し、理由の無い `Low` は `reasons` を出さない。
/// `plan_sha256` は計画の JSON で決まり、計画が変われば変わる。
#[test]
fn health_review_risk_assessment_round_trips_and_plan_digest_is_stable() {
    let high = RiskAssessment::new(RiskLevel::High, vec!["new_fallback".into()]);
    let text = serde_json::to_string(&high).unwrap();
    assert_eq!(text, r#"{"level":"high","reasons":["new_fallback"]}"#);
    assert_eq!(serde_json::from_str::<RiskAssessment>(&text).unwrap(), high);
    let low: RiskAssessment = serde_json::from_str(r#"{"level":"low"}"#).unwrap();
    assert_eq!(low, RiskAssessment::new(RiskLevel::Low, Vec::new()));
    assert_eq!(serde_json::to_string(&low).unwrap(), r#"{"level":"low"}"#);
    assert!(serde_json::from_str::<RiskAssessment>(r#"{"level":"medium"}"#).is_err());

    let spec = plan();
    let digest = plan_sha256(&spec);
    assert_eq!(digest.len(), 64);
    assert_eq!(digest, plan_sha256(&spec.clone()));
    let mut other = spec;
    other.rationale = "changed".into();
    assert_ne!(digest, plan_sha256(&other));
}

/// D3.5・D4.3・D4.4 の 4 つの event は JSON で往復し、任意の欄（`policy_sha256`・`findings`）の無い JSON も読める。
#[test]
fn health_review_risk_events_round_trip_and_read_without_optional_fields() {
    use crate::Event;
    let spec = plan();
    let events = [
        Event::DesignReviewRequested {
            plan_sha256: plan_sha256(&spec),
            plan: Box::new(spec.clone()),
            risk: RiskAssessment::new(RiskLevel::High, vec!["new_fallback".into()]),
            policy_sha256: Some("d".repeat(64)),
        },
        Event::DesignReviewed {
            plan_sha256: "p".repeat(64),
            review_run: "run-rv".into(),
            pass: false,
            findings: vec![json!({
                "kind": "dual_source_of_truth", "severity": "blocking", "reason": "two owners"
            })],
        },
        Event::HealthReviewInputBuilt {
            review_run: "run-rv".into(),
            repo_id: "agent-platform".into(),
            target_sha: "a".repeat(40),
            reviewed_sha: "b".repeat(40),
            definition_sha256: "d".repeat(64),
            input: json!({"repo_id": "agent-platform", "owner_rows": []}),
        },
        Event::HealthReviewInputSkipped {
            review_run: "run-rv".into(),
            repo_id: "agent-platform".into(),
            reason: "remote_workspace".into(),
            target_sha: "a".repeat(40),
            reviewed_sha: "b".repeat(40),
            policy: Some(
                serde_json::json!({"source": "unavailable", "changed": false, "files": []}),
            ),
        },
    ];
    let names = [
        "design_review_requested",
        "design_reviewed",
        "health_review_input_built",
        "health_review_input_skipped",
    ];
    for (event, name) in events.iter().zip(names) {
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], name);
        let back: Event = serde_json::from_value(value).unwrap();
        assert_eq!(&back, event);
    }

    let requested: Event = serde_json::from_value(json!({
        "type": "design_review_requested",
        "plan_sha256": "p",
        "plan": serde_json::to_value(&spec).unwrap(),
        "risk": {"level": "unknown", "reasons": ["review_policy_unavailable:x"]}
    }))
    .unwrap();
    let Event::DesignReviewRequested {
        policy_sha256,
        risk,
        ..
    } = &requested
    else {
        panic!("{requested:?}")
    };
    assert_eq!(*policy_sha256, None);
    assert_eq!(risk.level, RiskLevel::Unknown);
    assert!(
        !serde_json::to_string(&requested)
            .unwrap()
            .contains("policy_sha256")
    );
    let reviewed: Event = serde_json::from_value(json!({
        "type": "design_reviewed", "plan_sha256": "p", "review_run": "r", "pass": true
    }))
    .unwrap();
    assert_eq!(
        reviewed,
        Event::DesignReviewed {
            plan_sha256: "p".into(),
            review_run: "r".into(),
            pass: true,
            findings: Vec::new(),
        }
    );
    assert!(
        !serde_json::to_string(&reviewed)
            .unwrap()
            .contains("findings")
    );
}
