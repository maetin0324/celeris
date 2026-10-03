use std::path::Path;
use std::sync::Mutex;

use task_core::ArtifactRef;

use super::*;
use crate::protocol::{PROTOCOL_VERSION, RunContext};

/// ADR-0052 D2: フォールバックの前置きは `langmem_run.py` の `EXTRACTION_INSTRUCTIONS` を
/// **そのまま**使う（出典は python のランナー 1 つだけ。写し間違いが起きない）。
#[test]
fn the_fallback_preamble_reuses_the_python_runners_extraction_instructions() {
    let instructions = extraction_instructions();
    assert!(
        instructions.starts_with("You are the knowledge-base maintainer"),
        "{instructions}"
    );
    assert!(
        instructions.ends_with("Do not force a candidate just to produce output."),
        "末尾: {:?}",
        instructions.chars().rev().take(60).collect::<String>()
    );
    // 抜けやすい規則が入っていること（ADR-0047 D4「保存するもの・しないもの・出典」）。
    for needle in ["Never include secrets", "Always attach at least one source"] {
        assert!(instructions.contains(needle), "{needle}");
    }

    let preamble = knowledge_fallback_instructions("artifacts/knowledge-candidates.json");
    assert!(preamble.contains(instructions));
    assert!(preamble.contains("artifacts/knowledge-candidates.json"));
    assert!(preamble.contains("\"candidates\": []"));
    assert!(preamble.contains("他のファイルは作らない"));
    assert!(preamble.contains("道具は使わない"));
}

#[test]
fn provider_kind_langmem_fallback_describes_configured_endpoint() {
    let preamble = knowledge_fallback_instructions("artifacts/knowledge-candidates.json");
    assert!(preamble.contains("設定された `langmem` の接続先"));
    assert!(!preamble.contains("Qwen"));
}

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    heartbeat_count: Mutex<u32>,
    artifacts: Mutex<Vec<ArtifactRef>>,
}

impl EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
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

/// `local-deep-research` のテストと同じ理由（ADR-0010 D10 の ETXTBSY 対策）で、スタブは別プロセスに
/// 書かせる。**実物の `langmem_run.py` は使わない**（ネットワーク・本物の LLM 無しで検証できるのは
/// アダプタ ↔ python のプロトコルだけ。§7「adapter test with a fake extractor」）。
fn stub_langmem(dir: &Path, script: &str) -> LangMemConfig {
    let path = dir.join("langmem_stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    LangMemConfig {
        command: path.to_string_lossy().into_owned(),
        ..LangMemConfig::default()
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
        wall_clock: std::time::Duration::from_secs(30),
        idle_timeout: std::time::Duration::from_secs(30),
        kill_grace: std::time::Duration::from_millis(200),
    }
}

/// 固定の候補ファイルを書き、`CELERIS_RESULT` を出す偽の抽出器（本物の langmem_run.py の代わり）。
fn fake_extractor_script() -> String {
    r#"input="$2"
candidates_path=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['candidates_path'])" "$input")
echo 'progress: reading the task objective...'
echo 'progress: extracting candidates...'
mkdir -p "$(dirname "$candidates_path")"
cat > "$candidates_path" <<'JSON'
{"candidates": [
  {"op": "create", "path": "environment/tools/newtool.md", "title": "newtool",
   "tags": ["tool"], "scope": "environment", "body": "newtool の使い方。",
   "sources": ["task:01J1"], "confidence": "high"}
]}
JSON
echo 'CELERIS_RESULT {"summary": "1 件の知識の候補を抽出した", "candidates": 1}'
"#
    .to_string()
}

#[tokio::test]
async fn happy_path_writes_candidates_and_result_files() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_langmem(dir.path(), &fake_extractor_script());
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req.clone(), "run-1", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => {
            assert_eq!(summary, "1 件の知識の候補を抽出した")
        }
        other => panic!("expected done, got {other:?}"),
    }
    let progress = sink.progress.lock().unwrap();
    assert!(progress.iter().any(|m| m.contains("extracting candidates")));
    assert!(*sink.heartbeat_count.lock().unwrap() >= 2);
    let artifacts = sink.artifacts.lock().unwrap();
    assert_eq!(artifacts.len(), 1, "{artifacts:?}");
    assert_eq!(artifacts[0].name, "knowledge-candidates.json");
    assert_eq!(artifacts[0].path, "artifacts/knowledge-candidates.json");
    drop(artifacts);

    let candidates_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("artifacts/knowledge-candidates.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(candidates_json["candidates"].as_array().unwrap().len(), 1);

    let result_json = std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert_eq!(parsed["summary"], "1 件の知識の候補を抽出した");

    assert!(dir.path().join("runs/run-1/langmem_run.py").is_file());
    let script = std::fs::read_to_string(dir.path().join("runs/run-1/langmem_run.py")).unwrap();
    assert_eq!(script, RUNNER_SCRIPT);
    let seen_input: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("runs/run-1/langmem_input.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(seen_input["objective"], req.task.objective);
}

#[tokio::test]
async fn provider_kind_langmem_passes_proxy_cheap_model_to_stub() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_langmem(dir.path(), &fake_extractor_script());
    config.base_url = Some("http://127.0.0.1:18000/v1".to_string());
    config.model = Some("celeris/cheap".to_string());
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-proxy", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let input: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("runs/run-proxy/langmem_input.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(input["llm"]["model"], "celeris/cheap");
    assert_eq!(input["llm"]["base_url"], "http://127.0.0.1:18000/v1");
}

