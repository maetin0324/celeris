//! ADR 2026-11-07-health-review-integration D4.1・D5.1・I6・I7: health の wire 型の試験。

use std::collections::BTreeMap;

use serde_json::json;
use task_core::design_review::{PathClass, ReviewPolicy};

use super::*;
use crate::protocol::{
    ExecutionPlannerContext, ReviewOutput, ReviewRequest, ReviewVerdictOut, schema_value,
};

fn sample_input() -> HealthReviewInput {
    HealthReviewInput {
        repos: vec![HealthRepoInput {
            repo_id: "agent-platform".into(),
            target_sha: "a".repeat(40),
            reviewed_sha: "b".repeat(40),
            diff: DiffStat {
                files: 2,
                insertions: 10,
                deletions: 3,
                paths: vec![PathStat {
                    path: "crates/task-store/src/lib.rs".into(),
                    insertions: 10,
                    deletions: 3,
                }],
            },
            owner_rows: vec![OwnerRow {
                path: "crates/task-store/src/lib.rs".into(),
                row: "| store | crates/task-store | … |".into(),
            }],
            findings: HealthFindingsSummary {
                verdict: "fail".into(),
                reviewable: true,
                counts: HealthFindingCounts {
                    findings: 3,
                    fail_grade: 1,
                    new: 1,
                    worsened: 0,
                    existing: 2,
                },
                new: vec![HealthReportFinding {
                    rule_id: "size.file".into(),
                    fingerprint: "size.file:0123456789abcdef".into(),
                    severity: "error".into(),
                    delta: "new".into(),
                    path: Some("crates/task-store/src/lib.rs".into()),
                }],
                truncated: 0,
            },
            policy: HealthPolicyState {
                source: "base".into(),
                changed: false,
                files: vec![],
            },
            invariants: BTreeMap::from([("events-append-only".into(), "CLAUDE.md".into())]),
        }],
    }
}

#[test]
fn health_protocol_review_request_health_round_trips() {
    let req = ReviewRequest {
        summary: "s".into(),
        criteria: vec![0],
        health: Some(sample_input()),
        ..Default::default()
    };
    let text = serde_json::to_string(&req).unwrap();
    let back: ReviewRequest = serde_json::from_str(&text).unwrap();
    assert_eq!(back, req);
}

#[test]
fn health_protocol_review_output_verdict_round_trips() {
    let out = ReviewOutput {
        verdicts: vec![ReviewVerdictOut {
            criterion: 0,
            pass: true,
            reason: "ok".into(),
            repair: None,
        }],
        health: Some(HealthVerdictOut {
            findings: vec![
                HealthFindingOut {
                    kind: HealthFindingKind::RemovalMissing,
                    severity: HealthSeverity::Blocking,
                    path: Some("crates/a.rs".into()),
                    reason: "old path left".into(),
                    repair: Some(crate::protocol::ReviewRepairHint {
                        scope: "local".into(),
                        class: "other".into(),
                        hint: None,
                    }),
                },
                HealthFindingOut {
                    kind: HealthFindingKind::DuplicateImplementation,
                    severity: HealthSeverity::Advisory,
                    path: None,
                    reason: "similar helper".into(),
                    repair: None,
                },
            ],
            policy: Some(PolicyVerdictOut {
                approved: true,
                reason: "threshold change justified".into(),
            }),
        }),
    };
    let value = serde_json::to_value(&out).unwrap();
    assert_eq!(value["health"]["findings"][0]["kind"], "removal_missing");
    assert_eq!(value["health"]["findings"][0]["severity"], "blocking");
    assert_eq!(value["health"]["findings"][1]["severity"], "advisory");
    let back: ReviewOutput = serde_json::from_value(value).unwrap();
    assert_eq!(back, out);
}

#[test]
fn health_protocol_planner_review_policy_round_trips() {
    let ctx = ExecutionPlannerContext {
        review_policy: Some(ReviewPolicy {
            owner_map: Some("docs/architecture-map.md".into()),
            high_risk: vec![PathClass {
                class: "dispatch-state".into(),
                paths: vec!["^crates/task-store/".into()],
            }],
            invariants: BTreeMap::from([("task-state-in-sqlite".into(), "CLAUDE.md".into())]),
            definition_sha256: "c".repeat(64),
        }),
        ..Default::default()
    };
    let value = serde_json::to_value(&ctx).unwrap();
    assert_eq!(
        value["review_policy"]["high_risk"][0]["class"],
        "dispatch-state"
    );
    let back: ExecutionPlannerContext = serde_json::from_value(value).unwrap();
    assert_eq!(back, ctx);
}

