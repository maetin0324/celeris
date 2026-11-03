use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::llm_route::{
    AccountId, ApiFormat, HarnessSpec, LlmRoute, ModelId, SourceId, SourceKind, SourceSpec,
};
use crate::model_policy::Level;

const SECRETS: &[&str] = &[
    "sk-live-SECRET-TOKEN",
    "https://user:pw@api.secret.example",
    "CRED_REF_SECRET",
    "account-secret-name",
    "proxy-secret-via",
    "other-harness-secret-model",
    "secret-command",
];

fn key(source: &str, model: &str) -> CandidateKey {
    CandidateKey {
        source: SourceId::from(source),
        model: ModelId::from(model),
    }
}

/// 機密になりうる値を `SourceSpec`・`HarnessSpec`・`llm_routes`・他の harness の行に入れた registry。
fn registry_with_secrets() -> Registry {
    let mut r = Registry::default();
    r.sources.insert(
        SourceId::from("anthropic"),
        SourceSpec {
            kind: SourceKind::from("api"),
            api_format: ApiFormat::from("anthropic"),
            accounts: vec![AccountId::from("account-secret-name")],
            models: vec![ModelId::from("m-std")],
            max_runs: Some(2),
            account_max_runs: BTreeMap::from([(AccountId::from("account-secret-name"), 1)]),
            quota: Some(0.5),
            endpoint: Some("https://user:pw@api.secret.example".into()),
            credential_ref: Some("CRED_REF_SECRET".into()),
        },
    );
    r.harnesses.insert(
        HarnessId::from("claude_code"),
        HarnessSpec {
            kind: "claude_code".into(),
            api_formats: BTreeSet::new(),
            source_kinds: BTreeSet::new(),
            capabilities: BTreeSet::new(),
            command: Some("secret-command".into()),
            args: vec!["sk-live-SECRET-TOKEN".into()],
        },
    );
    r.llm_routes.push(LlmRoute {
        harness: HarnessId::from("claude_code"),
        key: key("anthropic", "m-std"),
        via: Some("proxy-secret-via".into()),
        from_api: None,
        to_api: None,
    });
    r.route_defaults.insert(
        HarnessId::from("claude_code"),
        vec![key("anthropic", "m-std"), key("anthropic", "m-big")],
    );
    r.route_defaults.insert(
        HarnessId::from("codex"),
        vec![key("openai", "other-harness-secret-model")],
    );
    r.lane_route_defaults.insert(
        (HarnessId::from("claude_code"), "cheap".into()),
        vec![key("anthropic", "m-small")],
    );
    r.lane_route_defaults.insert(
        (HarnessId::from("codex"), "cheap".into()),
        vec![key("openai", "other-harness-secret-model")],
    );
    r.route_class_defaults.insert(
        HarnessId::from("claude_code"),
        BTreeMap::from([(
            ExecutionClass::from("coding"),
            vec![key("anthropic", "m-big")],
        )]),
    );
    r.cos_route_candidates = vec![key("anthropic", "m-cos")];
    r
}

#[test]
fn script_compare_route_input_keeps_only_allowlisted_ids() {
    let r = registry_with_secrets();
    let input = RoutePriorityInput::redacted(
        &r,
        &HarnessId::from("claude_code"),
        Some("cheap"),
        false,
        Some(&ExecutionClass::from("coding")),
    );
    let json = serde_json::to_string(&input).unwrap();
    for secret in SECRETS {
        assert!(!json.contains(secret), "{secret} leaked into {json}");
    }
    let recorded = recorded_input(&input).unwrap();
    let recorded_json = serde_json::to_string(&recorded.input).unwrap();
    for secret in SECRETS {
        assert!(!recorded_json.contains(secret), "{secret} leaked");
    }
    assert_eq!(
        json,
        concat!(
            r#"{"harness":"claude_code","lane":"cheap","cos":false,"execution_class":"coding","#,
            r#""table":{"route_defaults":[{"source":"anthropic","model":"m-std"},{"source":"anthropic","model":"m-big"}],"#,
            r#""lane_route_defaults":[{"lane":"cheap","candidates":[{"source":"anthropic","model":"m-small"}]}],"#,
            r#""class_defaults":[{"execution_class":"coding","candidates":[{"source":"anthropic","model":"m-big"}]}],"#,
            r#""cos_route_candidates":[{"source":"anthropic","model":"m-cos"}]}}"#
        )
    );
}

