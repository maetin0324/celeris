//! ADR-0067 付記 2026-10-07: remote（cluster）の task と、question / wait（blocked）で終わった run でも
//! `artifacts_dir` の成果物が `ArtifactProduced{declared:false}` として登録される（`GET /tasks/{id}/artifacts` の
//! 材料）。重複は `(path, sha256)` で見る。偽の ssh（stub）と一時ディレクトリだけで、外部ネットワークに出ない。

use std::collections::VecDeque;

use super::*;

/// `req.artifacts_dir` に `files` を書いてから、用意した終端を順に返す（尽きたら `Done`）。
struct WritesArtifactsAdapter {
    files: StdMutex<Vec<(String, String)>>,
    script: StdMutex<VecDeque<Terminal>>,
}

impl WritesArtifactsAdapter {
    fn new(files: Vec<(&str, &str)>, script: Vec<Terminal>) -> Arc<Self> {
        Arc::new(WritesArtifactsAdapter {
            files: StdMutex::new(
                files
                    .into_iter()
                    .map(|(p, c)| (p.to_string(), c.to_string()))
                    .collect(),
            ),
            script: StdMutex::new(script.into_iter().collect()),
        })
    }
    fn set_files(&self, files: Vec<(&str, &str)>) {
        *self.files.lock().unwrap() = files
            .into_iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
    }
}

#[async_trait]
impl WorkerAdapter for WritesArtifactsAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        for (rel, content) in self.files.lock().unwrap().iter() {
            let path = req.artifacts_dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let terminal = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            });
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn question() -> Terminal {
    Terminal::Question {
        text: "which figure set?".into(),
    }
}

fn waiting() -> Terminal {
    Terminal::Waiting {
        request: task_core::cluster_job::ClusterJobWaitRequest {
            cluster: Some("sirius".into()),
            scheduler: task_core::cluster_job::ClusterScheduler::Pbs,
            jobs: vec!["43508".into()],
            poll_secs: Some(10),
            timeout_secs: None,
            summary: "submitted main-final".into(),
        },
        checkpoint: None,
        usage: None,
    }
}

fn produced(store: &dyn TaskStore, id: TaskId) -> Vec<(String, task_core::ArtifactRef)> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, ev)| match ev {
            Event::ArtifactProduced { run_id, artifact } => Some((run_id, artifact)),
            _ => None,
        })
        .collect()
}

/// remote の task の `run_worker` を偽の ssh（exit 0・shared mode）で回す。`dir` は手元の写し。
async fn run_remote(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    task_id: TaskId,
    stub: &std::path::Path,
    dir: PathBuf,
    run_id: &str,
) -> Result<RunOutcome, AdapterError> {
    let mut settings = SshSettings::new("sirius", "sirius", PathBuf::from("/work/NBB/cmp4"));
    settings.sync = SyncMode::None;
    settings.task_id = task_id.to_string();
    settings.ssh_command = vec![stub.to_string_lossy().into_owned()];
    run_worker(
        store,
        adapter,
        Vec::new(),
        task_id,
        Tier::Standard,
        dir,
        run_id,
        RunLimits {
            wall_clock: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        LeaseRenewal {
            ttl: Duration::from_secs(60),
            every: Duration::from_secs(30),
        },
        Some(settings),
        None,
        RunExtras::default(),
        Vec::new(),
        Vec::new(),
        DelegationLimits::default(),
        None,
        None,
        ContainerDecision::Host,
        CargoTargetPlan::None,
    )
    .await
}

/// D3-a: remote の task で run が question で終わっても、写しの `artifacts/` の成果物（md/png/svg/pdf/csv）が
/// その run の id で登録される。`checkpoint.json` は拾わない。
#[tokio::test]
async fn a_remote_run_that_ends_with_a_question_registers_its_artifacts() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 0);
    let task = remote_task(tmp.path(), Some(WorkspaceMode::Shared), Vec::new());
    store.insert(&task).unwrap();
    let adapter = WritesArtifactsAdapter::new(
        vec![
            ("cmp4/final/report.md", "# report\n"),
            ("cmp4/final/F1-bandwidth.png", "png"),
            ("cmp4/final/F1-bandwidth.svg", "<svg/>"),
            ("cmp4/final/F1-bandwidth.pdf", "%PDF"),
            ("cmp4/final/summary.csv", "a,b\n"),
            ("cmp4/final/tables.md", "| a |\n"),
            ("checkpoint.json", "{}"),
        ],
        vec![question()],
    );
    let mirror = tmp.path().join("mirror");
    let outcome = run_remote(
        store.clone(),
        adapter,
        task.id,
        &stub,
        mirror.clone(),
        "run-q",
    )
    .await;
    assert!(
        matches!(
            outcome,
            Ok(RunOutcome {
                terminal: Terminal::Question { .. },
                ..
            })
        ),
        "{outcome:?}"
    );
    let got = produced(store.as_ref(), task.id);
    let paths: Vec<&str> = got.iter().map(|(_, a)| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "artifacts/cmp4/final/report.md",
            "artifacts/cmp4/final/tables.md",
            "artifacts/cmp4/final/F1-bandwidth.pdf",
            "artifacts/cmp4/final/summary.csv",
            "artifacts/cmp4/final/F1-bandwidth.png",
            "artifacts/cmp4/final/F1-bandwidth.svg",
        ],
        "{got:?}"
    );
    assert!(
        got.iter()
            .all(|(run_id, a)| run_id == "run-q" && !a.declared)
    );
    // `GET /tasks/{id}/artifacts` が解決する path: 写し（`local_dir`）からの相対。
    for (_, a) in &got {
        assert!(mirror.join(&a.path).is_file(), "{}", a.path);
        assert_eq!(
            task_worker::artifact::sha256_file(&mirror.join(&a.path)).unwrap(),
            a.sha256
        );
    }
}

