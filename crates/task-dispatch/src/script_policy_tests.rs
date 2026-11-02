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

    fn route_priority(&self, _input: RoutePriorityInput, deadline: Duration) -> PolicyFuture<'_> {
        self.lane_rules(LaneRulesInput::redacted(&features()), deadline)
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

fn key(source: &str, model: &str) -> task_core::llm_route::CandidateKey {
    task_core::llm_route::CandidateKey {
        source: source.into(),
        model: model.into(),
    }
}

/// harness `h` の lane `cheap` の列 [a/m, b/m]、harness 全体の列 [c/m]、CoS の列 [d/m]。
fn route_registry() -> Registry {
    let mut r = Registry::default();
    r.route_defaults.insert("h".into(), vec![key("c", "m")]);
    r.lane_route_defaults.insert(
        ("h".into(), "cheap".to_owned()),
        vec![key("a", "m"), key("b", "m")],
    );
    r.route_defaults.insert("other".into(), vec![key("z", "m")]);
    r.cos_route_candidates = vec![key("d", "m")];
    r
}

fn route_draft(lane: Option<&str>) -> CompareDraft {
    CompareDraft::route_priority(
        "01GEN".into(),
        "ab".repeat(32),
        "1.0.0".into(),
        Some("01RUN".into()),
        &route_registry(),
        &"h".into(),
        lane,
        false,
        None,
    )
}

fn route_parts(draft: &CompareDraft) -> (RoutePriorityOutput, RoutePriorityInput) {
    match &draft.domain {
        DraftDomain::RoutePriority { input, rust } => (rust.clone(), input.clone()),
        DraftDomain::LaneRules { .. } => panic!("not route_priority"),
    }
}

#[test]
fn script_compare_route_priority_classify_match_mismatch_and_undeclared() {
    let (rust, input) = route_parts(&route_draft(Some("cheap")));
    assert_eq!(
        serde_json::to_value(&rust).unwrap(),
        serde_json::json!({"basis": "lane", "candidates": [
            {"source": "a", "model": "m"}, {"source": "b", "model": "m"}]})
    );
    let c = classify_route_priority(&rust, &input, &reply(serde_json::to_value(&rust).unwrap()));
    assert_eq!(c.outcome, CompareOutcome::Match);
    assert!(c.safety_violations.is_empty() && c.error.is_none());
    // 順序だけ違う・basis だけ違う・他の列の宣言済み候補は mismatch で、安全違反ではない。
    for ts in [
        serde_json::json!({"basis": "lane", "candidates": [
            {"source": "b", "model": "m"}, {"source": "a", "model": "m"}]}),
        serde_json::json!({"basis": "harness", "candidates": [
            {"source": "a", "model": "m"}, {"source": "b", "model": "m"}]}),
        serde_json::json!({"basis": "cos", "candidates": [{"source": "d", "model": "m"}]}),
        serde_json::json!({"basis": "none", "candidates": []}),
    ] {
        let c = classify_route_priority(&rust, &input, &reply(ts.clone()));
        assert_eq!(c.outcome, CompareOutcome::Mismatch, "{ts}");
        assert!(c.safety_violations.is_empty(), "{ts}");
        assert_eq!(c.ts, Some(ts));
    }
    // 他の harness の列（入力に無い）や存在しない候補は undeclared_candidate。
    for ts in [
        serde_json::json!({"basis": "harness", "candidates": [{"source": "z", "model": "m"}]}),
        serde_json::json!({"basis": "lane", "candidates": [
            {"source": "a", "model": "m"}, {"source": "a", "model": "other"}]}),
    ] {
        let c = classify_route_priority(&rust, &input, &reply(ts.clone()));
        assert_eq!(c.outcome, CompareOutcome::Mismatch, "{ts}");
        assert_eq!(c.safety_violations, vec!["undeclared_candidate".to_owned()]);
    }
}

#[test]
fn script_compare_route_priority_classify_invalid_and_errors() {
    let (rust, input) = route_parts(&route_draft(Some("cheap")));
    let many: Vec<serde_json::Value> = (0..=ROUTE_PRIORITY_CANDIDATES_MAX)
        .map(|_| serde_json::json!({"source": "a", "model": "m"}))
        .collect();
    let at_max = serde_json::json!({"basis": "lane", "candidates": many[1..].to_vec()});
    assert_eq!(
        classify_route_priority(&rust, &input, &reply(at_max)).outcome,
        CompareOutcome::Mismatch
    );
    for raw in [
        serde_json::json!({"basis": "lane", "candidates": many}),
        serde_json::json!({"basis": "unknown", "candidates": []}),
        serde_json::json!({"basis": "lane"}),
        serde_json::json!({"basis": "lane", "candidates": [], "extra": 1}),
        serde_json::json!({"basis": "lane", "candidates": [{"source": "a"}]}),
        serde_json::Value::Null,
    ] {
        let c = classify_route_priority(&rust, &input, &reply(raw.clone()));
        assert_eq!(c.outcome, CompareOutcome::InvalidOutput, "{raw}");
        assert_eq!(c.error.as_ref().unwrap().kind, "invalid_output");
        assert!(c.safety_violations.is_empty());
    }
    let mut denied = PolicyScriptError::new("denied", "capability");
    denied.safety_violation = Some("denied_capability".into());
    let c = classify_route_priority(&rust, &input, &TsResult::Error(denied));
    assert_eq!(c.outcome, CompareOutcome::TsError);
    assert_eq!(c.safety_violations, vec!["denied_capability".to_owned()]);
    let c = classify_route_priority(
        &rust,
        &input,
        &TsResult::DeadlineExceeded(SCRIPT_CALL_DEADLINE),
    );
    assert_eq!(c.outcome, CompareOutcome::TsError);
    assert_eq!(c.error.unwrap().kind, "timeout");
}

#[test]
fn script_compare_route_priority_draft_records_run_and_table_of_one_harness() {
    let draft = route_draft(None);
    let (rust, _) = route_parts(&draft);
    assert_eq!(
        serde_json::to_value(&rust).unwrap(),
        serde_json::json!({"basis": "harness", "candidates": [{"source": "c", "model": "m"}]})
    );
    let rec = draft
        .finish(
            &reply(serde_json::to_value(&rust).unwrap()),
            Duration::from_micros(5),
        )
        .unwrap();
    assert_eq!(rec.domain, ScriptDomain::RoutePriority);
    assert_eq!(rec.call_site, ROUTE_PRIORITY_CALL_SITE);
    assert_eq!(rec.run_id.as_deref(), Some("01RUN"));
    assert_eq!(rec.outcome, CompareOutcome::Match);
    let input = rec.input.unwrap();
    assert_eq!(input["harness"], "h");
    assert_eq!(input["lane"], serde_json::Value::Null);
    assert_eq!(input["cos"], false);
    // 他の harness（`other`）の列は入力に写さない（S2-3）。
    assert!(!input.to_string().contains("\"z\""), "{input}");
    let un = draft.unavailable("no policy script runtime").unwrap();
    assert_eq!(un.outcome, CompareOutcome::Unavailable);
    assert_eq!(un.call_site, ROUTE_PRIORITY_CALL_SITE);
}
