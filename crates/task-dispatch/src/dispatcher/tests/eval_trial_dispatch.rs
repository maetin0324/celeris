use super::*;

use std::process::Command;

#[derive(Clone)]
struct EvalStubAdapter {
    seen: Arc<StdMutex<Vec<RunRequest>>>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl WorkerAdapter for EvalStubAdapter {
    fn id(&self) -> &str {
        "eval-stub"
    }

    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(req);
        self.started.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "stub complete".into(),
                evidence: Vec::new(),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[tokio::test]
async fn eval_trial_reserved_slot_hides_checks_and_records_selected_version() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Eval Test",
            "-c",
            "user.email=eval@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
    );
    let base_commit = git(&repo, &["rev-parse", "HEAD"]);

    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let batch_id = ulid::Ulid::new().to_string();
    let specs = (0..2)
        .map(|trial_no| task_core::routing_eval_trial::EvalTrialSpec {
            manifest_sha: "sha256:manifest".into(),
            problem_id: "problem-1".into(),
            trial_no,
            // `dispatcher()` declares harness `test` and source `p1` (model `m`).
            source_id: "p1".into(),
            model_id: "m".into(),
            harness_id: "test".into(),
            repo: repo.to_string_lossy().into_owned(),
            base_commit: base_commit.clone(),
            objective: "Only this objective is visible.".into(),
            check_cmd: "do-not-show-this-secret-check".into(),
            check_timeout_secs: 5,
        })
        .collect::<Vec<_>>();
    sqlite.insert_eval_trials(&batch_id, &specs).unwrap();

    let seen = Arc::new(StdMutex::new(Vec::new()));
    let started = Arc::new(tokio::sync::Notify::new());
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: seen.clone(),
        started: started.clone(),
    });
    let mut dispatcher = dispatcher(store.clone(), adapter, 2);
    dispatcher.config.workspace_root = temp.path().join("data");
    dispatcher.config.eval.reserved_runs = 1;
    dispatcher.set_eval_version_probe(|_| Some("eval-stub 3.4.1".into()));

    assert_eq!(dispatcher.tick_eval_trials(), 1);
    assert_eq!(
        dispatcher.tick_eval_trials(),
        0,
        "reserved_runs=1 must prevent a second concurrent start"
    );
    assert_eq!(dispatcher.eval_runs.len(), 1);
    // Wait for the adapter's explicit started event; don't use a fixed delay.
    started.notified().await;
    let request = seen.lock().unwrap()[0].clone();
    assert!(request.task.acceptance.is_empty());
    assert_eq!(request.task.objective, "Only this objective is visible.");
    assert!(!request.context.session.as_ref().unwrap().resume);
    // The run carries the declared route (harness `test`, source `p1`, model `m`).
    assert_eq!(
        request
            .task
            .harness_id
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("test")
    );
    let route = request.llm_route.as_ref().expect("selected llm route");
    assert_eq!(route.harness_id, "test");
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("do-not-show-this-secret-check")
    );

    let trials = store.eval_trials_list(Some(&batch_id), None).unwrap();
    assert_eq!(
        trials
            .iter()
            .filter(|t| t.status == task_core::routing_eval_trial::EvalTrialStatus::Running)
            .count(),
        1
    );
    let decisions = sqlite
        .route_decisions_export(None, None, Some("eval"))
        .unwrap();
    assert!(decisions.iter().any(|d| d.decision.stage == "eval"
        && d.decision.output["selected"]["harness_version"] == "eval-stub 3.4.1"));
}

/// Scripted worker that follows the adapter contract with the request exactly as
/// `eval_run_request` built it: cwd is `req.cwd()`, the prompt is the real worker prompt, the
/// result goes to `artifacts_rel()/result.json` resolved from the cwd, the run log directory is
/// `<workspace>/runs/<run_id>` (as the codex/claude-code adapters do), and the result is read back
/// from `artifacts_dir/result.json` (a missing file is the codex adapter's `Terminal::Error`).
#[derive(Clone)]
struct ScriptedEvalWorker {
    /// `Some(message)`: end the run with `Terminal::Error { message }` instead of writing a result.
    fail_with: Option<String>,
    seen: Arc<StdMutex<Vec<(RunRequest, std::path::PathBuf)>>>,
}

#[async_trait]
impl WorkerAdapter for ScriptedEvalWorker {
    fn id(&self) -> &str {
        "eval-scripted"
    }

    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let run_dir = req.workspace.join("runs").join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        self.seen.lock().unwrap().push((req.clone(), run_dir));
        if let Some(message) = &self.fail_with {
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: message.clone(),
                    retryable: false,
                },
                exit_code: Some(1),
            });
        }
        let artifacts = req.artifacts_rel();
        let prompt =
            task_worker::claude_code::build_prompt(&req.task, &req.context, run_id, &artifacts);
        let told = format!("{artifacts}/result.json");
        assert!(prompt.contains(&told), "prompt must name {told}");
        let cwd = req.cwd().to_path_buf();
        // The worker's own change, seen by the hidden check.
        std::fs::write(cwd.join("solved.txt"), "ok").unwrap();
        let target = cwd.join(&told);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, r#"{"summary":"solved","evidence":[]}"#).unwrap();
        if !req.artifacts_dir.join("result.json").is_file() {
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: "worker exited without artifacts/result.json".into(),
                    retryable: false,
                },
                exit_code: Some(0),
            });
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "solved".into(),
                evidence: Vec::new(),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// One-trial batch on a fresh git repo; returns (tempdir, store, sqlite, batch_id, repo).
