use std::sync::Mutex;
use std::time::Duration;

use task_core::{ArtifactRef, DelegateTask};

use super::*;
use crate::protocol::{PROTOCOL_VERSION, RunContext};

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    /// ADR-0048 D2（Phase 60a）: 構造化した進行（`msg` と一緒に）。
    structured: Mutex<Vec<(String, task_core::ProgressFields)>>,
    delegated: Mutex<Vec<Vec<DelegateTask>>>,
    rate_limits: Mutex<Vec<task_core::RateLimitObservation>>,
    /// ADR-0054 D1（Phase 67）: `session_established` の呼び出し。
    sessions: Mutex<Vec<String>>,
    /// ADR-0054 D1（Phase 67）: `session_resume_failed` の呼び出し。
    resume_failed: Mutex<Vec<String>>,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: `policy_violation` の呼び出し。
    violations: Mutex<Vec<crate::tool_policy::ToolPolicyViolation>>,
}

impl EventSink for RecordingSink {
    fn policy_violation(&self, violation: &crate::tool_policy::ToolPolicyViolation) {
        self.violations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(violation.clone());
    }
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
    }
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        self.progress(msg);
        self.structured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((msg.to_string(), fields.clone()));
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, tasks: &[DelegateTask]) {
        self.delegated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tasks.to_vec());
    }
    fn rate_limit(&self, obs: task_core::RateLimitObservation) {
        self.rate_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(obs);
    }
    fn session_established(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id.to_string());
    }
    fn session_resume_failed(&self, reason: &str) {
        self.resume_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(reason.to_string());
    }
}

fn stub_codex(dir: &std::path::Path, script: &str) -> CodexConfig {
    let path = dir.join("codex_stub.sh");
    // ETXTBSY 対策（ADR-0010 D10）: テストプロセス自身が書き込み fd を持たないよう別プロセスで書く。
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    CodexConfig {
        command: path.to_string_lossy().into_owned(),
        env: vec![(
            "CODEX_HOME".into(),
            dir.join("codex-home").to_string_lossy().into_owned(),
        )],
        ..CodexConfig::default()
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
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    }
}

/// ADR-0048 D2（Phase 60a）: codex の `item.*` の標本（`tests/fixtures/codex-stream.jsonl`）を
/// `handle_line` に通し、`tool_use` / `tool_result` / `text` / `thinking`、それ以外は `status` に
/// なることを確かめる。`msg` は従来どおり行そのもの（500 バイトで切る）。
#[test]
fn json_events_map_to_structured_progress() {
    use task_core::ProgressKind;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex-stream.jsonl"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let sink = RecordingSink::default();
    let (mut signal, mut error) = (None, None);
    for line in text.lines() {
        handle_line(line, &sink, &mut signal, &mut error);
    }
    let items = sink
        .structured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let kinds: Vec<Option<ProgressKind>> = items.iter().map(|(_, f)| f.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Some(ProgressKind::Thinking),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::Text),
            Some(ProgressKind::Status),
        ],
        "{items:#?}"
    );
    assert_eq!(
        items[0].1.summary.as_deref(),
        Some("テストを回して確かめる")
    );
    assert_eq!(items[1].1.tool.as_deref(), Some("command_execution"));
    assert_eq!(
        items[1].1.summary.as_deref(),
        Some("cargo test --workspace")
    );
    assert_eq!(
        items[2].1.summary.as_deref(),
        Some("test result: ok. 812 passed")
    );
    assert!(!items[2].1.error);
    // `exit_code != 0` は失敗の印。
    assert!(items[3].1.error, "{:?}", items[3]);
    assert_eq!(items[4].1.summary.as_deref(), Some("テストは通りました。"));
    // 知らない item（`todo_list`）は節目として残る。
    assert_eq!(
        items[5].1.summary.as_deref(),
        Some("item.completed todo_list")
    );
    // `msg` は従来どおり行そのもの。
    assert!(items[1].0.contains("command_execution"), "{}", items[1].0);
    assert!(matches!(
        signal,
        Some(TurnSignal::Completed {
            usage: Some(Usage {
                cache_read_tokens: Some(4),
                cache_creation_tokens: Some(2),
                ..
            })
        })
    ));
    assert!(error.is_none());
}

#[tokio::test]
async fn happy_path_progress_and_done_from_result_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
echo '{"type":"thread.started"}'
echo '{"type":"item.started","item":{"type":"command_execution","command":"cargo test"}}'
printf '%s' '{"summary":"added usage example","evidence":[]}' > artifacts/result.json
echo '{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":20}}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-1", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done {
            summary,
            evidence,
            usage,
        } => {
            assert_eq!(summary, "added usage example");
            assert!(evidence.is_empty());
            assert_eq!(
                usage,
                Some(Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(20),
                    cache_read_tokens: Some(4),
                    cache_creation_tokens: None,
                    cost_usd: None,
                    duplicate_reads: None,
                    session_resumed: None,
                })
            );
        }
        other => panic!("expected done, got {other:?}"),
    }
    let progress = sink.progress.lock().unwrap();
    assert!(progress.iter().any(|m| m.contains("command_execution")));
    assert!(dir.path().join("runs/run-1/stdout.jsonl").is_file());

    // P-26 (ADR-0010 D10): the terminal is also normalized into `runs/<run_id>/result.json`,
    // readable by task-dispatch as a `WorkerMessage::Done`.
    let result_json = std::fs::read_to_string(dir.path().join("runs/run-1/result.json")).unwrap();
    match serde_json::from_str::<crate::protocol::WorkerMessage>(result_json.trim()).unwrap() {
        crate::protocol::WorkerMessage::Done { summary, .. } => {
            assert_eq!(summary, "added usage example")
        }
        other => panic!("expected done in result.json, got {other:?}"),
    }
}

/// 付属ファイル（入れ子を含む）を持つ skill を KB 側に作る。本文には印の文字列を入れる。
fn skills_fixture_kb(kb: &std::path::Path, name: &str) -> crate::protocol::SkillMount {
    let skill_dir = kb.join(name);
    std::fs::create_dir_all(skill_dir.join("rules/nested")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n\nBODY-MARKER レビューの手順\n"),
    )
    .unwrap();
    std::fs::write(skill_dir.join("rules/x.md"), "rule x\n").unwrap();
    std::fs::write(skill_dir.join("rules/nested/y.md"), "rule y\n").unwrap();
    crate::protocol::SkillMount {
        name: name.into(),
        path: skill_dir.display().to_string(),
        description: "d".into(),
    }
}

const SKILLS_OK_SCRIPT: &str = r#"mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#;

/// ADR-0127 D1/D3（旧 ADR-0056 D3 の試験を更新）: `context.skills` に乗った skill は、run 開始時に
/// `AGENTS.md` の `<!-- celeris:skills:start -->` 〜 `end` の節に名前・説明・パスの一覧として書かれる
/// （本文と `### <name>` の見出しは埋め込まない）。既存の内容は保つ。
#[tokio::test]
async fn mounted_skills_are_written_into_agents_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "# Notes\n\nBuild with cargo.\n",
    )
    .unwrap();
    let kb = tempfile::tempdir().unwrap();
    let mount = skills_fixture_kb(kb.path(), "rust-review");
    let adapter = CodexAdapter::new(stub_codex(dir.path(), SKILLS_OK_SCRIPT));
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.skills = vec![mount];
    let sink = RecordingSink::default();
    adapter
        .run(req, "run-skills", default_limits(), &sink)
        .await
        .unwrap();
    let agents_md = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("# Notes"), "{agents_md}");
    assert!(agents_md.contains("Build with cargo."), "{agents_md}");
    assert!(agents_md.contains("## Skills（celeris）"), "{agents_md}");
    assert!(
        agents_md.contains("- `rust-review` — d（`.agents/skills/rust-review/SKILL.md`）"),
        "{agents_md}"
    );
    assert!(!agents_md.contains("### rust-review"), "{agents_md}");
    assert!(!agents_md.contains("BODY-MARKER"), "{agents_md}");
}

