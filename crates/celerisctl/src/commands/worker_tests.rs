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
        providers: vec![],
        providers_include: None,
        providers_dir: None,
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
        knowledge: Default::default(),
        docs_maintenance: Default::default(),
        llm_proxy: Default::default(),
        sessions: Default::default(),
        mcp: Default::default(),
        source_path: None,
        execution: Default::default(),
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
        control_persist: "yes".into(),
    }
}

fn task_fixture(status: Status, workspace: WorkspaceSpec) -> Task {
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
            adapter: None,
        },
        workspace,
        budget: task_core::Budget {
            max_turns: 1,
            max_wall_secs: 30,
            max_retries: 0,
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
    let err = resolve_cluster_target(&config, &task, "local", None).unwrap_err();
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
    let err = resolve_cluster_target(&config, &task, "local", None).unwrap_err();
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
    let target = resolve_cluster_target(&config, &task, "local", None).unwrap();
    assert_eq!(target.remote_path, PathBuf::from("/remote/proj"));
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
    let target =
        resolve_cluster_target(&config, &task, "local", Some(Path::new("/remote/other"))).unwrap();
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
    let target = resolve_cluster_target(&config, &task, "local", None).unwrap();
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
        let err = resolve_cluster_target(&config, &task, "local", Some(Path::new("/remote/x")))
            .unwrap_err();
        assert!(err.to_string().contains("stop celeris or wait"), "{err}");
    }
}

// ---- ADR-0024 D2: resolve_account ----

fn args_fixture(account: Option<&str>) -> WorkerRunArgs {
    WorkerRunArgs {
        config: PathBuf::from("config.toml"),
        task: "01J000000000000000000000AA".into(),
        provider: None,
        adapter: None,
        workspace: None,
        cluster: None,
        account: account.map(str::to_string),
    }
}

fn pool_provider_config(accounts_dir: &Path, max_runs_per_account: usize) -> Config {
    let mut config = cluster_config(vec![]);
    config.providers = vec![celeris::config::ProviderConfig {
        tier_models: Default::default(),
        account_id: None,
        id: "pool".into(),
        adapter: "claude-code".into(),
        tiers: vec![task_core::Tier::Standard],
        concurrency: 1,
        model: String::new(),
        env: Default::default(),
        env_from_secrets: Default::default(),
        account_pool: true,
        command: None,
        args: None,
        settings: None,
    }];
    config.accounts = Some(celeris::config::AccountsConfig {
        claude_dir: Some(accounts_dir.to_path_buf()),
        codex_dir: None,
        max_runs_per_account,
        check_model: "haiku".into(),
    });
    config
}

#[test]
fn resolve_account_non_pool_provider_ignores_missing_account_and_rejects_explicit_one() {
    let config = cluster_config(vec![]);
    assert_eq!(
        resolve_account(&config, "p1", "claude-code", &args_fixture(None)).unwrap(),
        None
    );
    let err = resolve_account(&config, "p1", "claude-code", &args_fixture(Some("a"))).unwrap_err();
    assert!(
        err.to_string().contains("does not have account_pool"),
        "{err}"
    );
}

/// ADR-0026 D5: acp はアカウントのプールを使わない（`Config::validate` が `account_pool = true` を
/// claude-code/codex 以外で拒否しているので、acp プロバイダは常に `is_pool = false` になる）。
/// `--account` 無しなら無視され、`--account` を付けたらエラーになる（他の非プールプロバイダと同じ扱い）。
#[test]
fn resolve_account_ignores_or_rejects_account_flag_for_acp_provider() {
    let mut config = cluster_config(vec![]);
    config.providers = vec![celeris::config::ProviderConfig {
        tier_models: Default::default(),
        account_id: None,
        id: "opencode-qwen".into(),
        adapter: "acp".into(),
        tiers: vec![task_core::Tier::Standard],
        concurrency: 1,
        model: "qwen-local/qwen3.8-27b".into(),
        env: Default::default(),
        env_from_secrets: Default::default(),
        account_pool: false,
        command: None,
        args: None,
        settings: None,
    }];
    assert_eq!(
        resolve_account(&config, "opencode-qwen", "acp", &args_fixture(None)).unwrap(),
        None
    );
    let err =
        resolve_account(&config, "opencode-qwen", "acp", &args_fixture(Some("a"))).unwrap_err();
    assert!(
        err.to_string().contains("does not have account_pool"),
        "{err}"
    );
}

/// ADR-0026 D2: `celerisctl worker run` の `build_adapters` 経由でも acp プロバイダのアダプタが引ける
/// （`select_provider` / `resolve_account` を通した後の配線が壊れていないことの確認）。
#[test]
fn build_adapters_resolves_an_instance_for_an_acp_provider_selected_by_worker_run() {
    let mut config = cluster_config(vec![]);
    config.providers = vec![celeris::config::ProviderConfig {
        tier_models: Default::default(),
        account_id: None,
        id: "opencode-qwen".into(),
        adapter: "acp".into(),
        tiers: vec![task_core::Tier::Standard],
        concurrency: 1,
        model: "qwen-local/qwen3.8-27b".into(),
        env: Default::default(),
        env_from_secrets: Default::default(),
        account_pool: false,
        command: None,
        args: None,
        settings: None,
    }];
    let adapters = celeris::build_adapters(&config);
    let adapter = adapters
        .get("opencode-qwen")
        .expect("acp provider has an adapter instance");
    assert_eq!(adapter.id(), "acp");
}

