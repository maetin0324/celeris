use super::*;

#[test]
fn normalize_outcome_maps_done_question_and_error_to_exit_codes() {
    let (msg, exit) = normalize_outcome(Ok(RunOutcome {
        terminal: Terminal::Done {
            summary: "s".into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    }));
    assert!(matches!(msg, WorkerMessage::Done { .. }));
    assert_eq!(exit, 0);

    let (msg, exit) = normalize_outcome(Ok(RunOutcome {
        terminal: Terminal::Question { text: "q?".into() },
        exit_code: Some(0),
    }));
    assert!(matches!(msg, WorkerMessage::Question { .. }));
    assert_eq!(exit, 3);

    let (msg, exit) = normalize_outcome(Ok(RunOutcome {
        terminal: Terminal::Error {
            message: "m".into(),
            retryable: false,
        },
        exit_code: Some(1),
    }));
    assert!(matches!(
        msg,
        WorkerMessage::Error {
            provider_failure: None,
            ..
        }
    ));
    assert_eq!(exit, 4);
}

#[test]
fn normalize_outcome_maps_adapter_errors_to_provider_failure() {
    let (msg, exit) = normalize_outcome(Err(AdapterError::Throttled {
        retry_after: Duration::from_secs(7),
    }));
    assert!(matches!(
        msg,
        WorkerMessage::Error {
            provider_failure: Some(ProviderFailure::Throttled {
                retry_after_secs: 7
            }),
            retryable: true,
            ..
        }
    ));
    assert_eq!(exit, 4);

    let (msg, _) = normalize_outcome(Err(AdapterError::AuthFailed("nope".into())));
    assert!(matches!(
        msg,
        WorkerMessage::Error {
            provider_failure: Some(ProviderFailure::AuthFailed),
            ..
        }
    ));

    let (msg, _) = normalize_outcome(Err(AdapterError::Exhausted("nope".into())));
    assert!(matches!(
        msg,
        WorkerMessage::Error {
            provider_failure: Some(ProviderFailure::Exhausted),
            ..
        }
    ));

    let (msg, _) = normalize_outcome(Err(AdapterError::Other("boom".into())));
    assert!(matches!(
        msg,
        WorkerMessage::Error {
            provider_failure: None,
            ..
        }
    ));
}

/// `Config` を toml を経由せず直接組み立てる（celerisctl は `toml` crate に依存していないため）。
fn cluster_config(clusters: Vec<celeris::config::ClusterConfig>) -> Config {
    Config {
        llm_sources: Default::default(),
        harness_selection: Default::default(),
        harness_declarations: Default::default(),
        llm_routes: Vec::new(),
        route_defaults: Default::default(),
        route_selection: Default::default(),
        routing: Default::default(),
        browser: Default::default(),
        harnesses: Vec::new(),
        db: celeris::config::DbConfig {
            path: PathBuf::from("celeris.sqlite3"),
            ..Default::default()
        },
        workspace_root: PathBuf::from("workspaces"),
        tick_ms: 2000,
        max_concurrency: 2,
        lease_grace_secs: 60,
        idle_timeout_secs: 30,
        kill_grace_secs: 5,
        review_timeout_secs: 60,
        error_cooldown_secs: 30,
        retry_backoff_base_secs: 10,
        retry_backoff_max_secs: 300,
        max_requeues: 3,
        adapters: Default::default(),
        plan: Default::default(),
        reviewer: Default::default(),
        review: Default::default(),
        dispatch: Default::default(),
        api: Default::default(),
        clusters,
        roles: vec![],
        genres: vec![],
        conversation: None,
        org_include: None,
        org: vec![],
        delegation: Default::default(),
        reports: Default::default(),
        notify: Default::default(),
        accounts: None,
        secrets: None,
        memory: None,
        handoff: Default::default(),
        selfdeploy: Default::default(),
        workspace: Default::default(),
        scratch: Default::default(),
        github: Default::default(),
        containers: Default::default(),
        cron: Default::default(),
        maintenance: Default::default(),
        cos: Default::default(),
        storage: Default::default(),
        knowledge: Default::default(),
        docs_maintenance: Default::default(),
        llm_proxy: Default::default(),
        model_catalog: Default::default(),
        sessions: Default::default(),
        mcp: Default::default(),
        source_path: None,
        workspace_root_cli: None,
        source_sha256: None,
        applied_overrides: Vec::new(),
        execution: Default::default(),
        routing_eval: Default::default(),
        jev: Default::default(),
    }
}

fn cluster(id: &str, host: &str) -> celeris::config::ClusterConfig {
    celeris::config::ClusterConfig {
        id: id.into(),
        host: host.into(),
        work_dir: None,
        concurrency: 1,
        sync: "rsync".into(),
        auth: "manual".into(),
        delete_on_push: false,
        setup: vec![],
        env: std::collections::HashMap::new(),
        rsync_excludes: vec![],
        worktree_root: None,
        worktree_base: "HEAD".into(),
        worktree_paths: vec![],
        remove_worktree_when: "never".into(),
        forwards: vec![],
        master_launcher: "auto".into(),
        keepalive_secs: 0,
        liveness_probe_secs: 0,
        job_wait: Default::default(),
        control_persist: "yes".into(),
    }
}

fn task_fixture(status: Status, workspace: WorkspaceSpec) -> Task {
    let now = time::OffsetDateTime::now_utc();
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
        kind: task_core::TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
        },
        workspace,
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 30,
            max_retries: 0,
            max_tokens: None,
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

fn store() -> task_core::SqliteStore {
    task_core::SqliteStore::open_in_memory().unwrap()
}

#[test]
fn resolve_cluster_target_errors_when_cluster_missing() {
    let config = cluster_config(vec![]);
    let task = task_fixture(
        Status::Ready,
        WorkspaceSpec::Local {
            path: "/tmp/x".into(),
            mode: None,
        },
    );
    let err = resolve_cluster_target(&store(), &config, &task, "local", None).unwrap_err();
    assert!(
        err.to_string()
            .contains("cluster not found in config: local"),
        "{err}"
    );
}

#[test]
fn resolve_cluster_target_requires_workspace_arg_for_local_task() {
    let config = cluster_config(vec![cluster("local", "h")]);
    let task = task_fixture(
        Status::Ready,
        WorkspaceSpec::Local {
            path: "/tmp/x".into(),
            mode: None,
        },
    );
    let err = resolve_cluster_target(&store(), &config, &task, "local", None).unwrap_err();
    assert!(err.to_string().contains("has a local workspace"), "{err}");
}

#[test]
fn resolve_cluster_target_uses_task_remote_path_without_workspace_arg() {
    let config = cluster_config(vec![cluster("local", "h")]);
    let task = task_fixture(
        Status::Ready,
        WorkspaceSpec::Remote {
            cluster: "local".into(),
            path: "/remote/proj".into(),
            mode: None,
        },
    );
    let target = resolve_cluster_target(&store(), &config, &task, "local", None).unwrap();
    assert_eq!(target.remote_path, PathBuf::from("/remote/proj"));
    // 固定の無い task の写しは設定ファイルの root（ADR 2026-10-14-local-machine-settings D5）。
    assert_eq!(
        target.mirror_dir,
        config.workspace_root.join(task.id.to_string())
    );
    assert!(target.warning.is_none());
    assert_eq!(target.spec.host, "h");
}

#[test]
fn resolve_cluster_target_workspace_arg_overrides_task_path() {
    let config = cluster_config(vec![cluster("local", "h")]);
    let task = task_fixture(
        Status::Ready,
        WorkspaceSpec::Local {
            path: "/tmp/x".into(),
            mode: None,
        },
    );
    let target = resolve_cluster_target(
        &store(),
        &config,
        &task,
        "local",
        Some(Path::new("/remote/other")),
    )
    .unwrap();
    assert_eq!(target.remote_path, PathBuf::from("/remote/other"));
    assert!(target.warning.is_none());
}

#[test]
fn resolve_cluster_target_warns_when_task_targets_a_different_cluster() {
    let config = cluster_config(vec![cluster("local", "h")]);
    let task = task_fixture(
        Status::Ready,
        WorkspaceSpec::Remote {
            cluster: "other".into(),
            path: "/remote/proj".into(),
            mode: None,
        },
    );
    let target = resolve_cluster_target(&store(), &config, &task, "local", None).unwrap();
    assert_eq!(target.remote_path, PathBuf::from("/remote/proj"));
    assert!(target.warning.as_deref().unwrap().contains("other"));
}

#[test]
fn resolve_cluster_target_rejects_running_or_reviewing_task_even_with_workspace_arg() {
    let config = cluster_config(vec![cluster("local", "h")]);
    for status in [Status::Running, Status::Reviewing] {
        let task = task_fixture(
            status,
            WorkspaceSpec::Local {
                path: "/tmp/x".into(),
                mode: None,
            },
        );
        let err = resolve_cluster_target(
            &store(),
            &config,
            &task,
            "local",
            Some(Path::new("/remote/x")),
        )
        .unwrap_err();
        assert!(err.to_string().contains("stop celeris or wait"), "{err}");
    }
}

// ---- ADR 2026-10-11 llm-source-harness-selection: route selection ----

fn args_fixture() -> WorkerRunArgs {
    WorkerRunArgs {
        config: PathBuf::from("config.toml"),
        task: "01J000000000000000000000AA".into(),
        harness: None,
        source: None,
        model: None,
        workspace: None,
        cluster: None,
        account: None,
    }
}

/// A config with an OpenAI-compatible source, a Claude OAuth pool and three harnesses.
fn route_config(accounts_dir: &Path) -> Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // The loader still requires a provider row until the provider config is removed.
    let text = format!(
        r#"

[accounts]
claude_dir = "{claude}"

[llm_sources.qwen]
kind = "openai_compatible"
models = ["qwen3-coder"]
accounts = ["qwen"]
endpoint = "http://127.0.0.1:18000/v1"

[llm_sources.claude_main]
kind = "claude_oauth"
api_format = "claude_cli"
models = ["claude-sonnet-5-5", "claude-opus-5-5"]

[harnesses.claude]
kind = "claude_code"
api_formats = ["claude_cli"]
source_kinds = ["claude_oauth"]
command = "claude-declared"

[harnesses.pi]
kind = "pi"
extensions = ["extensions/hashline.ts"]
tools = ["hashline_read", "hashline_edit", "bash"]
api_formats = ["openai_compatible"]
source_kinds = ["openai_compatible"]

[harnesses.stub]
kind = "fake"

[route_defaults.claude]
candidates = [
  {{ source = "claude_main", model = "claude-sonnet-5-5" }},
  {{ source = "claude_main", model = "claude-opus-5-5" }},
]

[route_defaults.pi]
candidates = [{{ source = "qwen", model = "qwen3-coder" }}]
"#,
        claude = accounts_dir.join("claude").display()
    );
    std::fs::write(&path, text).unwrap();
    Config::load(&path).unwrap()
}