/// 追加のみ: 旧 daemon・旧 reviewer の JSON は新しい欄が無くても読め、`None` の欄は書き出さない。
#[test]
fn health_protocol_omitted_fields_default_and_are_not_written() {
    let req: ReviewRequest =
        serde_json::from_value(json!({"summary": "s", "evidence": [], "criteria": [0]})).unwrap();
    assert_eq!(req.health, None);
    let req_json = serde_json::to_value(&req).unwrap();
    assert!(req_json.get("health").is_none(), "{req_json}");

    let out: ReviewOutput = serde_json::from_value(
        json!({"verdicts": [{"criterion": 0, "pass": true, "reason": "ok"}]}),
    )
    .unwrap();
    assert_eq!(out.health, None);
    assert_eq!(
        serde_json::to_value(&out).unwrap(),
        json!({"verdicts": [{"criterion": 0, "pass": true, "reason": "ok"}]})
    );

    let mut planner = serde_json::to_value(ExecutionPlannerContext::default()).unwrap();
    planner.as_object_mut().unwrap().remove("review_policy");
    let ctx: ExecutionPlannerContext = serde_json::from_value(planner).unwrap();
    assert_eq!(ctx.review_policy, None);
    assert!(
        serde_json::to_value(&ctx)
            .unwrap()
            .get("review_policy")
            .is_none()
    );

    // 中の欄の既定: health の verdict は findings も policy も省略できる。
    let empty: HealthVerdictOut = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty, HealthVerdictOut::default());
    let policy: ReviewPolicy = serde_json::from_value(json!({"definition_sha256": "d"})).unwrap();
    assert_eq!(policy.owner_map, None);
    assert!(policy.high_risk.is_empty() && policy.invariants.is_empty());
}

/// I6: finding 種別と重さは固定で、未知の値は `review.json` の読み込みエラーになる。
#[test]
fn health_protocol_unknown_finding_kind_or_severity_is_rejected() {
    let with = |kind: &str, severity: &str| {
        json!({
            "verdicts": [],
            "health": {"findings": [{"kind": kind, "severity": severity, "reason": "r"}]}
        })
    };
    let err = serde_json::from_value::<ReviewOutput>(with("style_nit", "blocking")).unwrap_err();
    assert!(err.to_string().contains("unknown variant"), "{err}");
    let err =
        serde_json::from_value::<ReviewOutput>(with("removal_missing", "critical")).unwrap_err();
    assert!(err.to_string().contains("unknown variant"), "{err}");
    assert!(serde_json::from_value::<ReviewOutput>(with("removal_missing", "blocking")).is_ok());
}

/// D5.1 の表: 7 種の wire 名・`as_str`・schema の enum が一致し、局所修復を許すのは 2 種だけ。
#[test]
fn health_protocol_kinds_are_the_fixed_seven() {
    let names: Vec<&str> = HealthFindingKind::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(
        names,
        [
            "symptom_fix_outside_owner",
            "duplicate_implementation",
            "local_hook_in_shared_interface",
            "dual_source_of_truth",
            "unapproved_fallback_or_switch",
            "removal_missing",
            "test_change_unjustified",
        ]
    );
    for kind in HealthFindingKind::ALL {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
    }
    let local: Vec<&str> = HealthFindingKind::ALL
        .iter()
        .filter(|k| k.allows_local_repair())
        .map(|k| k.as_str())
        .collect();
    assert_eq!(local, ["removal_missing", "test_change_unjustified"]);

    let schema = schema_value();
    let defs = &schema["$defs"];
    let kinds: Vec<&str> = defs["HealthFindingKind"]["oneOf"]
        .as_array()
        .map(|variants| {
            variants
                .iter()
                .filter_map(|v| v["const"].as_str())
                .collect()
        })
        .or_else(|| {
            defs["HealthFindingKind"]["enum"]
                .as_array()
                .map(|v| v.iter().filter_map(|s| s.as_str()).collect())
        })
        .unwrap_or_default();
    assert_eq!(kinds, names, "{}", defs["HealthFindingKind"]);
}
