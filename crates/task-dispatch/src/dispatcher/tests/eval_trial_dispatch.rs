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
            source_id: "source-1".into(),
            model_id: "model-1".into(),
            harness_id: "eval-stub".into(),
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
    let mut dispatcher = dispatcher_with_adapter_id(store.clone(), adapter, 2, false, "eval-stub");
    dispatcher.config.workspace_root = temp.path().join("data");
    dispatcher.config.eval.reserved_runs = 1;
    dispatcher.set_provider_llm_sources(HashMap::from([("p1".into(), "source-1".into())]));
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