/// ADR-0127 D1/D2/D6: codex では skill のディレクトリが付属ファイルまで `.agents/skills/<name>/` に
/// 丸写しされ、unmount（次の run で `skills` 空）で写しと `AGENTS.md` の節が消える。人が置いた
/// `.agents/skills/<別名>/`・`.agents/` の他のファイル・`AGENTS.md` の区切りの外には触れない。
#[tokio::test]
async fn codex_skills_deliver_sibling_files_and_unmount_keeps_human_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Human notes\n").unwrap();
    std::fs::create_dir_all(dir.path().join(".agents/skills/human-skill")).unwrap();
    std::fs::write(
        dir.path().join(".agents/skills/human-skill/SKILL.md"),
        "human\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".agents/notes.txt"), "keep\n").unwrap();
    let kb = tempfile::tempdir().unwrap();
    let mount = skills_fixture_kb(kb.path(), "shadcn");
    let adapter = CodexAdapter::new(stub_codex(dir.path(), SKILLS_OK_SCRIPT));

    let mut req = sample_req(dir.path().to_path_buf());
    req.context.skills = vec![mount];
    let sink = RecordingSink::default();
    adapter
        .run(req, "run-skills-1", default_limits(), &sink)
        .await
        .unwrap();
    let copy = dir.path().join(".agents/skills/shadcn");
    assert!(
        std::fs::read_to_string(copy.join("SKILL.md"))
            .unwrap()
            .contains("BODY-MARKER")
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("rules/x.md")).unwrap(),
        "rule x\n"
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("rules/nested/y.md")).unwrap(),
        "rule y\n"
    );
    assert!(copy.join(".gitignore").is_file());
    let agents_md = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(
        agents_md.contains("`.agents/skills/shadcn/SKILL.md`"),
        "{agents_md}"
    );
    assert!(!agents_md.contains("BODY-MARKER"), "{agents_md}");

    let req = sample_req(dir.path().to_path_buf());
    adapter
        .run(req, "run-skills-2", default_limits(), &sink)
        .await
        .unwrap();
    assert!(!copy.exists());
    let agents_md = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(agents_md, "# Human notes\n");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".agents/skills/human-skill/SKILL.md")).unwrap(),
        "human\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".agents/notes.txt")).unwrap(),
        "keep\n"
    );
}

#[tokio::test]
async fn direct_reply_is_only_accepted_for_successful_conversations() {
    for (conversation, ending, file, done) in [
        (true, "echo '{\"type\":\"turn.completed\"}'", "", true),
        (false, "echo '{\"type\":\"turn.completed\"}'", "", false),
        (
            true,
            "echo '{\"type\":\"turn.failed\",\"error\":\"failed\"}'",
            "",
            false,
        ),
        (
            true,
            "echo '{\"type\":\"turn.completed\"}'; exit 1",
            "",
            false,
        ),
        (
            true,
            "echo '{\"type\":\"turn.completed\"}'",
            "mkdir -p artifacts; echo invalid > artifacts/result.json",
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config = stub_codex(
            dir.path(),
            &format!(
                "{file}\necho '{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"接続確認OK\"}}}}'\n{ending}"
            ),
        );
        let mut req = sample_req(dir.path().to_path_buf());
        if conversation {
            req.task.conversation = Some(task_core::MessageId::new());
        }
        let outcome = CodexAdapter::new(config)
            .run(req, "reply", default_limits(), &RecordingSink::default())
            .await
            .unwrap();
        assert_eq!(
            matches!(outcome.terminal, Terminal::Done { .. }),
            done,
            "{:?}",
            outcome.terminal
        );
    }
}

#[tokio::test]
async fn worktree_can_write_results_outside_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/code");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = stub_codex(
        dir.path(),
        r#"
artifact_root=''
while [ "$#" -gt 0 ]; do
    if [ "$1" = '--add-dir' ]; then shift; [ -n "$artifact_root" ] || artifact_root="$1"; fi
    shift
done
[ -n "$artifact_root" ] && [ -d "$artifact_root" ] || exit 10
[ "$PWD" != "$artifact_root" ] || exit 11
printf '%s' '{"summary":"worktree result saved","evidence":[]}' > "$artifact_root/result.json"
echo '{"type":"turn.completed"}'
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    // Covers shared task-specific artifact directories as well.
    req.artifacts_dir = dir.path().join(".taskd/artifacts/task-a");
    let result_path = req.artifact_path("result.json");
    let outcome = CodexAdapter::new(config)
        .run(
            req,
            "external-artifacts",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Done { summary, .. } if summary == "worktree result saved")
    );
    assert!(result_path.is_file());
    assert!(!work_dir.join("artifacts/result.json").exists());
}

/// ADR-0006 Phase 115 D1（本番障害 01M3915FARENW8M0JM11XVF6W0）: `work_dir != workspace` の run
/// では、プロンプト冒頭に cwd と成果物ディレクトリの絶対パスの注意が出る（`claude_code::build_prompt`
/// を再利用する `codex` アダプタでも同じ。D4(a)）。
#[tokio::test]
async fn work_dir_note_appears_in_the_prompt_when_work_dir_differs_from_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/agent-platform");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = stub_codex(
        dir.path(),
        r#"
artifact_root=''
while [ "$#" -gt 0 ]; do
    if [ "$1" = '--add-dir' ]; then shift; [ -n "$artifact_root" ] || artifact_root="$1"; fi
    shift
done
printf '%s' '{"summary":"ok","evidence":[]}' > "$artifact_root/result.json"
echo '{"type":"turn.completed"}'
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    let outcome = CodexAdapter::new(config)
        .run(req, "run-wd-1", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-wd-1/prompt.txt")).unwrap();
    assert!(
        prompt.contains(&format!("cwd は `{}`", work_dir.display())),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "成果物ディレクトリは `{}`",
            dir.path().join("artifacts").display()
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains("相対 `artifacts/` はリポジトリの中を指すので使わない"),
        "{prompt}"
    );
}

/// ADR-0006 Phase 115 D2（本番障害 01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD）:
/// `codex` が `--add-dir` を無視して（あるいは resume で落として）cwd 相対の `artifacts/result.json`
/// に書いてしまっても、正しい置き場へ移して採用し `Done` になる。worktree 側には残らない（D4(b)）。
#[tokio::test]
async fn a_result_json_written_under_work_dir_is_adopted_and_not_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/agent-platform");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"wrote to the worktree by mistake","evidence":[]}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    let outcome = CodexAdapter::new(config)
        .run(req, "run-wd-2", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => {
            assert_eq!(summary, "wrote to the worktree by mistake")
        }
        other => panic!("expected done, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap(),
        r#"{"summary":"wrote to the worktree by mistake","evidence":[]}"#
    );
    assert!(
        !work_dir.join("artifacts").exists(),
        "the stray artifacts/ dir under work_dir should be gone"
    );
}

#[tokio::test]
async fn turn_failed_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"turn.failed","error":"sandbox denied write"}'"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-2", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("sandbox denied write"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// 実機（codex-cli 0.154.0, 2026-09-14 確認）では `turn.failed.error` はオブジェクト
/// （`{"message":"..."}`）で返ってくる。文字列を仮定すると読み落とす回帰テスト。
#[tokio::test]
async fn turn_failed_with_object_shaped_error_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"turn.failed","error":{"message":"the model is not supported"}}'"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-2b", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("the model is not supported"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// `turn.failed.error.message` が供給側失敗の文言に一致すれば `AdapterError::Exhausted` として
/// 返る（ADR-0010 D5）。result.json も書かれる。
#[tokio::test]
async fn turn_failed_classified_as_exhausted_surfaces_as_adapter_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"turn.failed","error":{"message":"You'"'"'ve hit your usage limit"}}'"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-2c", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::Exhausted(_)), "{err:?}");
    assert!(dir.path().join("runs/run-2c/result.json").is_file());
}

/// `turn.*` を一度も観測できずに exit した場合も `{"type":"error",...}` の直前の行を分類する
/// （ADR-0010 D5）。
#[tokio::test]
async fn crash_with_matching_error_line_is_classified_as_provider_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"error","message":"429 Too Many Requests"}'
exit 9
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-2d", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::Throttled { .. }), "{err:?}");
}

/// ADR-0036 D1/D2: 共有 workspace のタスクは `.taskd/artifacts/<task_id>/result.json` を読む。
#[tokio::test]
async fn a_shared_workspace_task_uses_its_own_artifacts_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p .taskd/artifacts/T1
printf '%s' '{"summary":"mine","evidence":[]}' > .taskd/artifacts/T1/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts/result.json"),
        r#"{"summary":"sibling"}"#,
    )
    .unwrap();
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.artifacts_dir = dir.path().join(".taskd/artifacts/T1");
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-shared", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "mine"),
        other => panic!("expected done, got {other:?}"),
    }
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-shared/prompt.txt")).unwrap();
    assert!(
        prompt.contains(".taskd/artifacts/T1/result.json"),
        "{prompt}"
    );
}

#[tokio::test]
async fn success_without_result_file_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), r#"echo '{"type":"turn.completed"}'"#);
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-3", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("artifacts/result.json"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn question_in_result_file_blocks_task() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"question":"which crate version?"}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-4", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Question { text } => assert_eq!(text, "which crate version?"),
        other => panic!("expected question, got {other:?}"),
    }
}

#[tokio::test]
async fn crash_without_turn_message_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), "exit 9");
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-5", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.exit_code, Some(9));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=9"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// クラッシュ前に `artifacts/result.json` が存在していても、`turn.completed`/`turn.failed` を
/// 一度も観測できなければ信用しない（ADR-0006 D4 と同じ回帰、ADR-0008 D3）。
#[tokio::test]
async fn stale_result_file_without_turn_message_is_not_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"looks done but crashed before saying so","evidence":[]}' > artifacts/result.json
exit 9
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-6", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.exit_code, Some(9));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("exit=9"), "{message}");
        }
        other => panic!("expected error (stale file must not be trusted), got {other:?}"),
    }
}

