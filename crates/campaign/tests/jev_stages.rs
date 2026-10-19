//! Jev 段階別集計の fixture 試験（実 API は呼ばない）。
//! CLI 呼び出し形: `celeris-campaign jev-stages --records <json>`。
use celeris_campaign::jev_stages::run_json;
use serde_json::{Value, json};
use std::process::Command;

fn ev(
    status: Value,
    base: &str,
    proposed: Option<&str>,
    fin: &str,
    basis: &str,
    ov: Option<&str>,
) -> Value {
    let mut la = json!({"mode":"decision_assisted","classifier_status":status,"real_api_verified":false,
        "base_lane":base,"final_lane":fin,"basis":basis});
    if let Some(p) = proposed {
        la["proposed_lane"] = json!(p);
    }
    if let Some(o) = ov {
        la["overridden_by"] = json!(o);
    }
    json!({"type":"routing_decided","record":{"lane_assist":la}})
}

fn rec(arm: &str, event: Value, model: (&str, &str), result: &str, cost: f64) -> Value {
    json!({"arm":arm,"task_id":format!("t-{arm}-{result}-{model:?}"),"event":event,
        "version":"jev-1.14","jev_usage":[{"cost_usd":0.002}],"confidence":0.99,
        "run":{"proposed_model":model.0,"exec_model":model.1},
        "outcome":{"result":result,"retries":1,"cost_usd":cost}})
}

fn full() -> Value {
    let ok = || json!({"status":"jev_ok"});
    json!([
        // Jev が引き上げ、model も変わる
        rec(
            "C",
            ev(
                ok(),
                "standard",
                Some("frontier"),
                "frontier",
                "classification_raise",
                None
            ),
            ("sonnet", "opus"),
            "pass",
            1.0
        ),
        // 提案は frontier だが天井で止まる
        rec(
            "C",
            ev(
                ok(),
                "standard",
                Some("frontier"),
                "standard",
                "classification_raise",
                Some("ceiling")
            ),
            ("sonnet", "sonnet"),
            "fail",
            0.5
        ),
        // 明示 lane
        rec(
            "C",
            ev(
                ok(),
                "standard",
                Some("frontier"),
                "standard",
                "fixed_fallback",
                Some("explicit")
            ),
            ("sonnet", "sonnet"),
            "pass",
            0.5
        ),
        // 分類 fallback
        rec(
            "C",
            ev(
                json!({"status":"fell_back_to_fixed","reason":"timeout"}),
                "standard",
                None,
                "standard",
                "fixed_fallback",
                None
            ),
            ("sonnet", "sonnet"),
            "pass",
            0.4
        ),
        // 未試行
        rec(
            "C",
            ev(
                json!({"status":"not_attempted"}),
                "standard",
                None,
                "standard",
                "fixed_fallback",
                None
            ),
            ("sonnet", "sonnet"),
            "pass",
            0.4
        ),
        rec(
            "B",
            json!({"type":"routing_decided","record":{}}),
            ("sonnet", "sonnet"),
            "pass",
            0.3
        ),
    ])
}

#[test]
fn every_stage_is_reported_on_a_full_fixture() {
    let r = run_json(&full()).expect("agg");
    let c = &r.by_arm["C"];
    assert_eq!(c.trials, 5);
    assert_eq!(c.classification_attempted, 4);
    assert_eq!((c.jev_ok, c.jev_fallback, c.not_attempted), (3, 1, 1));
    assert_eq!(c.lane_proposal_changed, 3);
    assert_eq!(c.final_lane_changed, 1);
    assert_eq!(c.final_lane_counts["standard"], 4);
    assert_eq!(c.final_lane_counts["frontier"], 1);
    assert_eq!((c.model_changed, c.lane_changed_and_model_changed), (1, 1));
    assert_eq!(c.suppressed.ceiling, 1);
    assert_eq!(c.suppressed.explicit, 1);
    assert_eq!(c.suppressed.classifier_fallback_reasons["timeout"], 1);
    assert_eq!(c.suppressed.not_attempted, 1);
    assert_eq!(c.results["pass"], 4);
    assert_eq!(c.results["fail"], 1);
    assert_eq!(c.retries, 5);
    assert!((c.total_cost_usd - 2.8).abs() < 1e-9);
    assert_eq!(c.jev_calls, 5);
    // B は lane_assist 欄が無い: missing として数える（fallback と混ぜない）
    assert_eq!(r.by_arm["B"].missing, 1);
    assert_eq!(r.total.trials, 6);
}

#[test]
fn zero_change_fixture_stays_zero() {
    let ok = json!({"status":"jev_ok"});
    let recs = json!([
        rec(
            "C",
            ev(
                ok.clone(),
                "standard",
                Some("standard"),
                "standard",
                "fixed_rule",
                None
            ),
            ("sonnet", "sonnet"),
            "pass",
            0.5
        ),
        rec(
            "C",
            ev(
                ok,
                "standard",
                Some("standard"),
                "standard",
                "fixed_rule",
                None
            ),
            ("sonnet", "sonnet"),
            "pass",
            0.5
        ),
    ]);
    let c = &run_json(&recs).expect("agg").by_arm["C"];
    assert_eq!(c.jev_ok, 2);
    assert_eq!(c.lane_proposal_changed, 0);
    assert_eq!(c.final_lane_changed, 0);
    assert_eq!(c.model_changed, 0);
    assert_eq!(c.model_unknown, 0);
    assert_eq!(
        c.suppressed.explicit
            + c.suppressed.ceiling
            + c.suppressed.availability
            + c.suppressed.run_fallback
            + c.suppressed.budget,
        0
    );
    // JSON でも 0 のまま出る（欄が消えない）
    let j = serde_json::to_value(c).expect("ser");
    assert_eq!(j["model_changed"], 0);
    assert_eq!(j["suppressed"]["ceiling"], 0);
    assert_eq!(j["lane_proposal_changed"], 0);
}

#[test]
fn confidence_is_ignored() {
    let mut a = full();
    for r in a.as_array_mut().expect("arr") {
        r["confidence"] = json!(0.01);
    }
    let x = serde_json::to_value(run_json(&a).expect("a")).expect("ser");
    let y = serde_json::to_value(run_json(&full()).expect("b")).expect("ser");
    assert_eq!(x, y);
}

#[test]
fn missing_cost_and_models_stay_missing() {
    let ok = json!({"status":"jev_ok"});
    let recs = json!([{"arm":"C","task_id":"x","event":ev(ok, "standard", Some("standard"), "standard", "fixed_rule", None),
        "jev_usage":[{"cost_usd":null}]}]);
    let c = &run_json(&recs).expect("agg").by_arm["C"];
    assert_eq!(
        (
            c.jev_cost_missing,
            c.version_missing,
            c.model_unknown,
            c.result_missing,
            c.total_cost_missing
        ),
        (1, 1, 1, 1, 1)
    );
    assert_eq!(c.chain_ok, 0);
}

#[test]
fn cli_prints_stage_json() {
    let dir = tempfile::tempdir().expect("dir");
    let p = dir.path().join("r.json");
    std::fs::write(&p, full().to_string()).expect("w");
    let out = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
        .args(["jev-stages", "--records"])
        .arg(&p)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["by_arm"]["C"]["jev_ok"], 3);
    assert_eq!(v["total"]["trials"], 6);
}
