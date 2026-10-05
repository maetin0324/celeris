use std::sync::Mutex;

use task_core::ArtifactRef;

use super::*;
use crate::protocol::{PROTOCOL_VERSION, RunContext};

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    artifacts: Mutex<Vec<ArtifactRef>>,
    heartbeat_count: Mutex<u32>,
    /// ADR-0044 D2（Phase 53）: `{"type":"comment"}` の本文。
    comments: Mutex<Vec<String>>,
}

impl EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
    }
    fn comment(&self, body: &str) {
        self.comments
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(body.to_string());
    }
    fn artifact(&self, artifact: &ArtifactRef) {
        self.artifacts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(artifact.clone());
    }
    fn heartbeat(&self) {
        *self
            .heartbeat_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) += 1;
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

#[tokio::test]
async fn routing_context_ref_is_not_persisted_in_request_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.context_ref = Some("opaque-reference".into());
    write_run_request(dir.path(), &req, "run").await;
    let snapshot = std::fs::read_to_string(dir.path().join("request.json")).unwrap();
    assert!(!snapshot.contains("opaque-reference"));
    assert!(!snapshot.contains("context_ref"));
    assert_eq!(req.context.context_ref.as_deref(), Some("opaque-reference"));
}

fn default_limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    }
}

fn sh_spec(script: &str) -> SubprocessSpec {
    SubprocessSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        container: None,
    }
}

#[tokio::test]
async fn happy_path_progress_artifact_done() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat > received.json; \
             mkdir -p artifacts && echo hi > artifacts/out.txt; \
             echo '{\"type\":\"progress\",\"msg\":\"working\"}'; \
             echo '{\"type\":\"artifact\",\"name\":\"out\",\"path\":\"artifacts/out.txt\",\"kind\":\"txt\"}'; \
             echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-1", &default_limits(), &sink)
        .await
        .unwrap();

    assert_eq!(outcome.exit_code, Some(0));
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "ok"),
        other => panic!("expected done, got {other:?}"),
    }
    assert_eq!(sink.progress.lock().unwrap().len(), 1);
    let artifacts = sink.artifacts.lock().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].name, "out");
    assert_eq!(artifacts[0].sha256.len(), 64);
    // progress・artifact・done の 3 行分は少なくとも heartbeat が呼ばれている（ADR-0010 D7）。
    assert!(*sink.heartbeat_count.lock().unwrap() >= 3);

    let received = std::fs::read_to_string(dir.path().join("received.json")).unwrap();
    assert!(received.contains(r#""type":"run""#));

    let run_dir = dir.path().join("runs/run-1");
    assert!(run_dir.join("stdout.jsonl").is_file());
    assert!(run_dir.join("stderr.log").is_file());
    assert!(run_dir.join("result.json").is_file());

    // ADR-0023 D2: ワーカーに渡した指示そのものが残る（stdin に書いたものと同じ内容）。
    let request = std::fs::read_to_string(run_dir.join("request.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&request).expect("request.json は JSON");
    assert_eq!(parsed["type"], "run");
    assert_eq!(parsed["task"]["id"], req.task.id.to_string());
    assert!(request.contains('\n'), "人が読めるよう整形して書く");
}

/// Phase 119 D2: run が**自分から** `done` で終わっても（タイムアウトや cancel ではない）、
/// ワーカーが `&` で起こしたバックグラウンドジョブ（本番で見つかった `sleep 3600` の再現）は
/// `reap_after_terminal` の中の `process_group::sweep` が片付ける。孫がプロセスグループの長
/// （`sh` 自身）より長生きしても、`sh` が exit した瞬間に掃除されることを確認する。
#[tokio::test]
async fn a_normally_finished_run_reaps_a_background_grandchild() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let spec = sh_spec(&format!(
        "cat >/dev/null; \
             sleep 300 & echo $! > {pidfile}; \
             echo '{{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}}'",
        pidfile = pidfile.to_string_lossy()
    ));
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-grandchild", &default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { .. } => {}
        other => panic!("expected done, got {other:?}"),
    }

    // 孫の pid が書かれるのを待つ（上限付き）。
    let mut grandchild: Option<i32> = None;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&pidfile)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            grandchild = Some(pid);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let grandchild = grandchild.unwrap_or_else(|| panic!("grandchild pid was never written"));

    // `run_subprocess` が返った時点で、正常終了経路の `sweep` が孫を片付けているはず。
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{grandchild}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the background grandchild {grandchild} is still alive after a normal `done` finish");
}

