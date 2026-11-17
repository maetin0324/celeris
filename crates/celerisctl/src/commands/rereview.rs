//! `celerisctl rereview` — ADR-0051 / ADR-0054 Phase 113 D3。
//!
//! 引数解析・`task_ops::comment::rereview` の呼び出し・出力整形だけをここで行う（`cancel.rs` と同じ
//! 形）。判断と検証は `task_ops::comment::rereview`（`done`/`failed` は `Reviewer` 条件を持つ通常タスクだけ。
//! `failed` は直前の遷移が `review_fail` のときだけ。reviewing で再同期が上限で止まっている task は
//! reviewer 条件・support task を問わず遷移なしで再開する）に移した。新しい実装 run は起こさない（既存成果を
//! 部署のレビュアーで再判定するだけ）。

use std::process::ExitCode;

use clap::Args;

use task_ops::comment::rereview as ops_rereview;

use task_core::SqliteStore;

use crate::error::CliError;
use crate::outln;

#[derive(Args, Debug)]
pub struct RereviewArgs {
    pub id: String,
}

pub fn run(store: &SqliteStore, args: RereviewArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;
    let result = ops_rereview(store, id, None)?;
    outln!("{:?}", result.to);
    Ok(ExitCode::SUCCESS)
}
