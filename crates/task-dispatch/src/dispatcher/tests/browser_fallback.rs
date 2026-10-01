use super::super::worker_task::{RunAdapterPrep, prepare_run_adapter};
use super::*;

// ---- ADR-0107: browser fallback 候補の run ごとの準備 ----

/// `prepare_run_adapter` が施した包みを run 時に記録するテスト用アダプタ。
#[derive(Clone)]
struct PrepRecorder {
    id: &'static str,
    env: Vec<(String, String)>,
    removed: Vec<String>,
    mode: Option<String>,
    model: Option<String>,
    seen: Arc<StdMutex<Vec<PrepSeen>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrepSeen {
    env: Vec<(String, String)>,
    removed: Vec<String>,
    mode: Option<String>,
    model: Option<String>,
    tier: Tier,
}

#[async_trait]
impl WorkerAdapter for PrepRecorder {
    fn id(&self) -> &str {
        self.id
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(PrepSeen {
            env: self.env.clone(),
            removed: self.removed.clone(),
            mode: self.mode.clone(),
            model: self.model.clone(),
            tier: req.task.worker_hint.tier,
        });
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            model: Some(model.into()),
            ..self.clone()
        }))
    }
    fn with_permission_mode(&self, mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        Some(Arc::new(Self {
            mode: Some(mode.into()),
            ..self.clone()
        }))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut env = self.env.clone();
        env.extend(extra.iter().cloned());
        Some(Arc::new(Self {
            env,
            ..self.clone()
        }))
    }
    fn with_env_removed(&self, keys: &[String]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut removed = self.removed.clone();
        removed.extend(keys.iter().cloned());
        Some(Arc::new(Self {
            removed,
            ..self.clone()
        }))
    }
}

struct PrepNullSink;
impl EventSink for PrepNullSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<Arc<dyn task_worker::browser_live::ControlGate>> {
        Some(Arc::new(task_worker::browser_live::InMemoryGate::new()))
    }
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &ArtifactRef) {}
}

/// `self.adapters` の entry と同じく `TieredAdapter` で包んだ記録アダプタ。
fn prep_tiered(id: &'static str, seen: &Arc<StdMutex<Vec<PrepSeen>>>) -> Arc<dyn WorkerAdapter> {
    use task_core::model_routing::ModelBinding;
    Arc::new(task_worker::tiered::TieredAdapter {
        base: Arc::new(PrepRecorder {
            id,
            env: Vec::new(),
            removed: Vec::new(),
            mode: None,
            model: None,
            seen: Arc::clone(seen),
        }),
        models: [
            (Tier::Frontier, "frontier-id"),
            (Tier::Standard, "standard-id"),
        ]
        .into_iter()
        .map(|(tier, id)| {
            (
                tier,
                ModelBinding {
                    name: id.into(),
                    model_id: Some(id.into()),
                    unavailable_reason: None,
                    reasoning_effort: None,
                },
            )
        })
        .collect(),
        account_id: None,
        credential_error: None,
    })
}

async fn prep_run_both(prep: &RunAdapterPrep, tier: Tier) -> (bool, bool, Vec<PrepSeen>) {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let task_id = TaskId::new();
    let (primary, primary_env) =
        prepare_run_adapter(prep_tiered("claude-code", &seen), prep, task_id);
    let (candidate, candidate_env) = prepare_run_adapter(prep_tiered("acp", &seen), prep, task_id);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    task.worker_hint.tier = tier;
    let req = RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task,
        workspace: dir.path().to_path_buf(),
        work_dir: None,
        artifacts_dir: dir.path().join("artifacts"),
        context: RunContext::default(),
    };
    let limits = RunLimits {
        wall_clock: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(10),
    };
    for adapter in [primary, candidate] {
        adapter
            .run(req.clone(), "run1", limits, &PrepNullSink)
            .await
            .unwrap();
    }
    let seen = seen.lock().unwrap().clone();
    (primary_env, candidate_env, seen)
}

