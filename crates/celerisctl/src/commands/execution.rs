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
    /// ADR-0079 R5b-fix1: active な計画を人が版更新する（origin human の replan。`PUT /tasks/{id}/execution-plan`
    /// に active な計画があるときと同じ）。計画の全体を渡す。done の WU は同じ key・kind・phase・depends_on で残し、
    /// spec のほかの欄（checks など）は上書きできる。
    Replan(ExecutionPlanReplanArgs),
}

#[derive(Args, Debug)]
pub struct ExecutionPlanReplanArgs {
    pub task_id: String,
    /// 新しい版の計画の全体（`celeris.execution-plan/1`〜`/3` の JSON）。`-` で stdin から読む。
    #[arg(long)]
    pub file: PathBuf,
    /// 版の履歴に残す理由。
    #[arg(long, default_value = "replan (human celerisctl)")]
    pub reason: String,
    /// daemon の `config.toml`（`[execution.tree]` を読む）。省略時は `CELERIS_CONFIG`。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: Option<PathBuf>,
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
            ExecutionPlanCommand::Replan(args) => run_replan(store, args),
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

pub(crate) fn read_plan_file(path: &PathBuf) -> Result<String, CliError> {
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

fn run_replan(store: &dyn TaskStore, args: ExecutionPlanReplanArgs) -> Result<ExitCode, CliError> {
    let task_id = parse_task_id(&args.task_id)?;
    let text = read_plan_file(&args.file)?;
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&text)
        .map_err(|e| CliError::msg(format!("invalid execution plan JSON: {e}")))?;
    let limits = effective_limits(args.config.as_ref())?;
    let (plan, diff) = task_ops::execution::replan(
        store,
        task_id,
        spec,
        args.reason,
        PlanOrigin::Human,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )?;
    outln!(
        "{} version={} origin={} status={} added={:?} changed={:?} removed={:?} overridden_done={:?}",
        plan.id,
        plan.version,
        PlanOrigin::Human.as_str(),
        plan.status.as_str(),
        diff.added,
        diff.changed,
        diff.removed,
        diff.overridden_done
    );
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
#[path = "execution_tests.rs"]
mod tests;