/// `langmem` が import できない（venv 未セットアップ）は `retryable = false`（ADR-0047 D4）。
#[tokio::test]
async fn missing_langmem_package_is_not_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_langmem(
        dir.path(),
        "echo 'CELERIS_LANGMEM_MISSING: No module named langmem' 1>&2\nexit 1\n",
    );
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-2", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(!retryable, "{message}");
            assert!(message.contains("langmem is not importable"), "{message}");
            assert!(message.contains("setup-langmem.sh"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
    assert!(!dir.path().join("artifacts/result.json").exists());
}

#[tokio::test]
async fn a_non_zero_exit_without_the_missing_marker_is_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_langmem(dir.path(), "echo 'boom' 1>&2\nexit 7\n");
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-3", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=7"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// 候補ファイルを書かずに終わるのは（`CELERIS_RESULT` があっても）retryable エラー。
#[tokio::test]
async fn missing_candidates_file_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_langmem(
        dir.path(),
        "echo 'CELERIS_RESULT {\"summary\": \"x\", \"candidates\": 0}'\n",
    );
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-4", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("knowledge-candidates.json"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// 候補が 0 件でも（空の `candidates: []`）正常終了になる（ADR-0047 D4「何も抽出するものが無ければ
/// 空でよい」）。
#[tokio::test]
async fn zero_candidates_is_still_done() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"input="$2"
candidates_path=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['candidates_path'])" "$input")
mkdir -p "$(dirname "$candidates_path")"
printf '{"candidates": []}' > "$candidates_path"
echo 'CELERIS_RESULT {"summary": "", "candidates": 0}'
"#;
    let config = stub_langmem(dir.path(), script);
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-5", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert!(summary.contains('0'), "{summary}"),
        other => panic!("expected done, got {other:?}"),
    }
}

/// 壁時計の超過でプロセスグループごと SIGKILL する（他の python サイドカーと同じ確認方法）。
#[tokio::test]
async fn wall_clock_exceeded_kills_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid.txt");
    let config = stub_langmem(
        dir.path(),
        &format!(
            "echo $$ > {pid}\nwhile true; do sleep 0.1; done\n",
            pid = pid_file.display()
        ),
    );
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: std::time::Duration::from_millis(300),
        idle_timeout: std::time::Duration::from_secs(30),
        kill_grace: std::time::Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-6", limits, &sink).await.unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("wall clock exceeded"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
    let pid_text = std::fs::read_to_string(&pid_file)
        .expect("stub should have recorded its pid before looping");
    let pid: i32 = pid_text
        .trim()
        .parse()
        .expect("pid.txt should contain a pid");
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "process {pid} should have been killed"
    );
}

/// `[adapters.langmem].idle_timeout_secs` はハーネスの予算を上回れない（min を取る）。
#[tokio::test]
async fn idle_timeout_secs_cannot_exceed_the_harness_budget() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid.txt");
    let mut config = stub_langmem(
        dir.path(),
        &format!(
            "echo $$ > {pid}\nwhile true; do sleep 0.1; done\n",
            pid = pid_file.display()
        ),
    );
    config.idle_timeout_secs = Some(3600); // 大きすぎる値。
    let adapter = LangMemAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: std::time::Duration::from_secs(30),
        idle_timeout: std::time::Duration::from_millis(300),
        kill_grace: std::time::Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-7", limits, &sink).await.unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(matches!(outcome.terminal, Terminal::Error { .. }));
}