/// 前回の run が残した `artifacts/result.json` は、今回の run 開始時に消される（ADR-0006 D3 と同じ）。
#[tokio::test]
async fn stale_result_file_from_previous_run_is_cleared_before_this_run() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts/result.json"),
        r#"{"summary":"stale from a previous attempt","evidence":[]}"#,
    )
    .unwrap();
    let config = stub_codex(dir.path(), r#"echo '{"type":"turn.completed"}'"#);
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-7", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("artifacts/result.json"), "{message}");
        }
        other => {
            panic!("expected error (stale file must be cleared, not reused), got {other:?}")
        }
    }
}

/// ADR-0072 D7/§6 (i)（Phase E1）: codex には turn の上限が無いので、wall-clock の打ち切りが
/// continuation の唯一の入口になる（`Terminal::BudgetExhausted{kind: WallClock}`）。
#[tokio::test]
async fn wall_clock_exceeded_kills_and_reports_budget_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), "sleep 30");
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-8", limits, &sink).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::BudgetExhausted { kind, message, .. } => {
            assert_eq!(kind, task_core::BudgetKind::WallClock);
            assert!(message.contains("wall clock exceeded"), "{message}");
        }
        other => panic!("expected budget_exhausted, got {other:?}"),
    }
}

/// ADR-0072 D9（Phase E1）: `result.json` の `{"yield": {...}}` が `Terminal::Yielded` になる。
#[tokio::test]
async fn result_yield_becomes_terminal_yielded() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"yield":{"completed":["A"],"next_action":"do B"}}' > artifacts/result.json
echo '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":3}}'
"#,
    );
    let adapter = CodexAdapter::new(config);
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

#[tokio::test]
async fn idle_timeout_kills_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"item.started","item":{"type":"agent_message"}}'
sleep 30
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(300),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-9", limits, &sink).await.unwrap();
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
async fn malformed_evidence_in_result_file_does_not_fail_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"all good","evidence":["cargo test: 4 passed",{"criterion":0,"command":"cargo test","exit":0,"stdout_tail":""},42]}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-10", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done {
            summary, evidence, ..
        } => {
            assert_eq!(summary, "all good");
            assert_eq!(evidence.len(), 1);
            assert_eq!(evidence[0].command.as_deref(), Some("cargo test"));
        }
        other => panic!("expected done, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_result_file_json_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf 'not json' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-11", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("not valid JSON"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// D3 の核心契約（`exec --json`、`--model` の位置、プロンプトが最終引数）が壊れてもテストが
/// 緑のままにならないよう、実際に渡された引数をファイルに記録して検証する（監査で指摘）。
#[tokio::test]
async fn command_line_has_exec_json_model_then_prompt_as_last_arg() {
    let dir = tempfile::tempdir().unwrap();
    let config = CodexConfig {
        command: {
            let path = dir.path().join("codex_stub.sh");
            // 引数は改行を含みうる（プロンプト）ので NUL 区切りで記録する。
            // ETXTBSY 対策（ADR-0010 D10）: 別プロセスで書く。
            crate::test_support::write_executable(
                &path,
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\" >> \"$(dirname \"$0\")/args.log\"; done\necho '{\"type\":\"turn.completed\"}'\n",
            );
            path.to_string_lossy().into_owned()
        },
        extra_args: vec!["--sandbox".into(), "read-only".into()],
        model: Some("gpt-5-codex".into()),
        reasoning_effort: None,
        env: vec![(
            "CODEX_HOME".into(),
            dir.path().join("codex-home").to_string_lossy().into_owned(),
        )],
        container: None,
        resume_mode: CodexResumeMode::default(),
        resume_bypass: CodexResumeBypass::default(),
        subagents: Default::default(),
    };
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-12", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Error { .. }));

    let args_log = std::fs::read_to_string(dir.path().join("args.log")).unwrap();
    let args: Vec<&str> = args_log.split('\0').filter(|s| !s.is_empty()).collect();
    assert_eq!(
        args.len(),
        16,
        "expected exactly one trailing prompt arg, got {args:?}"
    );
    assert_eq!(
        &args[..7],
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "--model",
            "gpt-5-codex"
        ]
    );
    assert_eq!(args[7], "--add-dir");
    assert_eq!(args[8], dir.path().join("artifacts").to_str().unwrap());
    assert_eq!(&args[9..11], ["--sandbox", "read-only"]);
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent の無効化は `extra_args` の後ろ。
    assert_eq!(
        &args[11..15],
        [
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.multi_agent_v2=false"
        ]
    );
    // F5-fix10: 最終引数は stdin を指す `-`。プロンプト本文はどの引数にも載らない。
    assert_eq!(args[15], "-", "{args:?}");
    assert!(
        args.iter().all(|a| !a.contains("# Task:")),
        "the prompt must not be passed via argv: {args:?}"
    );
}

/// F5-fix10（本番障害 run 01M3Q21Z9JQWWANGHXJPNH1F8X）: MAX_ARG_STRLEN（131072）を超える 200 KiB の
/// プロンプトも、stdin から欠けずに届く（fake の codex が stdin をそのままファイルに写す）。
#[tokio::test]
async fn f5_fix10_a_200_kib_prompt_reaches_codex_intact_through_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        &format!("cat > stdin.log\n{}", args_log_script()),
    );
    let mut req = sample_req(dir.path().to_path_buf());
    let filler = "0123456789abcdef".repeat(200 * 1024 / 16);
    req.task.objective = format!("BEGIN-OBJECTIVE {filler} END-OBJECTIVE");
    let outcome = CodexAdapter::new(config)
        .run(
            req,
            "run-f5fix10-codex",
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
    let got = std::fs::read_to_string(dir.path().join("stdin.log")).unwrap();
    let recorded =
        std::fs::read_to_string(dir.path().join("runs/run-f5fix10-codex/prompt.txt")).unwrap();
    assert!(got.len() > crate::subprocess::MAX_SINGLE_ARG_BYTES);
    assert_eq!(got, recorded, "stdin carries exactly the recorded prompt");
    assert!(got.contains(&filler));
    let args = captured_args(dir.path());
    assert_eq!(args.last().map(String::as_str), Some("-"), "{args:?}");
    assert!(args.iter().all(|a| a.len() < 4096), "{args:?}");
}

/// ADR-0016 M8: `codex` も run の終わりに `artifacts/delegate.json` があれば `sink.delegate` を 1 回呼ぶ。
#[tokio::test]
async fn delegate_json_written_by_worker_is_forwarded_to_sink() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
printf '%s' '{"summary":"delegated two subtasks","evidence":[]}' > artifacts/result.json
printf '%s' '{"tasks":[{"title":"a","objective":"do a","acceptance":[{"text":"c","check":{"type":"human"}}]},{"title":"b","objective":"do b","acceptance":[{"text":"c","check":{"type":"human"}}]}]}' > artifacts/delegate.json
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-13", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let delegated = sink.delegated.lock().unwrap();
    assert_eq!(delegated.len(), 1);
    assert_eq!(delegated[0].len(), 2);
}

/// ADR-0025 D3: `token_count` の `rate_limits` を解析すると `sink.rate_limit` に観測値が渡る。
#[tokio::test]
async fn token_count_event_line_is_forwarded_to_the_sink() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"mkdir -p artifacts
echo '{"type":"token_count","rate_limits":{"primary":{"used_percent":14.0,"window_minutes":300,"resets_in_seconds":3600},"secondary":{"used_percent":24.0,"window_minutes":10080,"resets_in_seconds":432000}}}'
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-rate-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let observed = sink.rate_limits.lock().unwrap();
    assert_eq!(observed.len(), 1);
    let obs = &observed[0];
    assert_eq!(obs.five_hour.map(|w| w.utilization), Some(0.14));
    assert_eq!(obs.seven_day.map(|w| w.utilization), Some(0.24));
}

