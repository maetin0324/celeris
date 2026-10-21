use super::*;

use std::process::Command;

/// The env an eval adapter was given through `with_env` (the per-trial `CARGO_TARGET_DIR`).
type EvalEnv = Vec<(String, String)>;

fn env_value(env: &EvalEnv, name: &str) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

#[derive(Clone)]
struct EvalStubAdapter {
    seen: Arc<StdMutex<Vec<(RunRequest, EvalEnv)>>>,
    started: Arc<tokio::sync::Notify>,
    env: EvalEnv,
    /// `true`: the run never finishes (a daemon that dies mid-run).
    hang: bool,
}

#[async_trait]
impl WorkerAdapter for EvalStubAdapter {
    fn id(&self) -> &str {
        "eval-stub"
    }

    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
    }

    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push((req, self.env.clone()));
        self.started.notify_one();
        if self.hang {
            std::future::pending::<()>().await;
        }
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

/// The dispatcher holds its own instance id (`daemon_instances` row), as an active daemon does:
/// only then does it start, recover and clean up evaluation trials
/// (ADR 2026-10-14-eval-verify-isolation D1).
fn own_instance(d: &mut Dispatcher, instance_id: &str) {
    d.set_orphan_takeover(crate::orphan::OrphanTakeover {
        instance_id: instance_id.into(),
        freshness: Duration::from_secs(60),
        pid_alive: Arc::new(crate::orphan::proc_pid_alive),
    });
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
        env: Vec::new(),
        hang: false,
    });
    let mut dispatcher = dispatcher(store.clone(), adapter, 2);
    own_instance(&mut dispatcher, "01EVALDAEMON00000000000000");
    dispatcher.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("data"), None);
    scratch_on(&mut dispatcher, &temp.path().join("scratch"));
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
    let request = seen.lock().unwrap()[0].0.clone();
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
    /// The env given through `with_env` (cargo would build into its `CARGO_TARGET_DIR`).
    env: EvalEnv,
}

#[async_trait]
impl WorkerAdapter for ScriptedEvalWorker {
    fn id(&self) -> &str {
        "eval-scripted"
    }

    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
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
        // What a cargo build in the run would leave behind in the trial's target.
        if let Some(target) = env_value(&self.env, "CARGO_TARGET_DIR") {
            std::fs::create_dir_all(std::path::Path::new(&target).join("debug")).unwrap();
            std::fs::write(std::path::Path::new(&target).join("debug/artifact"), "x").unwrap();
        }
        self.seen.lock().unwrap().push((req.clone(), run_dir));
        if let Some(message) = &self.fail_with {
            return Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: message.clone(),
                    retryable: false,
                    usage: None,
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
                    usage: None,
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
    own_instance(&mut dispatcher, "01EVALDAEMON00000000000000");
    dispatcher.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(root.join("data"), None);
    scratch_on(&mut dispatcher, &root.join("scratch"));
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
            env: Vec::new(),
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
            env: Vec::new(),
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
    // An abnormal end also removes the trial's target and releases its lease.
    let (target, lease) = trial_target(temp.path(), &trial.trial_id);
    assert!(!target.exists(), "{}", target.display());
    assert!(released(&lease));
}

/// `<root>/scratch/targets/agent-eval-<trial_id>/{target,lease.json}` (the pool `scratch_on` set up).
fn trial_target(
    root: &std::path::Path,
    trial_id: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let pool = task_worker::scratch::Pool::new(root.join("scratch"));
    let owner =
        task_worker::scratch::Owner::parse(&format!("agent-eval-{trial_id}")).expect("owner");
    (pool.target_dir(&owner), pool.lease_path(&owner))
}

fn released(lease: &std::path::Path) -> bool {
    task_worker::scratch::read_lease(lease)
        .unwrap()
        .is_some_and(|l| l.released_at.is_some())
}

/// ADR 2026-10-14-eval-trial-scratch-target D1: two trials running at the same time get their own
/// `CARGO_TARGET_DIR` under the scratch pool (never the root fs, never shared), and the request's
/// `cargo_target_dir` is the same path the adapter's env carries.
#[tokio::test]
async fn eval_run_request_sets_per_trial_cargo_target_dir() {
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
            source_id: "p1".into(),
            model_id: "m".into(),
            harness_id: "test".into(),
            repo: repo.to_string_lossy().into_owned(),
            base_commit: base_commit.clone(),
            objective: "Build it.".into(),
            check_cmd: "true".into(),
            check_timeout_secs: 5,
        })
        .collect::<Vec<_>>();
    sqlite.insert_eval_trials(&batch_id, &specs).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: seen.clone(),
        started: Arc::new(tokio::sync::Notify::new()),
        env: Vec::new(),
        // Both runs stay in flight: the two targets exist at the same time.
        hang: true,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    own_instance(&mut d, "01EVALDAEMON00000000000000");
    d.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("data"), None);
    let scratch = scratch_on(&mut d, &temp.path().join("scratch"));
    d.config.eval.reserved_runs = 2;
    d.set_eval_version_probe(|_| Some("eval-stub 1.0.0".into()));

    assert_eq!(d.tick_eval_trials(), 2);
    // Wait for both adapters to have been called (event wait, no fixed delay).
    let deadline = Instant::now() + Duration::from_secs(30);
    while seen.lock().unwrap().len() < 2 {
        assert!(Instant::now() < deadline, "both eval runs must start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let seen = seen.lock().unwrap().clone();
    let trials = store.eval_trials_list(Some(&batch_id), None).unwrap();
    let targets_dir = scratch.pool().targets_dir();
    let mut dirs = Vec::new();
    for (req, env) in &seen {
        let dir = env_value(env, "CARGO_TARGET_DIR").expect("CARGO_TARGET_DIR in the run env");
        let dir = std::path::PathBuf::from(dir);
        assert!(dir.starts_with(&targets_dir), "{}", dir.display());
        assert_eq!(req.cargo_target_dir.as_ref(), Some(&dir));
        assert!(dir.is_dir(), "allocated before the run: {}", dir.display());
        // `[scratch.cargo]` rides along, as for task runs.
        assert_eq!(env_value(env, "CARGO_INCREMENTAL").as_deref(), Some("0"));
        // The owner is the trial (`agent-eval-<trial_id>`).
        let trial = trials
            .iter()
            .find(|t| {
                req.cwd()
                    .starts_with(temp.path().join("data/eval").join(&t.trial_id))
            })
            .expect("the request's trial");
        assert_eq!(dir, trial_target(temp.path(), &trial.trial_id).0);
        dirs.push(dir);
    }
    assert_ne!(
        dirs[0], dirs[1],
        "concurrent trials must not share a target"
    );
    d.abort_eval_runs();
    // Stopping the dispatcher removes the targets of the aborted trials.
    for dir in &dirs {
        assert!(!dir.exists(), "{}", dir.display());
    }
}

/// The run builds into the trial's target; once the hidden check has scored the trial the target
/// is gone and the lease carries `released_at`.
#[tokio::test]
async fn eval_trial_target_removed_after_scoring() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("test -f solved.txt");
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: seen.clone(),
            env: Vec::new(),
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
    let (target, lease) = trial_target(temp.path(), &trial.trial_id);
    let (req, _) = seen.lock().unwrap()[0].clone();
    // The run was given this target (and wrote into it).
    assert_eq!(req.cargo_target_dir.as_ref(), Some(&target));
    assert!(!target.exists(), "{}", target.display());
    assert!(released(&lease));
    // The work tree is removed as before.
    assert!(!std::path::Path::new(trial.worktree_path.as_deref().unwrap()).exists());
}

