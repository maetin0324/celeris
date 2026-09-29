//! `celerisctl plan` — DESIGN.md §5.9 / ADR-0007 D6 / ADR-0010 D4（P-19）。
//!
//! 引数解析・`task_ops::plan::create_plan` の呼び出し・出力整形だけをここで行う。判断は
//! `task_ops::plan`（ADR-0013 D7）に移した。
//!
//! ADR-0079（Phase R5b-prep）: HTTP の `POST /plans` は R5a で 410（U-R6）。このコマンドは DB に直接 `kind = plan` の
//! 分解 task を作る運用の道具として残すが、作るたびに [`ADR_0079_NOTE`] を stderr に出す（新しい仕事は root task と
//! その計画〈plan/3〉で表す）。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use task_core::TaskStore;
use task_ops::plan::{NewPlanSpec, create_plan};
use time::OffsetDateTime;

use crate::commands::add::TierArg;
use crate::error::CliError;
use crate::outln;

/// ADR-0079 R5b-prep: `celerisctl plan` が作るたびに出す注記（stderr。stdout は従来どおり task id だけ）。
pub const ADR_0079_NOTE: &str = "note: ADR-0079 — a kind=plan decomposition task is the legacy path (POST /plans is 410 since R5a). \
For new work create a root task (POST /tasks) and give it a celeris.execution-plan/3 plan (planner, or PUT /tasks/{id}/execution-plan).";

const DEFAULT_MAX_TURNS: u32 = 30;
const DEFAULT_MAX_WALL_SECS: u64 = 900;
const DEFAULT_MAX_RETRIES: u32 = 1;

#[derive(Args, Debug)]
pub struct PlanArgs {
    /// 大目標。1行目の先頭80文字が `title` になる。
    pub goal: String,

    /// ワークスペースのローカルパス。省略時はカレントディレクトリ。
    #[arg(long)]
    pub workspace: Option<PathBuf>,

    #[arg(long, value_enum, default_value = "frontier")]
    pub tier: TierArg,

    #[arg(long, default_value_t = 0)]
    pub priority: i32,

    #[arg(long, default_value_t = DEFAULT_MAX_TURNS)]
    pub max_turns: u32,

    #[arg(long, default_value_t = DEFAULT_MAX_WALL_SECS)]
    pub max_wall_secs: u64,

    #[arg(long, default_value_t = DEFAULT_MAX_RETRIES)]
    pub max_retries: u32,
}

pub fn run(store: &dyn TaskStore, args: PlanArgs) -> Result<ExitCode, CliError> {
    let spec = NewPlanSpec {
        goal: args.goal,
        workspace: args.workspace,
        tier: args.tier.into(),
        priority: args.priority,
        max_turns: args.max_turns,
        max_wall_secs: args.max_wall_secs,
        max_retries: args.max_retries,
    };

    let task = create_plan(store, spec, OffsetDateTime::now_utc())?;
    eprintln!("{ADR_0079_NOTE}");

    outln!("{}", task.id);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{SqliteStore, Status};

    fn base_args(goal: &str) -> PlanArgs {
        PlanArgs {
            goal: goal.to_string(),
            workspace: Some(PathBuf::from("/tmp/workspace")),
            tier: TierArg::Frontier,
            priority: 0,
            max_turns: DEFAULT_MAX_TURNS,
            max_wall_secs: DEFAULT_MAX_WALL_SECS,
            max_retries: DEFAULT_MAX_RETRIES,
        }
    }

    #[test]
    fn run_inserts_plan_task() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let args = base_args("add CLI argument parsing to hello-crate");

        let result = run(&store, args).expect("run plan");
        assert_eq!(result, ExitCode::SUCCESS);

        let tasks = store.list(None).expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, Status::Draft);
    }

    #[test]
    fn run_with_blank_goal_returns_error() {
        let store = SqliteStore::open_in_memory().expect("open store");
        let args = base_args("   \n  \t ");

        let result = run(&store, args);
        assert!(matches!(result, Err(CliError::Message(_))));
    }
}