/// `with_env` の追加分は既存の同名キーより後に環境を組み立てるので勝つ（他アダプタと同じ規則）。
#[tokio::test]
async fn with_env_overrides_a_same_name_key_already_in_config_env() {
    let dir = tempfile::tempdir().unwrap();
    let out_file = dir.path().join("env-seen.txt");
    let script = format!(
        "printf '%s' \"$OPENAI_API_KEY\" > {out}\n\
             echo 'CELERIS_RESULT {{\"summary\": \"ok\", \"candidates\": 0}}'\n\
             input=\"$2\"\n\
             candidates_path=$(python3 -c \"import json,sys; print(json.load(open(sys.argv[1]))['candidates_path'])\" \"$input\")\n\
             mkdir -p \"$(dirname \"$candidates_path\")\"\n\
             printf '{{\"candidates\": []}}' > \"$candidates_path\"\n",
        out = out_file.display()
    );
    let mut config = stub_langmem(dir.path(), &script);
    config
        .env
        .push(("OPENAI_API_KEY".to_string(), "old-key".to_string()));
    let base = LangMemAdapter::new(config);
    let with_env = base
        .with_env(&[("OPENAI_API_KEY".to_string(), "new-key".to_string())])
        .expect("langmem supports with_env");

    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = with_env
        .run(req, "run-8", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(seen, "new-key");
}

/// ADR-0139 D4: 起動 env の鍵を `artifacts/seen-env.txt` に写してから偽の抽出器を走らせるスタブ。
fn env_recording_script() -> String {
    format!(
        "artifacts=$(python3 -c \"import json,sys,os; print(os.path.dirname(json.load(open(sys.argv[1]))['candidates_path']))\" \"$2\")\n\
         mkdir -p \"$artifacts\"\n\
         printf '%s' \"${{OPENAI_API_KEY-<unset>}}\" > \"$artifacts/seen-env.txt\"\n\
         {}",
        fake_extractor_script()
    )
}

/// ADR-0139 D4: tier（cheap / standard）と provider の env（古い `OPENAI_API_KEY` あり / なし）の
/// どの組み合わせでも、langmem の起動 env と入力 JSON に設定された鍵が入る（LLM は呼ばない）。
#[tokio::test]
async fn langmem_api_key_reaches_runner_env_for_every_tier_and_provider_env() {
    use crate::tiered::TieredAdapter;
    for tier in [task_core::Tier::Cheap, task_core::Tier::Standard] {
        for provider_env in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut config = stub_langmem(dir.path(), &env_recording_script());
            config.base_url = Some("http://127.0.0.1:18100/v1".to_string());
            config.model = Some("celeris/cheap".to_string());
            config.api_key = Some("proxy-bearer".to_string());
            if provider_env {
                // `[adapters.langmem].env` / provider 行の env に残った古い値。
                config
                    .env
                    .push(("OPENAI_API_KEY".to_string(), "stale-row".to_string()));
            }
            let tiered = TieredAdapter {
                base: Arc::new(LangMemAdapter::new(config)),
                models: task_core::model_routing::TierModels::new(),
                account_id: None,
                credential_error: None,
            };
            // dispatcher がアカウント等で重ねる env（`with_env`）も鍵を上書きしない。
            let adapter: Arc<dyn WorkerAdapter> = if provider_env {
                tiered
                    .with_env(&[("OPENAI_API_KEY".to_string(), "stale-extra".to_string())])
                    .unwrap()
            } else {
                Arc::new(tiered)
            };
            let mut req = sample_req(dir.path().to_path_buf());
            req.task.worker_hint.tier = tier;
            let sink = RecordingSink::default();
            let outcome = adapter
                .run(req, "run-key", default_limits(), &sink)
                .await
                .unwrap();
            assert!(
                matches!(outcome.terminal, Terminal::Done { .. }),
                "{tier:?} provider_env={provider_env}: {:?}",
                outcome.terminal
            );
            let seen = std::fs::read_to_string(dir.path().join("artifacts/seen-env.txt")).unwrap();
            assert_eq!(seen, "proxy-bearer", "{tier:?} provider_env={provider_env}");
            let input: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.path().join("runs/run-key/langmem_input.json"))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(input["llm"]["api_key"], "proxy-bearer");
            assert_eq!(input["llm"]["base_url"], "http://127.0.0.1:18100/v1");
        }
    }
}

/// ADR-0139 D4: 鍵の env 名は provider で決まり、鍵が無ければ何も入れない（provider の env を消さない）。
#[test]
fn langmem_api_key_env_follows_provider_and_is_empty_without_key() {
    let mut config = LangMemConfig {
        api_key: Some("k".to_string()),
        ..LangMemConfig::default()
    };
    assert_eq!(
        api_key_env(&config),
        vec![("OPENAI_API_KEY".to_string(), "k".to_string())]
    );
    config.provider = LangMemProvider::Anthropic;
    assert_eq!(
        api_key_env(&config),
        vec![("ANTHROPIC_API_KEY".to_string(), "k".to_string())]
    );
    config.api_key = None;
    assert!(api_key_env(&config).is_empty());
    config.api_key = Some(String::new());
    assert!(api_key_env(&config).is_empty());
}

/// ADR-0139 D4: ランナーは JSON に鍵が無ければ env の鍵を使う（埋め込みの台本の文面で固定）。
#[test]
fn langmem_runner_falls_back_to_env_api_key() {
    assert!(RUNNER_SCRIPT.contains(
        "os.environ.get(\"ANTHROPIC_API_KEY\" if provider == \"anthropic\" else \"OPENAI_API_KEY\")"
    ));
    assert!(RUNNER_SCRIPT.contains("api_key = llm.get(\"api_key\") or env_key or \"not-needed\""));
}