/// The hidden check runs with the trial's `CARGO_TARGET_DIR` (the same one the run had).
#[tokio::test]
async fn eval_trial_check_uses_trial_target_dir() {
    let probe = tempfile::tempdir().unwrap();
    let out = probe.path().join("check-env.txt");
    let check = format!(
        "test -d \"$CARGO_TARGET_DIR\" && printf '%s' \"$CARGO_TARGET_DIR\" > '{}'",
        out.display()
    );
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture(&check);
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: seen.clone(),
            env: Vec::new(),
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
    let (target, _) = trial_target(temp.path(), &trial.trial_id);
    let (req, _) = seen.lock().unwrap()[0].clone();
    assert_eq!(req.cargo_target_dir.as_ref(), Some(&target));
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        target.display().to_string()
    );
    assert!(!target.exists());
}

/// A daemon that died mid-run: the next dispatcher's recovery marks the trial `error` and removes
/// the target the dead run left behind.
#[tokio::test]
async fn eval_trial_target_removed_on_recovery() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let hung: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: seen.clone(),
        started: Arc::new(tokio::sync::Notify::new()),
        env: Vec::new(),
        hang: true,
    });
    let mut dead = dispatcher(store.clone(), hung.clone(), 2);
    own_instance(&mut dead, "01DEADDAEMON00000000000000");
    dead.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("data"), None);
    scratch_on(&mut dead, &temp.path().join("scratch"));
    dead.config.eval.reserved_runs = 1;
    dead.set_eval_version_probe(|_| Some("eval-stub 1.0.0".into()));
    assert_eq!(dead.tick_eval_trials(), 1);
    let trial = only_trial(&store, &batch_id);
    let (target, lease) = trial_target(temp.path(), &trial.trial_id);
    assert!(target.is_dir());
    // The daemon is gone: its run is not tracked by the next one.
    for (_, entry) in dead.eval_runs.drain() {
        entry.handle.abort();
    }

    let mut next = dispatcher(store.clone(), hung, 2);
    own_instance(&mut next, "01NEXTDAEMON00000000000000");
    next.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("data"), None);
    scratch_on(&mut next, &temp.path().join("scratch"));
    next.config.eval.reserved_runs = 0;
    next.tick_eval_trials();

    let trial = only_trial(&store, &batch_id);
    assert_eq!(
        trial.status,
        task_core::routing_eval_trial::EvalTrialStatus::Error
    );
    assert!(!target.exists(), "{}", target.display());
    assert!(released(&lease));
}

