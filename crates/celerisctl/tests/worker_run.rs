//! `celerisctl worker run` の統合テスト（ADR-0012 D4）。実バイナリ（`CARGO_BIN_EXE_celerisctl`）を
//! `fake` アダプタ設定で起動し、ネットワークに出ずに done/question/error/provider_failure/
//! `--harness`・`--account` による route の選択・running タスクの `--workspace` 要求を確認する。
//! DB への書き込みが一切無いこと（`events_for` の件数・タスクの status が変わらない）も検証する。

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use task_core::{
    Budget, Check, Criterion, Event, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier,
    WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn sample_task(status: Status, workspace: WorkspaceSpec) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "do something".to_string(),
        objective: "do it".to_string(),
        acceptance: vec![Criterion {
            text: "ok".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace,
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 30,
            max_retries: 1,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write fake script");
    path
}

/// A fake LLM source and two fake harnesses (`stub` is the default; `other` passes `other` to the
/// script) with `route_defaults`.
fn write_config(dir: &Path, script: &Path) -> PathBuf {
    let workspace_root = dir.join("workspaces");
    let text = format!(
        r#"workspace_root = {workspace_root:?}

[adapters.fake]
command = ["sh", {script:?}]

[llm_sources.fake]
kind = "fake"
accounts = ["local"]
models = ["fake"]

[harnesses.stub]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[harnesses.other]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = [{script:?}, "other"]

[harness_selection]
execution_class_defaults = {{}}

[[harness_selection.rules]]
harness = "stub"
required_capabilities = []

[route_defaults.stub]
candidates = [{{ source = "fake", model = "fake" }}]

[route_defaults.other]
candidates = [{{ source = "fake", model = "fake" }}]
"#,
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, text).expect("write config");
    path
}

fn run_celerisctl(db: &Path, args: &[&str]) -> Output {
    Command::new(common::celerisctl_path())
        .arg("--db")
        .arg(db)
        .args(args)
        .output()
        .expect("run celerisctl")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn manual_run_cannot_silently_drop_browser_capability() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("celeris.sqlite3");
    let script = write_script(tmp.path(), "must-not-run.sh", "touch invoked\n");
    let config = write_config(tmp.path(), &script);
    let store = SqliteStore::open(&db).unwrap();
    let mut task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: tmp.path().join("ws"),
            mode: None,
        },
    );
    task.skills = vec!["browser-enabled".into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://example.com".into()],
    });
    store.insert(&task).unwrap();
    let out = run_celerisctl(
        &db,
        &[
            "worker",
            "run",
            "--config",
            config.to_str().unwrap(),
            "--task",
            &task.id.to_string(),
        ],
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("browser capability requires dispatcher execution")
    );
    assert!(!tmp.path().join("ws").exists());
    assert!(store.events_for(task.id).unwrap().is_empty());
}

/// done: progress / artifact を逐次出し、`context.answers` に事前の `Answered` イベントが載り、
/// DB には一切書き込まれない（events_for の件数・status が不変）。
#[test]
fn done_prints_progress_artifact_and_result_without_touching_the_store() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    // `prepare` が artifacts/ を作った後にスクリプトが動くので、成果物はスクリプト内で書けばよい
    // （`artifact::resolve` は実在チェックをするため、`artifact` メッセージより前に書く）。
    let script = write_script(
        tmp.path(),
        "done.sh",
        r#"cat > stdin.json
echo '{"type":"progress","msg":"working"}'
echo hi > artifacts/out.txt
echo '{"type":"artifact","name":"out","path":"artifacts/out.txt"}'
echo '{"type":"done","summary":"ok","evidence":[]}'
"#,
    );

    let config_path = write_config(tmp.path(), &script);
    let workspace_dir = tmp.path().join("ws");

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("unused"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");
    store
        .append_event(
            task.id,
            &Event::Answered {
                question: "q?".to_string(),
                answer: "a!".to_string(),
            },
        )
        .expect("append answered");

    let events_before = store.events_for(task.id).expect("events_for before");

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--workspace",
            workspace_dir.to_str().expect("utf8"),
        ],
    );

    let stdout = stdout_of(&out);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("progress: working"), "{stdout}");
    assert!(stdout.contains("artifact: out"), "{stdout}");
    assert!(stdout.contains("result: {\"type\":\"done\""), "{stdout}");

    let stdin_json =
        std::fs::read_to_string(workspace_dir.join("stdin.json")).expect("read stdin.json");
    assert!(
        stdin_json.contains(r#""answers":[{"question":"q?","answer":"a!"}]"#),
        "{stdin_json}"
    );

    let events_after = store.events_for(task.id).expect("events_for after");
    assert_eq!(events_after.len(), events_before.len());
    let fetched = store.get(task.id).expect("get").expect("task exists");
    assert_eq!(fetched.status, Status::Ready);
}

