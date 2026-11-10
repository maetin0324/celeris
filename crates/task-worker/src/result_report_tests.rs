//! 結果契約（ADR 2026-11-10-adapter-result-contract D1・(4) 4a）の共通試験。harness adapter 5 本は
//! `interpret_result_text`・`read_result_file` を 1 か所で呼ぶので、契約（優先順位・無効値・寛容な
//! `evidence`・usage の渡し方・誤配置の回収）はここで正本を直接試す。adapter ごとの試験は結線だけを見る。

use super::*;
use crate::adapter::Terminal;
use crate::protocol::{Evidence, RunContext, RunRequest};

const REL: &str = "artifacts";

const WAIT_FIELDS: &str = r#""type":"wait","kind":"cluster_job","cluster":"sirius","jobs":["42634"],"scheduler":"pbs","summary":"submitted 1 job""#;

fn usage() -> Option<task_core::Usage> {
    Some(task_core::Usage {
        input_tokens: Some(10),
        output_tokens: Some(20),
        ..Default::default()
    })
}

fn interpret(text: &str) -> Terminal {
    interpret_result_text(text, REL, usage())
}

fn error_message(terminal: Terminal) -> String {
    match terminal {
        Terminal::Error {
            message,
            retryable,
            usage,
        } => {
            assert!(retryable, "{message}");
            assert_eq!(usage, None, "{message}");
            message
        }
        other => panic!("expected a retryable error, got {other:?}"),
    }
}

