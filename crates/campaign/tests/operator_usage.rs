use celeris_campaign::operator_usage::{Category, Mapping, convert};
use serde_json::{Value, json};
use std::{fs, process::Command};

fn response(session: &str, id: &str, second: u32) -> Value {
    json!({"type":"assistant", "sessionId":session,"timestamp":format!("2026-10-27T00:00:{second:02}Z"),
        "isSidechain":false, "message":{"id":id,"model":"claude-test", "content":[{"text":"secret"}],
        "usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":0}}})
}
fn lines(v: &[Value]) -> String {
    v.iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
fn mapping() -> Vec<Mapping> {
    serde_json::from_value(json!([
        {"session_id":"s","start":null,"end":"2026-10-27T00:00:10Z","category":"prep","trial_key":"trial-a"},
        {"session_id":"s","start":"2026-10-27T00:00:10Z","end":"2026-10-27T00:00:20Z","category":"observe","trial_key":"trial-a"},
        {"session_id":"s","start":"2026-10-27T00:00:20Z","end":"2026-10-27T00:00:30Z","category":"intervene","trial_key":"trial-b"},
        {"session_id":"s","start":"2026-10-27T00:00:30Z","end":"2026-10-27T00:00:40Z","category":"shared","trial_key":null}
    ])).unwrap()
}
#[test]
fn categories_boundaries_unmapped_and_missing_usage() {
    let mut input: Vec<_> = [0, 10, 20, 30, 40]
        .iter()
        .enumerate()
        .map(|(i, t)| response("s", &i.to_string(), *t))
        .collect();
    let mut sub = response("unknown", "sub", 15);
    sub["isSidechain"] = json!(true);
    sub["agentId"] = json!("agent-1");
    sub["message"].as_object_mut().unwrap().remove("usage");
    input.push(sub);
    input.push(json!({"type":"user", "message":{"content":"secret"}}));
    let rows = convert(&lines(&input), &mapping()).unwrap();
    assert_eq!(rows.len(), 6);
    for (i, category) in [
        Category::Prep,
        Category::Observe,
        Category::Intervene,
        Category::Shared,
        Category::SharedUnattributable,
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(&rows[i].category, category);
    }
    assert_eq!(rows[0].trial_key.as_deref(), Some("trial-a"));
    assert_eq!(rows[2].trial_key.as_deref(), Some("trial-b"));
    assert!(rows[3..].iter().all(|r| r.trial_key.is_none()));
    assert_eq!(
        (
            rows[0].input_tokens,
            rows[0].output_tokens,
            rows[0].cache_read_tokens,
            rows[0].cache_creation_tokens
        ),
        (Some(10), Some(20), Some(30), Some(0))
    );
    let sub = &rows[5];
    assert!(sub.is_subagent);
    assert_eq!(
        (
            sub.input_tokens,
            sub.output_tokens,
            sub.cache_read_tokens,
            sub.cache_creation_tokens
        ),
        (None, None, None, None)
    );
    let serialized = serde_json::to_value(sub).unwrap();
    assert!(serialized["output_tokens"].is_null());
    assert!(!serialized.to_string().contains("secret"));
}
#[test]
fn repeated_message_snapshots_are_not_added_and_crossing_is_unattributable() {
    let a = response("s", "one", 9);
    let mut b = response("s", "one", 10);
    b["message"]["usage"]["output_tokens"] = json!(50);
    let rows = convert(&lines(&[b.clone(), a, b]), &mapping()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].output_tokens, Some(50));
    assert_eq!(rows[0].input_tokens, Some(10));
    assert_eq!(rows[0].start.as_deref(), Some("2026-10-27T00:00:09Z"));
    assert_eq!(rows[0].end.as_deref(), Some("2026-10-27T00:00:10Z"));
    assert_eq!(rows[0].category, Category::SharedUnattributable);
}
#[test]
fn missing_time_and_partial_tokens_remain_null() {
    let mut a = response("s", "one", 9);
    a.as_object_mut().unwrap().remove("timestamp");
    a["message"]["usage"] = json!({"input_tokens":0});
    let rows = convert(&lines(&[a]), &mapping()).unwrap();
    assert_eq!(rows[0].category, Category::SharedUnattributable);
    assert_eq!(rows[0].start, None);
    assert_eq!(rows[0].end, None);
    assert_eq!(rows[0].input_tokens, Some(0));
    assert_eq!(rows[0].output_tokens, None);
}
#[test]
fn rejects_invalid_maps_and_transcripts() {
    let mut map = mapping();
    map[1].start = Some("2026-10-27T00:00:09Z".into());
    assert!(convert("", &map).unwrap_err().contains("overlapping"));
    let mut map = mapping();
    map[0].trial_key = None;
    assert!(convert("", &map).is_err());
    let mut map = mapping();
    map[0].end = Some("bad".into());
    assert!(convert("", &map).is_err());
    for (key, value) in [("input_tokens", json!(-1)), ("output_tokens", json!("20"))] {
        let mut a = response("s", "one", 9);
        a["message"]["usage"][key] = value;
        assert!(convert(&a.to_string(), &[]).is_err());
    }
    let mut a = response("s", "one", 9);
    a["message"]["id"] = Value::Null;
    assert!(convert(&a.to_string(), &[]).is_err());
    assert!(convert("{", &[]).is_err());
    let a = response("s", "one", 9);
    let mut b = a.clone();
    b["message"]["model"] = json!("other");
    assert!(convert(&lines(&[a, b]), &[]).is_err());
}
#[test]
fn cli_import_is_persistent_idempotent_and_conflicts_are_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("transcript.jsonl");
    let map = dir.path().join("map.json");
    let out = dir.path().join("operator-usage.jsonl");
    fs::write(&transcript, response("s", "one", 9).to_string()).unwrap();
    fs::write(&map, "[]").unwrap();
    let run = || {
        Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
            .arg("operator-usage")
            .arg("--transcript")
            .arg(&transcript)
            .arg("--session-map")
            .arg(&map)
            .output()
            .unwrap()
    };
    for added in [1, 0] {
        let result = run();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap()["added"],
            added
        );
    }
    let before = fs::read(&out).unwrap();
    assert_eq!(String::from_utf8_lossy(&before).lines().count(), 1);
    let mut changed = response("s", "one", 9);
    changed["message"]["usage"]["output_tokens"] = json!(99);
    fs::write(&transcript, lines(&[response("s", "new", 9), changed])).unwrap();
    assert!(!run().status.success());
    assert_eq!(before, fs::read(&out).unwrap());
    fs::write(
        &transcript,
        lines(&[response("s", "one", 9), response("other", "one", 9)]),
    )
    .unwrap();
    assert!(run().status.success());
    assert_eq!(fs::read_to_string(&out).unwrap().lines().count(), 2);
}

#[test]
fn concurrent_cli_imports_preserve_both_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.json");
    fs::write(&map, "[]").unwrap();
    let mut children = Vec::new();
    for session in ["a", "b"] {
        let path = dir.path().join(format!("{session}.jsonl"));
        fs::write(&path, response(session, "one", 9).to_string()).unwrap();
        children.push(
            Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
                .arg("operator-usage")
                .arg("--transcript")
                .arg(path)
                .arg("--session-map")
                .arg(&map)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("operator-usage.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn timestamps_compare_instants_and_map_schema_is_strict() {
    let mut a = response("s", "one", 10);
    a["timestamp"] = json!("2026-10-27T09:00:10+09:00");
    assert_eq!(
        convert(&a.to_string(), &mapping()).unwrap()[0].category,
        Category::Observe
    );
    let bad = json!([{"session_id":"s","start":null,"end":null,"category":"prep","trial_key":"a","typo":true}]);
    assert!(serde_json::from_value::<Vec<Mapping>>(bad).is_err());
}
