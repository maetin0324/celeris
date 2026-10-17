//! `celeris-campaign usage`: A のアカウントを含む 7 日使用率の確認と、腕ごとの tokens（A を含む）。
use celeris_campaign::usage::report;
use serde_json::json;

fn accounts(lab: Option<f64>) -> serde_json::Value {
    let usage = lab.map(
        |u| json!({"seven_day":{"utilization":u,"resets_at":"x"},"observed_at":"t","source":"run"}),
    );
    json!({"items":[
        {"source_id":"claude_oauth","account_id":"lab","usage":usage},
        {"source_id":"claude_oauth","account_id":"personal","usage":{"seven_day":{"utilization":0.37,"resets_at":"x"},"observed_at":"t","source":"run"}},
        {"source_id":"codex_oauth","account_id":"plus","usage":{"seven_day":{"utilization":0.0,"resets_at":"x"},"observed_at":"t","source":"run"}}
    ]})
}

fn summary() -> serde_json::Value {
    json!({"cells":{
        "A":{"tokens":{"input":28,"output":7637,"cache_read":441342,"cache_creation":38621,"usage_missing_trials":0}},
        "B":{"tokens":{"input":0,"output":0,"cache_read":0,"cache_creation":0,"usage_missing_trials":1}}}})
}

#[test]
fn reports_the_a_account_and_per_arm_tokens() {
    let (v, viol) = report(&accounts(Some(0.39)), &summary(), "lab", 0.45, 0.21);
    assert!(viol.is_empty(), "{viol:?}");
    assert_eq!(v["campaign_tokens_by_arm"]["A"]["tokens_all"], 487_628);
    let a = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["a_arm"] == true)
        .unwrap();
    assert_eq!(a["account_id"], "lab");
    assert_eq!(a["seven_day_utilization"], 0.39);
}

#[test]
fn over_the_limit_or_unobserved_a_usage_is_a_violation() {
    let (_, viol) = report(&accounts(Some(0.5)), &summary(), "lab", 0.45, 0.21);
    assert!(viol.iter().any(|x| x.contains("exceeds")), "{viol:?}");
    // A のアカウントの使用率が観測できないままでは A の消費を確認できない。
    let (_, viol) = report(&accounts(None), &summary(), "lab", 0.45, 0.21);
    assert!(viol.iter().any(|x| x.contains("not observed")), "{viol:?}");
    let (_, viol) = report(&accounts(Some(0.1)), &summary(), "ghost", 0.45, 0.21);
    assert!(viol.iter().any(|x| x.contains("not in GET")), "{viol:?}");
}