/// 不変条件: `question`（文字列）> `wait` > `summary` > `yield`（ADR-0072 D9、ADR-0090 D1）。
/// 既存の試験は adapter ごとに 1〜2 組だけを fake の実行で見ており、順位の全組を 1 か所で見る試験が無い。
#[test]
fn result_contract_priority_order() {
    assert!(matches!(
        interpret(r#"{"question":"which one?","summary":"s","yield":{}}"#),
        Terminal::Question { ref text, .. } if text == "which one?"
    ));
    assert!(matches!(
        interpret(&format!(r#"{{"question":"which one?",{WAIT_FIELDS}}}"#)),
        Terminal::Question { .. }
    ));
    // `question` が文字列でなければ `wait` を先に見る（`ResultFile` に読めないので、`wait` が無ければ無効値）。
    assert!(matches!(
        interpret(&format!(r#"{{"question":1,{WAIT_FIELDS}}}"#)),
        Terminal::Waiting { .. }
    ));
    match interpret(&format!(r#"{{{WAIT_FIELDS},"yield":{{}}}}"#)) {
        Terminal::Waiting { request, .. } => {
            assert_eq!(request.jobs, vec!["42634".to_string()]);
            assert_eq!(request.summary, "submitted 1 job");
        }
        other => panic!("expected waiting, got {other:?}"),
    }
    // 入れ子の形（`{"summary": .., "wait": {..}}`）も `summary` より先。
    assert!(matches!(
        interpret(r#"{"summary":"submitted","wait":{"kind":"cluster_job","jobs":["1"]}}"#),
        Terminal::Waiting { .. }
    ));
    assert!(matches!(
        interpret(r#"{"summary":"done","yield":{"completed":["a"]}}"#),
        Terminal::Done { ref summary, .. } if summary == "done"
    ));
    match interpret(r#"{"yield":{"completed":["a"],"next_action":"b"}}"#) {
        Terminal::Yielded { checkpoint, .. } => assert_eq!(checkpoint["next_action"], "b"),
        other => panic!("expected yielded, got {other:?}"),
    }
}

/// 不変条件: `evidence` は配列の要素のうち `Evidence` に読めたものだけを残し、配列でなければ空。run は
/// 失敗しない（ADR-0006 D3）。既存の `malformed_evidence_in_result_file_does_not_fail_the_run` は adapter
/// ごとの写しで、正本の関数を直接試していなかった。
#[test]
fn result_contract_lenient_evidence() {
    let kept = Evidence {
        criterion: 0,
        command: Some("cargo test".into()),
        exit: Some(0),
        stdout_tail: None,
    };
    let items = serde_json::json!([
        "plain string",
        {"criterion": 0, "command": "cargo test", "exit": 0},
        {"criterion": "first"},
        42
    ]);
    assert_eq!(lenient_evidence(items.clone()), vec![kept.clone()]);
    assert_eq!(
        lenient_evidence(serde_json::json!({"criterion": 0})),
        Vec::new()
    );
    assert_eq!(lenient_evidence(serde_json::Value::Null), Vec::new());

    let text = serde_json::json!({"summary": "ok", "evidence": items}).to_string();
    match interpret(&text) {
        Terminal::Done { evidence, .. } => assert_eq!(evidence, vec![kept]),
        other => panic!("expected done, got {other:?}"),
    }
    match interpret(r#"{"summary":"ok","evidence":"all tests passed"}"#) {
        Terminal::Done { evidence, .. } => assert!(evidence.is_empty()),
        other => panic!("expected done, got {other:?}"),
    }
}

/// 不変条件: JSON でない本文・欄の型が違う本文は `Error{retryable: true, usage: None}` で、文言は
/// `<rel>/result.json is not valid JSON: …`。既存の `invalid_result_file_json_is_retryable_error` は
/// claude_code・codex の写しで、pi・acp・aider には無かった。
#[test]
fn result_contract_invalid_json_is_retryable_error() {
    for text in [
        "not json",
        "{\"summary\": ",
        r#"{"question":1}"#,
        r#"{"summary":["a"]}"#,
        "[1]",
    ] {
        let message = error_message(interpret(text));
        assert!(
            message.starts_with("artifacts/result.json is not valid JSON: "),
            "{text}: {message}"
        );
    }
}

/// 不変条件: どの欄も無い JSON は `Error{retryable: true}`（`… has neither 'summary', 'question' nor 'yield'`）、
/// `wait` が不正なら `Error{retryable: true}`（`result.json has an invalid cluster job wait: …`）。現行は
/// どの adapter にもこの試験が無かった。
#[test]
fn result_contract_no_known_field_is_retryable_error() {
    for text in [
        "{}",
        r#"{"evidence": []}"#,
        r#"{"summary":null,"report":{"kind":"result"}}"#,
    ] {
        assert_eq!(
            error_message(interpret(text)),
            "artifacts/result.json has neither 'summary', 'question' nor 'yield'",
            "{text}"
        );
    }
    for text in [
        r#"{"type":"wait","kind":"cluster_job","jobs":[]}"#,
        r#"{"summary":"s","wait":"soon"}"#,
    ] {
        let message = error_message(interpret(text));
        assert!(
            message.starts_with("result.json has an invalid cluster job wait: "),
            "{text}: {message}"
        );
    }
}

/// 不変条件: 渡した usage は Done・Question・Yielded・Waiting にそのまま載り、Error には載らない（`None`）。
/// 現行は adapter の試験が usage を見ていなかった。
#[test]
fn result_contract_usage_passthrough() {
    let carried = [
        interpret(r#"{"summary":"s"}"#),
        interpret(r#"{"question":"q"}"#),
        interpret(r#"{"yield":{}}"#),
        interpret(&format!("{{{WAIT_FIELDS}}}")),
    ];
    for terminal in carried {
        let got = match &terminal {
            Terminal::Done { usage, .. }
            | Terminal::Question { usage, .. }
            | Terminal::Yielded { usage, .. }
            | Terminal::Waiting { usage, .. } => *usage,
            other => panic!("unexpected terminal {other:?}"),
        };
        assert_eq!(got, usage(), "{terminal:?}");
    }
    assert!(matches!(
        interpret_result_text(r#"{"summary":"s"}"#, REL, None),
        Terminal::Done { usage: None, .. }
    ));
    for text in ["not json", "{}", r#"{"type":"wait","jobs":[]}"#] {
        error_message(interpret(text));
    }
}

fn request(workspace: &std::path::Path, context: RunContext) -> RunRequest {
    RunRequest {
        llm_route: None,
        cargo_target_dir: None,
        protocol: crate::protocol::PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace: workspace.to_path_buf(),
        work_dir: None,
        context,
    }
}

/// 不変条件: `read_result_file` は結果ファイルを要る run だけ誤配置の回収を通し（複数の候補は `Misplaced`、
/// 1 つなら置き場に写して読む）、無ければ `Missing`、あれば本文を返す（ADR 2026-11-10 D1）。pi・acp・aider は
/// これまで adapter で誤配置を見ていなかったので、adapter の試験では検出できない。
#[tokio::test]
async fn read_result_file_recovers_strays_only_for_runs_that_require_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
    let req = request(dir.path(), RunContext::default());
    std::fs::create_dir_all(&req.artifacts_dir).unwrap();
    assert!(req.requires_result_file());
    assert!(matches!(
        read_result_file(&req, started).await,
        ResultFileRead::Missing
    ));

    std::fs::write(dir.path().join("result.json"), r#"{"summary":"stray"}"#).unwrap();
    match read_result_file(&req, started).await {
        ResultFileRead::Text(text) => assert_eq!(text, r#"{"summary":"stray"}"#),
        other => panic!("expected the single stray to be recovered, got {other:?}"),
    }

    std::fs::remove_file(req.artifacts_dir.join("result.json")).unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/result.json"), "{}").unwrap();
    match read_result_file(&req, started).await {
        ResultFileRead::Misplaced(error) => match *error {
            Terminal::Error {
                message, retryable, ..
            } => {
                assert!(!retryable);
                assert!(
                    message.starts_with(crate::result_location::MISPLACED_RESULT_MARKER),
                    "{message}"
                );
            }
            other => panic!("expected a misplaced error, got {other:?}"),
        },
        other => panic!("expected misplaced, got {other:?}"),
    }

    // CoS chat の run は結果ファイルを要らないので、cwd を探さない。
    let mut context = RunContext::default();
    context.cos_chat = Some(Default::default());
    let chat = request(dir.path(), context);
    assert!(!chat.requires_result_file());
    assert!(matches!(
        read_result_file(&chat, started).await,
        ResultFileRead::Missing
    ));
}