/// A `daemon_instances` row for an evaluation-trial owner: `heartbeat_at` and `pid` decide whether
/// recovery treats it as alive (ADR 2026-10-14-eval-verify-isolation D3.4).
fn eval_owner_row(
    instance_id: &str,
    role: task_core::InstanceRole,
    pid: u32,
    heartbeat_at: time::OffsetDateTime,
) -> task_core::DaemonInstance {
    task_core::DaemonInstance {
        instance_id: instance_id.into(),
        release: "old".into(),
        pid,
        role,
        started_at: heartbeat_at,
        heartbeat_at,
        handoff_requested_at: None,
        drained_at: None,
    }
}

/// The old instance (`owner`) starts a trial whose run hangs; returns it with the trial's work tree.
fn start_hung_eval_trial(
    temp: &std::path::Path,
    store: &Arc<dyn TaskStore>,
    batch_id: &str,
    owner: &str,
) -> (Dispatcher, task_core::routing_eval_trial::EvalTrial) {
    let hung: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: Arc::new(StdMutex::new(Vec::new())),
        started: Arc::new(tokio::sync::Notify::new()),
        env: Vec::new(),
        hang: true,
    });
    let mut old = dispatcher(store.clone(), hung, 2);
    own_instance(&mut old, owner);
    old.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.join("data"), None);
    scratch_on(&mut old, &temp.join("scratch"));
    old.config.eval.reserved_runs = 1;
    old.set_eval_version_probe(|_| Some("eval-stub 1.0.0".into()));
    assert_eq!(old.tick_eval_trials(), 1);
    let trial = only_trial(store, batch_id);
    assert_eq!(trial.instance_id.as_deref(), Some(owner));
    (old, trial)
}

/// The new instance after a live promotion: owns its instance id, starts nothing, only recovers.
fn recovering_dispatcher(temp: &std::path::Path, store: &Arc<dyn TaskStore>) -> Dispatcher {
    let mut next = eval_dispatcher(
        store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: Arc::new(StdMutex::new(Vec::new())),
            env: Vec::new(),
        },
        temp,
    );
    next.config.eval.reserved_runs = 0;
    next
}

/// ADR 2026-10-14-eval-verify-isolation D3.1・D3.4: the claim records the owner instance, and the new
/// instance's recovery leaves a `running` trial of a live owner (the draining old instance, fresh
/// heartbeat, live pid) alone: the trial stays `running`, its work tree and target stay, and its
/// `runs` row stays `running`.
#[tokio::test]
async fn eval_recovery_keeps_running_trial_of_live_owner() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
    let owner = "01OLDDAEMON000000000000000";
    store
        .instance_register(&eval_owner_row(
            owner,
            task_core::InstanceRole::Draining,
            std::process::id(),
            time::OffsetDateTime::now_utc(),
        ))
        .unwrap();
    let (mut old, trial) = start_hung_eval_trial(temp.path(), &store, &batch_id, owner);
    let worktree = std::path::PathBuf::from(trial.worktree_path.clone().unwrap());
    let run_id = trial.run_id.clone().unwrap();
    let (target, _lease) = trial_target(temp.path(), &trial.trial_id);
    assert!(worktree.is_dir() && target.is_dir());

    let mut next = recovering_dispatcher(temp.path(), &store);
    next.tick_eval_trials();
    next.tick_eval_trials();

    let after = only_trial(&store, &batch_id);
    assert_eq!(
        after.status,
        task_core::routing_eval_trial::EvalTrialStatus::Running
    );
    assert_eq!(after.error, None);
    assert_eq!(after.instance_id.as_deref(), Some(owner));
    assert!(worktree.is_dir(), "{}", worktree.display());
    assert!(target.is_dir(), "{}", target.display());
    assert_eq!(
        store.run_index_get(&run_id).unwrap().unwrap().status,
        task_core::RunIndexStatus::Running
    );
    for (_, entry) in old.eval_runs.drain() {
        entry.handle.abort();
    }
}