/// question: exit 3, `result: {"type":"question"...}`。
#[test]
fn question_exits_three() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    let script = write_script(
        tmp.path(),
        "question.sh",
        r#"cat > /dev/null
echo '{"type":"question","text":"which one?"}'
"#,
    );
    let config_path = write_config(tmp.path(), &script);

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("unused"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--workspace",
            tmp.path().join("ws").to_str().expect("utf8"),
        ],
    );

    let stdout = stdout_of(&out);
    assert_eq!(
        out.status.code(),
        Some(3),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains(r#"result: {"type":"question""#), "{stdout}");
}

/// `--harness` starts that harness's declared command; without it the task's harness selection
/// (`[[harness_selection.rules]]`) decides. The printed route names harness, source, model, account.
#[test]
fn harness_flag_starts_the_declared_harness_command() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    let script = write_script(
        tmp.path(),
        "harness.sh",
        "cat > /dev/null\necho \"{\\\"type\\\":\\\"progress\\\",\\\"msg\\\":\\\"harness=${1:-stub}\\\"}\"\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'\n",
    );
    let config_path = write_config(tmp.path(), &script);

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("unused"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--harness",
            "other",
            "--workspace",
            tmp.path().join("ws-other").to_str().expect("utf8"),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = stdout_of(&out);
    assert!(stdout.contains("harness=other"), "{stdout}");
    assert!(
        stdout.contains("harness=other source=fake model=fake account=local"),
        "{stdout}"
    );

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--workspace",
            tmp.path().join("ws-default").to_str().expect("utf8"),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout_of(&out).contains("harness=stub"),
        "{}",
        stdout_of(&out)
    );
}

/// running/reviewing のタスクは `--workspace` 無しでは拒否し、指定すれば実行できる。
#[test]
fn running_task_requires_explicit_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    let script = write_script(
        tmp.path(),
        "done.sh",
        "cat > /dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'\n",
    );
    let config_path = write_config(tmp.path(), &script);

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Running,
        WorkspaceSpec::Local {
            path: PathBuf::from("running-task-ws"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");

    let without_workspace = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
        ],
    );
    assert_eq!(
        without_workspace.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&without_workspace.stderr)
    );

    let with_workspace = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--workspace",
            tmp.path().join("separate-ws").to_str().expect("utf8"),
        ],
    );
    assert_eq!(
        with_workspace.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&with_workspace.stderr)
    );
}

/// An undeclared harness fails closed with exit 1.
#[test]
fn unknown_harness_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    let script = write_script(
        tmp.path(),
        "done.sh",
        "cat > /dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'\n",
    );
    let config_path = write_config(tmp.path(), &script);

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("unused"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--harness",
            "nope",
            "--workspace",
            tmp.path().join("ws").to_str().expect("utf8"),
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `error.provider_failure` 付きのワーカー error は、アダプタの `Err` 経由で
/// `provider_failure` を保ったまま exit 4 になる（ADR-0010 D5）。
#[test]
fn error_with_provider_failure_exits_four_and_preserves_provider_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("celeris.sqlite3");
    let script = write_script(
        tmp.path(),
        "throttled.sh",
        r#"cat > /dev/null
echo '{"type":"error","message":"429","retryable":true,"provider_failure":{"kind":"throttled","retry_after_secs":7}}'
"#,
    );
    let config_path = write_config(tmp.path(), &script);

    let store = SqliteStore::open(&db_path).expect("open store");
    let task = sample_task(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("unused"),
            mode: None,
        },
    );
    store.insert(&task).expect("insert task");

    let out = run_celerisctl(
        &db_path,
        &[
            "worker",
            "run",
            "--config",
            config_path.to_str().expect("utf8"),
            "--task",
            &task.id.to_string(),
            "--workspace",
            tmp.path().join("ws").to_str().expect("utf8"),
        ],
    );

    let stdout = stdout_of(&out);
    assert_eq!(
        out.status.code(),
        Some(4),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(r#""provider_failure":{"kind":"throttled","retry_after_secs":7}"#),
        "{stdout}"
    );
}