/// ADR-0025 D2: `with_env` の追加分（`CODEX_HOME`）は既存の同名キーより後に環境を組み立てるので勝つ。
#[tokio::test]
async fn with_env_overrides_a_same_name_key_already_in_config_env() {
    let dir = tempfile::tempdir().unwrap();
    let out_file = dir.path().join("env-seen.txt");
    let mut config = CodexConfig {
        command: {
            let path = dir.path().join("codex_stub.sh");
            crate::test_support::write_executable(
                &path,
                &format!(
                    "#!/bin/sh\nmkdir -p artifacts\nprintf '%s' \"$CODEX_HOME\" > {out}\nprintf '%s' '{{\"summary\":\"ok\",\"evidence\":[]}}' > artifacts/result.json\necho '{{\"type\":\"turn.completed\"}}'\n",
                    out = out_file.display()
                ),
            );
            path.to_string_lossy().into_owned()
        },
        ..CodexConfig::default()
    };
    config
        .env
        .push(("CODEX_HOME".to_string(), "old-account-dir".to_string()));
    let base = CodexAdapter::new(config);
    let with_env = base
        .with_env(&[("CODEX_HOME".to_string(), "new-account-dir".to_string())])
        .expect("codex supports with_env");

    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = with_env
        .run(req, "run-env-1", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(seen, dir.path().join("new-account-dir").to_string_lossy());
}
#[tokio::test]
async fn tier_binding_reaches_cli_model_argument_and_preserves_account_env() {
    use task_core::{Tier, model_routing::ModelBinding};
    for tier in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
        let dir = tempfile::tempdir().unwrap();
        let config = stub_codex(
            dir.path(),
            r#"
for a in "$@"; do printf '%s\0' "$a" >> args.log; done
printf '%s' "$ROUTING_ACCOUNT" > account.log
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#,
        );
        let expected = format!("explicit-{tier:?}");
        let adapter = crate::tiered::TieredAdapter {
            base: Arc::new(CodexAdapter::new(config)),
            account_id: Some("account-a".into()),
            credential_error: None,
            models: [(
                tier,
                ModelBinding {
                    name: "requested-name".into(),
                    model_id: Some(expected.clone()),
                    unavailable_reason: None,
                    reasoning_effort: None,
                },
            )]
            .into(),
        };
        let adapter = adapter
            .with_env(&[("ROUTING_ACCOUNT".into(), "account-a".into())])
            .unwrap();
        assert_eq!(adapter.account_id(), Some("account-a"));
        let mut req = sample_req(dir.path().to_path_buf());
        req.task.worker_hint.tier = tier;
        let _ = adapter
            .run(req, "tier-run", default_limits(), &RecordingSink::default())
            .await
            .unwrap();
        let args = std::fs::read_to_string(dir.path().join("args.log")).unwrap();
        let args: Vec<_> = args.split('\0').collect();
        let model = args.windows(2).find(|pair| pair[0] == "--model").unwrap()[1];
        assert_eq!(model, expected);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("account.log")).unwrap(),
            "account-a"
        );
    }
}

/// ADR-0069 Phase 118 D1: tier ごとの `reasoning_effort` が実際に `-c
/// model_reasoning_effort="<値>"` として（fresh 起動で）渡ること。`model_id` と両方を検証する。
#[tokio::test]
async fn tier_reasoning_effort_reaches_cli_as_a_dash_c_config_override() {
    use task_core::{Tier, model_routing::ModelBinding};
    let cases = [
        (Tier::Frontier, "explicit-frontier", "high"),
        (Tier::Standard, "explicit-standard", "medium"),
        (Tier::Cheap, "explicit-cheap", "low"),
    ];
    for (tier, model_id, effort) in cases {
        let dir = tempfile::tempdir().unwrap();
        let config = stub_codex(dir.path(), args_log_script());
        let adapter = crate::tiered::TieredAdapter {
            base: Arc::new(CodexAdapter::new(config)),
            account_id: None,
            credential_error: None,
            models: [(
                tier,
                ModelBinding {
                    name: "requested-name".into(),
                    model_id: Some(model_id.into()),
                    unavailable_reason: None,
                    reasoning_effort: Some(effort.into()),
                },
            )]
            .into(),
        };
        let mut req = sample_req(dir.path().to_path_buf());
        req.task.worker_hint.tier = tier;
        let _ = adapter
            .run(
                req,
                "effort-run",
                default_limits(),
                &RecordingSink::default(),
            )
            .await
            .unwrap();
        let args = captured_args(dir.path());
        let model = args.windows(2).find(|pair| pair[0] == "--model").unwrap()[1].clone();
        assert_eq!(model, model_id);
        let expected_override = format!("model_reasoning_effort=\"{effort}\"");
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == expected_override),
            "expected -c {expected_override:?} in {args:?}"
        );
    }
}

/// ADR-0069 Phase 118 D1: `exec resume` の形でも同じ `-c model_reasoning_effort=…` が乗る
/// （resume のホワイトリストは `-c key=value` を任意個数許す。Phase 68c）。
#[tokio::test]
async fn resume_run_also_carries_the_reasoning_effort_config_override() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.model = Some("gpt-6-astra".into());
    config.reasoning_effort = Some("high".into());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-effort".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-resume-effort",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(args[0], "exec");
    assert_eq!(args[1], "resume");
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == "model_reasoning_effort=\"high\""),
        "{args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == "model=\"gpt-6-astra\""),
        "{args:?}"
    );
}

/// ADR-0069 Phase 118 D1: effort が設定されていなければ `-c model_reasoning_effort=…` は現れない
/// （既定の後方互換）。
#[tokio::test]
async fn without_reasoning_effort_no_config_override_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(
            req,
            "run-no-effort",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        !args.iter().any(|a| a.starts_with("model_reasoning_effort")),
        "{args:?}"
    );
}

fn args_log_script() -> &'static str {
    r#"
for a in "$@"; do printf '%s\0' "$a" >> args.log; done
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#
}

fn captured_args(dir: &std::path::Path) -> Vec<String> {
    let args = std::fs::read_to_string(dir.join("args.log")).unwrap();
    args.split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// ADR-0095 D-b: the same rules are present before both fresh and resumed codex processes start.
#[tokio::test]
async fn systemd_deny_rules_are_installed_for_fresh_and_resume() {
    for resume in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut config = stub_codex(
            dir.path(),
            r#"for a in "$@"; do printf '%s\0' "$a" >> args.log; done
printf '%s' "$CODEX_HOME" > codex-home-seen
test -f "$CODEX_HOME/rules/celeris-deny.rules" || exit 19
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'"#,
        );
        config.extra_args = vec!["--approve-for-me".into()];
        let mut req = sample_req(dir.path().to_path_buf());
        if resume {
            req.context.session = Some(crate::protocol::SessionHandle {
                adapter: CodexAdapter::ID.into(),
                session_id: "thread-systemd-deny".into(),
                resume: true,
            });
        }
        let rules_path = dir.path().join("codex-home/rules/celeris-deny.rules");
        std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();
        std::fs::write(&rules_path, "stale rules").unwrap();
        let outcome = CodexAdapter::new(config)
            .run(
                req,
                "run-systemd-deny",
                default_limits(),
                &RecordingSink::default(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome.terminal, Terminal::Done { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("codex-home-seen")).unwrap(),
            dir.path().join("codex-home").to_string_lossy()
        );
        let args = captured_args(dir.path());
        assert_eq!(args.contains(&"resume".to_string()), resume, "{args:?}");
        if resume {
            assert!(args.contains(&"approval_policy=\"never\"".to_string()));
        } else {
            assert!(args.contains(&"--approve-for-me".to_string()));
        }
        assert!(!args.contains(&"--ignore-rules".to_string()));
        let rules = std::fs::read_to_string(rules_path).unwrap();
        assert_eq!(rules, systemd_deny_rules());
        for prefix in SYSTEMD_DENY_PREFIXES {
            let pattern = serde_json::to_string(prefix).unwrap();
            assert!(
                rules.contains(&format!(
                    "prefix_rule(pattern={pattern}, decision=\"forbidden\""
                )),
                "missing {pattern}"
            );
        }
    }
}

#[tokio::test]
async fn systemd_deny_rejects_ignore_rules_before_spawning_codex() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), "touch spawned");
    config.extra_args = vec!["--ignore-rules".into()];
    let err = CodexAdapter::new(config)
        .run(
            sample_req(dir.path().to_path_buf()),
            "run-systemd-deny",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("--ignore-rules"));
    assert!(!dir.path().join("spawned").exists());
}

/// ADR-0054 D1（Phase 67）: `context.session` が無ければ Phase 66 までと同じ（resume の引数は付かない）。
#[tokio::test]
async fn without_a_session_no_resume_flags_are_added() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(req, "run-1", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("experimental_resume=")));
}

/// ADR-0054 D2（Phase 68）: CoS の対話 run（`conversation_addressee = Secretary`）だけ
/// `sandbox_mode="read-only"`。それ以外は従来どおり `workspace-write`。
#[tokio::test]
async fn the_cos_conversation_run_gets_a_readonly_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let _ = adapter
        .run(req, "run-cos", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        args.contains(&"sandbox_mode=\"read-only\"".to_string()),
        "{args:?}"
    );
    assert!(!args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
}

/// 対話でない run・CoS 以外の対話には従来どおり `workspace-write`。
#[tokio::test]
async fn non_cos_runs_keep_the_workspace_write_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(
            req,
            "run-plain",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        args.contains(&"sandbox_mode=\"workspace-write\"".to_string()),
        "{args:?}"
    );
}

// ADR-0054 Phase 68b（本番障害 2026-09-21 15:04 UTC、release b24bae9a796a: CoS の対話が codex に
// 割り当たり、`session.resume=true` の run が `error: unexpected argument '--add-dir' found` で
// exit 2 を 2 回連発した）。
//
// Usage 行は実機（`~/.local/bin/codex exec --help` / `~/.local/bin/codex exec resume --help`、
// codex-cli 0.155.1、2026-09-21）で確認したものをそのまま貼る:
//
//   $ codex exec --help
//   Usage: codex exec [OPTIONS] [PROMPT]
//          codex exec [OPTIONS] <COMMAND> [ARGS]
//   OPTIONS（抜粋）: -c/--config <key=value>, -m/--model <MODEL>, --add-dir <DIR>, --json,
//   --skip-git-repo-check, -s/--sandbox <SANDBOX_MODE> ...
//
//   $ codex exec resume --help
//   Usage: codex exec resume [OPTIONS] [SESSION_ID] [PROMPT]
//   OPTIONS（抜粋）: -c/--config <key=value>, --last, --all, -m/--model <MODEL>, --json,
//   --skip-git-repo-check ... — `--add-dir` も `-s/--sandbox` も無い。
//
// つまり `-c/--config`（sandbox_mode の指定に使う）は両方で通るが、`--add-dir` は `exec resume` では
// 拒否される。celeris 側の対応: `exec resume` のときだけ `--add-dir` を落とす（resume 先のスレッドは
// 最初の（非 resume の）`exec` 呼び出しで受け取った writable-roots をそのまま引き継ぐ前提。ADR-0054
// Phase 68b 追記参照）。

