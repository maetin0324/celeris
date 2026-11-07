//! ADR 2026-11-07-health-review-integration D3.5・D4.1 の wire 型の試験（prefix `health_review_design_protocol_`）。

use serde_json::json;
use task_core::design_review::{PathClass, ReviewPolicy, RiskAssessment, RiskLevel};
use task_core::execution_plan::{DesignBrief, DesignOption};

use super::*;
use crate::protocol::{
    HealthFindingKind, HealthReviewInput, HealthSeverity, ReviewOutput, ReviewRequest,
    ReviewVerdictOut, RunContext, schema_value,
};

fn brief() -> DesignBrief {
    DesignBrief {
        root_cause: "retry state lives in two places".into(),
        owner: "crates/task-core/src/store".into(),
        options: vec![
            DesignOption {
                name: "move to store".into(),
                chosen: true,
                reason: "single source".into(),
            },
            DesignOption {
                name: "sync both".into(),
                chosen: false,
                reason: "dual state".into(),
            },
        ],
        new_state: vec!["design review wait".into()],
        removals: vec!["in-memory retry map".into()],
        ..DesignBrief::default()
    }
}

fn request() -> DesignReviewRequest {
    DesignReviewRequest {
        brief: Some(brief()),
        plan_summary: "units: store-move (crates/task-core/src/store/)".into(),
        risk: RiskAssessment {
            level: RiskLevel::High,
            reasons: vec!["path_class:store+new_state".into()],
        },
        review_policy: Some(ReviewPolicy {
            owner_map: Some("docs/architecture-map.md".into()),
            high_risk: vec![PathClass {
                class: "store".into(),
                paths: vec!["^crates/task-core/src/store/".into()],
            }],
            invariants: Default::default(),
            definition_sha256: "d".repeat(64),
        }),
    }
}

/// 設計 review の run の `context.review.design` は往復し、`summary` などは空のまま読める。
#[test]
fn health_review_design_protocol_request_round_trips() {
    let context = RunContext {
        review: Some(ReviewRequest {
            design: Some(Box::new(request())),
            ..Default::default()
        }),
        ..RunContext::default()
    };
    let value = serde_json::to_value(&context).unwrap();
    assert_eq!(value["review"]["design"]["risk"]["level"], "high");
    assert_eq!(
        value["review"]["design"]["review_policy"]["high_risk"][0]["class"],
        "store"
    );
    let back: RunContext = serde_json::from_value(value).unwrap();
    assert_eq!(back, context);

    // brief と方針の無い `unknown` の依頼は 2 欄を出さない。
    let unknown = DesignReviewRequest {
        brief: None,
        plan_summary: "s".into(),
        risk: RiskAssessment {
            level: RiskLevel::Unknown,
            reasons: vec!["review_policy_unavailable:remote workspace".into()],
        },
        review_policy: None,
    };
    let value = serde_json::to_value(&unknown).unwrap();
    assert!(value.get("brief").is_none() && value.get("review_policy").is_none());
    assert_eq!(
        serde_json::from_value::<DesignReviewRequest>(value).unwrap(),
        unknown
    );
}

/// reviewer の `review.json` の `design` は往復し、finding の種別は health verdict と同じ固定 7 種（未知は拒否）。
#[test]
fn health_review_design_protocol_verdict_round_trips_and_rejects_unknown_kind() {
    let out = ReviewOutput {
        verdicts: Vec::new(),
        health: None,
        design: Some(DesignVerdictOut {
            pass: false,
            findings: vec![HealthFindingOut {
                kind: HealthFindingKind::DualSourceOfTruth,
                severity: HealthSeverity::Blocking,
                path: Some("crates/task-core/src/store/tasks.rs".into()),
                reason: "retry state kept in the dispatcher too".into(),
                repair: None,
            }],
        }),
    };
    let value = serde_json::to_value(&out).unwrap();
    assert_eq!(
        value["design"]["findings"][0]["kind"],
        "dual_source_of_truth"
    );
    assert!(value.get("health").is_none());
    let back: ReviewOutput = serde_json::from_value(value).unwrap();
    assert_eq!(back, out);

    let passed: ReviewOutput =
        serde_json::from_value(json!({"verdicts": [], "design": {"pass": true}})).unwrap();
    assert_eq!(
        passed.design,
        Some(DesignVerdictOut {
            pass: true,
            findings: Vec::new()
        })
    );
    let bad = json!({"verdicts": [], "design": {"pass": false,
        "findings": [{"kind": "style_nit", "severity": "blocking", "reason": "x"}]}});
    assert!(serde_json::from_value::<ReviewOutput>(bad).is_err());
}

/// 旧い JSON（`design`・`brief` の無いもの）は今までどおり読め、`None` は出力に欄を出さない。
#[test]
fn health_review_design_protocol_old_json_reads_and_omits_new_fields() {
    let req: ReviewRequest =
        serde_json::from_value(json!({"summary": "s", "evidence": [], "criteria": [0]})).unwrap();
    assert_eq!(req.design, None);
    assert!(!serde_json::to_string(&req).unwrap().contains("design"));

    let out: ReviewOutput = serde_json::from_value(json!({"verdicts": [
        {"criterion": 0, "pass": true, "reason": "ok"}
    ]}))
    .unwrap();
    assert_eq!(out.design, None);
    assert_eq!(
        out.verdicts,
        vec![ReviewVerdictOut {
            criterion: 0,
            pass: true,
            reason: "ok".into(),
            repair: None
        }]
    );
    assert!(!serde_json::to_string(&out).unwrap().contains("design"));

    let input: HealthReviewInput = serde_json::from_value(json!({"repos": []})).unwrap();
    assert_eq!(input.brief, None);
    assert_eq!(serde_json::to_value(&input).unwrap(), json!({"repos": []}));
    let with_brief = HealthReviewInput {
        repos: Vec::new(),
        brief: Some(Box::new(brief())),
    };
    let value = serde_json::to_value(&with_brief).unwrap();
    assert_eq!(value["brief"]["removals"][0], "in-memory retry map");
    assert_eq!(
        serde_json::from_value::<HealthReviewInput>(value).unwrap(),
        with_brief
    );
}

/// 生成した schema に新しい型が載る（`docs/protocol/worker-protocol.schema.json` との一致は
/// `committed_schema_matches_generated` が見る）。
#[test]
fn health_review_design_protocol_schema_has_design_types() {
    let schema = serde_json::to_string(&schema_value()).unwrap();
    for name in [
        "DesignReviewRequest",
        "DesignVerdictOut",
        "RiskAssessment",
        "RiskLevel",
        "DesignBrief",
    ] {
        assert!(schema.contains(&format!("\"{name}\"")), "{name}");
    }
}
