//! 付記 S2: `classify_lane_rules` と `call_lane_rules` の単体試験（名前に script_compare を含む）。

use super::*;
use task_core::model_policy::{Level, match_rule};

fn features() -> TaskFeatures {
    TaskFeatures {
        judgment: Level::Low,
        ambiguity: Level::Low,
        verifiability: Level::High,
        reversibility: Level::High,
        consequence: Level::Low,
        context_size: Level::Low,
        tool_intensity: Level::Low,
        expected_length: Level::Low,
        cross_cutting: Level::Low,
    }
}

fn rust() -> LaneRulesOutput {
    match_rule(&features()).into()
}

fn reply(output: serde_json::Value) -> TsResult {
    TsResult::Reply(PolicyReply {
        output,
        isolate_epoch: 3,
    })
}

#[test]
fn script_compare_classify_match_needs_all_three_fields_byte_equal() {
    let r = rust();
    let c = classify_lane_rules(
        &r,
        r.proposed,
        &LaneCeiling::default(),
        &reply(serde_json::to_value(&r).unwrap()),
    );
    assert_eq!(c.outcome, CompareOutcome::Match);
    for field in ["rule_id", "condition"] {
        let mut v = serde_json::to_value(&r).unwrap();
        v[field] = serde_json::json!(format!("{} ", v[field].as_str().unwrap()));
        let c = classify_lane_rules(&r, r.proposed, &LaneCeiling::default(), &reply(v));
        assert_eq!(c.outcome, CompareOutcome::Mismatch, "{field}");
    }
    let mut v = serde_json::to_value(&r).unwrap();
    v["proposed"] = serde_json::json!("frontier");
    let c = classify_lane_rules(&r, r.proposed, &LaneCeiling::default(), &reply(v));
    assert_eq!(c.outcome, CompareOutcome::Mismatch);
}

#[test]
fn script_compare_classify_key_order_is_not_a_difference() {
    let r = rust();
    let raw: serde_json::Value = serde_json::from_str(&format!(
        r#"{{"condition":{},"proposed":"{}","rule_id":{}}}"#,
        serde_json::to_string(&r.condition).unwrap(),
        serde_json::to_value(r.proposed).unwrap().as_str().unwrap(),
        serde_json::to_string(&r.rule_id).unwrap(),
    ))
    .unwrap();
    let c = classify_lane_rules(&r, r.proposed, &LaneCeiling::default(), &reply(raw));
    assert_eq!(c.outcome, CompareOutcome::Match);
}

#[test]
fn script_compare_classify_invalid_outputs() {
    let r = rust();
    let long = "x".repeat(129);
    let cases = [
        serde_json::json!({"rule_id": "a", "proposed": "cheap"}),
        serde_json::json!({"rule_id": "a", "proposed": "cheap", "condition": "", "x": 1}),
        serde_json::json!({"rule_id": "a", "proposed": "Cheap", "condition": ""}),
        serde_json::json!({"rule_id": "", "proposed": "cheap", "condition": ""}),
        serde_json::json!({"rule_id": long, "proposed": "cheap", "condition": ""}),
        serde_json::json!({"rule_id": "a", "proposed": "cheap", "condition": "c".repeat(513)}),
        serde_json::json!("standard"),
    ];
    for raw in cases {
        let c = classify_lane_rules(&r, r.proposed, &LaneCeiling::default(), &reply(raw.clone()));
        assert_eq!(c.outcome, CompareOutcome::InvalidOutput, "{raw}");
        assert_eq!(c.error.unwrap().kind, "invalid_output");
        assert_eq!(c.ts, Some(raw));
    }
}

#[test]
fn script_compare_classify_errors_and_denied_safety_violation() {
    let r = rust();
    let ceiling = LaneCeiling::default();
    let c = classify_lane_rules(
        &r,
        r.proposed,
        &ceiling,
        &TsResult::DeadlineExceeded(SCRIPT_CALL_DEADLINE),
    );
    assert_eq!(c.outcome, CompareOutcome::TsError);
    assert_eq!(c.error.unwrap().kind, "timeout");
    let mut denied = PolicyScriptError::new("denied", "capability");
    denied.safety_violation = Some("denied_capability".into());
    let c = classify_lane_rules(&r, r.proposed, &ceiling, &TsResult::Error(denied));
    assert_eq!(c.outcome, CompareOutcome::TsError);
    assert_eq!(c.safety_violations, vec!["denied_capability".to_owned()]);
    let c = classify_lane_rules(
        &r,
        r.proposed,
        &ceiling,
        &TsResult::Error(PolicyScriptError::new("invalid_output", "undefined")),
    );
    assert_eq!(c.outcome, CompareOutcome::InvalidOutput);
    let long = "é".repeat(400);
    let c = classify_lane_rules(
        &r,
        r.proposed,
        &ceiling,
        &TsResult::Error(PolicyScriptError::new("exception", long)),
    );
    assert!(c.error.unwrap().message.len() <= 512);
}

#[test]
fn script_compare_draft_records_redacted_input_and_rust_output() {
    let draft = CompareDraft::lane_rules(
        "01GEN".into(),
        "ab".repeat(32),
        "1.0.0".into(),
        &features(),
        rust().proposed,
        &LaneCeiling::default(),
    );
    let rec = draft
        .finish(
            &reply(serde_json::to_value(rust()).unwrap()),
            Duration::from_micros(7),
        )
        .unwrap();
    assert_eq!(rec.domain, ScriptDomain::LaneRules);
    assert_eq!(rec.isolate_epoch, 3);
    assert_eq!(rec.elapsed_us, 7);
    assert_eq!(rec.outcome, CompareOutcome::Match);
    assert_eq!(rec.rust, serde_json::to_value(rust()).unwrap());
    assert!(rec.input_digest.starts_with("sha256:"));
    assert_eq!(
        rec.input,
        Some(serde_json::json!({ "features": serde_json::to_value(features()).unwrap() }))
    );
    let un = draft.unavailable("no policy script runtime").unwrap();
    assert_eq!(un.outcome, CompareOutcome::Unavailable);
    assert_eq!(un.input_digest, rec.input_digest);
}

struct Slow;

impl PolicyScript for Slow {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: "ab".repeat(32),
            host_api: "1.0.0".into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(PolicyReply {
                output: serde_json::Value::Null,
                isolate_epoch: 0,
            })
        })
    }
}

#[tokio::test(start_paused = true)]
async fn script_compare_call_stops_waiting_at_deadline() {
    let (result, elapsed) = call_lane_rules(
        &Slow,
        LaneRulesInput::redacted(&features()),
        SCRIPT_CALL_DEADLINE,
    )
    .await;
    assert_eq!(result, TsResult::DeadlineExceeded(SCRIPT_CALL_DEADLINE));
    assert_eq!(elapsed, SCRIPT_CALL_DEADLINE);
}
