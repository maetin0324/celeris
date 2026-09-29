mod tests {
    use super::*;
    use task_core::{Check, Event, SqliteStore, Status};

    fn base_args() -> AddArgs {
        AddArgs {
            title: "do something".to_string(),
            objective: "make it work".to_string(),
            // ADR-0067 D2: `--accept`（human）チェックには artifacts か知識ベースの参照が要る。
            accept: vec!["it works".to_string()],
            check_cmd: vec![],
            check_artifact: vec!["result.md".to_string()],
            check_reviewer: vec![],
            kind: KindArg::Execute,
            tier: Some(TierArg::Standard),
            priority: 0,
            parent: None,
            depends_on: vec![],
            max_turns: Some(10),
            max_wall_secs: Some(600),
            max_retries: 2,
            role: None,
            genre: None,
            aggregate: false,
            config: None,
            workspace: Some(PathBuf::from("/tmp/workspace")),
            cluster: None,
        }
    }

    #[test]
    fn run_inserts_task_and_created_event() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let args = base_args();

        let result = run(&store, args).expect("run add");
        assert_eq!(result, ExitCode::SUCCESS);

        let tasks = store.list(None).expect("list tasks");
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];

        assert_eq!(task.title, "do something");
        assert_eq!(task.status, Status::Draft);

        let events = store.events_for(task.id).expect("events_for");
        assert_eq!(events.len(), 1);
        match &events[0].1 {
            Event::Created { task: created, .. } => assert_eq!(created.id, task.id),
            other => panic!("expected Created event, got {other:?}"),
        }
    }

    #[test]
    fn run_with_approval_kind_starts_ready() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.kind = KindArg::Approval;

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, Status::Ready);
    }

    #[test]
    fn run_with_invalid_parent_id_returns_error() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.parent = Some("not-a-valid-id".to_string());

        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
    }

    #[test]
    fn run_acceptance_order_is_accept_then_cmd_then_artifact_then_reviewer() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.accept = vec!["human check".to_string()];
        args.check_cmd = vec!["cargo test".to_string()];
        args.check_artifact = vec!["bench.json".to_string()];
        args.check_reviewer = vec!["looks good".to_string()];

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        let task = &tasks[0];
        assert_eq!(task.acceptance.len(), 4);
        assert_eq!(task.acceptance[0].check, Check::Human);
        assert!(matches!(task.acceptance[1].check, Check::Command { .. }));
        assert!(matches!(
            task.acceptance[2].check,
            Check::ArtifactExists { .. }
        ));
        assert_eq!(task.acceptance[3].check, Check::Reviewer);
    }

    #[test]
    fn run_without_any_acceptance_criterion_errors_and_inserts_nothing() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.accept = vec![];
        args.check_artifact = vec![];

        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
        assert!(store.list(None).expect("list tasks").is_empty());
    }

    /// ADR-0016 D1 / M3: `--role` + `--config` で、省略した tier / 予算が役割の既定になる。
    #[test]
    fn run_with_role_and_config_applies_role_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[[roles]]
id = "lead"
tier = "frontier"
max_turns = 40
max_wall_secs = 1800
instructions = "You lead the work."

[[providers]]
id = "fake-local"
adapter = "fake"
"#,
        )
        .expect("write config");

        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.role = Some("lead".to_string());
        args.config = Some(config_path);
        args.tier = None;
        args.max_turns = None;
        args.max_wall_secs = None;

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        let task = &tasks[0];
        assert_eq!(task.role.as_deref(), Some("lead"));
        assert_eq!(task.worker_hint.tier, Tier::Frontier);
        assert_eq!(
            (task.budget.max_turns, task.budget.max_wall_secs),
            (40, 1800)
        );
        // --max-retries は役割の既定を持たない（既定 2 のまま）。
        assert_eq!(task.budget.max_retries, 2);
        assert!(!task.aggregate);
    }

    /// `--config` 無しの `--role` は役割名だけを保存する（既定は全体の既定。警告は stderr）。
    #[test]
    fn run_with_role_but_no_config_stores_the_name_only() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.role = Some("lead".to_string());
        args.aggregate = true;
        args.tier = None;
        args.max_turns = None;
        args.max_wall_secs = None;

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        let task = &tasks[0];
        assert_eq!(task.role.as_deref(), Some("lead"));
        assert_eq!(task.worker_hint.tier, Tier::Standard);
        assert_eq!(
            (task.budget.max_turns, task.budget.max_wall_secs),
            (10, 600)
        );
        assert!(task.aggregate);
    }

    /// ADR-0027 D1: `--genre` + `--config` で、その分野の `default_role` の役割の既定（tier / 予算）が
    /// role 未指定の子に効く。
    #[test]
    fn run_with_genre_and_config_applies_genre_default_role_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[[roles]]
id = "literature-reader"
tier = "standard"
max_turns = 5
max_wall_secs = 1200

[[genres]]
id = "literature"
description = "related work survey"
default_role = "literature-reader"
roles = ["literature-reader"]

[[providers]]
id = "fake-local"
adapter = "fake"
"#,
        )
        .expect("write config");

        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.genre = Some("literature".to_string());
        args.config = Some(config_path);
        args.tier = None;
        args.max_turns = None;
        args.max_wall_secs = None;

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        let task = &tasks[0];
        assert_eq!(task.role, None, "genre alone must not set role");
        assert_eq!(task.genre.as_deref(), Some("literature"));
        assert_eq!(task.worker_hint.tier, Tier::Standard);
        assert_eq!(
            (task.budget.max_turns, task.budget.max_wall_secs),
            (5, 1200)
        );
    }

    /// `--config` 無しの `--genre` は分野名だけを保存する（検証しない。警告は stderr）。
    #[test]
    fn run_with_genre_but_no_config_stores_the_name_only() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.genre = Some("literature".to_string());

        run(&store, args).expect("run add");

        let tasks = store.list(None).expect("list tasks");
        let task = &tasks[0];
        assert_eq!(task.genre.as_deref(), Some("literature"));
    }

    /// ADR-0027 D1: `--config` があるとき、知らない `--genre` や `--genre` + `--role` の不整合はエラー（何も挿入しない）。
    #[test]
    fn run_with_unknown_genre_or_role_genre_mismatch_errors_when_config_given() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[[roles]]
id = "implementer"

[[genres]]
id = "coding"
description = "d"
roles = ["implementer"]

[[providers]]
id = "fake-local"
adapter = "fake"
"#,
        )
        .expect("write config");

        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.genre = Some("literature".to_string());
        args.config = Some(config_path.clone());
        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
        assert!(store.list(None).expect("list tasks").is_empty());

        let mut args = base_args();
        args.genre = Some("coding".to_string());
        args.role = Some("literature-scout".to_string());
        args.config = Some(config_path);
        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
        assert!(store.list(None).expect("list tasks").is_empty());
    }

    /// 読めない `--config` はエラー（何も挿入しない）。
    #[test]
    fn run_with_unreadable_config_errors_and_inserts_nothing() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.config = Some(PathBuf::from("/nonexistent/config.toml"));

        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
        assert!(store.list(None).expect("list tasks").is_empty());
    }

    #[test]
    fn run_with_missing_dependency_errors_and_inserts_nothing() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let mut args = base_args();
        args.depends_on = vec![TaskId::new().to_string()];

        let result = run(&store, args);
        assert!(result.is_err());
        assert!(store.list(None).expect("list tasks").is_empty());
    }
}
