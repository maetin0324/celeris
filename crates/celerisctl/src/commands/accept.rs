//! `celerisctl accept` — ADR-0070 D2 追記（Phase 116）。
//!
//! 引数解析・`task_ops::gate::accept` の呼び出し・出力整形だけをここで行う（`cancel.rs` と同じ形）。
//! `draft` だけを `ready` にする（`approve` は `Approval` タスクの承認とも兼用でわかりにくいので、
//! こちらは名前で意図を明確にする専用の道具）。

use std::process::ExitCode;

use clap::Args;

use task_ops::gate::accept as ops_accept;

use task_core::SqliteStore;

use crate::error::CliError;
use crate::outln;

#[derive(Args, Debug)]
pub struct AcceptArgs {
    pub id: String,
}

pub fn run(store: &SqliteStore, args: AcceptArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;
    let result = ops_accept(store, id, None)?;
    outln!("{:?}", result.to);
    Ok(ExitCode::SUCCESS)
}