/// (a) CoS の新規（fresh）run: `codex exec --json --skip-git-repo-check -c sandbox_mode="read-only"
/// --add-dir <artifacts_dir> <prompt>`。read-only sandbox と `--add-dir` が両方乗ることを確認する
/// （`exec` の usage 行に `--add-dir` があることに対応。上のコメント参照）。
#[tokio::test]
async fn phase_68b_fresh_cos_run_argv_has_readonly_sandbox_and_add_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let _ = adapter
        .run(
            req,
            "run-68b-fresh",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    let artifacts_dir = dir.path().join("artifacts").to_str().unwrap().to_string();
    assert_eq!(args.len(), 12, "{args:?}");
    assert_eq!(
        &args[..7],
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"read-only\"",
            "--add-dir",
            artifacts_dir.as_str(),
        ],
        "{args:?}"
    );
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent の無効化が `-` の直前に付く。
    assert_eq!(
        &args[7..11],
        [
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.multi_agent_v2=false"
        ],
        "{args:?}"
    );
    assert_eq!(
        args[11], "-",
        "F5-fix10: the prompt positional is `-` (stdin): {args:?}"
    );
}

/// (b) CoS の継続（resume）run: production の再現。`--add-dir` を落とし、`-c sandbox_mode="read-only"`
/// は維持したまま `codex exec resume <id> --json --skip-git-repo-check -c sandbox_mode="read-only"
/// <prompt>` になることを確認する（`exec resume` の usage 行に `--add-dir` が無いことに対応）。
#[tokio::test]
async fn phase_68b_resume_cos_run_argv_drops_add_dir_keeps_readonly_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-68b".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-68b-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(args.len(), 12, "{args:?}");
    assert_eq!(
        &args[..7],
        [
            "exec",
            "resume",
            "thread-68b",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"read-only\"",
        ],
        "{args:?}"
    );
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent の無効化が `-` の直前に付く。
    assert_eq!(
        &args[7..11],
        [
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.multi_agent_v2=false"
        ],
        "{args:?}"
    );
    assert_eq!(
        args[11], "-",
        "F5-fix10: the prompt positional is `-` (stdin): {args:?}"
    );
    assert!(
        !args.contains(&"--add-dir".to_string()),
        "exec resume must not receive --add-dir (see usage-line comment above): {args:?}"
    );
}

/// (c) 通常（非 CoS）の run: 従来どおり `workspace-write` + `--add-dir` を維持し、Phase 68b の変更が
/// 対話以外の run に影響しないことを確認する。
#[tokio::test]
async fn phase_68b_normal_run_argv_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let _ = adapter
        .run(
            req,
            "run-68b-normal",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    let artifacts_dir = dir.path().join("artifacts").to_str().unwrap().to_string();
    assert_eq!(args.len(), 12, "{args:?}");
    assert_eq!(
        &args[..7],
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "--add-dir",
            artifacts_dir.as_str(),
        ],
        "{args:?}"
    );
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent の無効化が `-` の直前に付く。
    assert_eq!(
        &args[7..11],
        [
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.multi_agent_v2=false"
        ],
        "{args:?}"
    );
    assert_eq!(
        args[11], "-",
        "F5-fix10: the prompt positional is `-` (stdin): {args:?}"
    );
}

// ---- ADR-0074 Phase F5-fix4（本番障害 01M3JXB3DHVBWKWKPW04DTG6SJ。`git merge main` が worktree の
// 登録元 `…/.git/worktrees/<name>/ORIG_HEAD` を書けず落ちた）: fresh の `workspace-write` run は
// cwd（と兄弟の repos）の gitdir と common dir を `--add-dir` で足す。ここから ----

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `<root>/origin-<name>` に commit 1 つの登録元リポジトリを作り、`<task_dir>/repos/<name>` に
/// worktree を切る。返り値は (worktree, per-worktree gitdir, common dir)。後の 2 つは canonical。
fn make_task_worktree(
    root: &std::path::Path,
    task_dir: &std::path::Path,
    name: &str,
) -> (std::path::PathBuf, String, String) {
    let origin = root.join(format!("origin-{name}"));
    std::fs::create_dir_all(&origin).unwrap();
    git_in(&origin, &["init", "-q", "-b", "main"]);
    git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = task_dir.join("repos").join(name);
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git_in(
        &origin,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("celeris/{name}"),
            wt.to_str().unwrap(),
        ],
    );
    let common = origin.join(".git").canonicalize().unwrap();
    let gitdir = common.join("worktrees").join(name).canonicalize().unwrap();
    (
        wt,
        gitdir.to_str().unwrap().to_string(),
        common.to_str().unwrap().to_string(),
    )
}

fn add_dir_values(args: &[String]) -> Vec<&str> {
    args.windows(2)
        .filter(|w| w[0] == "--add-dir")
        .map(|w| w[1].as_str())
        .collect()
}

/// cwd が worktree の fresh run: `--add-dir <artifacts> --add-dir <gitdir> --add-dir <common>`、
/// 兄弟のリポジトリ（`repos/` の他の worktree）の gitdir / common dir も続く。
#[tokio::test]
async fn f5_fix4_worktree_cwd_adds_gitdir_and_common_dir() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path().join("task");
    let (wt, gitdir, common) = make_task_worktree(dir.path(), &task_dir, "code");
    let (_other, other_gitdir, other_common) = make_task_worktree(dir.path(), &task_dir, "other");
    // `kind = dir` の兄弟（シンボリックリンク）は足さない。
    std::os::unix::fs::symlink(
        dir.path().join("origin-code"),
        task_dir.join("repos/linked"),
    )
    .unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let mut req = sample_req(task_dir.clone());
    req.work_dir = Some(wt.clone());
    let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
    let _ = CodexAdapter::new(config)
        .run(
            req,
            "run-f5fix4-wt",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(&wt);
    assert_eq!(
        &args[..15],
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "--add-dir",
            artifacts_dir.as_str(),
            "--add-dir",
            gitdir.as_str(),
            "--add-dir",
            common.as_str(),
            "--add-dir",
            other_gitdir.as_str(),
            "--add-dir",
            other_common.as_str(),
        ],
        "{args:?}"
    );
    assert_eq!(args.len(), 20, "{args:?}");
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent の無効化が `-` の直前に付く。
    assert_eq!(
        &args[15..19],
        [
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.multi_agent_v2=false"
        ],
        "{args:?}"
    );
    assert_eq!(
        args[19], "-",
        "F5-fix10: the prompt positional is `-` (stdin): {args:?}"
    );
}

/// git でない cwd（兄弟も git でない）の fresh run は成果物ディレクトリだけ（従来どおり）。
#[tokio::test]
async fn f5_fix4_non_git_cwd_adds_only_the_artifacts_dir() {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("repos/plain");
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::create_dir_all(dir.path().join("repos/also-plain")).unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let mut req = sample_req(dir.path().to_path_buf());
    req.work_dir = Some(work_dir.clone());
    let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
    let _ = CodexAdapter::new(config)
        .run(
            req,
            "run-f5fix4-plain",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(&work_dir);
    assert_eq!(add_dir_values(&args), [artifacts_dir.as_str()], "{args:?}");
}

/// read-only の CoS run には git の管理領域を足さない（読み取り専用の意味を変えない）。
#[tokio::test]
async fn f5_fix4_readonly_cos_run_does_not_add_git_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path().join("task");
    let (wt, _gitdir, _common) = make_task_worktree(dir.path(), &task_dir, "code");
    let config = stub_codex(dir.path(), args_log_script());
    let mut req = sample_req(task_dir.clone());
    req.work_dir = Some(wt.clone());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
    let _ = CodexAdapter::new(config)
        .run(
            req,
            "run-f5fix4-cos",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(&wt);
    assert_eq!(add_dir_values(&args), [artifacts_dir.as_str()], "{args:?}");
}

/// resume（`exec resume`）には従来どおり `--add-dir` を一つも付けない（worktree の cwd でも）。
#[tokio::test]
async fn f5_fix4_exec_resume_still_has_no_add_dir_in_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path().join("task");
    let (wt, _gitdir, _common) = make_task_worktree(dir.path(), &task_dir, "code");
    let config = stub_codex(dir.path(), args_log_script());
    let mut req = sample_req(task_dir.clone());
    req.work_dir = Some(wt.clone());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-f5fix4".to_string(),
        resume: true,
    });
    let _ = CodexAdapter::new(config)
        .run(
            req,
            "run-f5fix4-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(&wt);
    assert_eq!(&args[..2], ["exec", "resume"], "{args:?}");
    assert!(!args.contains(&"--add-dir".to_string()), "{args:?}");
}