#[test]
fn resolve_account_pool_provider_without_accounts_section_errors() {
    let mut config = cluster_config(vec![]);
    config.providers = vec![celeris::config::ProviderConfig {
        tier_models: Default::default(),
        account_id: None,
        id: "pool".into(),
        adapter: "claude-code".into(),
        tiers: vec![task_core::Tier::Standard],
        concurrency: 1,
        model: String::new(),
        env: Default::default(),
        env_from_secrets: Default::default(),
        account_pool: true,
        command: None,
        args: None,
        settings: None,
    }];
    let err = resolve_account(&config, "pool", "claude-code", &args_fixture(None)).unwrap_err();
    assert!(
        err.to_string().contains("[accounts] is not configured"),
        "{err}"
    );
}

#[test]
fn resolve_account_explicit_flag_validates_the_id_and_directory() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("b")).unwrap();
    let config = pool_provider_config(tmp.path(), 2);

    assert_eq!(
        resolve_account(&config, "pool", "claude-code", &args_fixture(Some("b"))).unwrap(),
        Some("b".into())
    );

    let err = resolve_account(
        &config,
        "pool",
        "claude-code",
        &args_fixture(Some("missing")),
    )
    .unwrap_err();
    assert!(err.to_string().contains("account not found"), "{err}");

    let err =
        resolve_account(&config, "pool", "claude-code", &args_fixture(Some("../x"))).unwrap_err();
    assert!(err.to_string().contains("invalid --account id"), "{err}");
}

#[test]
fn resolve_account_without_flag_picks_the_account_with_more_headroom_from_the_persisted_book() {
    let tmp = tempfile::tempdir().unwrap();
    for id in ["a", "b"] {
        std::fs::create_dir_all(tmp.path().join(id)).unwrap();
        std::fs::write(tmp.path().join(id).join(".credentials.json"), "{}").unwrap();
    }
    let mut book = task_dispatch::AccountBook::load(&tmp.path().join(".celeris-usage.json"));
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let window = |u: f64| task_core::RateLimitObservation {
        five_hour: Some(task_core::RateWindow {
            utilization: u,
            resets_at: now + 90_000,
        }),
        seven_day: None,
        status: None,
        resets_at: None,
        observed_at: now,
    };
    book.record_observation("a", window(0.9), task_dispatch::ObservationSource::Run);
    book.record_observation("b", window(0.1), task_dispatch::ObservationSource::Run);
    book.save().unwrap();

    let config = pool_provider_config(tmp.path(), 2);
    assert_eq!(
        resolve_account(&config, "pool", "claude-code", &args_fixture(None)).unwrap(),
        Some("b".into())
    );
}

#[test]
fn resolve_account_no_eligible_account_errors() {
    let tmp = tempfile::tempdir().unwrap();
    // ディレクトリはあるがログインしていない（.credentials.json が無い）ので選べない。
    std::fs::create_dir_all(tmp.path().join("a")).unwrap();
    let config = pool_provider_config(tmp.path(), 2);
    let err = resolve_account(&config, "pool", "claude-code", &args_fixture(None)).unwrap_err();
    assert!(err.to_string().contains("no eligible account"), "{err}");
}

/// ADR-0025 D2: codex の `account_pool` プロバイダは `[accounts] codex_dir` の下から選び、`auth.json` を
/// ログイン済みの目印にする（claude-code の `.credentials.json` とは別物）。
#[test]
fn resolve_account_codex_pool_provider_uses_codex_dir_and_auth_json_marker() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("c")).unwrap();
    std::fs::write(tmp.path().join("c").join("auth.json"), "{}").unwrap();
    let mut config = cluster_config(vec![]);
    config.providers = vec![celeris::config::ProviderConfig {
        tier_models: Default::default(),
        account_id: None,
        id: "pool".into(),
        adapter: "codex".into(),
        tiers: vec![task_core::Tier::Standard],
        concurrency: 1,
        model: String::new(),
        env: Default::default(),
        env_from_secrets: Default::default(),
        account_pool: true,
        command: None,
        args: None,
        settings: None,
    }];
    config.accounts = Some(celeris::config::AccountsConfig {
        claude_dir: None,
        codex_dir: Some(tmp.path().to_path_buf()),
        max_runs_per_account: 2,
        check_model: "haiku".into(),
    });

    assert_eq!(
        resolve_account(&config, "pool", "codex", &args_fixture(Some("c"))).unwrap(),
        Some("c".into())
    );
    assert_eq!(
        resolve_account(&config, "pool", "codex", &args_fixture(None)).unwrap(),
        Some("c".into())
    );

    let err = resolve_account(&config, "pool", "claude-code", &args_fixture(None)).unwrap_err();
    assert!(
        err.to_string().contains("not a pool adapter")
            || err.to_string().contains("no root configured"),
        "{err}"
    );
}
