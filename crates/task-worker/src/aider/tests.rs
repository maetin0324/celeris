use std::sync::Mutex;
use std::time::Duration;

use task_core::{ArtifactRef, DelegateTask};

use super::*;
use crate::protocol::{PROTOCOL_VERSION, RunContext};

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    delegated: Mutex<Vec<Vec<DelegateTask>>>,
}

impl EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, tasks: &[DelegateTask]) {
        self.delegated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tasks.to_vec());
    }
}

fn stub_aider(dir: &std::path::Path, script: &str) -> AiderConfig {
    let path = dir.join("aider_stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    AiderConfig {
        command: path.to_string_lossy().into_owned(),
        ..AiderConfig::default()
    }
}

fn sample_req(workspace: std::path::PathBuf) -> RunRequest {
    RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace,
        work_dir: None,
        context: RunContext::default(),
    }
}

fn default_limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(200),
    }
}

/// F5-fix10: 200 KiB のプロンプトも argv ではなく `--message-file <run dir のファイル>` で渡り、spawn は
/// E2BIG で落ちない。fake の aider がそのファイルを写し、`prompt.txt` と一致することを見る。
#[tokio::test]
async fn f5_fix10_a_200_kib_prompt_is_passed_via_message_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(
        dir.path(),
        r#"for a in "$@"; do printf '%s\0' "$a" >> args.log; done
while [ $# -gt 0 ]; do
  if [ "$1" = "--message-file" ]; then cp "$2" message.copy; fi
  shift
done
mkdir -p artifacts
printf '%s' '{"summary": "ok", "evidence": []}' > artifacts/result.json
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    let filler = "0123456789abcdef".repeat(200 * 1024 / 16);
    req.task.objective = format!("BEGIN-OBJECTIVE {filler} END-OBJECTIVE");
    let outcome = AiderAdapter::new(config)
        .run(
            req,
            "run-f5fix10",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .expect("a 200 KiB prompt must not fail to spawn");
    assert!(
        matches!(outcome.terminal, Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    let got = std::fs::read_to_string(dir.path().join("message.copy")).unwrap();
    let recorded = std::fs::read_to_string(dir.path().join("runs/run-f5fix10/prompt.txt")).unwrap();
    assert!(got.len() > crate::subprocess::MAX_SINGLE_ARG_BYTES);
    assert_eq!(got, recorded);
    let args_log = std::fs::read_to_string(dir.path().join("args.log")).unwrap();
    let args: Vec<&str> = args_log.split('\0').filter(|s| !s.is_empty()).collect();
    assert!(!args.contains(&"--message"), "{args:?}");
    assert!(args.iter().all(|a| !a.contains("BEGIN-OBJECTIVE")));
}

#[tokio::test]
async fn a_successful_edit_produces_done_with_parsed_token_usage() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary": "fixed the typo", "evidence": []}' > artifacts/result.json
echo 'Applied edit to README.md'
echo 'Tokens: 120 sent, 34 received.'
echo 'Cost: $0.0011 message, $0.0011 session.'
"#,
    );
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-1", default_limits(), &sink)
        .await
        .expect("run succeeds");
    match outcome.terminal {
        Terminal::Done { summary, usage, .. } => {
            assert_eq!(summary, "fixed the typo");
            let usage = usage.expect("usage parsed from aider's own token line");
            assert_eq!(usage.input_tokens, Some(120));
            assert_eq!(usage.output_tokens, Some(34));
            assert!((usage.cost_usd.unwrap() - 0.0011).abs() < 1e-9);
        }
        other => panic!("expected Done, got {other:?}"),
    }
    assert!(
        sink.progress
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("Applied edit"))
    );
    assert!(dir.path().join("runs/run-1/stdout.log").is_file());
}

#[tokio::test]
async fn missing_result_json_is_a_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(dir.path(), "echo 'nothing to do here'\n");
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-2", default_limits(), &sink)
        .await
        .expect("run itself does not error (no provider-failure text)");
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("result.json"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_question_in_result_json_becomes_terminal_question() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"question": "which file?"}' > artifacts/result.json
"#,
    );
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-3", default_limits(), &sink)
        .await
        .expect("run succeeds");
    assert_eq!(
        outcome.terminal,
        Terminal::Question {
            text: "which file?".into()
        }
    );
}

