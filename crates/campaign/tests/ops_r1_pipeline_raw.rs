use celeris_campaign::ops_r1_a_raw::{self, Metadata, Stop};
use serde_json::{Value, json};

fn metadata() -> Metadata {
    serde_json::from_value(json!({
        "trial_id":"trial-A", "condition_id":"condition-A", "session_id":"session-A",
        "started_at":"2026-10-30T00:00:00Z", "ended_at":"2026-10-30T00:01:00Z",
        "stop":"exited", "exit_code":0,
        "claude_code":{"version":"stub-1", "sha256":"a".repeat(64)},
        "workspace_sha256_initial":"b".repeat(64), "cost_usd":0.25
    }))
    .unwrap()
}

fn message(id: &str, session: &str, model: &str, output: u64) -> Value {
    json!({"type":"assistant", "sessionId":session, "timestamp":"2026-10-30T00:00:30Z",
        "message":{"id":id, "model":model, "usage":{"input_tokens":10,
        "output_tokens":output, "cache_read_input_tokens":30, "cache_creation_input_tokens":40}}})
}

#[test]
fn ops_r1_pipeline_raw_deduplicates_usage_and_preserves_stops() {
    let transcript = [
        message("m1", "session-A", "sonnet", 1),
        message("m1", "session-A", "sonnet", 2),
        message("m2", "session-A", "opus", 3),
        message("m1", "other-session", "other", 999),
        json!({"type":"user", "message":"ignored"}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    for stop in [
        Stop::Exited,
        Stop::Cutoff,
        Stop::TokenCap,
        Stop::OperatorStop,
    ] {
        let mut m = metadata();
        m.stop = stop;
        if stop != Stop::Exited {
            m.exit_code = None;
        }
        let raw = ops_r1_a_raw::build(&transcript, m.clone()).unwrap();
        assert_eq!(raw.metadata.stop, stop);
        assert_eq!(raw.token_usage.input, Some(20));
        assert_eq!(raw.token_usage.output, Some(5));
        assert_eq!(raw.token_usage.cache_read, Some(60));
        assert_eq!(raw.token_usage.cache_creation, Some(80));
        assert_eq!(raw.models, ["opus", "sonnet"]);
        assert_eq!(raw.metadata.cost_usd, Some(0.25));
        let bytes = raw.render().unwrap();
        assert_eq!(
            bytes,
            ops_r1_a_raw::build(&transcript, m)
                .unwrap()
                .render()
                .unwrap()
        );
        let decoded: ops_r1_a_raw::Raw = serde_json::from_str(&bytes).unwrap();
        assert_eq!(decoded, raw);
        let v: Value = serde_json::from_str(&bytes).unwrap();
        assert_eq!(v["schema"], "ops-r1-a-raw/1");
        assert_eq!(v["session_id"], "session-A");
        assert_eq!(v["stop"], serde_json::to_value(stop).unwrap());
    }
}

#[test]
fn ops_r1_pipeline_raw_missing_is_null_and_bad_evidence_rejected() {
    let empty = ops_r1_a_raw::build("", metadata()).unwrap();
    assert_eq!(empty.token_usage.input, None);
    let mut msg = message("m1", "session-A", "sonnet", 2);
    msg["message"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("cache_read_input_tokens");
    let raw = ops_r1_a_raw::build(&msg.to_string(), metadata()).unwrap();
    assert_eq!(raw.token_usage.cache_read, None);
    assert_eq!(raw.token_usage.input, Some(10));
    for key in [
        "trial_id",
        "condition_id",
        "session_id",
        "workspace_sha256_initial",
        "started_at",
    ] {
        let mut v = serde_json::to_value(metadata()).unwrap();
        v[key] = json!("");
        assert!(ops_r1_a_raw::build("", serde_json::from_value(v).unwrap()).is_err());
    }
    let mut m = metadata();
    m.exit_code = None;
    assert!(ops_r1_a_raw::build("", m).is_err());
    let mut m = metadata();
    m.ended_at = "2026-10-29T00:00:00Z".into();
    assert!(ops_r1_a_raw::build("", m).is_err());
    let mut m = metadata();
    m.cost_usd = Some(-1.0);
    assert!(ops_r1_a_raw::build("", m).is_err());
    assert!(ops_r1_a_raw::build("not json", metadata()).is_err());
    let bad = message("m1", "session-A", "sonnet", u64::MAX).to_string()
        + "\n"
        + &message("m2", "session-A", "sonnet", 1).to_string();
    assert!(ops_r1_a_raw::build(&bad, metadata()).is_err());
}

#[test]
fn ops_r1_pipeline_raw_cli_writes_only_output_and_is_repeatable() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    let meta = dir.path().join("metadata.json");
    std::fs::write(
        &transcript,
        message("m1", "session-A", "sonnet", 2).to_string(),
    )
    .unwrap();
    std::fs::write(&meta, serde_json::to_string(&metadata()).unwrap()).unwrap();
    let before = std::fs::read(&transcript).unwrap();
    let mut outputs = Vec::new();
    for name in ["one.json", "two.json"] {
        let out = dir.path().join(name);
        let args = [
            "ops-r1-a-raw".into(),
            "--transcript".into(),
            transcript.display().to_string(),
            "--metadata".into(),
            meta.display().to_string(),
            "--out".into(),
            out.display().to_string(),
        ];
        assert!(ops_r1_a_raw::cli(&args).is_ok());
        outputs.push(std::fs::read(out).unwrap());
        let mut bad = args.clone();
        bad[6] = transcript.display().to_string();
        assert!(ops_r1_a_raw::cli(&bad).is_err());
    }
    assert_eq!(outputs[0], outputs[1]);
    assert_eq!(before, std::fs::read(transcript).unwrap());
}