/// ADR 2026-10-14-eval-verify-isolation D3.4・D3.5: when the recorded owner is dead (stale heartbeat, or
/// a pid that is gone), the new instance's recovery ends the trial as `error`
/// (`interrupted: eval recovery: …`), removes its work tree and target, and closes its `runs` row as
/// `cancelled`.
#[tokio::test]
async fn eval_recovery_errors_trial_of_dead_owner_and_cancels_run_row() {
    let now = time::OffsetDateTime::now_utc();
    let dead_owners = [
        (
            "stale-heartbeat",
            std::process::id(),
            now - time::Duration::hours(1),
        ),
        ("no-pid", u32::MAX, now),
    ];
    for (label, pid, heartbeat_at) in dead_owners {
        let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
        let owner = "01OLDDAEMON000000000000000";
        store
            .instance_register(&eval_owner_row(
                owner,
                task_core::InstanceRole::Draining,
                pid,
                heartbeat_at,
            ))
            .unwrap();
        let (mut old, trial) = start_hung_eval_trial(temp.path(), &store, &batch_id, owner);
        // The old instance died: nothing holds its run any more.
        for (_, entry) in old.eval_runs.drain() {
            entry.handle.abort();
        }
        let worktree = std::path::PathBuf::from(trial.worktree_path.clone().unwrap());
        let run_id = trial.run_id.clone().unwrap();
        let (target, lease) = trial_target(temp.path(), &trial.trial_id);

        let mut next = recovering_dispatcher(temp.path(), &store);
        next.tick_eval_trials();

        let after = only_trial(&store, &batch_id);
        assert_eq!(
            after.status,
            task_core::routing_eval_trial::EvalTrialStatus::Error,
            "{label}"
        );
        assert_eq!(
            after.error.as_deref(),
            Some("interrupted: eval recovery: the instance that started the run is gone"),
            "{label}"
        );
        assert!(!worktree.exists(), "{label}: {}", worktree.display());
        assert!(!target.exists(), "{label}: {}", target.display());
        assert!(released(&lease), "{label}");
        assert_eq!(
            store.run_index_get(&run_id).unwrap().unwrap().status,
            task_core::RunIndexStatus::Cancelled,
            "{label}"
        );
    }
}

/// An evaluation run that finishes as done only when the test releases it (the old instance's run that
/// is still going when the live promotion starts).
#[derive(Clone)]
struct GatedEvalAdapter {
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl WorkerAdapter for GatedEvalAdapter {
    fn id(&self) -> &str {
        "eval-gated"
    }

    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    fn with_env(&self, _extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(self.clone()))
    }

    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.release.notified().await;
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "gated complete".into(),
                evidence: Vec::new(),
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// The new instance's ownerless-runs reconciliation, run now (not waiting for its interval).
fn reconcile_ownerless_now(d: &mut Dispatcher) {
    d.ownerless_reconciled_at = None;
    d.reconcile_ownerless_runs();
}

fn run_row_status(store: &Arc<dyn TaskStore>, run_id: &str) -> task_core::RunIndexStatus {
    store.run_index_get(run_id).unwrap().unwrap().status
}

/// ADR 2026-10-14-eval-verify-isolation D3.2・D3.3・D3.4: a live promotion while the old instance runs a
/// trial. The old instance (draining) counts the evaluation run in `in_flight` and stays draining; the
/// new instance's recovery and ownerless reconciliation leave the trial and its `runs` row `running`.
/// When the run ends, the old instance closes the `runs` row and leaves the trial in `scoring` without
/// scoring it (its drain then ends); the new instance scores it.
#[tokio::test]
async fn live_handoff_keeps_running_eval_trial() {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
    let owner = "01OLDDAEMON000000000000000";
    store
        .instance_register(&eval_owner_row(
            owner,
            task_core::InstanceRole::Active,
            std::process::id(),
            time::OffsetDateTime::now_utc(),
        ))
        .unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut old = dispatcher(
        store.clone(),
        Arc::new(GatedEvalAdapter {
            release: release.clone(),
        }),
        2,
    );
    own_instance(&mut old, owner);
    old.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("data"), None);
    scratch_on(&mut old, &temp.path().join("scratch"));
    old.config.eval.reserved_runs = 1;
    old.set_eval_version_probe(|_| Some("eval-gated 1.0.0".into()));
    assert_eq!(old.tick_eval_trials(), 1);
    let trial = only_trial(&store, &batch_id);
    assert_eq!(trial.status, S::Running);
    let run_id = trial.run_id.clone().unwrap();
    let worktree = std::path::PathBuf::from(trial.worktree_path.clone().unwrap());

    // The live promotion: the old instance drains, the evaluation run keeps it in flight.
    old.set_accepting_new_work(false);
    assert_eq!(old.in_flight(), 1);
    store
        .instance_register(&eval_owner_row(
            owner,
            task_core::InstanceRole::Draining,
            std::process::id(),
            time::OffsetDateTime::now_utc(),
        ))
        .unwrap();

    // The new instance becomes active, recovers and reconciles: the trial is the old instance's.
    let mut next = recovering_dispatcher(temp.path(), &store);
    next.tick_eval_trials();
    reconcile_ownerless_now(&mut next);
    next.tick_eval_trials();
    let during = only_trial(&store, &batch_id);
    assert_eq!(during.status, S::Running);
    assert_eq!(during.error, None);
    assert!(worktree.is_dir(), "{}", worktree.display());
    assert_eq!(
        run_row_status(&store, &run_id),
        task_core::RunIndexStatus::Running
    );
    // A draining tick starts nothing and does not end the drain while the run is going.
    old.tick().unwrap();
    assert_eq!(old.in_flight(), 1);

    // The old instance's run ends: it closes the runs row and leaves the trial for the new instance.
    release.notify_one();
    assert!(
        run_until_state(&mut old, || run_row_status(&store, &run_id)
            != task_core::RunIndexStatus::Running)
        .await
    );
    assert_eq!(old.in_flight(), 0, "the drain ends with the evaluation run");
    assert_eq!(
        run_row_status(&store, &run_id),
        task_core::RunIndexStatus::Completed
    );
    assert_eq!(
        only_trial(&store, &batch_id).status,
        S::Scoring,
        "a draining instance does not score"
    );
    assert!(worktree.is_dir(), "{}", worktree.display());