/// `git_admin_dirs` は作業ツリーの最上位だけを見る（サブディレクトリからは上位の `.git` を拾わない）。
#[test]
fn f5_fix4_git_admin_dirs_ignores_subdirectories_and_plain_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let task_dir = dir.path().join("task");
    let (wt, gitdir, common) = make_task_worktree(dir.path(), &task_dir, "code");
    assert_eq!(
        crate::local_worktree::git_admin_dirs(&wt),
        [
            std::path::PathBuf::from(&gitdir),
            std::path::PathBuf::from(&common)
        ]
    );
    let sub = wt.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(crate::local_worktree::git_admin_dirs(&sub).is_empty());
    assert!(crate::local_worktree::git_admin_dirs(dir.path()).is_empty());
    // 通常のリポジトリは `.git` 1 つ（gitdir == common dir）。
    let origin = dir.path().join("origin-code");
    assert_eq!(
        crate::local_worktree::git_admin_dirs(&origin),
        [std::path::PathBuf::from(&common)]
    );
}

// ---- ADR-0074 Phase F5-fix4 ここまで ----

// ---- ADR-0075 R7-8（本番 run 01M3VCWE54P73CPFG09ZSW6Q6M: `…/scratch/targets/<owner>/target/debug` が
// `Read-only file system`）: fresh の `workspace-write` run は env の `CARGO_TARGET_DIR` も `--add-dir` で足す。ここから ----

/// dispatcher と同じく `with_env` で scratch の env を重ねた adapter（`WorkerAdapter` のまま返す）。
fn adapter_with_env(config: CodexConfig, env: &[(&str, String)]) -> Arc<dyn WorkerAdapter> {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    CodexAdapter::new(config).with_env(&env).unwrap()
}

/// fresh の `workspace-write` run: `--add-dir <artifacts> --add-dir <CARGO_TARGET_DIR>`。存在しない target は先に作る
/// （codex は存在しない root を書けるようにしない）。同名の `CARGO_TARGET_DIR` は後勝ち（`with_env` の規則）。
#[tokio::test]
async fn r7_8_fresh_workspace_write_run_adds_the_cargo_target_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("scratch/targets/task-T/wu-W/target");
    let mut config = stub_codex(dir.path(), args_log_script());
    config.env.push((
        "CARGO_TARGET_DIR".to_string(),
        dir.path().join("operator-target").display().to_string(),
    ));
    let adapter = adapter_with_env(
        config,
        &[
            ("CARGO_TARGET_DIR", target.display().to_string()),
            ("CARGO_INCREMENTAL", "0".to_string()),
        ],
    );
    let req = sample_req(dir.path().to_path_buf());
    let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
    assert!(!target.exists());
    let _ = adapter
        .run(req, "run-r7-8", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(
        add_dir_values(&args),
        [artifacts_dir.as_str(), target.to_str().unwrap()],
        "only the last CARGO_TARGET_DIR is granted: {args:?}"
    );
    assert!(
        target.is_dir(),
        "the target dir is created before codex starts"
    );
    assert_eq!(args.last().map(String::as_str), Some("-"), "{args:?}");
}

/// read-only の CoS run には足さず、作りもしない。
#[tokio::test]
async fn r7_8_readonly_cos_run_does_not_add_the_cargo_target_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("scratch/targets/task-T/target");
    let adapter = adapter_with_env(
        stub_codex(dir.path(), args_log_script()),
        &[("CARGO_TARGET_DIR", target.display().to_string())],
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
    let _ = adapter
        .run(
            req,
            "run-r7-8-cos",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(add_dir_values(&args), [artifacts_dir.as_str()], "{args:?}");
    assert!(!target.exists());
}

/// `exec resume` には従来どおり `--add-dir` を付けない（最初の fresh `exec` の許可を引き継ぐ前提。ADR-0075 R7-8 決定 2）。
#[tokio::test]
async fn r7_8_exec_resume_has_no_add_dir_even_with_a_cargo_target_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("scratch/targets/task-T/target");
    let adapter = adapter_with_env(
        stub_codex(dir.path(), args_log_script()),
        &[("CARGO_TARGET_DIR", target.display().to_string())],
    );
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-r7-8".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-r7-8-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(&args[..2], ["exec", "resume"], "{args:?}");
    assert!(!args.contains(&"--add-dir".to_string()), "{args:?}");
}

/// 空・相対パスの `CARGO_TARGET_DIR` は足さない（cwd 相対なら cwd の中で、既に書ける）。
#[tokio::test]
async fn r7_8_empty_or_relative_cargo_target_dir_is_not_added() {
    for value in ["", "target"] {
        let dir = tempfile::tempdir().unwrap();
        let adapter = adapter_with_env(
            stub_codex(dir.path(), args_log_script()),
            &[("CARGO_TARGET_DIR", value.to_string())],
        );
        let req = sample_req(dir.path().to_path_buf());
        let artifacts_dir = req.artifacts_dir.to_str().unwrap().to_string();
        let _ = adapter
            .run(
                req,
                "run-r7-8-rel",
                default_limits(),
                &RecordingSink::default(),
            )
            .await
            .unwrap();
        let args = captured_args(dir.path());
        assert_eq!(
            add_dir_values(&args),
            [artifacts_dir.as_str()],
            "value {value:?}: {args:?}"
        );
    }
}

// ---- ADR-0075 R7-8 ここまで ----

// ADR-0054 Phase 68c（本番障害 2026-09-21 15:44 UTC、release a2942d5d8a94。68b 配備後、fresh run は
// 成功したが resume run が `--approve-for-me`（`[adapters.codex] extra_args` 由来）で exit 2）:
//
//   error: unexpected argument '--approve-for-me' found
//   Usage: codex exec resume --json --skip-git-repo-check --config <key=value> <SESSION_ID> [PROMPT]
//
// `~/.local/bin/codex exec resume --help`（codex-cli 0.155.1。Phase 68c で再確認。テキストは
// Phase 68b の確認と同一）:
//
//   Usage: codex exec resume [OPTIONS] [SESSION_ID] [PROMPT]
//   Options（全量）: -c/--config <key=value>, --last, --all, --enable <FEATURE>,
//   --disable <FEATURE>, -i/--image <FILE>, --strict-config, -m/--model <MODEL>,
//   --dangerously-bypass-approvals-and-sandbox, --dangerously-bypass-hook-trust, --worktree,
//   --thread-source <SOURCE>, --skip-git-repo-check, --ephemeral, --ignore-user-config,
//   --ignore-rules, --output-schema <FILE>, --json, -o/--output-last-message <FILE>, -h/--help
//   （`--add-dir`・`-s/--sandbox`・`--approve-for-me` は無い）。
//
// `--help` の OPTIONS 一覧は `-m/--model` を含むが、本番の実際のエラーが示した usage 行は
// `--json`・`--skip-git-repo-check`・`--config`（＋位置引数）だけに絞られていた。`--help` の記載と
// 実際に受け付けられる集合が一致しない疑いがあるため、`exec resume` の argv はここから
// **ホワイトリスト方式**にした: `--json`・`--skip-git-repo-check`・`-c/--config`（複数可）＋
// session id ＋ prompt 以外は一切乗せない。

// ADR-0054 Phase 112 D1（本番障害 2026-09-23。Phase 68c 配備後、resume run が `--approve-for-me` を
// 丸ごと落とした結果、CoS の対話が result.json を書けないまま最終メッセージに生の JSON を吐いていた）:
// `exec resume` のホワイトリスト（`--json`・`--skip-git-repo-check`・`-c/--config`・位置引数）は
// 維持したまま、`extra_args` の個々のフラグを `-c key=value` に翻訳できるものは翻訳し、翻訳できない
// ものだけを落とす（`translate_resume_extra_args` 参照）。

/// D4(a): resume run の argv に `--approve-for-me` がそのまま乗らないこと、`-c
/// approval_policy="never"`・`-c sandbox_mode="workspace-write"` に翻訳されて乗ること、ホワイト
/// リスト外のフラグが無いこと（production の `--approve-for-me` 拒否の回帰、かつ Phase 68c の
/// 「丸ごと落とす」を「翻訳する」に変えたことの確認）。
#[tokio::test]
async fn phase_112_resume_translates_approve_for_me_to_config_overrides_and_drops_the_raw_flag() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.model = Some("gpt-5-codex".into());
    config.extra_args = vec!["--approve-for-me".into()];
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-112".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-112-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    // The whitelist pasted in the module comment above: --json, --skip-git-repo-check,
    // -c/--config (repeatable). Every other token that looks like a flag (starts with '-') is a
    // regression.
    // F5-fix10: `-` is the stdin prompt positional, not a flag.
    const WHITELIST: &[&str] = &["--json", "--skip-git-repo-check", "-c", "-"];
    for arg in &args {
        if arg.starts_with('-') {
            assert!(
                WHITELIST.contains(&arg.as_str()),
                "flag {arg:?} is not on the `exec resume` whitelist: {args:?}"
            );
        }
    }
    assert!(
        !args.contains(&"--approve-for-me".to_string()),
        "the raw operator flag must not reach `exec resume`: {args:?}"
    );
    assert!(!args.contains(&"--add-dir".to_string()), "{args:?}");
    assert!(!args.contains(&"--model".to_string()), "{args:?}");
    // The model still reaches codex, just via `-c model="..."` instead of `--model`.
    assert!(
        args.contains(&"model=\"gpt-5-codex\"".to_string()),
        "{args:?}"
    );
    // The unconditional CoS `sandbox_mode="read-only"` (ADR-0054 D2/Phase 68) is still there,
    // but the translation of `--approve-for-me` appends its own `-c` pairs after it.
    assert!(
        args.contains(&"sandbox_mode=\"read-only\"".to_string()),
        "{args:?}"
    );
    assert!(
        args.contains(&"approval_policy=\"never\"".to_string()),
        "{args:?}"
    );
    assert!(
        args.contains(&"sandbox_mode=\"workspace-write\"".to_string()),
        "{args:?}"
    );
}