/// The real codex CLI path: the declared harness command is started with the route's model and
/// the chosen OAuth account's directory, without touching the store or the account book.
#[cfg(unix)]
#[test]
fn codex_route_reaches_the_cli_without_mutating_the_store() {
    use std::os::unix::fs::PermissionsExt;
    for (flag, expected_account, error) in [
        (None, Some("a"), ""),
        (Some("b"), Some("b"), ""),
        (Some("missing"), None, "no route"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("accounts");
        for id in ["a", "b"] {
            std::fs::create_dir_all(root.join(id)).unwrap();
            std::fs::write(root.join(id).join("auth.json"), "{}").unwrap();
        }
        let book_before = std::fs::read(root.join(".celeris-usage.json")).ok();
        let script = write_script(
            tmp.path(),
            "codex-stub",
            r#"#!/bin/sh
printf '%s\n' "$CODEX_HOME" > account.txt
printf '%s\n' "$@" > args.txt
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
echo '{"type":"turn.completed"}'
"#,
        );
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                r#"
workspace_root = {:?}
[accounts]
codex_dir = {:?}
[llm_sources.codex]
kind = "codex_oauth"
models = ["fixture-model"]
[harnesses.codex]
kind = "codex"
api_formats = ["codex_oauth"]
source_kinds = ["codex_oauth"]
command = {:?}
[route_defaults.codex]
candidates = [{{ source = "codex", model = "fixture-model" }}]
"#,
                tmp.path().join("workspaces"),
                root,
                script
            ),
        )
        .unwrap();
        let ws = tmp.path().join("ws");
        let db = tmp.path().join("tasks.db");
        let store = SqliteStore::open(&db).unwrap();
        let task = sample_task(
            Status::Ready,
            WorkspaceSpec::Local {
                path: ws.clone(),
                mode: None,
            },
        );
        store.insert(&task).unwrap();
        let id = task.id.to_string();
        let mut args = vec![
            "worker",
            "run",
            "--config",
            config.to_str().unwrap(),
            "--task",
            &id,
            "--harness",
            "codex",
        ];
        if let Some(flag) = flag {
            args.extend(["--account", flag]);
        }
        let out = run_celerisctl(&db, &args);
        let stdout = stdout_of(&out);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if let Some(account) = expected_account {
            assert!(out.status.success(), "{stdout}\n{stderr}");
            let argv = std::fs::read_to_string(ws.join("args.txt")).unwrap();
            assert!(argv.contains("--model\nfixture-model\n"), "{argv}");
            assert_eq!(
                std::fs::read_to_string(ws.join("account.txt"))
                    .unwrap()
                    .trim(),
                root.join(account).to_str().unwrap()
            );
            assert!(
                stdout.contains(&format!(
                    "harness=codex source=codex model=fixture-model account={account}"
                )),
                "{stdout}"
            );
        } else {
            assert!(!out.status.success(), "{stdout}");
            assert!(stderr.contains(error), "expected {error}: {stderr}");
            assert!(!ws.join("args.txt").exists(), "CLI must not start");
        }
        assert_eq!(
            std::fs::read(root.join(".celeris-usage.json")).ok(),
            book_before
        );
        assert_eq!(store.get(task.id).unwrap().unwrap(), task);
        assert!(store.events_for(task.id).unwrap().is_empty());
    }
}