fn login(accounts_dir: &Path, id: &str) {
    let dir = accounts_dir.join("claude").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
}

fn local_task() -> Task {
    task_fixture(
        Status::Ready,
        WorkspaceSpec::Local {
            path: PathBuf::from("w"),
            mode: None,
        },
    )
}

#[test]
fn worker_route_cli_uses_the_given_harness_and_the_first_declared_candidate() {
    let accounts = tempfile::tempdir().unwrap();
    login(accounts.path(), "ca");
    let config = route_config(accounts.path());
    let registry = config.llm_route_registry().unwrap();
    let mut args = args_fixture();
    args.harness = Some("claude".into());
    let route = select_worker_route(&config, &registry, &local_task(), &args).unwrap();
    assert_eq!(
        (
            route.harness_id.as_str(),
            route.source_id.as_str(),
            route.model_id.as_str(),
            route.account_id.as_str()
        ),
        ("claude", "claude_main", "claude-sonnet-5-5", "ca")
    );
    assert_eq!(
        route.credential,
        Some(
            task_worker::protocol::CredentialReference::AccountDirectory {
                path: accounts.path().join("claude").join("ca"),
                env: task_core::AccountAdapter::ClaudeCode.env_var().into(),
            }
        )
    );
    let adapter = route_launch_adapter(&config, &route).unwrap();
    assert_eq!(adapter.id(), "claude-code");
}