    // The new instance scores it.
    next.tick_eval_trials();
    let scored = only_trial(&store, &batch_id);
    assert_eq!(scored.status, S::Passed, "error: {:?}", scored.error);
    assert_eq!(scored.check_exit, Some(0));
    assert!(!worktree.exists(), "{}", worktree.display());
    reconcile_ownerless_now(&mut next);
    assert_eq!(
        run_row_status(&store, &run_id),
        task_core::RunIndexStatus::Completed
    );
}

/// ADR 2026-10-14-eval-verify-isolation D3.5・D3.6: no `eval` runs row is left `running` after a handoff.
/// The active instance's ownerless reconciliation closes as `cancelled` a running `eval` row whose trial
/// is already terminal (the 08:53Z rows), whose trial is gone, and whose trial's owner is dead; the row
/// of a live owner's running trial stays. A drain timeout (`abort_all_runs`) closes the old instance's
/// own evaluation runs.
#[tokio::test]
async fn eval_run_row_not_left_running_after_handoff() {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    let now = time::OffsetDateTime::now_utc();
    let owner = "01OLDDAEMON000000000000000";

    // (a) The trial is already terminal (`error`), its runs row is still running.
    {
        let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
        store
            .instance_register(&eval_owner_row(
                owner,
                task_core::InstanceRole::Draining,
                std::process::id(),
                now,
            ))
            .unwrap();
        let (mut old, trial) = start_hung_eval_trial(temp.path(), &store, &batch_id, owner);
        for (_, entry) in old.eval_runs.drain() {
            entry.handle.abort();
        }
        let run_id = trial.run_id.clone().unwrap();
        store
            .eval_trial_finish(
                &trial.trial_id,
                S::Error,
                None,
                false,
                Some("daemon recovery: run disappeared while trial was running"),
            )
            .unwrap();
        assert_eq!(
            run_row_status(&store, &run_id),
            task_core::RunIndexStatus::Running
        );
        let mut next = recovering_dispatcher(temp.path(), &store);
        reconcile_ownerless_now(&mut next);
        assert_eq!(
            run_row_status(&store, &run_id),
            task_core::RunIndexStatus::Cancelled,
            "terminal trial"
        );
    }

    // (b) The trial's recorded owner is dead (a pid that is gone); (c) its owner is alive.
    for (label, pid, closed) in [
        ("dead owner", u32::MAX, true),
        ("live owner", std::process::id(), false),
    ] {
        let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
        store
            .instance_register(&eval_owner_row(
                owner,
                task_core::InstanceRole::Draining,
                pid,
                now,
            ))
            .unwrap();
        let (mut old, trial) = start_hung_eval_trial(temp.path(), &store, &batch_id, owner);
        let run_id = trial.run_id.clone().unwrap();
        let mut next = recovering_dispatcher(temp.path(), &store);
        reconcile_ownerless_now(&mut next);
        let expected = if closed {
            task_core::RunIndexStatus::Cancelled
        } else {
            task_core::RunIndexStatus::Running
        };
        assert_eq!(run_row_status(&store, &run_id), expected, "{label}");
        for (_, entry) in old.eval_runs.drain() {
            entry.handle.abort();
        }
    }

    // (d) No trial holds the running eval row at all.
    {
        let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
        let run_id = ulid::Ulid::new().to_string();
        store
            .run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: batch_id.clone(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Eval,
                seq: 0,
                status: task_core::RunIndexStatus::Running,
                adapter: None,
                model: None,
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: now
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
                finished_at: None,
            })
            .unwrap();
        let mut next = recovering_dispatcher(temp.path(), &store);
        reconcile_ownerless_now(&mut next);
        assert_eq!(
            run_row_status(&store, &run_id),
            task_core::RunIndexStatus::Cancelled,
            "no trial"
        );
    }

    // (e) The old instance's drain times out: its evaluation run is stopped and its row closed.
    {
        let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
        store
            .instance_register(&eval_owner_row(
                owner,
                task_core::InstanceRole::Draining,
                std::process::id(),
                now,
            ))
            .unwrap();
        let (mut old, trial) = start_hung_eval_trial(temp.path(), &store, &batch_id, owner);
        let run_id = trial.run_id.clone().unwrap();
        old.set_accepting_new_work(false);
        assert_eq!(old.in_flight(), 1);
        assert_eq!(old.abort_all_runs(), 1);
        assert_eq!(old.in_flight(), 0);
        assert_eq!(
            run_row_status(&store, &run_id),
            task_core::RunIndexStatus::Cancelled,
            "drain timeout"
        );
        assert_eq!(only_trial(&store, &batch_id).status, S::Error);
    }
}