// ADR-0107 D2: these tests start with the dispatcher's candidate selection, then exercise the
// worker supervisor with exactly that list. The ledger is a test fixture, not certification.
fn browser_fallback_test_ledger(dir: &Path, ids: &[&str]) -> PathBuf {
    let path = dir.join("conformance.json");
    let cases = [
        "open_allowed_origin",
        "refuse_denied_origin",
        "resume_after_crash",
        "snapshot_has_refs",
        "click_by_ref",
        "screenshot_artifact",
        "download_to_artifacts",
    ];
    let results: Vec<_> = ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "backend_id": id, "version": "0.38.1", "passed": cases,
            })
        })
        .collect();
    std::fs::write(
        &path,
        serde_json::json!({
            "schema": 1, "source": "celeris-browser-conformance", "results": results,
        })
        .to_string(),
    )
    .unwrap();
    path
}

#[derive(Clone)]
struct BrowserFallbackHarness {
    adapter_id: &'static str,
    fails: bool,
    sessions: Arc<StdMutex<Vec<(String, String)>>>,
}

#[async_trait]
impl WorkerAdapter for BrowserFallbackHarness {
    fn id(&self) -> &str {
        self.adapter_id
    }
    async fn run(
        &self,
        req: RunRequest,
        _: &str,
        _: RunLimits,
        _: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let browser = req.context.browser.as_ref().expect("browser context");
        self.sessions
            .lock()
            .unwrap()
            .push((self.adapter_id.into(), browser.run.session_id.clone()));
        if self.fails {
            Err(AdapterError::Other("primary failed".into()))
        } else {
            Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "fallback completed".into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            })
        }
    }
}

type BrowserFallbackFixture = (
    Dispatcher,
    Arc<dyn TaskStore>,
    Task,
    Arc<StdMutex<Vec<(String, String)>>>,
);

fn browser_fallback_dispatcher(dir: &Path, secondary_concurrency: usize) -> BrowserFallbackFixture {
    use crate::policy::{ProviderSpec, StaticPolicy};
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir,
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    store.insert(&task).unwrap();
    let sessions = Arc::new(StdMutex::new(Vec::new()));
    let primary: Arc<dyn WorkerAdapter> = Arc::new(BrowserFallbackHarness {
        adapter_id: "acp",
        fails: true,
        sessions: sessions.clone(),
    });
    let secondary: Arc<dyn WorkerAdapter> = Arc::new(BrowserFallbackHarness {
        adapter_id: "claude-code",
        fails: false,
        sessions: sessions.clone(),
    });
    let mut d = dispatcher(store.clone(), primary.clone(), 1);
    d.adapters = HashMap::from([("p1".into(), primary), ("p2".into(), secondary)]);
    d.policy = Box::new(StaticPolicy::new(
        vec![
            ProviderSpec {
                id: "p1".into(),
                adapter: "acp".into(),
                tiers: vec![Tier::Standard],
                concurrency: 1,
                model: "m".into(),
            },
            ProviderSpec {
                id: "p2".into(),
                adapter: "claude-code".into(),
                tiers: vec![Tier::Standard],
                concurrency: secondary_concurrency,
                model: "m".into(),
            },
        ],
        Duration::from_secs(60),
    ));
    (d, store, task, sessions)
}

fn browser_fallback_request(task: Task, dir: &Path) -> RunRequest {
    use task_core::{
        BrowserAction, BrowserCapability, BrowserDomainMode, BrowserTaskPolicy, EffectiveProfile,
    };
    RunRequest {
        protocol: PROTOCOL_VERSION,
        task,
        workspace: dir.to_path_buf(),
        work_dir: None,
        artifacts_dir: dir.join("artifacts"),
        cargo_target_dir: None,
        context: RunContext {
            profile: Some(EffectiveProfile {
                browser: Some(BrowserCapability {
                    allowed_domains: vec!["example.com".into()],
                    ..Default::default()
                }),
                ..Default::default()
            }),
            browser_policy: Some(BrowserTaskPolicy {
                policy_id: "public".into(),
                revision: 1,
                domain_mode: BrowserDomainMode::CommonHosts,
                navigation_origins: vec![],
                network_domains: vec!["example.com".into()],
                allowed_actions: vec![BrowserAction::Navigate],
                approval_actions: vec![],
                credential_policy_ids: vec![],
                artifact_policy_id: None,
            }),
            ..Default::default()
        },
    }
}