#[test]
fn script_compare_lane_input_is_nine_levels_only() {
    let features = TaskFeatures {
        judgment: Level::High,
        ambiguity: Level::Low,
        verifiability: Level::High,
        reversibility: Level::Medium,
        consequence: Level::Low,
        context_size: Level::Medium,
        tool_intensity: Level::High,
        expected_length: Level::Low,
        cross_cutting: Level::Medium,
    };
    let input = LaneRulesInput::redacted(&features);
    let value = serde_json::to_value(&input).unwrap();
    let obj = value.as_object().unwrap();
    assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["features"]);
    let axes = obj["features"].as_object().unwrap();
    assert_eq!(axes.len(), 9);
    for v in axes.values() {
        assert!(matches!(v.as_str(), Some("low" | "medium" | "high")), "{v}");
    }
    // 入力 DTO は未知の欄を受けない（機密を足した DTO を TS 側から戻されても読まない）。
    let mut extra = value.clone();
    extra["token"] = serde_json::json!("sk-live-SECRET-TOKEN");
    assert!(serde_json::from_value::<LaneRulesInput>(extra).is_err());
}

#[test]
fn script_compare_recorded_input_digest_and_truncation() {
    let r = registry_with_secrets();
    let input = RoutePriorityInput::redacted(&r, &HarnessId::from("claude_code"), None, true, None);
    let a = recorded_input(&input).unwrap();
    let b = recorded_input(&input).unwrap();
    assert_eq!(a, b);
    assert!(a.input_digest.starts_with("sha256:"));
    assert_eq!(a.input_digest.len(), 7 + 64);
    assert!(!a.input_truncated);
    assert_eq!(
        a.input_digest,
        sha256_digest(&serde_json::to_vec(&input).unwrap())
    );

    // 64 KiB を超えたら値を捨てて digest だけを残す。
    let mut big = Registry::default();
    big.route_defaults.insert(
        HarnessId::from("h"),
        (0..2000)
            .map(|i| key("s", &format!("model-{i:08}")))
            .collect(),
    );
    let big_input = RoutePriorityInput::redacted(&big, &HarnessId::from("h"), None, false, None);
    let rec = recorded_input(&big_input).unwrap();
    assert!(serde_json::to_vec(&big_input).unwrap().len() > RECORDED_INPUT_MAX);
    assert!(rec.input_truncated);
    assert!(rec.input.is_none());
    assert!(rec.input_digest.starts_with("sha256:"));
}

#[test]
fn script_compare_error_message_cut_at_512_bytes_on_char_boundary() {
    let short = CompareError::new("exception", "boom");
    assert_eq!(short.message, "boom");
    let long = "あ".repeat(400); // 1200 byte
    let cut = truncate_error_message(&long);
    assert!(cut.len() <= ERROR_MESSAGE_MAX);
    assert!(cut.len() > ERROR_MESSAGE_MAX - 3);
    assert!(long.starts_with(&cut));
}

#[test]
fn script_domain_names_and_modes_parse() {
    assert_eq!(
        ScriptDomain::parse("lane_rules"),
        Some(ScriptDomain::LaneRules)
    );
    assert_eq!(
        ScriptDomain::parse("route_priority"),
        Some(ScriptDomain::RoutePriority)
    );
    assert_eq!(ScriptDomain::parse("routing_shadow"), None);
    assert_eq!(ScriptDomain::parse("direct_route"), None);
    for m in ["rust", "compare", "canary", "ts"] {
        assert_eq!(ScriptMode::parse(m).map(ScriptMode::as_str), Some(m));
    }
    assert!(super::actor_may_enable_ts("human:web"));
    for actor in [
        "human:",
        "cos:t",
        "operator:claude-code",
        "mcp:c",
        "system:script-startup",
        "",
    ] {
        assert!(!super::actor_may_enable_ts(actor), "{actor}");
    }
}