/// With scratch disabled the trial fails before a work tree or target is made (no root-fs target).
#[tokio::test]
async fn eval_trial_fails_without_scratch() {
    let (temp, store, _sqlite, batch_id, _repo) = eval_fixture("true");
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: seen.clone(),
            env: Vec::new(),
        },
        temp.path(),
    );
    d.config.scratch = task_worker::scratch::ScratchSettings::disabled();
    d.tick_eval_trials();

    let trial = only_trial(&store, &batch_id);
    assert_eq!(
        trial.status,
        task_core::routing_eval_trial::EvalTrialStatus::Error
    );
    assert!(
        trial
            .error
            .as_deref()
            .unwrap_or("")
            .contains("scratch is disabled"),
        "{:?}",
        trial.error
    );
    assert!(seen.lock().unwrap().is_empty(), "no run without a target");
    assert!(
        !temp
            .path()
            .join("data/eval")
            .join(&trial.trial_id)
            .join("repo")
            .exists()
    );
}

/// Trials on a fresh git repo, one per `(source_id, trial_no)` in the given (creation) order.
fn eval_queue(
    sources: &[(&str, u32)],
) -> (
    tempfile::TempDir,
    Arc<dyn TaskStore>,
    Arc<SqliteStore>,
    String,
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
    let specs = sources
        .iter()
        .map(
            |(source, trial_no)| task_core::routing_eval_trial::EvalTrialSpec {
                manifest_sha: "sha256:manifest".into(),
                problem_id: "problem-1".into(),
                trial_no: *trial_no,
                source_id: (*source).into(),
                model_id: "m".into(),
                harness_id: "test".into(),
                repo: repo.to_string_lossy().into_owned(),
                base_commit: base_commit.clone(),
                objective: "Create solved.txt.".into(),
                check_cmd: "true".into(),
                check_timeout_secs: 5,
            },
        )
        .collect::<Vec<_>>();
    sqlite.insert_eval_trials(&batch_id, &specs).unwrap();
    (temp, store, sqlite, batch_id)
}

/// Sources `p1` and `p2`, each with one account `default` that runs one run at a time; the
/// runs never finish, so a started trial keeps its account for the whole test.
fn two_source_eval_dispatcher(
    store: &Arc<dyn TaskStore>,
    root: &std::path::Path,
    reserved_runs: usize,
) -> Dispatcher {
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: Arc::new(StdMutex::new(Vec::new())),
        started: Arc::new(tokio::sync::Notify::new()),
        env: Vec::new(),
        hang: true,
    });
    let mut d = dispatcher(store.clone(), Arc::clone(&adapter), 1);
    own_instance(&mut d, "01EVALDAEMON00000000000000");
    install_test_routes(&mut d, adapter, &[("p1", "test"), ("p2", "test")], 1);
    d.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(root.join("data"), None);
    scratch_on(&mut d, &root.join("scratch"));
    d.config.eval.reserved_runs = reserved_runs;
    d.set_eval_version_probe(|_| Some("eval-stub 1.0.0".into()));
    d
}

fn trial_statuses(
    store: &Arc<dyn TaskStore>,
    batch_id: &str,
) -> Vec<(String, u32, task_core::routing_eval_trial::EvalTrialStatus)> {
    store
        .eval_trials_list(Some(batch_id), None)
        .unwrap()
        .into_iter()
        .map(|t| (t.source_id, t.trial_no, t.status))
        .collect()
}

/// Head-of-line blocking: the queue starts with `p1` trials and `p1`'s only account fills up on
/// the first one. The `p2` trial behind them still starts in the same tick, and the deferred
/// `p1` trials stay `queued` in their order.
#[tokio::test]
async fn eval_trial_full_account_does_not_block_other_source_in_same_tick() {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    let (temp, store, _sqlite, batch_id) =
        eval_queue(&[("p1", 0), ("p1", 1), ("p1", 2), ("p2", 0)]);
    let mut d = two_source_eval_dispatcher(&store, temp.path(), 4);

    assert_eq!(
        d.tick_eval_trials(),
        2,
        "p1's first trial and the p2 trial behind the deferred p1 trials start in one tick"
    );
    assert_eq!(
        trial_statuses(&store, &batch_id),
        vec![
            ("p1".into(), 0, S::Running),
            ("p1".into(), 1, S::Queued),
            ("p1".into(), 2, S::Queued),
            ("p2".into(), 0, S::Running),
        ]
    );
    assert_eq!(d.eval_runs.len(), 2);
    // Both accounts are full now: the next tick starts nothing and leaves the queue as it is.
    assert_eq!(d.tick_eval_trials(), 0);
    assert_eq!(
        trial_statuses(&store, &batch_id)
            .iter()
            .filter(|(_, _, s)| *s == S::Queued)
            .count(),
        2
    );
    for entry in d.eval_runs.values() {
        entry.handle.abort();
    }
}