struct BrowserFallbackSink;
impl EventSink for BrowserFallbackSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<Arc<dyn task_worker::browser_live::ControlGate>> {
        Some(Arc::new(task_worker::browser_live::InMemoryGate::new()))
    }
    fn progress(&self, _: &str) {}
    fn artifact(&self, _: &ArtifactRef) {}
}

#[tokio::test]
async fn dispatch_browser_fallback_primary_fails_alternate_runs_in_fresh_session() {
    // This exercise starts bubblewrap and Chromium. Some build sandboxes forbid user namespaces;
    // detect that host constraint before testing the dispatcher/browser integration.
    let namespace_available = std::process::Command::new("/usr/bin/bwrap")
        .args([
            "--unshare-user",
            "--uid",
            "0",
            "--gid",
            "0",
            "--ro-bind",
            "/",
            "/",
            "--",
            "/bin/true",
        ])
        .status()
        .is_ok_and(|status| status.success());
    if !namespace_available {
        eprintln!("skipping browser runtime integration: user namespaces unavailable");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // The browser run path refuses to start without the isolated runtime (P4-A).
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap();
    task_worker::browser::configure_isolated_runtime(task_worker::browser::IsolatedBrowserConfig {
        live_sessions: None,
        resolver: Some("127.0.0.1".parse().unwrap()),
        record_dir: std::env::temp_dir().join(format!(
            "celeris-browser-dispatch-unit-{}",
            std::process::id()
        )),
        bwrap: "/usr/bin/bwrap".into(),
        sandboxd: bin.join("celeris-browser-sandboxd"),
        egress: bin.join("celeris-browser-egress"),
    });
    let (d, _, task, sessions) = browser_fallback_dispatcher(dir.path(), 1);
    let record = browser_fallback_test_ledger(dir.path(), &["acp", "claude-code"]);
    let candidates =
        d.browser_fallback_candidates(task.id, "fallback-run", &"p1".into(), "acp", Some(&record));
    assert_eq!(
        candidates.iter().map(|a| a.id()).collect::<Vec<_>>(),
        vec!["claude-code"]
    );
    let executable = dir.path().join("fake-agent-browser");
    std::fs::write(&executable, "#!/bin/sh\nif [ \"$1\" = '--version' ]; then echo 'agent-browser 0.38.1'; exit 0; fi\necho '{\"success\":true}'\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let outcome = task_worker::browser::run_with_executable_candidates_record(
        d.adapters.get("p1").unwrap().clone(),
        candidates,
        browser_fallback_request(task, dir.path()),
        "fallback-run",
        RunLimits {
            wall_clock: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        &BrowserFallbackSink,
        &executable,
        None,
        &record,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = sessions.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].1, seen[1].1);
    assert!(
        dir.path()
            .join("runs/fallback-run/browser-fallback-1/config.json")
            .exists()
    );
}

#[test]
fn dispatch_browser_fallback_disabled_provider_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let (d, _, task, sessions) = browser_fallback_dispatcher(dir.path(), 0);
    let record = browser_fallback_test_ledger(dir.path(), &["acp", "claude-code"]);
    assert!(
        d.browser_fallback_candidates(task.id, "run", &"p1".into(), "acp", Some(&record))
            .is_empty()
    );
    assert!(sessions.lock().unwrap().is_empty());
}

#[test]
fn dispatch_browser_fallback_unhealthy_provider_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let (mut d, _, task, _) = browser_fallback_dispatcher(dir.path(), 1);
    let record = browser_fallback_test_ledger(dir.path(), &["acp", "claude-code"]);
    d.policy.report("p2".into(), &ProviderOutcome::AuthFailed);
    assert!(
        d.browser_fallback_candidates(task.id, "run", &"p1".into(), "acp", Some(&record))
            .is_empty()
    );
}