#[tokio::test]
async fn non_json_lines_are_ignored_and_subsequent_done_wins() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null; \
             echo 'not json at all'; \
             echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-2", &default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "ok"),
        other => panic!("expected done, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_type_is_protocol_violation() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec("cat >/dev/null; echo '{\"type\":\"bogus\"}'");
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-3", &default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(!retryable);
            assert!(message.contains("protocol violation"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// ADR-0044 D2（Phase 53）: `{"type":"comment","body":"…"}` は**非終端**で、シンクの
/// `comment` に渡る（`progress` とは別の口）。run はそのまま続いて `done` で終わる。
/// `PROTOCOL_VERSION` は上げない（追加のみ。この行を出さないワーカーはそのまま動く）。
#[tokio::test]
async fn a_comment_line_is_passed_to_the_sink_and_does_not_end_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null\necho '{\"type\":\"comment\",\"body\":\"ビルドが通った\"}'\necho '{\"type\":\"progress\",\"msg\":\"next\"}'\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-comment", &default_limits(), &sink)
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    assert_eq!(
        sink.comments
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        vec!["ビルドが通った".to_string()]
    );
    assert_eq!(
        sink.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        vec!["next".to_string()],
        "コメントは progress には入らない"
    );
    assert_eq!(PROTOCOL_VERSION, 4, "追加のみなので版数は上げない");
}

#[tokio::test]
async fn exit_without_terminal_message_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec("cat >/dev/null; exit 3");
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-4", &default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.exit_code, Some(3));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=3"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn idle_timeout_kills_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec("cat >/dev/null; sleep 30");
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(300),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = run_subprocess(&spec, &req, "run-5", &limits, &sink)
        .await
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("idle timeout"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn wall_clock_exceeded_kills_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null; \
             while true; do echo '{\"type\":\"progress\",\"msg\":\"tick\"}'; sleep 0.1; done",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = run_subprocess(&spec, &req, "run-6", &limits, &sink)
        .await
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("wall clock exceeded"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn lines_after_terminal_are_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null; \
             echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'; \
             echo '{\"type\":\"error\",\"message\":\"late\",\"retryable\":false}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-7", &default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "ok"),
        other => panic!("expected done, got {other:?}"),
    }
}

/// `error.provider_failure` が付いていれば、result.json を書いた上で `AdapterError` として返す
/// （ADR-0010 D5）。heartbeat は observed 行数（progress + error）以上呼ばれる（D7）。
#[tokio::test]
async fn provider_failure_is_classified_and_result_json_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null; \
             echo '{\"type\":\"progress\",\"msg\":\"a\"}'; \
             echo '{\"type\":\"error\",\"message\":\"boom\",\"retryable\":true,\
             \"provider_failure\":{\"kind\":\"throttled\",\"retry_after_secs\":7}}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = run_subprocess(&spec, &req, "run-9", &default_limits(), &sink)
        .await
        .expect_err("expected provider failure to surface as an AdapterError");
    match err {
        AdapterError::Throttled { retry_after } => {
            assert_eq!(retry_after, Duration::from_secs(7))
        }
        other => panic!("expected Throttled, got {other:?}"),
    }
    assert!(dir.path().join("runs/run-9/result.json").is_file());
    assert!(*sink.heartbeat_count.lock().unwrap() >= 2);
}

#[tokio::test]
async fn artifact_path_escaping_workspace_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let spec = sh_spec(
        "cat >/dev/null; \
             echo '{\"type\":\"artifact\",\"name\":\"bad\",\"path\":\"../x\"}'; \
             echo '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = run_subprocess(&spec, &req, "run-8", &default_limits(), &sink)
        .await
        .unwrap();
    assert!(sink.artifacts.lock().unwrap().is_empty());
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "ok"),
        other => panic!("expected done, got {other:?}"),
    }
}