/// A tick claims each trial at most once and does not walk the queue: once `p1` is full, the
/// store skips `p1` for the rest of the tick, so the tick ends after one deferral. The store work
/// of the tick (connections taken) is the same for 3 and for 300 queued `p1` trials.
#[tokio::test]
async fn eval_trial_tick_claims_each_trial_at_most_once() {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    let mut costs = Vec::new();
    for size in [3u32, 300] {
        let queue = (0..size).map(|n| ("p1", n)).collect::<Vec<_>>();
        let (temp, store, sqlite, batch_id) = eval_queue(&queue);
        let mut d = two_source_eval_dispatcher(&store, temp.path(), 4);

        let before = sqlite.lock_counts();
        assert_eq!(d.tick_eval_trials(), 1);
        costs.push(sqlite.lock_counts().since(before));

        let trials = store.eval_trials_list(Some(&batch_id), None).unwrap();
        assert_eq!(trials[0].status, S::Running);
        assert!(trials[1..].iter().all(|t| t.status == S::Queued));
        // The deferred trial went back to `queued` once; a second unclaim in the tick would fail.
        assert!(sqlite.eval_trial_unclaim(&trials[1].trial_id).is_err());
        // With `p1` skipped there is nothing left to claim, so the tick stopped there.
        assert!(
            sqlite
                .eval_trial_claim_next("test-instance", &["p1".into()])
                .unwrap()
                .is_none()
        );
        for entry in d.eval_runs.values() {
            entry.handle.abort();
        }
    }
    assert_eq!(
        costs[0], costs[1],
        "the tick's store work must not grow with the number of queued trials"
    );
}

/// ADR 2026-10-14-eval-verify-isolation D2: a normal-mode dispatcher that ends `running` trials
/// as `error` removes only the work trees under `<workspace_root>/eval/<trial_id>/` (real paths) and
/// only the targets under its scratch pool. A work tree in another temp dir, one reached through a
/// symlink out of the root, or one under another trial's dir is left alone with its files; the
/// trial still ends as `error` with the recovery reason.
#[tokio::test]
async fn eval_cleanup_refuses_paths_outside_roots() {
    let (temp, store, _sqlite, batch_id) =
        eval_queue(&[("p1", 0), ("p1", 1), ("p1", 2), ("p1", 3)]);
    let outside = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let mut ids = Vec::new();
    while let Some(trial) = store.eval_trial_claim_next("test-instance", &[]).unwrap() {
        ids.push(trial.trial_id);
    }
    assert_eq!(ids.len(), 4);
    let seed = |dir: &std::path::Path| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("keep.txt"), "x").unwrap();
    };
    // 0: inside its own trial dir (removed). 1: an absolute path in another temp dir (kept).
    // 2: its own trial dir, but `repo` is a symlink out of the root (kept). 3: another trial's dir (kept).
    let inside = data.join("eval").join(&ids[0]).join("repo");
    seed(&inside);
    let foreign = outside.path().join("prod-worktree");
    seed(&foreign);
    let linked_to = outside.path().join("linked-worktree");
    seed(&linked_to);
    let linked = data.join("eval").join(&ids[2]).join("repo");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&linked_to, &linked).unwrap();
    let other_trial = data.join("eval").join("not-this-trial").join("repo");
    seed(&other_trial);
    for (id, path) in ids.iter().zip([&inside, &foreign, &linked, &other_trial]) {
        store
            .eval_trial_mark_started(id, &format!("run-{id}"), "d", "v", &path.to_string_lossy())
            .unwrap();
    }
    // Targets: the pool's own (removed), a look-alike outside the pool (kept), and trial 2's owner
    // dir in the pool being a symlink out of the pool (kept).
    let (pool_target, _) = trial_target(temp.path(), &ids[0]);
    seed(&pool_target);
    let lookalike = outside
        .path()
        .join("targets")
        .join(format!("agent-eval-{}", ids[0]))
        .join("target");
    seed(&lookalike);
    let (linked_target, _) = trial_target(temp.path(), &ids[2]);
    let linked_owner_to = outside.path().join("linked-owner");
    seed(&linked_owner_to.join("target"));
    std::fs::create_dir_all(linked_target.parent().unwrap().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&linked_owner_to, linked_target.parent().unwrap()).unwrap();

    let mut d = eval_dispatcher(
        &store,
        ScriptedEvalWorker {
            fail_with: None,
            seen: Arc::new(StdMutex::new(Vec::new())),
            env: Vec::new(),
        },
        temp.path(),
    );
    d.config.eval.reserved_runs = 0;
    d.tick_eval_trials();

    for trial in store.eval_trials_list(Some(&batch_id), None).unwrap() {
        assert_eq!(
            trial.status,
            task_core::routing_eval_trial::EvalTrialStatus::Error
        );
        assert_eq!(
            trial.error.as_deref(),
            Some("interrupted: eval recovery: the instance that started the run is gone")
        );
    }
    assert!(!inside.exists(), "{}", inside.display());
    assert!(foreign.join("keep.txt").is_file());
    assert!(linked_to.join("keep.txt").is_file());
    assert!(other_trial.join("keep.txt").is_file());
    assert!(!pool_target.exists(), "{}", pool_target.display());
    assert!(lookalike.join("keep.txt").is_file());
    assert!(linked_owner_to.join("target/keep.txt").is_file());
}