#[test]
fn worker_route_cli_model_flag_selects_that_model() {
    let accounts = tempfile::tempdir().unwrap();
    login(accounts.path(), "ca");
    let config = route_config(accounts.path());
    let registry = config.llm_route_registry().unwrap();
    let mut args = args_fixture();
    args.harness = Some("claude".into());
    args.model = Some("claude-opus-5-5".into());
    let route = select_worker_route(&config, &registry, &local_task(), &args).unwrap();
    assert_eq!(route.model_id, "claude-opus-5-5");
}

#[test]
fn worker_route_cli_openai_compatible_source_carries_its_endpoint() {
    let accounts = tempfile::tempdir().unwrap();
    let config = route_config(accounts.path());
    let registry = config.llm_route_registry().unwrap();
    let mut args = args_fixture();
    args.harness = Some("pi".into());
    let route = select_worker_route(&config, &registry, &local_task(), &args).unwrap();
    assert_eq!(
        (route.source_id.as_str(), route.account_id.as_str()),
        ("qwen", "qwen")
    );
    assert_eq!(
        route.endpoint.and_then(|e| e.base_url).as_deref(),
        Some("http://127.0.0.1:18000/v1")
    );
}

#[test]
fn worker_route_cli_fails_closed() {
    let accounts = tempfile::tempdir().unwrap();
    let config = route_config(accounts.path());
    let registry = config.llm_route_registry().unwrap();
    let task = local_task();
    // Unknown harness.
    let mut args = args_fixture();
    args.harness = Some("aider".into());
    let err = select_worker_route(&config, &registry, &task, &args).unwrap_err();
    assert!(err.to_string().contains("aider"), "{err}");
    // No logged-in account in the OAuth pool.
    args.harness = Some("claude".into());
    let err = select_worker_route(&config, &registry, &task, &args).unwrap_err();
    assert!(err.to_string().contains("no route"), "{err}");
    // A harness without declared defaults.
    args.harness = Some("stub".into());
    let err = select_worker_route(&config, &registry, &task, &args).unwrap_err();
    assert!(err.to_string().contains("route_defaults"), "{err}");
    // Unknown source and an invalid account id.
    let mut args = args_fixture();
    args.source = Some("nowhere".into());
    let err = select_worker_route(&config, &registry, &task, &args).unwrap_err();
    assert!(err.to_string().contains("nowhere"), "{err}");
    let mut args = args_fixture();
    args.account = Some("../x".into());
    let err = select_worker_route(&config, &registry, &task, &args).unwrap_err();
    assert!(err.to_string().contains("invalid --account"), "{err}");
}

#[test]
fn worker_route_cli_account_flag_narrows_the_pool() {
    let accounts = tempfile::tempdir().unwrap();
    login(accounts.path(), "ca");
    login(accounts.path(), "cb");
    let config = route_config(accounts.path());
    let registry = config.llm_route_registry().unwrap();
    let mut args = args_fixture();
    args.harness = Some("claude".into());
    args.account = Some("cb".into());
    let route = select_worker_route(&config, &registry, &local_task(), &args).unwrap();
    assert_eq!(route.account_id, "cb");
    args.account = Some("missing".into());
    assert!(select_worker_route(&config, &registry, &local_task(), &args).is_err());
}
