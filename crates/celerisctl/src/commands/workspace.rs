//! `celerisctl workspace prune`（ADR-0066 D2。Phase 110b）と
//! `celerisctl workspace backfill-artifacts`（ADR-0067 付記 2026-10-07 D3-d）。
//!
//! `prune`: dispatcher の tick が自動で行う「終端になってから `prune_after_secs` 経った作業場所から、ビルド
//! 生成物（`target/` 等）だけを刈る」を、人が本番の `prune_after_secs` を待たずに手で回すための道具。
//! `--dry-run` は消さずに候補を列挙するだけ。
//!
//! `backfill-artifacts`: remote（cluster）の task で run 後の走査が走らなかった頃の取りこぼし
//! （手元の写しの `artifacts/` にあるのに `Event::ArtifactProduced` に無い成果物）を一度だけ補完する。
//! 何度流しても同じ結果（`(path, sha256)` の重複規則）。`--dry-run` は登録せず列挙するだけ。

use std::path::PathBuf;
use std::process::ExitCode;

use celeris::Config;
use clap::{Args, Subcommand};
use task_core::{Event, TaskStore};
use time::OffsetDateTime;

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum WorkspaceCommand {
    /// 終端になってから `prune_after_secs` 経った作業場所から、ビルド生成物だけを刈る（ADR-0066 D2）。
    Prune(PruneArgs),
    /// 既存 task の未申告成果物の取りこぼしを補完する（ADR-0067 付記 2026-10-07 D3-d）。`--task` が無ければ
    /// remote（cluster）の task 全部。
    BackfillArtifacts(BackfillArtifactsArgs),
}

pub fn run(store: &dyn TaskStore, command: WorkspaceCommand) -> Result<ExitCode, CliError> {
    match command {
        WorkspaceCommand::Prune(args) => run_prune(store, args),
        WorkspaceCommand::BackfillArtifacts(args) => run_backfill_artifacts(store, args),
    }
}

#[derive(Args, Debug)]
pub struct BackfillArtifactsArgs {
    /// `config.toml`。省略時は環境変数 `CELERIS_CONFIG`。`workspace_root` を読む。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: PathBuf,

    /// この task だけ（作業場所の種別を問わない）。無ければ remote（cluster）の task 全部。
    #[arg(long)]
    pub task: Option<String>,

    /// 登録せず、見つかる成果物を列挙するだけ。
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run_backfill_artifacts(
    store: &dyn TaskStore,
    args: BackfillArtifactsArgs,
) -> Result<ExitCode, CliError> {
    use task_dispatch::undeclared_artifacts::backfill;

    let config = Config::load(&args.config).map_err(|e| {
        CliError::msg(format!(
            "failed to load config {}: {e}",
            args.config.display()
        ))
    })?;
    let targets = match &args.task {
        Some(id) => {
            let id = crate::error::parse_task_id(id)?;
            let task = store
                .get(id)?
                .ok_or_else(|| CliError::msg(format!("task {id} not found")))?;
            vec![task]
        }
        None => backfill::default_targets(store)?,
    };
    let mut total = 0usize;
    for task in &targets {
        let report = backfill::backfill_task(store, task, &config.workspace_root, args.dry_run)?;
        if report.artifacts.is_empty() {
            continue;
        }
        total += report.artifacts.len();
        for artifact in &report.artifacts {
            outln!(
                "{} {} {} ({} bytes sha256 {})",
                task.id,
                if args.dry_run {
                    "見つけた"
                } else {
                    "登録した"
                },
                artifact.path,
                artifact_size(
                    &task_ops::workspace::local_dir(task, &config.workspace_root),
                    artifact
                ),
                &artifact.sha256[..12.min(artifact.sha256.len())]
            );
        }
    }
    if args.dry_run {
        outln!(
            "{} task を見て {total} 件の成果物が補完できます（--dry-run）。",
            targets.len()
        );
    } else {
        outln!(
            "{} task を見て {total} 件の成果物を補完しました。",
            targets.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn artifact_size(workspace: &std::path::Path, artifact: &task_core::ArtifactRef) -> u64 {
    std::fs::metadata(workspace.join(&artifact.path))
        .map(|m| m.len())
        .unwrap_or(0)
}

#[derive(Args, Debug)]
pub struct PruneArgs {
    /// `config.toml`。省略時は環境変数 `CELERIS_CONFIG`。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: PathBuf,

    /// 消さずに、刈れる作業場所と対象パスを列挙するだけ。
    #[arg(long)]
    pub dry_run: bool,

    /// `[workspace] prune_after_secs` を上書きする（秒）。
    #[arg(long)]
    pub older_than: Option<u64>,
}

pub fn run_prune(store: &dyn TaskStore, args: PruneArgs) -> Result<ExitCode, CliError> {
    let config = Config::load(&args.config).map_err(|e| {
        CliError::msg(format!(
            "failed to load config {}: {e}",
            args.config.display()
        ))
    })?;
    let after_secs = args.older_than.unwrap_or(config.workspace.prune_after_secs);
    let now = OffsetDateTime::now_utc();
    let candidates = task_worker::workspace_prune::find_prune_candidates(
        store,
        &config.workspace_root,
        now,
        after_secs,
    )?;

    if candidates.is_empty() {
        outln!("刈れる作業場所はありません（prune_after_secs = {after_secs}）。");
        return Ok(ExitCode::SUCCESS);
    }

    let mut pruned = 0usize;
    for candidate in &candidates {
        let paths: Vec<String> = candidate
            .paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        if args.dry_run {
            outln!("{} {}", candidate.task_id, paths.join(", "));
            continue;
        }
        let removed = task_worker::workspace_prune::prune(candidate);
        if removed.is_empty() {
            continue;
        }
        let removed = task_worker::workspace_prune::relative_removed(&candidate.task_dir, &removed);
        outln!("{} 刈った: {}", candidate.task_id, removed.join(", "));
        store.append_event(candidate.task_id, &Event::WorkspacePruned { removed })?;
        pruned += 1;
    }

    if args.dry_run {
        outln!("{} 件の作業場所が刈れます（--dry-run）。", candidates.len());
    } else {
        outln!("{pruned} 件の作業場所を刈りました。");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
