use super::*;
use crate::dispatcher::worker_task::drain_remote_progress_notes;
use std::sync::Mutex as SyncMutex;

#[derive(Default)]
struct RecordingSink {
    lines: SyncMutex<Vec<(String, bool)>>,
}

impl task_worker::EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push((msg.to_string(), false));
        }
    }
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push((msg.to_string(), fields.error));
        }
    }
    fn artifact(&self, _artifact: &task_core::ArtifactRef) {}
}

fn done() -> Result<RunOutcome, AdapterError> {
    Ok(RunOutcome {
        terminal: Terminal::Question { text: "q".into() },
        exit_code: Some(0),
    })
}

fn ws_with(dir: &std::path::Path, program: &str) -> SshWorkspace {
    let mut settings = SshSettings::new("sirius", "sirius", "/work/x");
    settings.ssh_command = vec![program.to_string()];
    settings.rsync_command = vec![program.to_string()];
    SshWorkspace::new(dir, settings)
}

/// 1 run につき push は 1 回で、成功すれば進行を 1 行残し、run の結果はそのまま返る。
#[tokio::test]
async fn a_finished_run_pushes_once_and_logs_one_progress_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = ws_with(dir.path(), "true");
    let sink = RecordingSink::default();
    let out = push_remote_after_run(&ws, &sink, done()).await;
    assert!(out.is_ok(), "{out:?}");
    let lines = sink.lines.lock().expect("lock").clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0]
            .0
            .starts_with("pushed the workspace to cluster sirius:/work/x")
    );
    assert!(!lines[0].1);
    assert!(!ws.push_pending());
}

/// push が落ちたら error 付きの進行を残し、成功した run でも失敗として返す（黙って review に進まない）。
/// 印は残る（次の pull は `--delete` の前に push をやり直す）。
#[tokio::test]
async fn a_failed_push_is_reported_and_fails_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = ws_with(dir.path(), "false");
    let sink = RecordingSink::default();
    let out = push_remote_after_run(&ws, &sink, done()).await;
    assert!(out.is_err(), "{out:?}");
    let lines = sink.lines.lock().expect("lock").clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0]
            .0
            .contains("push to cluster sirius:/work/x after the run failed")
    );
    assert!(lines[0].1, "error flag");
    assert!(ws.push_pending());
    // run 自体が失敗していれば、その失敗をそのまま返す。
    let out = push_remote_after_run(&ws, &sink, Err(AdapterError::Other("boom".into()))).await;
    assert!(
        matches!(out, Err(AdapterError::Other(ref m)) if m == "boom"),
        "{out:?}"
    );
}

/// ADR-0079 付記「R6-1」D6: remote の準備の進行の行（R6-3 の submodule の初期化など）は run の進行に 1 行ずつ
/// 残る。準備で何も溜まらなければ何も書かない（新しい `SshWorkspace` は空）。
#[test]
fn remote_prepare_notes_are_drained_into_worker_progress() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = ws_with(dir.path(), "true");
    let sink = RecordingSink::default();
    assert_eq!(
        drain_remote_progress_notes(ws.take_progress_notes(), &sink),
        0
    );
    assert!(sink.lines.lock().expect("lock").is_empty());
    let notes = vec![
        "initialised 2 submodules in /work/x/.celeris-worktrees/t on cluster sirius".to_string(),
        "second note".to_string(),
    ];
    assert_eq!(drain_remote_progress_notes(notes.clone(), &sink), 2);
    let lines = sink.lines.lock().expect("lock").clone();
    assert_eq!(
        lines,
        notes.into_iter().map(|n| (n, false)).collect::<Vec<_>>()
    );
}