/// D4(b): 新規スレッド（resume なし）の run では `--approve-for-me` が翻訳されず、従来どおりそのまま
/// argv に乗ること（fresh は `exec resume` のホワイトリストの対象外なので、D1 の翻訳は resume だけの
/// 話であることの回帰）。
#[tokio::test]
async fn phase_112_fresh_session_keeps_operator_extra_args_unmodified() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.extra_args = vec!["--approve-for-me".into()];
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    // No `context.session`: this is a fresh (non-resuming) run.
    let _ = adapter
        .run(
            req,
            "run-112-fresh",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()), "{args:?}");
    assert!(
        args.contains(&"--approve-for-me".to_string()),
        "fresh runs pass extra_args through unmodified: {args:?}"
    );
    assert!(
        !args.contains(&"approval_policy=\"never\"".to_string()),
        "fresh runs don't need translation: {args:?}"
    );
}

/// D1: `--full-auto`・`--sandbox <mode>`・`--ask-for-approval <policy>` も `-c` に翻訳される。
#[tokio::test]
async fn phase_112_full_auto_and_sandbox_and_ask_for_approval_translate_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.extra_args = vec![
        "--full-auto".into(),
        "--sandbox".into(),
        "danger-full-access".into(),
        "--ask-for-approval".into(),
        "on-request".into(),
    ];
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-112b".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-112-translate",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    for raw in ["--full-auto", "--sandbox", "--ask-for-approval"] {
        assert!(!args.contains(&raw.to_string()), "{raw} leaked: {args:?}");
    }
    assert!(
        args.contains(&"approval_policy=\"on-failure\"".to_string()),
        "--full-auto: {args:?}"
    );
    assert!(
        args.contains(&"sandbox_mode=\"danger-full-access\"".to_string()),
        "--sandbox danger-full-access: {args:?}"
    );
    assert!(
        args.contains(&"approval_policy=\"on-request\"".to_string()),
        "--ask-for-approval on-request: {args:?}"
    );
}

/// D2: 翻訳できないフラグが残り、`resume_bypass` も設定されていないときは resume 自体を諦め、
/// 新規スレッド（`codex exec`、`resume` サブコマンド無し）として走る。新規スレッドには untranslatable
/// なフラグを含め `extra_args` が丸ごと乗る。
#[tokio::test]
async fn phase_112_untranslatable_extra_args_without_bypass_skip_resume_and_run_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.extra_args = vec!["--unknown-flag".into()];
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-112c".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-112-skip-resume",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        !args.contains(&"resume".to_string()),
        "an untranslatable extra_arg without resume_bypass must not attempt resume: {args:?}"
    );
    assert!(!args.contains(&"thread-112c".to_string()), "{args:?}");
    assert!(
        args.contains(&"--unknown-flag".to_string()),
        "the fresh run gets the full, untranslated extra_args: {args:?}"
    );
}

/// ADR-0095 D-b: `resume_bypass = Dangerous` でも未検証の bypass は使わず fresh に戻す。
#[tokio::test]
async fn systemd_deny_disables_dangerous_resume_bypass() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.extra_args = vec!["--unknown-flag".into()];
    config.resume_bypass = CodexResumeBypass::Dangerous;
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-112d".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-112-bypass",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()), "{args:?}");
    assert!(
        !args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()),
        "{args:?}"
    );
    assert!(
        args.contains(&"--unknown-flag".to_string()),
        "the fresh run receives the operator flag: {args:?}"
    );
}

/// D3: `result.json` を書かない（書けない）run が、最終メッセージに `result.json` と同じ形の JSON
/// （`summary` + `actions`）を吐いたときは、それを `result.json` として回収し、`Done.summary` には
/// 生の JSON 文字列ではなく JSON の `summary` を使う。回収したことが `sink.progress` に残る。
#[tokio::test]
async fn phase_112_a_recoverable_final_message_is_written_as_result_json_and_its_summary_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo '{"type":"item.completed","item":{"type":"agent_message","text":"{\"summary\":\"直すタスクを作りました\",\"actions\":[{\"type\":\"create_task\",\"title\":\"直す\",\"objective\":\"直して\"}]}"}}'
echo '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.task.conversation = Some(task_core::MessageId::new());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-112-recover", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => {
            assert_eq!(summary, "直すタスクを作りました");
        }
        other => panic!("expected done, got {other:?}"),
    }
    let result_json = std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert!(
        value.get("actions").is_some_and(|a| a.is_array()),
        "{value:?}"
    );
    let progress = sink.progress.lock().unwrap();
    assert!(
        progress.iter().any(|m| m.contains("result recovered")),
        "{progress:?}"
    );
}

/// ADR-0054 D1（Phase 67）: 継続セッション（`resume: true`）かつ `resume_mode = ExecResume`（既定）
/// なら `codex exec resume <id> …`。
#[tokio::test]
async fn a_continuing_session_uses_exec_resume_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-123".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-2", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(args[0], "exec");
    assert_eq!(args[1], "resume");
    assert_eq!(args[2], "thread-123");
    assert!(!args.iter().any(|a| a.starts_with("experimental_resume=")));
}

/// ADR-0054 D1（Phase 67）: `resume_mode = ExperimentalResume`（`resume` サブコマンドの無い古い版）
/// なら `-c experimental_resume=<id>` に切り替える。
#[tokio::test]
async fn a_continuing_session_uses_experimental_resume_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.resume_mode = CodexResumeMode::ExperimentalResume;
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-123".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-3", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()));
    assert!(
        args.contains(&"experimental_resume=thread-123".to_string()),
        "{args:?}"
    );
}

/// ADR-0054 D1（Phase 67）: 新規セッション（`resume: false`）は resume の引数を付けない
/// （codex は `--session-id` 相当の「これから使う id を固定する」手段を持たないため）。
#[tokio::test]
async fn a_fresh_session_adds_no_resume_flags() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "placeholder".to_string(),
        resume: false,
    });
    let _ = adapter
        .run(req, "run-4", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()));
    assert!(!args.iter().any(|a| a.starts_with("experimental_resume=")));
}

/// `context.session` が別アダプタ向けなら無視する。
#[tokio::test]
async fn a_session_for_another_adapter_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(dir.path(), args_log_script());
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: "claude-code".to_string(),
        session_id: "cc-session".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(req, "run-5", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"resume".to_string()));
}

/// ADR-0054 D1（Phase 67）: `thread.started` に `thread_id` があれば `session_established` へ報告する。
/// フィールド名は未検証（コメント参照）だが、パーサの挙動そのものはこの偽の CLI で確認できる。
#[tokio::test]
async fn thread_started_with_a_thread_id_reports_session_established() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"
mkdir -p artifacts
printf '%s\n' '{"type":"thread.started","thread_id":"thread-xyz"}'
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter
        .run(req, "run-6", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(
        sink.sessions.lock().unwrap().as_slice(),
        &["thread-xyz".to_string()]
    );
}

/// ADR-0054 Phase 67c: `session_configured`（実装によっては `thread.started` の代わりにこの type
/// 名を使うことがある）の `session_id` からも id を拾う。
#[tokio::test]
async fn session_configured_with_a_session_id_reports_session_established() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"
mkdir -p artifacts
printf '%s\n' '{"type":"session_configured","session_id":"sess-abc"}'
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter
        .run(req, "run-6b", default_limits(), &sink)
        .await
        .unwrap();
    assert_eq!(
        sink.sessions.lock().unwrap().as_slice(),
        &["sess-abc".to_string()]
    );
}

/// ADR-0054 Phase 67c: `thread.started` に id が無ければ `session_established` は呼ばない
/// （`node_sessions.session_id` は空文字のまま残り、Phase 67c の自己修復
/// — `crate::sessions::session_id_is_valid_for_adapter` が空文字を無効扱いする — に委ねる）。
#[tokio::test]
async fn thread_started_without_an_id_does_not_report_session_established() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"
mkdir -p artifacts
printf '%s\n' '{"type":"thread.started"}'
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"turn.completed"}'
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter
        .run(req, "run-6c", default_limits(), &sink)
        .await
        .unwrap();
    assert!(sink.sessions.lock().unwrap().is_empty());
}

