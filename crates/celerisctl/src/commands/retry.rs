//! `celerisctl retry` — Phase 31（実機の事故、2026-09-18）/ ADR-0070 D2（Phase 116）。
//!
//! 引数解析・`task_ops::retry::retry_task` の呼び出し・出力整形だけをここで行う（`cancel.rs` /
//! `rereview.rs` と同じ形）。判断と検証は `task_ops::retry::retry_task`（`failed`/`cancelled` から
//! だけ許す）に移した。`failed`/`cancelled` を複製した**新しいタスク**を作る（`reset_attempts: true`
//! が既定 — 複製先の `attempts` は常に `0`。既存の状態機械は変えない）。
//!
//! ADR-0070 D2 追記（Phase 116。本番で確認: `--accept` を付け忘れると `draft` のまま止まり、
//! 「やり直したのに動かない」状態になった）: **既定で `ready`** から始まる。`draft` のまま
//! 始めたいときだけ `--draft` を付ける（`POST /tasks/{id}/retry` の `accept` の既定と揃える）。

use std::process::ExitCode;

use clap::Args;

use task_ops::retry::retry_task as ops_retry;
use time::OffsetDateTime;

use task_core::SqliteStore;

use crate::error::CliError;
use crate::outln;

#[derive(Args, Debug)]
pub struct RetryArgs {
    pub id: String,
    /// 新しいタスクを `draft` のまま始める（既定は `ready`）。
    #[arg(long)]
    pub draft: bool,
}

pub fn run(store: &SqliteStore, args: RetryArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;
    let result = ops_retry(
        store,
        id,
        !args.draft,
        None,
        super::CLI_ACTOR,
        OffsetDateTime::now_utc(),
    )?;
    outln!("{}", result.task_id);
    if !result.rewired.is_empty() {
        outln!("rewired: {:?}", result.rewired);
    }
    Ok(ExitCode::SUCCESS)
}