/// D3-a / D3-b: wait（cluster job 待ち）で終わった run も登録し、次の run（question）で同じ中身は増えず、
/// 中身が変わった file だけ新しい版として登録される。`Done` でも同じ。
#[tokio::test]
async fn remote_runs_deduplicate_by_path_and_hash_across_wait_question_and_done() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 0);
    let task = remote_task(tmp.path(), Some(WorkspaceMode::Shared), Vec::new());
    store.insert(&task).unwrap();
    let adapter = WritesArtifactsAdapter::new(
        vec![
            ("cmp4/report.md", "# v1\n"),
            ("cmp4/node-hours.md", "3.2\n"),
        ],
        vec![
            waiting(),
            question(),
            Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
        ],
    );
    let mirror = tmp.path().join("mirror");

    let out = run_remote(
        store.clone(),
        adapter.clone(),
        task.id,
        &stub,
        mirror.clone(),
        "run-wait",
    )
    .await;
    assert!(
        matches!(
            out,
            Ok(RunOutcome {
                terminal: Terminal::Waiting { .. },
                ..
            })
        ),
        "{out:?}"
    );
    let got = produced(store.as_ref(), task.id);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.iter().all(|(run_id, _)| run_id == "run-wait"));

    // 同じ中身で question → 増えない。
    let out = run_remote(
        store.clone(),
        adapter.clone(),
        task.id,
        &stub,
        mirror.clone(),
        "run-q",
    )
    .await;
    assert!(
        matches!(
            out,
            Ok(RunOutcome {
                terminal: Terminal::Question { .. },
                ..
            })
        ),
        "{out:?}"
    );
    assert_eq!(produced(store.as_ref(), task.id).len(), 2);

    // report.md だけ中身が変わって done → その 1 件だけ新しい版。
    adapter.set_files(vec![
        ("cmp4/report.md", "# v2\n"),
        ("cmp4/node-hours.md", "3.2\n"),
    ]);
    let out = run_remote(store.clone(), adapter, task.id, &stub, mirror, "run-done").await;
    assert!(
        matches!(
            out,
            Ok(RunOutcome {
                terminal: Terminal::Done { .. },
                ..
            })
        ),
        "{out:?}"
    );
    let got = produced(store.as_ref(), task.id);
    assert_eq!(got.len(), 3, "{got:?}");
    let (run_id, newest) = got.last().unwrap();
    assert_eq!(run_id, "run-done");
    assert_eq!(newest.path, "artifacts/cmp4/report.md");
    assert_ne!(newest.sha256, got[0].1.sha256);
}

/// D3-a: yield で終わった run は登録しない（中間物を一覧に混ぜない。終端で拾う）。
#[tokio::test]
async fn a_remote_run_that_yields_does_not_register_artifacts() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let stub = write_stub_ssh(tmp.path(), 0);
    let task = remote_task(tmp.path(), Some(WorkspaceMode::Shared), Vec::new());
    store.insert(&task).unwrap();
    let adapter = WritesArtifactsAdapter::new(
        vec![("cmp4/report.md", "# draft\n")],
        vec![Terminal::Yielded {
            checkpoint: serde_json::Value::Null,
            usage: None,
        }],
    );
    let out = run_remote(
        store.clone(),
        adapter,
        task.id,
        &stub,
        tmp.path().join("mirror"),
        "run-y",
    )
    .await;
    assert!(
        matches!(
            out,
            Ok(RunOutcome {
                terminal: Terminal::Yielded { .. },
                ..
            })
        ),
        "{out:?}"
    );
    assert!(produced(store.as_ref(), task.id).is_empty());
}

/// D3-a: local の git worktree の task でも、dispatcher の経路で run が question で終わると
/// `artifacts/` の成果物が登録され、task は blocked（判断待ち）になる。
#[tokio::test]
async fn a_git_worktree_run_that_ends_with_a_question_registers_its_artifacts() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = WritesArtifactsAdapter::new(
        vec![("report.md", "# which?\n"), ("fig.svg", "<svg/>")],
        vec![question()],
    );
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Blocked);

    let got = produced(store.as_ref(), task_id);
    let paths: Vec<&str> = got.iter().map(|(_, a)| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["artifacts/report.md", "artifacts/fig.svg"],
        "{got:?}"
    );
    assert!(got.iter().all(|(_, a)| !a.declared));
    assert_eq!(got[1].1.kind, "svg");
}
