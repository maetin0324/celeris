//! `celerisctl execution plan set|show`（ADR-0072 D14。Phase E2）と `execution phase-gate`
//! （ADR-0074 D2.4。Phase F3 途中確認）。
//!
//! `set` は JSON ファイル（または `-` で stdin）から `celeris.execution-plan/1` を読み、D14 の検証を
//! 通してから採用する（`task_ops::execution::adopt_plan`、origin は常に `human`）。`show` は現在の
//! `active` な計画と WorkUnit を出す。判断は `task_ops::execution` にあり、ここは引数解析・I/O・
//! 出力整形だけ（ADR-0013 D7）。
//!
//! ADR-0079 R5b-prep: `set`（別名 `put`）は `--config`（省略時は `CELERIS_CONFIG`）の `[execution.tree]` を daemon と
//! 同じ実効の上限として検証に使い、/3 は planner の計画と同じ経路（unit の gate・決定の要求・木の上限・unit の
//! `adopt`）で採用する（`task_ops::execution::adopt_human_plan`）。設定が無ければ木は無効（/3 は `TreeDisabled`）。
//! `celerisctl tree adopt` は `POST /tasks/{id}/tree/adopt` と同じ操作（ADR-0079 D15）。

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_core::{ExecutionLimits, PlanOrigin, TaskStore};
use time::OffsetDateTime;

use crate::error::{CliError, parse_task_id};
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum ExecutionCommand {
    Plan {
        #[command(subcommand)]
        command: ExecutionPlanCommand,
    },
    /// ADR-0074 D2.4（Phase F3 途中確認）: 工程の後の途中確認（`blocked(awaiting_human)`）に応える
    /// （`POST /tasks/{id}/execution/phase-gate` と同じ操作）。
    PhaseGate(ExecutionPhaseGateArgs),
}

/// `celerisctl execution phase-gate <task> <action>` の `action`。
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum PhaseGateActionArg {
    /// 次の工程へ進める。
    Continue,
    /// replan の planner run を起こす（`--note` 必須）。
    Replan,
    /// 取り下げる（cancel）。
    Withdraw,
}

#[derive(Args, Debug)]
pub struct ExecutionPhaseGateArgs {
    pub task_id: String,
    #[arg(value_enum)]
    pub action: PhaseGateActionArg,
    /// 人の指示（`continue` では任意、`replan` では必須）。
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ExecutionPlanCommand {
    /// 計画を採用する（origin は常に human）。E2 は新規のみ: 既に active な計画があれば失敗する。
    /// ADR-0079 R5b-prep: 別名 `put`（`PUT /tasks/{id}/execution-plan` と同じ）。
    #[command(alias = "put")]
    Set(ExecutionPlanSetArgs),
    /// 現在の active な計画と WorkUnit を表示する。
    Show(ExecutionPlanShowArgs),
}

#[derive(Args, Debug)]
pub struct ExecutionPlanSetArgs {
    pub task_id: String,
    /// `celeris.execution-plan/1`〜`/3` の JSON ファイル。`-` で stdin から読む。
    #[arg(long)]
    pub file: PathBuf,
    /// ADR-0079 R5b-prep: daemon の `config.toml`（`[execution.tree]` を読む）。省略時は `CELERIS_CONFIG`。
    /// どちらも無ければ既定の上限（木は無効。/3 は `TreeDisabled` で拒否）。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: Option<PathBuf>,
}

/// ADR-0079 D15: `celerisctl tree adopt`。
#[derive(Subcommand, Debug)]
pub enum TreeCommand {
    /// 採用済みの /3 の計画の kind task の unit（`adopt: <task>` を持つ）に既存の task を結ぶ
    /// （`POST /tasks/{id}/tree/adopt` と同じ）。
    Adopt(TreeAdoptArgs),
}

#[derive(Args, Debug)]
pub struct TreeAdoptArgs {
    /// 計画を持つ task（木の root）。
    pub root: String,
    /// 採用する既存の task。
    #[arg(long)]
    pub task: String,
    /// unit の段階の key。
    #[arg(long)]
    pub stage: String,
    /// kind task の unit の key。
    #[arg(long)]
    pub unit: String,
    /// daemon の `config.toml`（`[execution.tree] enabled` を読む）。省略時は `CELERIS_CONFIG`。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: Option<PathBuf>,
}

/// ADR-0079 R5b-prep: daemon と同じ実効の上限（`[execution.tree]` だけが設定から来る。celeris の
/// `dispatch_config` と同じ）。設定が無ければ既定（木は無効）。
fn effective_limits(config: Option<&PathBuf>) -> Result<ExecutionLimits, CliError> {
    let Some(path) = config else {
        return Ok(ExecutionLimits::default());
    };
    let cfg = celeris::Config::load(path)
        .map_err(|e| CliError::msg(format!("failed to load config {}: {e}", path.display())))?;
    Ok(ExecutionLimits {
        tree: cfg.execution.tree.limits(),
        ..ExecutionLimits::default()
    })
}

pub fn run_tree(store: &dyn TaskStore, command: TreeCommand) -> Result<ExitCode, CliError> {
    match command {
        TreeCommand::Adopt(args) => run_tree_adopt(store, args),
    }
}

fn run_tree_adopt(store: &dyn TaskStore, args: TreeAdoptArgs) -> Result<ExitCode, CliError> {
    let root = parse_task_id(&args.root)?;
    let task_id = parse_task_id(&args.task)?;
    let limits = effective_limits(args.config.as_ref())?;
    let outcome = task_ops::tree_adopt::adopt(
        store,
        root,
        &task_ops::tree_adopt::AdoptRequest {
            task_id,
            stage: args.stage,
            unit_key: args.unit,
        },
        &limits.tree,
        "human",
        OffsetDateTime::now_utc(),
    )?;
    outln!(
        "adopted {} as unit {} (stage {}) of {}: unit {} — {}",
        outcome.task_id,
        outcome.unit_key,
        outcome.stage,
        root,
        outcome.unit_status.as_str(),
        outcome.detail
    );
    Ok(ExitCode::SUCCESS)
}

#[derive(Args, Debug)]
pub struct ExecutionPlanShowArgs {
    pub task_id: String,
}

pub fn run(store: &dyn TaskStore, command: ExecutionCommand) -> Result<ExitCode, CliError> {
    match command {
        ExecutionCommand::Plan { command } => match command {
            ExecutionPlanCommand::Set(args) => run_set(store, args),
            ExecutionPlanCommand::Show(args) => run_show(store, args),
        },
        ExecutionCommand::PhaseGate(args) => run_phase_gate(store, args),
    }
}

fn run_phase_gate(
    store: &dyn TaskStore,
    args: ExecutionPhaseGateArgs,
) -> Result<ExitCode, CliError> {
    use task_ops::phase_gate::PhaseGateAction;
    let task_id = parse_task_id(&args.task_id)?;
    let action = match args.action {
        PhaseGateActionArg::Continue => PhaseGateAction::Continue,
        PhaseGateActionArg::Replan => PhaseGateAction::Replan,
        PhaseGateActionArg::Withdraw => PhaseGateAction::Withdraw,
    };
    let result = task_ops::phase_gate::phase_gate(store, task_id, action, args.note)?;
    outln!(
        "phase gate {}; task {} moved to {:?} ({})",
        action.as_str(),
        result.id,
        result.to,
        result.reason
    );
    Ok(ExitCode::SUCCESS)
}

fn read_plan_file(path: &PathBuf) -> Result<String, CliError> {
    if path.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::msg(format!("cannot read stdin: {e}")))?;
        Ok(text)
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::msg(format!("cannot read {}: {e}", path.display())))
    }
}