/// ADR 2026-10-14-eval-verify-isolation D1: a dispatcher without its own instance id (the
/// `--mode verify` daemon on the copy of the production DB, with the production config) does not
/// touch evaluation trials. Production-like `running` / `scoring` / `queued` trials whose work
/// trees and scratch target sit at absolute paths outside the verify workspace root stay exactly as
/// they were after full ticks: files, trial rows, `runs` rows (role `eval`) and the scratch lease.
#[tokio::test]
async fn verify_daemon_leaves_foreign_eval_trial_files() {
    use task_core::routing_eval_trial::EvalTrialStatus as S;
    let (temp, store, _sqlite, batch_id) = eval_queue(&[("p1", 0), ("p1", 1), ("p1", 2)]);
    // The production side: its work trees and its scratch pool, outside the verify root.
    let prod = tempfile::tempdir().unwrap();
    let prod_scratch = prod.path().join("scratch");
    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(
            store
                .eval_trial_claim_next("test-instance", &[])
                .unwrap()
                .unwrap()
                .trial_id,
        );
    }
    let pool = task_worker::scratch::Pool::new(&prod_scratch);
    let mut files = Vec::new();
    let mut leases = Vec::new();
    for id in &ids {
        let worktree = prod.path().join("workspaces/eval").join(id).join("repo");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join("keep.txt"), "x").unwrap();
        let run_id = format!("run-{id}");
        store
            .eval_trial_mark_started(id, &run_id, "d", "v", &worktree.to_string_lossy())
            .unwrap();
        store
            .run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: batch_id.clone(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Eval,
                seq: 1,
                status: task_core::RunIndexStatus::Running,
                adapter: Some("test".into()),
                model: Some("m".into()),
                account: Some("default".into()),
                session_id: Some(format!("session-{id}")),
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            })
            .unwrap();
        let owner = task_worker::scratch::Owner::parse(&format!("agent-eval-{id}")).expect("owner");
        let no_distance = |_: &task_worker::scratch::AdoptCandidate| None;
        let repo = temp.path().join("repo");
        let allocation = task_worker::scratch::allocate(
            &pool,
            &task_worker::scratch::AllocateRequest {
                owner: &owner,
                repo_path: &repo,
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &no_distance,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        std::fs::write(allocation.target_dir.join("keep.txt"), "x").unwrap();
        files.push(worktree.join("keep.txt"));
        files.push(allocation.target_dir.join("keep.txt"));
        leases.push(pool.lease_path(&owner));
    }
    // One `running` (its run is not held by anyone here), one `scoring`, one still `queued`.
    store.eval_trial_mark_scoring(&ids[1]).unwrap();

    let snapshot = |store: &Arc<dyn TaskStore>| {
        let trials = store.eval_trials_list(Some(&batch_id), None).unwrap();
        let runs = ids
            .iter()
            .map(|id| store.run_index_get(&format!("run-{id}")).unwrap())
            .collect::<Vec<_>>();
        // The size measurement is `scratch_gc`'s (path #6 of the risk table, a later task), not
        // an eval write; everything else of the lease (release included) must stay as it was.
        let leases = leases
            .iter()
            .map(|l| {
                task_worker::scratch::read_lease(l)
                    .unwrap()
                    .map(|mut lease| {
                        lease.size_bytes = None;
                        lease.measured_at = None;
                        lease
                    })
            })
            .collect::<Vec<_>>();
        (trials, runs, leases)
    };
    let before = snapshot(&store);
    assert_eq!(
        before.0.iter().map(|t| t.status).collect::<Vec<_>>(),
        vec![S::Running, S::Scoring, S::Queued]
    );
    assert!(before.1.iter().all(|r| r.is_some()));
    assert!(before.2.iter().all(|l| l.is_some()));

    // verify: the production config (scratch pool, reserved runs), a staging workspace root, and
    // no `set_orphan_takeover`. It accepts new work, so the full tick reaches the eval step.
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(EvalStubAdapter {
        seen: seen.clone(),
        started: Arc::new(tokio::sync::Notify::new()),
        env: Vec::new(),
        hang: true,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    d.config.workspace_roots =
        task_core::local_machine::WorkspaceRoots::new(temp.path().join("staging"), None);
    scratch_on(&mut d, &prod_scratch);
    d.config.eval.reserved_runs = 2;
    d.set_eval_version_probe(|_| Some("eval-stub 1.0.0".into()));
    d.set_accepting_new_work(true);
    for _ in 0..2 {
        d.tick().unwrap();
    }
    assert_eq!(d.tick_eval_trials(), 0);
    d.abort_eval_runs();
    d.interrupt_runs_on_shutdown();

    assert!(d.eval_runs.is_empty());
    assert!(seen.lock().unwrap().is_empty(), "no eval run started");
    assert_eq!(snapshot(&store), before);
    for file in &files {
        assert!(file.is_file(), "{}", file.display());
    }
    assert!(!temp.path().join("staging/eval").exists());
}
