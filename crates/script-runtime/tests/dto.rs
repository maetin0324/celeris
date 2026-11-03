//! Host API v1 の DTO の写像（境界 ADR 2026-11-02 (c)）と JSON Schema の生成物の一致。

use celeris_script::ScriptError;
use celeris_script::dto::{
    Capability, HostError, HostErrorCode, HostResult, SelftestEchoReply, SelftestEchoRequest,
    SelftestObservation, SelftestPriority, Timestamp, U64String, host_api_schema_json,
};
use serde_json::json;

fn request() -> serde_json::Value {
    json!({
        "task_id": "01M6DJ6QCMCWT80MTYJ7SNE0XK",
        "note": null,
        "tokens": "9007199254740993",
        "priority": "normal",
        "at": "2026-11-02T09:00:00Z",
    })
}

/// Option は null で常に出し、欄を省いたら拒否する。
#[test]
fn dto_null_required() {
    let req: SelftestEchoRequest = serde_json::from_value(request()).expect("valid");
    assert_eq!(req.note, None);
    let out = serde_json::to_value(&req).expect("serialize");
    assert_eq!(out["note"], serde_json::Value::Null);
    assert!(out.as_object().expect("object").contains_key("note"));

    let mut missing = request();
    missing.as_object_mut().expect("object").remove("note");
    let err = serde_json::from_value::<SelftestEchoRequest>(missing).expect_err("missing note");
    assert!(err.to_string().contains("missing field `note`"), "{err}");

    let reply = SelftestEchoReply {
        task_id: "t".into(),
        note: None,
        tokens: U64String(0),
        priority: SelftestPriority::Low,
        at: Timestamp::parse("2026-11-02T09:00:00Z").expect("time"),
        observation: SelftestObservation::Empty,
    };
    let v = serde_json::to_value(&reply).expect("serialize");
    assert_eq!(v["note"], serde_json::Value::Null);
    assert_eq!(v["observation"], json!({"kind": "empty"}));
    // schema でも required に入る。
    let schema: serde_json::Value =
        serde_json::from_str(&host_api_schema_json()).expect("schema json");
    let required = &schema["$defs"]["SelftestEchoRequest"]["required"];
    assert!(
        required
            .as_array()
            .expect("required")
            .contains(&json!("note")),
        "{required}"
    );
}

/// 2^53−1 を超えうる整数は 10 進の string。number や先頭 0 は拒否する。
#[test]
fn dto_bigint_as_string() {
    let req: SelftestEchoRequest = serde_json::from_value(request()).expect("valid");
    assert_eq!(req.tokens, U64String(9_007_199_254_740_993));
    assert_eq!(
        serde_json::to_value(U64String(u64::MAX)).expect("serialize"),
        json!("18446744073709551615")
    );
    for bad in [
        json!(9_007_199_254_740_993u64),
        json!("012"),
        json!("-1"),
        json!(""),
        json!("1e3"),
        json!("18446744073709551616"),
    ] {
        let mut v = request();
        v["tokens"] = bad.clone();
        assert!(
            serde_json::from_value::<SelftestEchoRequest>(v).is_err(),
            "{bad} must be rejected"
        );
    }
    // 時刻は RFC 3339 の UTC（末尾 Z）だけ。
    assert!(Timestamp::parse("2026-11-02T18:00:00+09:00").is_err());
    assert!(Timestamp::parse("2026-11-02 09:00:00").is_err());
    let mut v = request();
    v["at"] = json!("2026-11-02T09:00:00+00:00");
    assert!(serde_json::from_value::<SelftestEchoRequest>(v).is_err());
}

/// unit の enum は snake_case の文字列、データ付きは tag=kind。未知の値は拒否する。
#[test]
fn dto_unknown_enum_rejected() {
    let mut v = request();
    v["priority"] = json!("urgent");
    assert!(serde_json::from_value::<SelftestEchoRequest>(v).is_err());
    let mut v = request();
    v["priority"] = json!("High");
    assert!(serde_json::from_value::<SelftestEchoRequest>(v).is_err());

    assert!(serde_json::from_value::<HostErrorCode>(json!("teapot")).is_err());
    assert_eq!(
        serde_json::from_value::<HostErrorCode>(json!("generation_gone")).expect("code"),
        HostErrorCode::GenerationGone
    );
    assert!(serde_json::from_value::<Capability>(json!("read:anything")).is_err());
    assert_eq!(
        serde_json::to_value(Capability::SelftestEchoTask).expect("serialize"),
        json!("selftest:echo_task")
    );
    assert!(serde_json::from_value::<SelftestObservation>(json!({"kind": "exploded"})).is_err());
    assert_eq!(
        serde_json::from_value::<SelftestObservation>(json!({"kind": "noted", "length": 3}))
            .expect("noted"),
        SelftestObservation::Noted { length: 3 }
    );

    // HostError の code は ScriptError の種類へ 1 対 1 で写る。
    let cases = [
        (
            HostErrorCode::Exception,
            ScriptError::Exception {
                message: "m".into(),
            },
        ),
        (HostErrorCode::Timeout, ScriptError::Timeout),
        (HostErrorCode::Busy, ScriptError::Busy),
        (
            HostErrorCode::InvalidOutput,
            ScriptError::InvalidOutput { reason: "m".into() },
        ),
        (
            HostErrorCode::Denied,
            ScriptError::Denied { reason: "m".into() },
        ),
        (HostErrorCode::Terminated, ScriptError::Terminated),
        (HostErrorCode::GenerationGone, ScriptError::GenerationGone),
    ];
    for (code, expected) in cases {
        assert_eq!(ScriptError::from(HostError::new(code, "m")), expected);
    }
}