fn run_set(store: &dyn TaskStore, args: ExecutionPlanSetArgs) -> Result<ExitCode, CliError> {
    let task_id = parse_task_id(&args.task_id)?;
    let text = read_plan_file(&args.file)?;
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&text)
        .map_err(|e| CliError::msg(format!("invalid execution plan JSON: {e}")))?;
    let limits = effective_limits(args.config.as_ref())?;
    let adopted = task_ops::execution::adopt_human_plan(
        store,
        task_id,
        spec,
        limits,
        "human",
        OffsetDateTime::now_utc(),
    )?;
    // ADR-0079「R5b-fix3」: gate の記録を人の compound にする（`POST /tasks/{id}/execution-plan` と同じ）。
    if let Err(e) =
        task_ops::regate::record_human_plan_gate(store, task_id, OffsetDateTime::now_utc())
    {
        eprintln!("warning: failed to record the human plan's gate decision: {e}");
    }
    let plan = adopted.plan;
    let units = store.work_units_for(task_id)?.len();
    outln!(
        "{} version={} origin={} status={} work_units={} decisions_raised={}",
        plan.id,
        plan.version,
        PlanOrigin::Human.as_str(),
        plan.status.as_str(),
        units,
        adopted.decisions_raised
    );
    for a in &adopted.adoptions {
        outln!(
            "  adopt {} -> {} adopted={} unit={} ({})",
            a.unit_key,
            a.task_id,
            a.adopted,
            a.unit_status.as_str(),
            a.detail
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_show(store: &dyn TaskStore, args: ExecutionPlanShowArgs) -> Result<ExitCode, CliError> {
    let task_id = parse_task_id(&args.task_id)?;
    let Some(view) = task_ops::execution::active_plan(store, task_id)? else {
        outln!("{task_id}: no execution plan");
        return Ok(ExitCode::SUCCESS);
    };
    outln!(
        "{} version={} origin={} status={}",
        view.plan.id,
        view.plan.version,
        view.plan.origin.as_str(),
        view.plan.status.as_str()
    );
    for wu in &view.work_units {
        let blocked = wu
            .blocked_reason
            .map(|r| format!(" blocked={}", r.as_str()))
            .unwrap_or_default();
        outln!(
            "  {} seq={} key={} kind={} status={}{} runs={} continuations={} retries={} depends_on={:?}",
            wu.id,
            wu.seq,
            wu.key,
            wu.kind.as_str(),
            wu.status.as_str(),
            blocked,
            wu.runs,
            wu.continuations,
            wu.retries,
            wu.depends_on
        );
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use task_core::{Budget, SqliteStore, Status, TaskId, TaskKind, WorkerHint, WorkspaceSpec};

    fn sample_task() -> task_core::Task {
        let now = OffsetDateTime::now_utc();
        task_core::Task {
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
}
