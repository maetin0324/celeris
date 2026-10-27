//! `celerisctl eval requests --since <RFC3339> --until <RFC3339> [--out <file>]` — 論理要求の台帳
//! （ADR 2026-10-27-eval-request-ledger）。組み立ては `SqliteStore::request_ledger`。DB は読むだけ。
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_core::SqliteStore;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum EvalCommand {
    /// 判断要求を論理要求 `(source_kind, source_key)` の 1 行にした台帳を JSON で出す。
    Requests(EvalRequestsArgs),
}

#[derive(Args, Debug)]
pub struct EvalRequestsArgs {
    /// 範囲の始め（含む。RFC 3339）。revision の取り込み時刻がこの範囲に入る要求を出す。
    #[arg(long)]
    pub since: String,
    /// 範囲の終わり（含まない。RFC 3339）。
    #[arg(long)]
    pub until: String,
    /// 出力ファイル。省くと標準出力。
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub fn run(store: &SqliteStore, command: EvalCommand) -> Result<ExitCode, CliError> {
    match command {
        EvalCommand::Requests(args) => requests(store, args),
    }
}

fn parse(flag: &str, value: &str) -> Result<OffsetDateTime, CliError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|e| CliError::msg(format!("--{flag} {value}: not RFC 3339 ({e})")))
}

fn requests(store: &SqliteStore, args: EvalRequestsArgs) -> Result<ExitCode, CliError> {
    let since = parse("since", &args.since)?;
    let until = parse("until", &args.until)?;
    if since >= until {
        return Err(CliError::msg("--since must be earlier than --until"));
    }
    let ledger = store.request_ledger(since, until)?;
    let mut text =
        serde_json::to_string_pretty(&ledger).map_err(|e| CliError::msg(format!("ledger: {e}")))?;
    text.push('\n');
    match &args.out {
        Some(path) => {
            fs::write(path, &text)
                .map_err(|e| CliError::msg(format!("write {}: {e}", path.display())))?;
            outln!(
                "exported {} requests ({} items) -> {}",
                ledger.summary.requests,
                ledger.summary.items,
                path.display()
            );
        }
        None => outln!("{}", text.trim_end()),
    }
    Ok(ExitCode::SUCCESS)
}
