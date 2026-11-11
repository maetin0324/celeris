//! 起動前通知の契約試験（ADR 2026-11-06-cos-run-launch-accounting-and-drain D1）。
//!
//! 実 process の stub を使い、`EventSink::before_spawn` が process を spawn する同じ関数の**直前**に
//! 1 回以上呼ばれること、`Err` なら spawn しないこと、spawn が失敗しても通知は残ることを固定する。
//! 順序は 1 本のログ file に通知側と stub 側が追記した行の並びで決める（時計に頼らない）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use task_core::ArtifactRef;

use crate::adapter::{AdapterError, EventSink, RunLimits, WorkerAdapter};
use crate::protocol::{PROTOCOL_VERSION, RunContext, RunRequest};

/// `before_spawn` が呼ばれた回数を数え、`log` に `notify` を追記するシンク。
struct NoticeSink {
    log: PathBuf,
    fail: Option<&'static str>,
    calls: Mutex<u32>,
}

impl NoticeSink {
    fn new(log: &Path, fail: Option<&'static str>) -> Self {
        Self {
            log: log.to_path_buf(),
            fail,
            calls: Mutex::new(0),
        }
    }
    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventSink for NoticeSink {
    fn before_spawn(&self) -> Result<(), String> {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        if let Some(reason) = self.fail {
            return Err(reason.to_string());
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| e.to_string())?;
        writeln!(f, "notify").map_err(|e| e.to_string())
    }
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &ArtifactRef) {}
}

fn request(workspace: &Path) -> RunRequest {
    RunRequest {
        llm_route: None,
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace: workspace.to_path_buf(),
        work_dir: None,
        context: RunContext::default(),
    }
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    }
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// stub が起動したら `spawned` をログへ追記し、終端 JSON を 1 行返して終わる。
fn stub_script(log: &Path) -> String {
    format!(
        "echo spawned >> '{}'\ncat >/dev/null 2>&1\necho '{{\"type\":\"done\",\"summary\":\"stub\",\"evidence\":[]}}'\n",
        log.display()
    )
}

fn write_stub(dir: &Path, log: &Path) -> PathBuf {
    let path = dir.join("stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{}", stub_script(log)));
    path
}

/// 同じ契約を満たすべき実経路の一覧。`missing` を指定すると存在しない program を起動させる。
fn adapters(dir: &Path, log: &Path, missing: bool) -> Vec<(&'static str, Box<dyn WorkerAdapter>)> {
    let program = if missing {
        dir.join("no-such-program").to_string_lossy().into_owned()
    } else {
        write_stub(dir, log).to_string_lossy().into_owned()
    };
    let ext = dir.join("ext.js");
    std::fs::write(&ext, "// stub extension\n").unwrap();
    vec![
        (
            "fake",
            Box::new(crate::FakeAdapter::new(vec![program.clone()])),
        ),
        (
            "claude-code",
            Box::new(crate::ClaudeCodeAdapter::new(crate::ClaudeCodeConfig {
                command: program.clone(),
                ..Default::default()
            })),
        ),
        (
            "codex",
            Box::new(crate::CodexAdapter::new(crate::CodexConfig {
                // 起動前の rules 作成も試験の一時領域に閉じ、継承した account を触らない。
                env: vec![(
                    "CODEX_HOME".into(),
                    dir.join("codex-home").display().to_string(),
                )],
                command: program.clone(),
                // 起動前の rules 作成も fixture 内に限る（実アカウントを継承しない）。
                env: vec![(
                    "CODEX_HOME".into(),
                    dir.join("codex-home").display().to_string(),
                )],
                ..Default::default()
            })),
        ),
        (
            "pi",
            Box::new(crate::PiAdapter::new(crate::PiConfig {
                command: program,
                model: Some("stub/model".into()),
                extensions: vec![ext],
                tools: vec!["read".into()],
                ..Default::default()
            })),
        ),
    ]
}

/// 通知が spawn より先（直前）に呼ばれる。順序はログの並びで固定する。
#[tokio::test]
async fn notice_is_called_before_the_process_is_spawned() {
    let names = ["fake", "claude-code", "codex", "pi"];
    for name in names {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("order.log");
        let (_, adapter) = adapters(dir.path(), &log, false)
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap();
        let sink = NoticeSink::new(&log, None);
        let result = adapter
            .run(request(dir.path()), "run-order", limits(), &sink)
            .await;
        assert_eq!(
            lines(&log),
            vec!["notify".to_string(), "spawned".to_string()],
            "{name}: notice must precede the spawn (and be sent once for one spawn): {result:?}"
        );
        assert_eq!(sink.calls(), 1, "{name}: one notice per spawn");
    }
}

/// 通知が Err なら process を spawn しない（stub は 1 行も書かない）。run は通知失敗で終わる。
#[tokio::test]
async fn a_failed_notice_prevents_the_spawn() {
    for name in ["fake", "claude-code", "codex", "pi"] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("order.log");
        let (_, adapter) = adapters(dir.path(), &log, false)
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap();
        let sink = NoticeSink::new(&log, Some("conflict: run already closed"));
        let result = adapter
            .run(request(dir.path()), "run-fail", limits(), &sink)
            .await;
        assert_eq!(sink.calls(), 1, "{name}: notice attempted");
        assert!(
            lines(&log).is_empty(),
            "{name}: the process must not start after a failed notice: {:?}",
            lines(&log)
        );
        match result {
            Err(AdapterError::Other(m)) => assert!(
                m.contains("conflict: run already closed"),
                "{name}: the notice error must reach the caller: {m}"
            ),
            other => panic!("{name}: expected a notice error, got {other:?}"),
        }
    }
}

/// spawn が失敗しても通知は取り消されない（通知は 1 回呼ばれ、失敗は Spawn として返る）。
#[tokio::test]
async fn a_spawn_failure_keeps_the_notice() {
    for name in ["fake", "claude-code", "codex", "pi"] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("order.log");
        let (_, adapter) = adapters(dir.path(), &log, true)
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap();
        let sink = NoticeSink::new(&log, None);
        let result = adapter
            .run(request(dir.path()), "run-nospawn", limits(), &sink)
            .await;
        assert_eq!(lines(&log), vec!["notify".to_string()], "{name}");
        assert_eq!(sink.calls(), 1, "{name}");
        assert!(
            matches!(result, Err(AdapterError::Spawn(_))),
            "{name}: expected Spawn error, got {result:?}"
        );
    }
}

/// 2 本目以降の起動（route failover）でも通知は呼ばれる。再送しても 1 本目の記録が変わらないことは
/// 永続側（`chat_run_mark_launched`）の冪等性が担う。ここでは「呼ばれても spawn が進む」ことを固定する。
#[tokio::test]
async fn a_second_launch_sends_the_notice_again() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("order.log");
    let adapter = crate::FakeAdapter::new(vec![
        write_stub(dir.path(), &log).to_string_lossy().into_owned(),
    ]);
    let sink = NoticeSink::new(&log, None);
    for run in ["route-1", "route-2"] {
        adapter
            .run(request(dir.path()), run, limits(), &sink)
            .await
            .unwrap();
    }
    assert_eq!(lines(&log), vec!["notify", "spawned", "notify", "spawned"]);
}