/// ADR-0072 D7/§6 (i)（Phase E1）: aider には turn の上限が無いので、wall-clock の打ち切りが
/// continuation の唯一の入口になる（`Terminal::BudgetExhausted{kind: WallClock}`）。
#[tokio::test]
async fn wall_clock_timeout_kills_the_process_and_becomes_budget_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(dir.path(), "sleep 30\n");
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let mut short_limits = default_limits();
    short_limits.wall_clock = Duration::from_millis(200);
    short_limits.idle_timeout = Duration::from_secs(5);
    let outcome = adapter
        .run(req, "run-4", short_limits, &sink)
        .await
        .expect("timeout is not a provider failure");
    match outcome.terminal {
        Terminal::BudgetExhausted { kind, message, .. } => {
            assert_eq!(kind, task_core::BudgetKind::WallClock);
            assert_eq!(message, "wall clock exceeded");
        }
        other => panic!("expected budget_exhausted, got {other:?}"),
    }
}

/// ADR-0072 D9（Phase E1）: `result.json` の `{"yield": {...}}` が `Terminal::Yielded` になる。
#[tokio::test]
async fn result_yield_becomes_terminal_yielded() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_aider(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"yield":{"completed":["A"],"next_action":"do B"}}' > artifacts/result.json
"#,
    );
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-yield", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Yielded { checkpoint, .. } => {
            assert_eq!(checkpoint["next_action"], "do B");
        }
        other => panic!("expected yielded, got {other:?}"),
    }
}

#[test]
fn parse_aider_usage_ignores_abbreviated_token_counts() {
    let usage = parse_aider_usage("blah\nTokens: 1.2k sent, 340 received.\n");
    let usage = usage.expect("the exact 'received' count is still recovered");
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, Some(340));
}

#[test]
fn parse_aider_usage_returns_none_when_nothing_matches() {
    assert_eq!(parse_aider_usage("nothing interesting here"), None);
}

#[test]
fn with_model_returns_a_new_adapter_carrying_the_model() {
    let adapter = AiderAdapter::new(AiderConfig::default());
    let with_model = adapter.with_model("claude-sonnet-5").unwrap();
    assert_eq!(with_model.id(), AiderAdapter::ID);
}

/// 実バイナリでの動作確認（ADR-0061, Phase 104。`docs/PROGRESS.md` Phase 104 参照）。`cargo test
/// --workspace` の既定では走らない（`aider` バイナリが要る。テストで外部ネットワークに出ない、
/// という CLAUDE.md の方針どおり、接続先はこのテストが自分で起こすローカルの HTTP モックだけ）。
/// `AIDER_TEST_BIN`（既定 `"aider"`）で使う実行ファイルを指定できる:
/// `AIDER_TEST_BIN=/path/to/aider cargo test -p task-worker aider::tests::real_aider_binary_end_to_end -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn real_aider_binary_end_to_end() {
    use std::io::{Read, Write};

    let bin = std::env::var("AIDER_TEST_BIN").unwrap_or_else(|_| "aider".into());
    if std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!(
            "skipping real_aider_binary_end_to_end: {bin} is not runnable (set AIDER_TEST_BIN)"
        );
        return;
    }

    // 最小の OpenAI 互換モック（127.0.0.1 だけを聞く。外部ネットワークには出ない）。1 リクエストだけ
    // 相手にして、`artifacts/result.json` を作る SEARCH/REPLACE diff を返す。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 8192];
        let mut received = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
            let Some(header_end) = received
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
            else {
                continue;
            };
            let header = String::from_utf8_lossy(&received[..header_end]);
            let content_length: usize = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().to_string())
                })
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if received.len() >= header_end + content_length {
                break;
            }
        }
        let body = br#"{"id":"mock","object":"chat.completion","created":0,"model":"mock","choices":[{"index":0,"message":{"role":"assistant","content":"artifacts/result.json\n```json\n<<<<<<< SEARCH\n=======\n{\"summary\": \"real aider binary smoke test\", \"evidence\": []}\n>>>>>>> REPLACE\n```\n"},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":20,"total_tokens":70}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    });

    let dir = tempfile::tempdir().unwrap();
    // aider は git リポジトリを期待する（repo-map 機能。commit はしない: `--no-auto-commits`）。
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "a@b.c"],
        vec!["config", "user.name", "test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .status()
            .expect("git available for the smoke test");
    }
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(dir.path())
        .status()
        .unwrap();

    let config = AiderConfig {
        command: bin,
        extra_args: vec!["--edit-format".into(), "diff".into(), "--no-stream".into()],
        model: Some("openai/mock-model".into()),
        env: vec![
            ("OPENAI_API_BASE".into(), format!("http://{addr}/v1")),
            ("OPENAI_API_KEY".into(), "dummy".into()),
        ],
        env_remove: Vec::new(),
        container: None,
    };
    let adapter = AiderAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "real-run", default_limits(), &sink)
        .await
        .expect("real aider binary run succeeds against the local mock");
    match outcome.terminal {
        Terminal::Done { summary, .. } => {
            assert_eq!(summary, "real aider binary smoke test");
        }
        other => panic!("expected Done from the real aider binary, got {other:?}"),
    }
    server.join().expect("mock server thread");
}
