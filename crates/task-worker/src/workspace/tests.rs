use super::*;
use std::time::Instant;
use task_core::WorkspaceSpec;

// `crate::protocol::tests::sample_task` は `mod tests` 自体が private で
// 到達不可のため（`sample_task` は `pub(crate)` だがモジュールに `pub` が無い）、
// ここでは同等のタスクをローカルに組み立てる。`protocol.rs` は編集禁止のため。
fn sample_task() -> Task {
    use task_core::*;
    let now = time::OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 60,
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

#[tokio::test]
async fn prepare_creates_subdirs_keeps_existing_and_copies_absolute_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("inputs")).expect("mkdir inputs");
    std::fs::write(dir.path().join("inputs/marker"), b"keep-me").expect("write marker");

    let input_src = tempfile::tempdir().expect("tempdir");
    let abs_input_path = input_src.path().join("data.txt");
    std::fs::write(&abs_input_path, b"hello").expect("write input");

    let ws = LocalWorkspace::new(dir.path());
    let mut task = sample_task();
    task.workspace = WorkspaceSpec::Local {
        path: dir.path().to_path_buf(),
        mode: None,
    };
    task.inputs = vec![task_core::ArtifactRef {
        name: "data.txt".into(),
        path: abs_input_path.to_string_lossy().into_owned(),
        sha256: String::new(),
        kind: "txt".into(),
        declared: true,
    }];

    let prepared = ws.prepare(&task).await.expect("prepare");
    assert_eq!(prepared, dir.path().canonicalize().expect("canonicalize"));
    assert!(dir.path().join("artifacts").is_dir());
    assert!(dir.path().join("inputs").is_dir());
    assert!(dir.path().join("runs").is_dir());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inputs/marker")).expect("read marker"),
        "keep-me"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("inputs/data.txt")).expect("read copied input"),
        "hello"
    );
}

#[tokio::test]
async fn prepare_uses_existing_relative_input_without_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("scripts")).expect("mkdir scripts");
    std::fs::write(dir.path().join("scripts/run.sh"), b"#!/bin/sh\n").expect("write script");

    let ws = LocalWorkspace::new(dir.path());
    let mut task = sample_task();
    task.workspace = WorkspaceSpec::Local {
        path: dir.path().to_path_buf(),
        mode: None,
    };
    task.inputs = vec![task_core::ArtifactRef {
        name: "run.sh".into(),
        path: "scripts/run.sh".into(),
        sha256: String::new(),
        kind: "sh".into(),
        declared: true,
    }];

    ws.prepare(&task).await.expect("prepare");
}

#[tokio::test]
async fn prepare_errors_on_missing_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let mut task = sample_task();
    task.workspace = WorkspaceSpec::Local {
        path: dir.path().to_path_buf(),
        mode: None,
    };
    task.inputs = vec![task_core::ArtifactRef {
        name: "missing.txt".into(),
        path: "does/not/exist.txt".into(),
        sha256: String::new(),
        kind: "txt".into(),
        declared: true,
    }];

    let err = ws.prepare(&task).await.expect_err("expected InputNotFound");
    assert!(matches!(err, WorkspaceError::InputNotFound(name) if name == "missing.txt"));
}

#[tokio::test]
async fn exec_captures_exit_code_and_stream_tails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let result = ws
        .exec("echo out; echo err 1>&2; exit 3", Duration::from_secs(5))
        .await
        .expect("exec");
    assert_eq!(result.exit, Some(3));
    assert!(result.stdout_tail.contains("out"));
    assert!(result.stderr_tail.contains("err"));
    assert!(!result.timed_out);
}

#[tokio::test]
async fn exec_times_out_and_kills_process_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let start = Instant::now();
    let result = ws
        .exec("sleep 30", Duration::from_millis(300))
        .await
        .expect("exec");
    assert!(result.timed_out);
    assert_eq!(result.exit, None);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn exec_runs_with_workspace_dir_as_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let result = ws.exec("pwd", Duration::from_secs(5)).await.expect("exec");
    let printed = result.stdout_tail.trim();
    let printed_canon = std::path::Path::new(printed)
        .canonicalize()
        .expect("canonicalize pwd output");
    let expected = dir.path().canonicalize().expect("canonicalize dir");
    assert_eq!(printed_canon, expected);
}

#[tokio::test]
async fn collect_returns_sorted_nested_artifacts_with_sha256() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("artifacts/sub")).expect("mkdir");
    std::fs::write(dir.path().join("artifacts/b.txt"), b"bbb").expect("write b");
    std::fs::write(dir.path().join("artifacts/sub/a.json"), b"{}").expect("write a");

    let ws = LocalWorkspace::new(dir.path());
    let task = sample_task();
    let refs = ws.collect(&task).await.expect("collect");

    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].path, "artifacts/b.txt");
    assert_eq!(refs[0].name, "b.txt");
    assert_eq!(refs[0].kind, "txt");
    assert_eq!(refs[1].path, "artifacts/sub/a.json");
    assert_eq!(refs[1].name, "a.json");
    assert_eq!(refs[1].kind, "json");
    assert_eq!(
        refs[1].sha256,
        crate::artifact::sha256_file(&dir.path().join("artifacts/sub/a.json")).expect("sha")
    );
}

/// ADR-0036 D1/D4: 親から workspace を継いだタスクは `.taskd/artifacts/<task_id>/` に置き、
/// `collect` はその中のファイルを **workspace 相対**のパスで返す（GUI の読み取り API がそのまま読める形）。
#[tokio::test]
async fn a_shared_workspace_gets_a_per_task_artifacts_dir_and_relative_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let mut child = sample_task();
    child.parent_id = Some(task_core::TaskId::new());
    child.workspace = WorkspaceSpec::Local {
        path: dir.path().to_path_buf(),
        mode: None,
    };

    ws.prepare(&child).await.expect("prepare");
    let own = dir
        .path()
        .join(".taskd")
        .join("artifacts")
        .join(child.id.to_string());
    assert!(
        own.is_dir(),
        "共有 workspace ではタスクごとのディレクトリを作る"
    );
    assert!(
        !dir.path().join("artifacts").exists(),
        "共有の `artifacts/` は作らない"
    );

    std::fs::write(own.join("report.md"), b"# r").expect("write report");
    // 兄弟が共有 `artifacts/` に置いた同名のファイルは、このタスクの成果物にはならない。
    std::fs::create_dir_all(dir.path().join("artifacts")).expect("mkdir");
    std::fs::write(dir.path().join("artifacts/report.md"), b"sibling").expect("write sibling");

    let refs = ws.collect(&child).await.expect("collect");
    assert_eq!(refs.len(), 1);
    assert_eq!(
        refs[0].path,
        format!(".taskd/artifacts/{}/report.md", child.id)
    );
    assert_eq!(refs[0].name, "report.md");
    assert_eq!(refs[0].kind, "md");
}

#[tokio::test]
async fn collect_returns_empty_when_no_artifacts_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = LocalWorkspace::new(dir.path());
    let task = sample_task();
    let refs = ws.collect(&task).await.expect("collect");
    assert!(refs.is_empty());
}