#[test]
fn dispatch_browser_fallback_no_conformant_candidate_records_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (d, store, task, _) = browser_fallback_dispatcher(dir.path(), 1);
    let record = browser_fallback_test_ledger(dir.path(), &["acp"]);
    assert!(
        d.browser_fallback_candidates(task.id, "run", &"p1".into(), "acp", Some(&record))
            .is_empty()
    );
    assert!(store.events_for(task.id).unwrap().iter().any(|(_, e)| matches!(e,
            Event::WorkerProgress { msg, .. } if msg.contains("browser fallback refused: no enabled, healthy, conformant alternate backend")
        )));
}

#[test]
fn dispatch_browser_fallback_credential_use_never_replays() {
    let dir = tempfile::tempdir().unwrap();
    let (d, store, task, _) = browser_fallback_dispatcher(dir.path(), 1);
    let record = browser_fallback_test_ledger(dir.path(), &["acp", "claude-code"]);
    let mut policy = browser_fallback_request(task.clone(), dir.path())
        .context
        .browser_policy
        .unwrap();
    policy
        .allowed_actions
        .push(task_core::BrowserAction::CredentialUse);
    store.browser_task_policy_set(task.id, &policy).unwrap();
    assert!(
        d.browser_fallback_candidates(task.id, "run", &"p1".into(), "acp", Some(&record))
            .is_empty()
    );
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e,
                Event::WorkerProgress { msg, .. } if msg.contains("CredentialUse policy")
            ))
    );
}

/// ADR-0107 D1: 候補は主 adapter と同じ除去 env・`CARGO_TARGET_DIR`・scratch env を受ける。
#[tokio::test]
async fn dispatch_browser_fallback_prep_candidate_gets_primary_env_and_target() {
    let prep = RunAdapterPrep {
        env: Some(task_worker::scratch::CargoEnv {
            set: vec![
                (
                    task_worker::build_cache::CARGO_TARGET_DIR_VAR.to_string(),
                    "/scratch/targets/task-x/target".into(),
                ),
                ("CARGO_INCREMENTAL".into(), "0".into()),
            ],
            remove: vec!["RUSTC_WRAPPER".into(), "SCCACHE_DIR".into()],
        }),
        followups_env: None,
        container: None,
        permission_mode: None,
    };
    let (primary_env, candidate_env, seen) = prep_run_both(&prep, Tier::Standard).await;
    assert!(primary_env && candidate_env);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], seen[1]);
    assert_eq!(
        seen[1].removed,
        vec!["RUSTC_WRAPPER".to_string(), "SCCACHE_DIR".to_string()]
    );
    assert!(seen[1].env.contains(&(
        task_worker::build_cache::CARGO_TARGET_DIR_VAR.to_string(),
        "/scratch/targets/task-x/target".to_string()
    )));
}

/// ADR-0107 D1: 候補は主 adapter と同じ実行 tier のモデルと planner の permission mode で走る。
#[tokio::test]
async fn dispatch_browser_fallback_prep_candidate_gets_primary_model_and_permission_mode() {
    let prep = RunAdapterPrep {
        env: None,
        followups_env: None,
        container: None,
        permission_mode: Some("plan".into()),
    };
    let (primary_env, candidate_env, seen) = prep_run_both(&prep, Tier::Frontier).await;
    assert!(!primary_env && !candidate_env);
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], seen[1]);
    assert_eq!(seen[1].model.as_deref(), Some("frontier-id"));
    assert_eq!(seen[1].mode.as_deref(), Some("plan"));
    assert_eq!(seen[1].tier, Tier::Frontier);
    assert!(seen[1].removed.is_empty() && seen[1].env.is_empty());
}

/// ADR-0107 D1: 準備が無い run（env・コンテナ・permission mode 無し）は包まずに素通しする。
#[tokio::test]
async fn dispatch_browser_fallback_prep_empty_prep_leaves_candidate_unwrapped() {
    let (primary_env, candidate_env, seen) =
        prep_run_both(&RunAdapterPrep::default(), Tier::Standard).await;
    assert!(!primary_env && !candidate_env);
    assert_eq!(seen[0], seen[1]);
    assert_eq!(seen[1].mode, None);
    assert_eq!(seen[1].model.as_deref(), Some("standard-id"));
}
