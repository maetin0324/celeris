//! `celerisctl cancel` — DESIGN.md §5.9 / ADR-0010 D4（P-18）。
//!
//! 引数解析・`task_ops::gate::cancel` の呼び出し・出力整形だけをここで行う。判断と検証は
//! `task_ops::gate`（ADR-0013 D7）に移した。CLI にはまだ `--expected` は無いので、常に
//! `expected = None` で呼ぶ（挙動は変えない）。

use std::process::ExitCode;

use clap::Args;

use task_ops::gate::cancel as ops_cancel;

use task_core::SqliteStore;

use crate::error::CliError;
use crate::outln;

#[derive(Args, Debug)]
pub struct CancelArgs {
    pub id: String,
}

pub fn run(store: &SqliteStore, args: CancelArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;
    let result = ops_cancel(store, id, None)?;
    outln!("{:?}", result.to);
    Ok(ExitCode::SUCCESS)
}