fn eval_fixture(
    check_cmd: &str,
) -> (
    tempfile::TempDir,
    Arc<dyn TaskStore>,
    Arc<SqliteStore>,
    String,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Eval Test",
            "-c",
            "user.email=eval@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
    );
    let base_commit = git(&repo, &["rev-parse", "HEAD"]);
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let batch_id = ulid::Ulid::new().to_string();
    sqlite
        .insert_eval_trials(
            &batch_id,
            &[task_core::routing_eval_trial::EvalTrialSpec {
                manifest_sha: "sha256:manifest".into(),
                problem_id: "problem-1".into(),
                trial_no: 0,
                source_id: "p1".into(),
                model_id: "m".into(),
                harness_id: "test".into(),
                repo: repo.to_string_lossy().into_owned(),
                base_commit,
                objective: "Create solved.txt.".into(),
                check_cmd: check_cmd.into(),
                check_timeout_secs: 5,
            }],
        )
        .unwrap();
    (temp, store, sqlite, batch_id, repo)
}

fn eval_dispatcher(
    store: &Arc<dyn TaskStore>,
    worker: ScriptedEvalWorker,
    root: &std::path::Path,
) -> Dispatcher {
    let mut dispatcher = dispatcher(store.clone(), Arc::new(worker), 2);
    dispatcher.config.workspace_root = root.join("data");
    dispatcher.config.eval.reserved_runs = 1;
    dispatcher.set_eval_version_probe(|_| Some("eval-scripted 1.0.0".into()));
    dispatcher
}

fn only_trial(
    store: &Arc<dyn TaskStore>,
    batch_id: &str,
) -> task_core::routing_eval_trial::EvalTrial {
    let mut trials = store.eval_trials_list(Some(batch_id), None).unwrap();
    assert_eq!(trials.len(), 1);
    trials.remove(0)
}

fn trial_settled(store: &Arc<dyn TaskStore>, batch_id: &str) -> bool {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    !matches!(
        only_trial(store, batch_id).status,
        S::Queued | S::Running | S::Scoring
    )
}

#[tokio::test]
async fn eval_trial_result_at_artifacts_rel() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("test -f solved.txt");
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: seen.clone(),
        },
        temp.path(),
    );
    assert!(run_until_state(&mut d, || trial_settled(&store, &batch_id)).await);

    let trial = only_trial(&store, &batch_id);
    assert_eq!(
        trial.status,
        task_core::routing_eval_trial::EvalTrialStatus::Passed,
        "error: {:?}",
        trial.error
    );
    assert_eq!(trial.check_exit, Some(0));
    let (req, run_dir) = seen.lock().unwrap()[0].clone();
    let worktree = std::path::PathBuf::from(trial.worktree_path.as_deref().unwrap());
    assert_eq!(req.cwd(), worktree.as_path());
    // The prompt's result.json and the adapter's result.json are the same file.
    assert_eq!(
        req.cwd().join(req.artifact_rel_path("result.json")),
        req.artifacts_dir.join("result.json")
    );
    assert!(req.artifacts_dir.join("result.json").is_file());
    // Neither the run log directory nor the artifacts live in the work tree.
    assert!(!run_dir.starts_with(&worktree), "{}", run_dir.display());
    assert!(!req.artifacts_dir.starts_with(&worktree));
    let runs = store.runs_for_task(batch_id.parse().unwrap()).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, task_core::RunIndexStatus::Completed);
}

#[tokio::test]
async fn eval_trial_error_keeps_reason() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
    let message = "codex exited without artifacts/result.json (stderr: boom)";
    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: Some(message.into()),
            seen: Arc::new(StdMutex::new(Vec::new())),
        },
        temp.path(),
    );
    assert!(run_until_state(&mut d, || trial_settled(&store, &batch_id)).await);

    let trial = only_trial(&store, &batch_id);
    assert_eq!(
        trial.status,
        task_core::routing_eval_trial::EvalTrialStatus::Error
    );
    assert_eq!(
        trial.error.as_deref(),
        Some(format!("run ended as error: {message}").as_str())
    );
    let runs = store.runs_for_task(batch_id.parse().unwrap()).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, task_core::RunIndexStatus::Failed);
    assert_ne!(runs[0].status, task_core::RunIndexStatus::Completed);
}