/// 知らない欄は拒否する（deny_unknown_fields）。
#[test]
fn dto_unknown_field_rejected() {
    let mut v = request();
    v["extra"] = json!(1);
    let err = serde_json::from_value::<SelftestEchoRequest>(v).expect_err("extra field");
    assert!(err.to_string().contains("unknown field `extra`"), "{err}");
    assert!(
        serde_json::from_value::<HostError>(json!({"code": "denied", "message": "x", "detail": 1}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<HostResult<SelftestEchoReply>>(
            json!({"kind": "error", "error": {"code": "denied", "message": "x"}, "extra": true})
        )
        .is_err()
    );
}

/// `script-schema` の出力と commit した生成物が byte 一致する。
#[test]
fn dto_schema_matches_committed() {
    let committed = include_str!("../../../docs/api/script-host/host-v1.schema.json");
    assert_eq!(
        host_api_schema_json(),
        committed,
        "regenerate: cargo run -q -p celeris-script --bin script-schema > docs/api/script-host/host-v1.schema.json"
    );
}

/// `cos_digest:read_run` の応答 DTO は task-core の `CosDigestInput`（正本）と同じ形（付記 S4-12）。
/// fixture の全行を両方の型で往復させ、直列化が byte 一致することを確かめる。
#[test]
fn cos_digest_input_matches_task_core() {
    use celeris_script::dto::CosDigestInput as HostInput;
    use task_core::chat::digest::CosDigestInput as CoreInput;

    let cases = include_str!("../../../control-ts/fixtures/cos_digest/cases.jsonl");
    let mut seen_decision = false;
    let mut n = 0usize;
    for line in cases.lines().filter(|l| !l.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line).expect("fixture row");
        let input = row["input"].clone();
        let host: HostInput = serde_json::from_value(input.clone()).expect("host DTO");
        let core: CoreInput = serde_json::from_value(input.clone()).expect("task-core DTO");
        let host_json = serde_json::to_string(&host).expect("host json");
        let core_json = serde_json::to_string(&core).expect("core json");
        assert_eq!(host_json, core_json, "{}", row["name"]);
        // 一方で直列化したものを他方で読める。
        let back: CoreInput = serde_json::from_str(&host_json).expect("host -> core");
        assert_eq!(back, core);
        seen_decision |= host.items.iter().any(|i| i.decision.is_some());
        n += 1;
    }
    assert!(n > 1000, "fixture rows {n}");
    assert!(seen_decision, "fixture must cover items with a decision");

    // 両方の schema の欄の名前が同じ。
    let host_schema: serde_json::Value =
        serde_json::from_str(&host_api_schema_json()).expect("schema");
    let core_schema = serde_json::to_value(schemars::schema_for!(CoreInput)).expect("core schema");
    let names = |v: &serde_json::Value| -> Vec<String> {
        let mut k: Vec<String> = v["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect();
        k.sort();
        k
    };
    assert_eq!(
        names(&host_schema["$defs"]["CosDigestInput"]),
        names(&core_schema)
    );
    for def in [
        "CosTriageItemReport",
        "CosTriageDecision",
        "CosTriageOption",
    ] {
        assert_eq!(
            names(&host_schema["$defs"][def]),
            names(&core_schema["$defs"][def]),
            "{def}"
        );
    }
}

/// Option の欄は schema でも null を許し、欄の省略は拒む（required に入る）。
#[test]
fn cos_digest_schema_nullable_and_required() {
    let schema: serde_json::Value =
        serde_json::from_str(&host_api_schema_json()).expect("schema json");
    let input = &schema["$defs"]["CosDigestInput"];
    assert_eq!(
        input["properties"]["reason"]["type"],
        json!(["string", "null"])
    );
    assert!(
        input["required"]
            .as_array()
            .expect("required")
            .contains(&json!("reason"))
    );
    assert_eq!(input["additionalProperties"], json!(false));
    let note = &schema["$defs"]["SelftestEchoRequest"]["properties"]["note"];
    assert_eq!(note["type"], json!(["string", "null"]));
    let caps = &schema["$defs"]["Capability"];
    let text = caps.to_string();
    for cap in ["cos_digest:list_runs", "cos_digest:read_run"] {
        assert!(text.contains(cap), "{text}");
    }
}