/// ADR-0054 D1（Phase 67）: `resume` を頼んだ run が turn.failed の `message` に「セッションが
/// 見つからない」旨の文言を含んで終わったら `session_resume_failed` を報告する（文言は未検証）。
#[tokio::test]
async fn a_rejected_resume_reports_session_resume_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"printf '%s\n' '{"type":"turn.failed","error":{"message":"session not found: 01ARZ3"}}'"#,
    );
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "01ARZ3".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter.run(req, "run-7", default_limits(), &sink).await;
    match outcome {
        Ok(o) => assert!(matches!(
            o.terminal,
            Terminal::Error {
                retryable: true,
                ..
            }
        )),
        Err(e) => panic!("expected Ok(Terminal::Error), got {e:?}"),
    }
    let failed = sink.resume_failed.lock().unwrap();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("session not found"), "{failed:?}");
}

/// resume していない run が同じ文言で失敗しても `session_resume_failed` は報告しない。
#[tokio::test]
async fn a_failure_without_resuming_does_not_report_session_resume_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"printf '%s\n' '{"type":"turn.failed","error":{"message":"session not found: 01ARZ3"}}'"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let _ = adapter.run(req, "run-8", default_limits(), &sink).await;
    assert!(sink.resume_failed.lock().unwrap().is_empty());
}

/// Phase 98（ADR-0054 追記。実機障害 2026-09-22 00:18 UTC、task 01M337NT3QT1FR1G6WHS9G6NDA）:
/// `codex exec resume` が**イベントを一つも出さずに** stderr に `list_turns is not supported yet
/// (code -32601)` を出して exit 1 したら、`session_resume_failed` を 1 回報告した上で、**同じ run の
/// 中で** resume 無しの fresh `codex exec` としてやり直し、run は done になる（stub は argv の
/// `$2` が `resume` かどうかで振る舞いを変える: resume 呼び出しは失敗を模し、resume 無しの呼び出しは
/// 正常な `thread.started`/`turn.completed` を出す）。
#[tokio::test]
async fn a_resume_rpc_failure_self_heals_within_the_same_run() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"if [ "$2" = "resume" ]; then
  echo 'Error: thread/resume: thread/resume failed: list_turns is not supported yet (code -32601)' >&2
  exit 1
else
  mkdir -p artifacts
  echo '{"type":"thread.started","thread_id":"thread-fresh-1"}'
  printf '%s' '{"summary":"done after fresh retry","evidence":[]}' > artifacts/result.json
  echo '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":7}}'
fi
"#,
    );
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "01ARZ3".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-resume-heal", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "done after fresh retry"),
        other => panic!("expected done after self-heal, got {other:?}"),
    }
    let failed = sink.resume_failed.lock().unwrap();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(
        failed[0].contains("thread/resume") || failed[0].contains("-32601"),
        "{failed:?}"
    );
    // やり直しは resume していない（`thread.started` からの id）ので、新しい id だけが 1 回報告される。
    let sessions = sink.sessions.lock().unwrap();
    assert_eq!(sessions.as_slice(), ["thread-fresh-1".to_string()]);
    // どちらの試行のログも残る（デバッグ用。1 回目は resume の argv、2 回目は fresh の argv）。
    assert!(
        dir.path()
            .join("runs/run-resume-heal/stdout.jsonl")
            .is_file()
    );
    assert!(
        dir.path()
            .join("runs/run-resume-heal/stdout.resume_retry.jsonl")
            .is_file()
    );
}

/// resume していない run は、この自己回復の対象にならない（同じ文言で失敗しても 1 回で終わる。
/// `session_resume_failed` も呼ばれない）。
#[tokio::test]
async fn a_non_resuming_run_is_unaffected_by_the_resume_rpc_self_heal() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_codex(
        dir.path(),
        r#"echo 'Error: thread/resume: thread/resume failed: list_turns is not supported yet (code -32601)' >&2
exit 1
"#,
    );
    let adapter = CodexAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-no-resume", default_limits(), &sink)
        .await;
    match outcome {
        Ok(o) => assert!(matches!(
            o.terminal,
            Terminal::Error {
                retryable: true,
                ..
            }
        )),
        Err(e) => panic!("expected Ok(Terminal::Error), got {e:?}"),
    }
    assert!(sink.resume_failed.lock().unwrap().is_empty());
    assert!(sink.sessions.lock().unwrap().is_empty());
    assert!(
        !dir.path()
            .join("runs/run-no-resume/stdout.resume_retry.jsonl")
            .exists(),
        "resume していない run はやり直さない"
    );
}

// ---- ADR 2026-10-07-worker-no-subagents-no-llm-cli: multi-agent の既定無効化と別 LLM の起動の検出 ----

fn has_config_override(args: &[String], kv: &str) -> bool {
    args.windows(2).any(|pair| pair[0] == "-c" && pair[1] == kv)
}

/// D2: `exec resume` でも `-c features.multi_agent=false` / `multi_agent_v2=false` が付く（`-c` は resume の
/// ホワイトリストにある）。運用側の `extra_args`（resume に翻訳できる本番の `--approve-for-me`）の後ろに来る。
/// （翻訳できない `extra_args` があると ADR-0095 D-b で fresh に倒れるので、ここでは翻訳できるものだけを置く。）
#[tokio::test]
async fn exec_resume_also_disables_multi_agent_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = stub_codex(dir.path(), args_log_script());
    config.extra_args = vec!["--approve-for-me".into()];
    let adapter = CodexAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: CodexAdapter::ID.to_string(),
        session_id: "thread-ma".to_string(),
        resume: true,
    });
    let _ = adapter
        .run(
            req,
            "run-resume-ma",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert_eq!(&args[..2], ["exec", "resume"]);
    assert!(
        has_config_override(&args, "features.multi_agent=false"),
        "{args:?}"
    );
    assert!(
        has_config_override(&args, "features.multi_agent_v2=false"),
        "{args:?}"
    );
    // 無効化は最後（`-` の直前）に来るので、前に何があっても勝つ。
    let last_override = args
        .iter()
        .rposition(|a| a == "features.multi_agent_v2=false")
        .unwrap();
    assert_eq!(args[last_override + 1], "-", "{args:?}");
}

/// D2/D7: `subagents = "allow"` のときだけ無効化が消える。`"allow_cos"` は CoS の対話 run にだけ効く。
#[tokio::test]
async fn only_an_explicit_subagents_setting_keeps_multi_agent_enabled() {
    use crate::tool_policy::SubagentPolicy;
    async fn args_for(
        policy: SubagentPolicy,
        addressee: Option<crate::protocol::ConversationAddressee>,
    ) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let mut config = stub_codex(dir.path(), args_log_script());
        config.subagents = policy;
        let mut req = sample_req(dir.path().to_path_buf());
        req.context.conversation_addressee = addressee;
        let _ = CodexAdapter::new(config)
            .run(
                req,
                "run-policy",
                default_limits(),
                &RecordingSink::default(),
            )
            .await
            .unwrap();
        captured_args(dir.path())
    }
    let disabled = |args: &[String]| has_config_override(args, "features.multi_agent=false");
    assert!(!disabled(&args_for(SubagentPolicy::Allow, None).await));
    assert!(disabled(&args_for(SubagentPolicy::Deny, None).await));
    assert!(disabled(&args_for(SubagentPolicy::AllowCos, None).await));
    assert!(!disabled(
        &args_for(
            SubagentPolicy::AllowCos,
            Some(crate::protocol::ConversationAddressee::Secretary)
        )
        .await
    ));
}

/// D5: `item.started` の `command_execution.command` から別 LLM CLI の起動を検出する（`item.completed` と
/// 普通の command は出さない）。
#[test]
fn command_executions_that_launch_other_llm_clis_are_reported() {
    use task_core::ToolPolicyKind;
    let sink = RecordingSink::default();
    let mut signal = None;
    let mut error = None;
    for line in [
        r#"{"type":"item.started","item":{"type":"command_execution","command":"cargo test --workspace"}}"#,
        r#"{"type":"item.started","item":{"type":"command_execution","command":"FOO=1 timeout 600 opencode run 'fix the failing test'"}}"#,
        r#"{"type":"item.completed","item":{"type":"command_execution","command":"FOO=1 timeout 600 opencode run 'fix the failing test'","exit_code":0,"aggregated_output":"ok"}}"#,
        r#"{"type":"item.started","item":{"type":"command_execution","command":"which claude"}}"#,
    ] {
        handle_line(line, &sink, &mut signal, &mut error);
    }
    let violations = sink
        .violations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    assert_eq!(violations.len(), 1, "{violations:#?}");
    assert_eq!(violations[0].kind, ToolPolicyKind::LlmCli);
    assert_eq!(violations[0].tool, "command_execution");
    assert_eq!(violations[0].matched, "opencode");
    let flagged = sink
        .structured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(m, f)| f.error && m.starts_with("policy: "))
        .count();
    assert_eq!(flagged, 1);
}
