//! ADR 2026-10-04-release-notes: `celerisctl release {notes,preview}`。
//!
//! - `notes`: `base..sha` の説明（`notes.json` と `notes.md`）を作る。release.sh が梱包の途中で呼ぶ。
//! - `preview`: `current` から対象リリースへ昇格したら入るものの要約。promote.sh が昇格の前に呼ぶ。
//!
//! どちらも **DB を開かない**（git とリリースのディレクトリだけ。`--api` を渡した `notes` だけ localhost の
//! daemon に task の題を訊く）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use celeris::release_notes::{
    self, HttpLookup, NoLookup, NotesInput, TaskLookup, preview_from_dir, render_markdown,
    render_preview_markdown,
};
use clap::{Args, Subcommand};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::CliError;
use crate::outln;

#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    /// `<out-dir>/notes.json` と `notes.md` を書く（git の `base..sha` から。決定的）。
    Notes(NotesArgs),
    /// 昇格の要約（`current` から対象まで）を出す。
    Preview(PreviewArgs),
}

#[derive(Debug, Args)]
pub struct NotesArgs {
    /// 梱包元の git リポジトリ。
    #[arg(long)]
    pub repo: PathBuf,
    /// 対象の commit（完全な sha でも ref でも）。
    #[arg(long)]
    pub sha: String,
    /// 起点（ビルド時の `current` の sha）。省略・未知なら説明は空。
    #[arg(long)]
    pub base: Option<String>,
    #[arg(long)]
    pub schema_from: Option<u32>,
    #[arg(long)]
    pub schema_to: Option<u32>,
    /// このリリースの `gate.json`（飛ばした段を拾う）。
    #[arg(long)]
    pub gate_json: Option<PathBuf>,
    /// daemon の API（task の題と配送記録を引く）。省略なら branch 名だけで判別する。
    #[arg(long)]
    pub api: Option<String>,
    /// API の token ファイル（無ければ token なし）。
    #[arg(long)]
    pub token_file: Option<PathBuf>,
    #[arg(long)]
    pub out_dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// 対象リリースの sha12。
    #[arg(value_name = "SHA12")]
    pub sha12: String,
    /// `releases` ディレクトリ（`current` はその 1 つ上）。
    #[arg(long)]
    pub releases_dir: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub fn run(command: ReleaseCommand) -> Result<ExitCode, CliError> {
    match command {
        ReleaseCommand::Notes(args) => {
            let line = run_notes(&args)?;
            outln!("{line}");
            Ok(ExitCode::SUCCESS)
        }
        ReleaseCommand::Preview(args) => match render_preview(&args)? {
            Some(text) => {
                outln!("{}", text.trim_end());
                Ok(ExitCode::SUCCESS)
            }
            None => {
                eprintln!(
                    "error: release {} not found in {}",
                    args.sha12,
                    args.releases_dir.display()
                );
                Ok(ExitCode::FAILURE)
            }
        },
    }
}

/// `preview` の出力文字列。対象のリリースが無ければ `None`。
pub fn render_preview(args: &PreviewArgs) -> Result<Option<String>, CliError> {
    let Some(p) = preview_from_dir(&args.releases_dir, &args.sha12) else {
        return Ok(None);
    };
    if args.json {
        let text = serde_json::to_string_pretty(&p).map_err(|e| CliError::msg(e.to_string()))?;
        Ok(Some(text))
    } else {
        Ok(Some(render_preview_markdown(&p)))
    }
}

/// 同じディレクトリに tmp を書いて rename する。
fn write_atomic(path: &Path, text: &str) -> Result<(), CliError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CliError::msg(format!("bad path {}", path.display())))?;
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    fs::write(&tmp, text).map_err(|e| CliError::msg(format!("write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|e| CliError::msg(format!("rename to {}: {e}", path.display())))
}

/// notes を作って書き、標準出力に出す 1 行を返す。
pub fn run_notes(args: &NotesArgs) -> Result<String, CliError> {
    let gate = match &args.gate_json {
        Some(p) => {
            let text = fs::read_to_string(p)
                .map_err(|e| CliError::msg(format!("read {}: {e}", p.display())))?;
            Some(
                serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|e| CliError::msg(format!("parse {}: {e}", p.display())))?,
            )
        }
        None => None,
    };
    let generated_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| CliError::msg(e.to_string()))?;

    let http = match &args.api {
        Some(api) => {
            let token = args
                .token_file
                .as_ref()
                .and_then(|p| fs::read_to_string(p).ok())
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            Some(HttpLookup::new(api, token).map_err(CliError::msg)?)
        }
        None => None,
    };
    let deliveries = http.as_ref().and_then(HttpLookup::deliveries);
    let lookup: &dyn TaskLookup = match &http {
        Some(h) => h,
        None => &NoLookup,
    };
    let input = NotesInput {
        repo: &args.repo,
        base: args.base.as_deref(),
        sha: &args.sha,
        schema_from: args.schema_from,
        schema_to: args.schema_to,
        gate: gate.as_ref(),
        deliveries: deliveries.as_deref(),
        generated_at,
    };
    let notes = release_notes::generate(&input, lookup).map_err(CliError::msg)?;

    fs::create_dir_all(&args.out_dir)
        .map_err(|e| CliError::msg(format!("mkdir {}: {e}", args.out_dir.display())))?;
    let json = serde_json::to_string_pretty(&notes).map_err(|e| CliError::msg(e.to_string()))?;
    write_atomic(&args.out_dir.join("notes.json"), &format!("{json}\n"))?;
    write_atomic(&args.out_dir.join("notes.md"), &render_markdown(&notes))?;
    Ok(format!(
        "release notes {}: tasks={} direct={} migrations={}",
        notes.sha12,
        notes.tasks.len(),
        notes.direct_commits.len(),
        notes.migrations.len()
    ))
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
