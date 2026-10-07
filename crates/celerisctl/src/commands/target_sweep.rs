//! ADR 2026-10-07-build-tmp-hygiene D1.4: `celerisctl target sweep [--root <path>]... [--dry-run | --apply] [--json]`。
//!
//! 共有 cargo target の古い・上限超過の項目を掃除する（走査・flock・削除は `task_dispatch::target_sweep`）。
//! 既定は `--dry-run`（木を変えない）。**DB は開かない。LLM は呼ばない。**

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_core::model::TargetSweepMode;
use task_dispatch::target_sweep::{self, SweepEnv, SweepReport, SystemEnv};
use task_worker::target_sweep::SweepParams;

use crate::error::CliError;
use crate::outln;

/// `--root` も設定も無いときの 2 つ目の既定 root（`scripts/dev/worktree-target-dir.sh` の既定と同じ）。
const DEFAULT_TMP_ROOT: &str = "/var/tmp/agent-platform-build";

#[derive(Debug, Subcommand)]
pub enum TargetCommand {
    /// 共有 cargo target の古い・上限超過の項目を消す（既定は `--dry-run`）。
    Sweep(SweepArgs),
}

#[derive(Debug, Args)]
pub struct SweepArgs {
    /// 掃除する root（繰り返し可）。省略時は既定の 2 つ（`<build_cache_dir>/cargo` と `/var/tmp/agent-platform-build`）。
    #[arg(long = "root")]
    pub roots: Vec<PathBuf>,
    /// 何も消さず、消す予定だけを出す（既定）。
    #[arg(long, conflicts_with = "apply")]
    pub dry_run: bool,
    /// 実際に消す。
    #[arg(long)]
    pub apply: bool,
    /// `SweepReport` を JSON で stdout へ出す。
    #[arg(long)]
    pub json: bool,
}

impl SweepArgs {
    pub fn mode(&self) -> TargetSweepMode {
        if self.apply {
            TargetSweepMode::Apply
        } else {
            TargetSweepMode::DryRun
        }
    }
}

/// `--config` 未指定で組む既定 roots。`build_cache_dir` は呼び出し側が `~` 展開済みの値を渡す。
pub fn default_roots(build_cache_dir: &Path) -> Vec<PathBuf> {
    vec![
        build_cache_dir.join("cargo"),
        PathBuf::from(DEFAULT_TMP_ROOT),
    ]
}

fn home_default_roots() -> Vec<PathBuf> {
    let base = match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h).join(".local/celeris/build-cache"),
        None => PathBuf::from("/var/tmp/celeris-build-cache"),
    };
    default_roots(&base)
}

pub fn run(command: TargetCommand) -> Result<ExitCode, CliError> {
    match command {
        TargetCommand::Sweep(args) => {
            let roots = if args.roots.is_empty() {
                home_default_roots()
            } else {
                args.roots.clone()
            };
            let report = sweep(&roots, args.mode(), &SystemEnv);
            print_report(&report, args.json)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub fn sweep(roots: &[PathBuf], mode: TargetSweepMode, env: &dyn SweepEnv) -> SweepReport {
    target_sweep::run_sweep(roots, &SweepParams::default(), mode, env)
}

fn print_report(report: &SweepReport, json: bool) -> Result<(), CliError> {
    if json {
        let s = serde_json::to_string_pretty(report)
            .map_err(|e| CliError::msg(format!("json: {e}")))?;
        outln!("{s}");
        return Ok(());
    }
    let verb = match report.mode {
        TargetSweepMode::Apply => "deleted",
        TargetSweepMode::DryRun => "would delete",
    };
    for r in &report.roots {
        outln!(
            "{}: {} {} item(s), {} bytes ({} -> {} bytes)",
            r.root.display(),
            verb,
            r.deleted_items,
            r.deleted_bytes,
            r.before_bytes,
            r.after_bytes
        );
    }
    outln!(
        "{} skipped, {} error(s){}",
        report.skipped.len(),
        report.errors.len(),
        if report.over_cap_unresolved {
            ", over cap unresolved"
        } else {
            ""
        }
    );
    for e in &report.errors {
        eprintln!("warning: {e}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "target_sweep_tests.rs"]
mod tests;
