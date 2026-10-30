//! ops-r2 の実験専用 operator session（ADR 2026-10-30-ops-r2-isolation-principal D7）。
//! 開始前の session-map 準備（ops-r2-session-prep.sh）→ 介入で分けた最終の対応表（ops-r2-session-map.py final）→
//! 親と subagent の transcript の取り込み（operator_usage::import、schema は今のまま）→ 消費の集計（summary）を
//! 通して、再取り込みで合計が変わらないこと・区間外は shared_unattributable・欠けた消費は null と理由を確かめる。
//! LLM・Claude Code・daemon は起動しない（Claude Code・celeris-campaign・celerisctl は stub の実行 file）。
use celeris_campaign::operator_usage::{Category, Mapping, UsageRow, import};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SESSION: &str = "0b8c1a2e-1111-4222-8333-944455556666";
const TRIAL_A: &str = "campaign-aaaa";
const TRIAL_B: &str = "campaign-bbbb";

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign")
}

fn ok(o: &Output, what: &str) {
    assert!(
        o.status.success(),
        "{what}: {}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn stub(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, "#!/bin/sh\nexit 0\n").expect("stub");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod");
    p
}

fn session_map(args: &[&str]) -> Output {
    Command::new("python3")
        .arg("-I")
        .arg(scripts().join("ops-r2-session-map.py"))
        .args(args)
        .output()
        .expect("python3")
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

/// 試行 A（prep 00:00–00:10、observe 00:10–00:30）と B（observe 00:40–00:50）の session を準備する。
fn prepare(root: &Path) -> PathBuf {
    let tools = root.join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let account = root.join("account");
    fs::create_dir_all(&account).expect("account");
    fs::write(account.join(".credentials.json"), "{}").expect("credentials");
    let token = root.join("token");
    fs::write(&token, "operator-token\n").expect("token");
    let trials = root.join("trials.json");
    fs::write(
        &trials,
        json!({"trials": [
            {"trial_key": TRIAL_A, "condition_id": "B-direct", "request_ref": "chat-message:req-a",
             "prep": {"start": "2026-10-30T00:00:00Z", "end": "2026-10-30T00:10:00Z"},
             "observe": {"start": "2026-10-30T00:10:00Z", "end": "2026-10-30T00:30:00Z"}},
            {"trial_key": TRIAL_B, "condition_id": "B-task", "request_ref": "chat-message:req-b",
             "observe": {"start": "2026-10-30T00:40:00Z", "end": "2026-10-30T00:50:00Z"}}
        ]})
        .to_string(),
    )
    .expect("trials");
    let evidence = root.join("evidence");
    let o = Command::new("sh")
        .arg(scripts().join("ops-r2-session-prep.sh"))
        .args(["--evidence", s(&evidence), "--trials", s(&trials)])
        .args(["--claude-version", "2.1.303", "--model", "claude-test"])
        .args(["--account-dir", s(&account), "--policy-version", "9"])
        .args(["--claude-bin", s(&stub(&tools, "claude"))])
        .args(["--campaign-bin", s(&stub(&tools, "celeris-campaign"))])
        .args(["--celerisctl", s(&stub(&tools, "celerisctl"))])
        .args(["--token-file", s(&token), "--api-url", "http://127.0.0.1:1"])
        .args(["--session-id", SESSION, "--now", "2026-10-29T23:00:00Z"])
        .output()
        .expect("sh");
    ok(&o, "ops-r2-session-prep.sh");
    evidence
}

fn op(record_type: &str, trial: Option<&str>, logical: &str, at: &str) -> Value {
    json!({"record_type": record_type, "trial_key": trial, "logical_op_id": logical, "attempt": 1,
        "op_kind": "system_recovery", "condition_id": trial.map(|t| if t == TRIAL_A { "B-direct" } else { "B-task" }),
        "actor": {"kind": "external_operator", "id": SESSION},
        "start": if record_type == "op_start" { Some(at) } else { None },
        "end": if record_type == "op_end" { Some(at) } else { None }})
}

/// 試行 A の observe の中の介入 00:15–00:20 と、人の op（数えない）。
fn human_ops_list(evidence: &Path, extra: &[Value]) -> PathBuf {
    let mut records = vec![
        op(
            "op_start",
            Some(TRIAL_A),
            "recover-1",
            "2026-10-30T00:15:00Z",
        ),
        op("op_end", Some(TRIAL_A), "recover-1", "2026-10-30T00:20:00Z"),
        json!({"record_type": "op_start", "trial_key": TRIAL_A, "logical_op_id": "h", "attempt": 1,
            "op_kind": "status_check", "actor": {"kind": "human", "id": "rmaeda"},
            "start": "2026-10-30T00:11:00Z", "end": null}),
    ];
    records.extend_from_slice(extra);
    let p = evidence.join("human-ops-list.json");
    fs::write(
        &p,
        json!({"records": records, "ops": [], "redo_counts": []}).to_string(),
    )
    .expect("list");
    p
}

fn finalize(evidence: &Path, list: &Path, out: &Path) -> Output {
    session_map(&[
        "final",
        "--base",
        s(&evidence.join("session-map.base.json")),
        "--trials",
        s(&evidence.join("session-trials.json")),
        "--human-ops-list",
        s(list),
        "--out",
        s(out),
    ])
}

fn response(id: &str, at: &str, agent: Option<&str>, usage: Option<(u64, u64)>) -> Value {
    let mut v = json!({"type": "assistant", "sessionId": SESSION, "timestamp": at,
        "isSidechain": agent.is_some(), "message": {"id": id, "model": "claude-test",
        "content": [{"type": "text", "text": "secret"}]}});
    if let Some(a) = agent {
        v["agentId"] = json!(a);
    }
    if let Some((i, o)) = usage {
        v["message"]["usage"] = json!({"input_tokens": i, "output_tokens": o,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0});
    }
    v
}

fn jsonl(rows: &[Value]) -> String {
    rows.iter().map(|r| format!("{r}\n")).collect()
}

/// snapshot: 親（prep・observe・intervene・区間外）と subagent 2 本（1 本は usage 欠け）。
fn snapshot(evidence: &Path) -> PathBuf {
    let dest = evidence.join("operator").join(SESSION).join("end");
    fs::create_dir_all(dest.join("subagents")).expect("snapshot");
    let user = json!({"type": "user", "sessionId": SESSION, "message": {"content": "secret"}});
    fs::write(
        dest.join("main.jsonl"),
        jsonl(&[
            user,
            response("m-prep", "2026-10-30T00:05:00Z", None, Some((100, 10))),
            response("m-obs", "2026-10-30T00:12:00Z", None, Some((200, 20))),
            // 同じ message の累積 snapshot。最大値を 1 回だけ数える。
            response("m-obs", "2026-10-30T00:12:01Z", None, Some((200, 25))),
            response("m-int", "2026-10-30T00:16:00Z", None, Some((300, 30))),
            response("m-b", "2026-10-30T00:45:00Z", None, Some((400, 40))),
            // どの区間にも入らない（試行の間の 00:35）。均等配分しない。
            response("m-gap", "2026-10-30T00:35:00Z", None, Some((500, 50))),
        ]),
    )
    .expect("main");
    fs::write(
        dest.join("subagents/agent-a1.jsonl"),
        jsonl(&[response(
            "s-int",
            "2026-10-30T00:17:00Z",
            Some("a1"),
            Some((1000, 100)),
        )]),
    )
    .expect("sub a1");
    fs::write(
        dest.join("subagents/agent-a2.jsonl"),
        jsonl(&[response(
            "s-nousage",
            "2026-10-30T00:18:00Z",
            Some("a2"),
            None,
        )]),
    )
    .expect("sub a2");
    dest
}

fn transcripts(dest: &Path) -> Vec<PathBuf> {
    let mut v = vec![dest.join("main.jsonl")];
    let mut subs: Vec<_> = fs::read_dir(dest.join("subagents"))
        .expect("subagents")
        .map(|e| e.expect("entry").path())
        .collect();
    subs.sort();
    v.extend(subs);
    v
}

fn rows(out: &Path) -> Vec<UsageRow> {
    fs::read_to_string(out)
        .expect("usage")
        .lines()
        .map(|l| serde_json::from_str(l).expect("row"))
        .collect()
}

fn total(rows: &[UsageRow]) -> (u64, u64) {
    rows.iter().fold((0, 0), |(i, o), r| {
        (
            i + r.input_tokens.unwrap_or(0),
            o + r.output_tokens.unwrap_or(0),
        )
    })
}

#[test]
fn prep_writes_session_map_and_trial_correspondence_before_the_trial() {
    let tmp = tempfile::tempdir().expect("tmp");
    let evidence = prepare(tmp.path());
    let base: Vec<Mapping> = serde_json::from_str(
        &fs::read_to_string(evidence.join("session-map.base.json")).expect("base"),
    )
    .expect("base is operator_usage::Mapping[]");
    let cats: Vec<_> = base
        .iter()
        .map(|m| (m.category, m.trial_key.as_deref()))
        .collect();
    assert_eq!(
        cats,
        [
            (Category::Prep, Some(TRIAL_A)),
            (Category::Observe, Some(TRIAL_A)),
            (Category::Observe, Some(TRIAL_B))
        ]
    );
    assert!(base.iter().all(|m| m.session_id == SESSION));
    let trials: Value =
        serde_json::from_str(&fs::read_to_string(evidence.join("session-trials.json")).expect("t"))
            .expect("json");
    assert_eq!(trials["trials"][1]["condition_id"], "B-task");
    assert_eq!(trials["trials"][0]["request_ref"], "chat-message:req-a");
    let ticket: Value =
        serde_json::from_str(&fs::read_to_string(evidence.join("ticket.json")).expect("ticket"))
            .expect("json");
    assert_eq!(ticket["session_id"], SESSION);
    assert!(
        ticket["transcript_expected"]
            .as_str()
            .is_some_and(|p| p.ends_with(&format!("{SESSION}.jsonl")))
    );
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(evidence.join("claude-config/settings.json")).expect("settings"),
    )
    .expect("json");
    assert_eq!(settings["cleanupPeriodDays"], 3650);
    assert_eq!(settings["permissions"]["deny"], json!(["Agent", "Task"]));
    let context = fs::read_to_string(evidence.join("context.md")).expect("context");
    assert!(context.contains(SESSION) && context.contains("chat-message:req-b"));
    assert!(!context.contains("{{"));
    let launch = fs::read_to_string(evidence.join("launch.sh")).expect("launch");
    assert!(launch.contains(&format!("--session-id {SESSION}")));
    assert!(launch.contains("CLAUDE_CONFIG_DIR"));
    let check = Command::new("sha256sum")
        .args(["-c", "SHA256SUMS"])
        .current_dir(&evidence)
        .output()
        .expect("sha256sum");
    ok(&check, "sha256sum -c");
    // 作り直しは別の dir に。旧証跡を上書きしない。
    let again = Command::new("sh")
        .arg(scripts().join("ops-r2-session-prep.sh"))
        .args(["--evidence", s(&evidence)])
        .output()
        .expect("sh");
    assert!(!again.status.success());
}

#[test]
fn reimporting_parent_and_subagent_transcripts_keeps_totals() {
    let tmp = tempfile::tempdir().expect("tmp");
    let evidence = prepare(tmp.path());
    let list = human_ops_list(&evidence, &[]);
    let map = evidence.join("session-map.json");
    ok(&finalize(&evidence, &list, &map), "final");
    let first = fs::read(&map).expect("map");
    let map2 = evidence.join("session-map.again.json");
    ok(&finalize(&evidence, &list, &map2), "final again");
    assert_eq!(
        first,
        fs::read(&map2).expect("map2"),
        "同じ入力なら byte 一致"
    );
    let parsed: Vec<Mapping> = serde_json::from_slice(&first).expect("Mapping[]");
    let intervene: Vec<_> = parsed
        .iter()
        .filter(|m| m.category == Category::Intervene)
        .collect();
    assert_eq!(intervene.len(), 1);
    assert_eq!(
        (intervene[0].start.as_deref(), intervene[0].end.as_deref()),
        (Some("2026-10-30T00:15:00Z"), Some("2026-10-30T00:20:00Z"))
    );
    // observe は介入の前後に分かれる。人の op は operator の区間にしない。
    assert_eq!(
        parsed
            .iter()
            .filter(|m| m.category == Category::Observe)
            .count(),
        3
    );

    let dest = snapshot(&evidence);
    let out = evidence.join("operator-usage.jsonl");
    let added: usize = transcripts(&dest)
        .iter()
        .map(|t| import(t, &map, &out).expect("import"))
        .sum();
    assert_eq!(added, 7);
    let bytes = fs::read(&out).expect("out");
    let once = rows(&out);
    assert_eq!(total(&once), (2500, 255));
    // 同じ snapshot を 2 回目に取り込む。途中の別 snapshot（同じ親と子）も取り込む。
    let copy = evidence.join("operator").join(SESSION).join("mid");
    fs::create_dir_all(copy.join("subagents")).expect("mid");
    for t in transcripts(&dest) {
        let rel = t.strip_prefix(&dest).expect("rel");
        fs::copy(&t, copy.join(rel)).expect("copy");
    }
    for t in transcripts(&dest).iter().chain(transcripts(&copy).iter()) {
        assert_eq!(import(t, &map, &out).expect("reimport"), 0);
    }
    assert_eq!(
        fs::read(&out).expect("out"),
        bytes,
        "再取り込みで内容が変わらない"
    );
    assert_eq!(total(&rows(&out)), (2500, 255));

    let by = |id: &str| {
        once.iter()
            .find(|r| r.message_id == id)
            .unwrap_or_else(|| panic!("{id}"))
            .clone()
    };
    assert_eq!(
        (by("m-prep").category, by("m-prep").trial_key.as_deref()),
        (Category::Prep, Some(TRIAL_A))
    );
    assert_eq!(by("m-obs").category, Category::Observe);
    assert_eq!(by("m-obs").output_tokens, Some(25));
    for id in ["m-int", "s-int", "s-nousage"] {
        assert_eq!(
            (by(id).category, by(id).trial_key.as_deref()),
            (Category::Intervene, Some(TRIAL_A)),
            "{id}"
        );
    }
    assert!(by("s-int").is_subagent && by("s-int").agent_id.as_deref() == Some("a1"));
    assert_eq!(
        (by("m-b").category, by("m-b").trial_key.as_deref()),
        (Category::Observe, Some(TRIAL_B))
    );
    let gap = by("m-gap");
    assert_eq!(
        (gap.category, gap.trial_key),
        (Category::SharedUnattributable, None)
    );
}

#[test]
fn missing_consumption_is_null_with_reason() {
    let tmp = tempfile::tempdir().expect("tmp");
    let evidence = prepare(tmp.path());
    let list = human_ops_list(&evidence, &[]);
    let map = evidence.join("session-map.json");
    ok(&finalize(&evidence, &list, &map), "final");
    let dest = snapshot(&evidence);
    let out = evidence.join("operator-usage.jsonl");
    for t in transcripts(&dest) {
        import(&t, &map, &out).expect("import");
    }
    let nousage = rows(&out)
        .into_iter()
        .find(|r| r.message_id == "s-nousage")
        .expect("row");
    assert_eq!(
        (nousage.input_tokens, nousage.output_tokens),
        (None, None),
        "欠けた usage は 0 にしない"
    );
    let trials = evidence.join("session-trials.json");
    let summary = evidence.join("summary.json");
    ok(
        &session_map(&[
            "summary",
            "--trials",
            s(&trials),
            "--usage",
            s(&out),
            "--snapshot",
            s(&dest),
            "--out",
            s(&summary),
        ]),
        "summary",
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&summary).expect("s")).expect("json");
    let group = |cat: &str, key: Option<&str>| {
        v["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .find(|g| g["category"] == cat && g["trial_key"] == json!(key))
            .unwrap_or_else(|| panic!("{cat} {key:?}"))
            .clone()
    };
    let int = group("intervene", Some(TRIAL_A));
    assert_eq!(int["input_tokens"], Value::Null);
    assert_eq!(int["reason"], "usage_missing:1");
    assert_eq!(int["messages"], 3);
    assert_eq!(int["request_ref"], "chat-message:req-a");
    let prep = group("prep", Some(TRIAL_A));
    assert_eq!(
        (prep["input_tokens"].clone(), prep["reason"].clone()),
        (json!(100), Value::Null)
    );
    assert_eq!(
        group("shared_unattributable", None)["input_tokens"],
        json!(500)
    );

    // transcript が無い session は行を作らず、全部 null と理由。
    let empty = evidence.join("operator").join(SESSION).join("lost");
    fs::create_dir_all(&empty).expect("lost");
    let missing = evidence.join("summary-missing.json");
    ok(
        &session_map(&[
            "summary",
            "--trials",
            s(&trials),
            "--usage",
            s(&evidence.join("none.jsonl")),
            "--snapshot",
            s(&empty),
            "--out",
            s(&missing),
        ]),
        "summary missing",
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&missing).expect("m")).expect("json");
    assert_eq!(v["reason"], "transcript_missing");
    for g in v["groups"].as_array().expect("groups") {
        assert_eq!(g["input_tokens"], Value::Null);
        assert_eq!(g["messages"], Value::Null);
        assert_eq!(g["reason"], "transcript_missing");
    }
}

#[test]
fn unclosed_or_misplaced_intervention_does_not_produce_a_map() {
    let tmp = tempfile::tempdir().expect("tmp");
    let evidence = prepare(tmp.path());
    let out = evidence.join("bad.json");
    // 終了の無い介入: 区間を決められないので作らない。
    let open = human_ops_list(
        &evidence,
        &[op(
            "op_start",
            Some(TRIAL_B),
            "relay-1",
            "2026-10-30T00:41:00Z",
        )],
    );
    assert!(!finalize(&evidence, &open, &out).status.success());
    // 別の試行の区間にかかる介入。
    let crossing = human_ops_list(
        &evidence,
        &[
            op("op_start", Some(TRIAL_B), "relay-2", "2026-10-30T00:25:00Z"),
            op("op_end", Some(TRIAL_B), "relay-2", "2026-10-30T00:26:00Z"),
        ],
    );
    assert!(!finalize(&evidence, &crossing, &out).status.success());
    assert!(!out.exists());
}
