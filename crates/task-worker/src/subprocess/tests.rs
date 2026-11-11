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
        llm_route: None,
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
    req.context.routing_context_ref = Some("opaque-reference".into());
    write_run_request(dir.path(), &req, "run").await;
    let snapshot = std::fs::read_to_string(dir.path().join("request.json")).unwrap();
    assert!(!snapshot.contains("opaque-reference"));
    assert!(!snapshot.contains("context_ref"));
    assert_eq!(
        req.context.routing_context_ref.as_deref(),
        Some("opaque-reference")
    );
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
        Terminal::Error {
            retryable, message, ..
        } => {
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
        Terminal::Error {
            retryable, message, ..
        } => {
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
        Terminal::Error {
            retryable, message, ..
        } => {
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
        Terminal::Error {
            retryable, message, ..
        } => {
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

// 実行共通処理（ADR 2026-11-10-adapter-result-contract D2・(4) 4b）の `prepare_run`・`finish_run`。harness
// adapter 5 本が 1 か所で呼ぶので、run の準備と終端の書き出しの規則はここで正本を直接試す。

/// 不変条件: `prepare_run` は前回の run が残した `artifacts/result.json`・`delegate.json` を消し（今回の結果と
/// 誤読しない。ADR-0006 D3）、`runs/<run_id>/request.json` を書く。browser の run は log を `/dev/null` にする。
/// これまで adapter ごとの写しで、codex・pi・aider には browser の log の分岐が無かった。
#[tokio::test]
async fn run_common_prepare_run_clears_previous_result_and_delegate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut req = sample_req(dir.path().to_path_buf());
    std::fs::create_dir_all(&req.artifacts_dir).expect("artifacts dir");
    let result = req.artifacts_dir.join("result.json");
    let delegate = req
        .artifacts_dir
        .join(crate::delegate_file::DELEGATE_FILE_NAME);
    std::fs::write(&result, r#"{"summary":"stale"}"#).expect("stale result");
    std::fs::write(&delegate, r#"{"tasks":[]}"#).expect("stale delegate");

    let paths = prepare_run(&req, "run-1", "stdout.log")
        .await
        .expect("prepare");
    let run_dir = dir.path().join("runs").join("run-1");
    assert_eq!(
        paths,
        RunPaths {
            run_dir: run_dir.clone(),
            stdout_log: run_dir.join("stdout.log"),
            stderr_log: run_dir.join("stderr.log"),
            result_path: result.clone(),
            artifacts_rel: req.artifacts_rel(),
        }
    );
    assert!(!result.exists());
    assert!(!delegate.exists());
    assert!(run_dir.join("request.json").is_file());

    req.context.browser = Some(crate::browser::BrowserContext {
        credential_used: false,
        approval_actions: Vec::new(),
        approved_operation: None,
        post_login: None,
        run: task_core::BrowserRun {
            task_id: req.task.id,
            run_id: "run-2".into(),
            session_id: "isolated-test".into(),
            state: task_core::BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: dir.path().join("celeris-browser.py"),
    });
    let browser = prepare_run(&req, "run-2", "stdout.jsonl")
        .await
        .expect("prepare browser run");
    assert_eq!(browser.stdout_log, Path::new("/dev/null"));
    assert_eq!(browser.stderr_log, Path::new("/dev/null"));
}

/// 不変条件: `finish_run` は終端を `runs/<run_id>/result.json` に書いてから、`Error` と供給側失敗の組だけを
/// `Err(AdapterError)`（account の cooldown の入口）にする。供給側失敗の無い `Error` と他の終端は `Ok`。
/// 写しのどれかが書き出しを飛ばしても、adapter の試験は戻り値しか見ないので検出できなかった。
#[tokio::test]
async fn run_common_finish_run_maps_provider_failure_to_adapter_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = Terminal::Error {
        message: "401 Unauthorized".into(),
        retryable: true,
        usage: None,
    };
    let outcome = finish_run(
        dir.path(),
        error.clone(),
        Some(ProviderFailure::AuthFailed),
        std::os::unix::process::ExitStatusExt::from_raw(1 << 8),
    )
    .await;
    assert!(
        matches!(&outcome, Err(AdapterError::AuthFailed(m)) if m == "401 Unauthorized"),
        "{outcome:?}"
    );
    let written = std::fs::read_to_string(dir.path().join("result.json")).expect("result.json");
    assert!(written.contains("401 Unauthorized"), "{written}");

    let outcome = finish_run(
        dir.path(),
        error.clone(),
        None,
        std::os::unix::process::ExitStatusExt::from_raw(1 << 8),
    )
    .await
    .expect("an error without a provider failure stays a terminal");
    assert_eq!(outcome.terminal, error);
    assert_eq!(outcome.exit_code, Some(1));

    let done = Terminal::Done {
        summary: "ok".into(),
        evidence: Vec::new(),
        usage: None,
    };
    let outcome = finish_run(
        dir.path(),
        done.clone(),
        Some(ProviderFailure::Exhausted),
        std::os::unix::process::ExitStatusExt::from_raw(0),
    )
    .await
    .expect("a provider failure only matters with an error terminal");
    assert_eq!(outcome.terminal, done);
    assert_eq!(outcome.exit_code, Some(0));
}

/// 不変条件（ADR-0090 D1・D5、ADR 2026-11-10-adapter-result-contract (4) 4b）: `finish_run` は `Terminal::Waiting`
/// を `runs/<run_id>/result.json` の 1 行の `{"type":"wait","kind":"cluster_job",...}`（cluster・scheduler・jobs・
/// poll_secs・timeout_secs・checkpoint・summary・usage）に書き、その行を daemon の再起動・orphan の引き継ぎと
/// 同じ手順（task-dispatch の `terminal_from_run_dir`: `serde_json::Value` に読み、`usage` を `task_core::Usage`
/// に読み、`adapter::wait_terminal`）で読むと元と同じ `Terminal::Waiting` に戻る。adapter の結線の試験は戻り値と
/// 行の一部しか見ず、dispatcher の cluster_job_wait の試験は scripted adapter が `Terminal` を直接返すので
/// run dir の行を通らない。`write_result_json` の Waiting の写像が欄を落とすとここだけが落ちる。
#[tokio::test]
async fn run_common_finish_run_writes_wait_line_for_waiting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let usage = task_core::Usage {
        input_tokens: Some(10),
        output_tokens: Some(20),
        cache_read_tokens: Some(5),
        cost_usd: Some(0.25),
        ..Default::default()
    };
    let checkpoint = serde_json::json!({"completed": ["submitted"], "next_action": "collect"});
    let request = task_core::cluster_job::ClusterJobWaitRequest {
        cluster: Some("sirius".into()),
        scheduler: task_core::cluster_job::ClusterScheduler::Pbs,
        jobs: vec!["42634.sirius-pbs".into(), "42635".into()],
        poll_secs: Some(300),
        timeout_secs: Some(43_200),
        summary: "submitted 2 jobs".into(),
    };
    let waiting = Terminal::Waiting {
        request: request.clone(),
        checkpoint: Some(checkpoint.clone()),
        usage: Some(usage),
    };

    let outcome = finish_run(
        dir.path(),
        waiting.clone(),
        None,
        std::os::unix::process::ExitStatusExt::from_raw(0),
    )
    .await
    .expect("a waiting terminal is not an adapter error");
    assert_eq!(outcome.terminal, waiting);
    assert_eq!(outcome.exit_code, Some(0));

    let raw = std::fs::read_to_string(dir.path().join("result.json")).expect("result.json");
    assert!(raw.ends_with('\n'), "{raw:?}");
    assert_eq!(raw.trim_end().lines().count(), 1, "{raw:?}");
    let line: serde_json::Value = serde_json::from_str(raw.trim()).expect("one JSON line");
    assert_eq!(line["type"], "wait");
    assert_eq!(line["kind"], task_core::cluster_job::WAIT_KIND_CLUSTER_JOB);
    assert_eq!(line["cluster"], "sirius");
    assert_eq!(line["scheduler"], "pbs");
    assert_eq!(
        line["jobs"],
        serde_json::json!(["42634.sirius-pbs", "42635"])
    );
    assert_eq!(line["poll_secs"], 300);
    assert_eq!(line["timeout_secs"], 43_200);
    assert_eq!(line["checkpoint"], checkpoint);
    assert_eq!(line["summary"], "submitted 2 jobs");
    assert_eq!(line["usage"]["input_tokens"], 10);
    assert_eq!(line["usage"]["cost_usd"], 0.25);

    // task-dispatch の `terminal_from_run_dir` と同じ読み直し。
    let reread_usage = line
        .get("usage")
        .and_then(|u| serde_json::from_value::<task_core::Usage>(u.clone()).ok());
    assert_eq!(reread_usage, Some(usage));
    let reread = crate::adapter::wait_terminal(&line, reread_usage)
        .expect("the run dir line is a wait declaration");
    assert_eq!(reread, waiting);
}
