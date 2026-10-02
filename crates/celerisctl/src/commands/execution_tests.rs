use super::*;
use std::io::Write as _;
use task_core::{Budget, SqliteStore, Status, TaskId, TaskKind, WorkerHint, WorkspaceSpec};

fn sample_task() -> task_core::Task {
    let now = OffsetDateTime::now_utc();
    task_core::Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "title".to_string(),
        objective: "objective".to_string(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: task_core::Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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

fn seed_task(store: &dyn TaskStore) -> TaskId {
    let task = sample_task();
    store.insert(&task).unwrap();
    task.id
}

fn plan_json() -> &'static str {
    r#"{
            "schema": "celeris.execution-plan/1",
            "rationale": "A then B",
            "work_units": [
                {"key": "a", "kind": "implement", "title": "A", "objective": "do A thoroughly"},
                {"key": "b", "kind": "implement", "title": "B", "objective": "do B thoroughly", "depends_on": ["a"]}
            ]
        }"#
}

#[test]
fn set_then_show_round_trips() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = seed_task(&store);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    std::fs::write(&path, plan_json()).unwrap();

    let result = run_set(
        &store,
        ExecutionPlanSetArgs {
            task_id: task_id.to_string(),
            file: path,
            config: None,
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);

    let units = store.work_units_for(task_id).unwrap();
    assert_eq!(units.len(), 2);

    let result = run_show(
        &store,
        ExecutionPlanShowArgs {
            task_id: task_id.to_string(),
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
}

#[test]
fn show_without_a_plan_succeeds_and_says_so() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = seed_task(&store);
    let result = run_show(
        &store,
        ExecutionPlanShowArgs {
            task_id: task_id.to_string(),
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
}

#[test]
fn set_rejects_an_invalid_plan() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task_id = seed_task(&store);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(
        f,
        r#"{{"schema":"celeris.execution-plan/1","rationale":"x","work_units":[]}}"#
    )
    .unwrap();

    let err = run_set(
        &store,
        ExecutionPlanSetArgs {
            task_id: task_id.to_string(),
            file: path,
            config: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, CliError::Message(_)), "{err:?}");
}

#[test]
fn set_rejects_an_unknown_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    std::fs::write(&path, plan_json()).unwrap();

    let err = run_set(
        &store,
        ExecutionPlanSetArgs {
            task_id: TaskId::new().to_string(),
            file: path,
            config: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, CliError::Message(_)), "{err:?}");
}

fn v3_plan_json(adopt: TaskId) -> String {
    serde_json::json!({
            "schema": "celeris.execution-plan/3",
            "rationale": "Phase 1 は採用し、Phase 2 は子 task",
            "stages": [
                {"key": "phase-1", "kind": "implement", "title": "Phase 1"},
                {"key": "phase-2", "kind": "implement", "title": "Phase 2"}
            ],
            "units": [
                {"key": "p1", "stage": "phase-1", "kind": "task", "title": "Phase 1", "objective": "already done",
                 "acceptance": [{"text": "reviewer", "check": {"type": "reviewer"}}], "adopt": adopt.to_string()},
                {"key": "p2", "stage": "phase-2", "kind": "task", "title": "Phase 2", "objective": "next phase",
                 "acceptance": [{"text": "reviewer", "check": {"type": "reviewer"}}], "depends_on": ["p1"]}
            ],
            "decisions": [
                {"key": "h1", "question": "どちらにするか", "options": [{"key": "a", "label": "A"}, {"key": "b", "label": "B"}],
                 "recommended": "a", "cost_of_reversal": "low", "needed_before": ["p2"]}
            ]
        })
        .to_string()
}

fn config_file(dir: &std::path::Path, tree_enabled: bool) -> PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
            &path,
            format!(
                "[[providers]]\nid = \"fake-local\"\nadapter = \"fake\"\n\n[execution.tree]\nenabled = {tree_enabled}\n"
            ),
        )
        .unwrap();
    path
}

/// ADR-0079 R5b-prep: `execution plan set`（別名 `put`）は `--config` の `[execution.tree]` で /3 を検証し、
/// 木の経路（unit の `adopt`・計画の決定）で採用する。設定が無い・木が無効なら /3 は `TreeDisabled`。
#[test]
fn set_adopts_a_v3_plan_with_the_configured_tree_limits() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        id: task_core::ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    store.project_create(&project).unwrap();
    let mut done = sample_task();
    done.status = Status::Done;
    done.project_id = Some(project.id);
    store.insert(&done).unwrap();
    let mut root = sample_task();
    root.project_id = Some(project.id);
    store.insert(&root).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    std::fs::write(&path, v3_plan_json(done.id)).unwrap();

    for config in [None, Some(config_file(dir.path(), false))] {
        let err = run_set(
            &store,
            ExecutionPlanSetArgs {
                task_id: root.id.to_string(),
                file: path.clone(),
                config,
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("[execution.tree] enabled = true"),
            "{err}"
        );
    }
    let result = run_set(
        &store,
        ExecutionPlanSetArgs {
            task_id: root.id.to_string(),
            file: path,
            config: Some(config_file(dir.path(), true)),
        },
    )
    .unwrap();
    assert_eq!(result, ExitCode::SUCCESS);
    let units = store.work_units_for(root.id).unwrap();
    let p1 = units.iter().find(|u| u.key == "p1").unwrap();
    assert_eq!(p1.status, task_core::WorkUnitStatus::Done);
    assert_eq!(
        p1.child_task_id.as_deref(),
        Some(done.id.to_string().as_str())
    );
    let decisions = store.decisions_list(Some(root.id)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].request.raised_by.origin,
        task_core::DecisionOrigin::Human
    );
    assert!(store.get(done.id).unwrap().unwrap().tree.is_some());
}

/// ADR-0079 D15: `celerisctl tree adopt` は木が無効なら拒否し、`put` の別名が clap に登録されている。
#[test]
fn tree_adopt_needs_the_tree_and_put_is_an_alias_of_set() {
    use clap::Parser as _;
    let store = SqliteStore::open_in_memory().unwrap();
    let root = seed_task(&store);
    let err = run_tree_adopt(
        &store,
        TreeAdoptArgs {
            root: root.to_string(),
            task: TaskId::new().to_string(),
            stage: "s1".into(),
            unit: "p1".into(),
            config: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("enabled = false"), "{err}");

    #[derive(clap::Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        command: ExecutionCommand,
    }
    let cli = Cli::try_parse_from([
        "x",
        "plan",
        "put",
        &root.to_string(),
        "--file",
        "-",
        "--config",
        "/nonexistent.toml",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        ExecutionCommand::Plan {
            command: ExecutionPlanCommand::Set(_)
        }
    ));
}

/// ADR-0074 D2.4: 途中確認で止まっていない Task（ここでは存在しない Task）には効かない。
#[test]
fn phase_gate_on_a_missing_task_is_an_error() {
    let store = task_core::SqliteStore::open_in_memory().expect("open store");
    let err = run_phase_gate(
        &store,
        ExecutionPhaseGateArgs {
            task_id: task_core::TaskId::new().to_string(),
            action: PhaseGateActionArg::Continue,
            note: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, CliError::Message(_)), "{err:?}");
}
