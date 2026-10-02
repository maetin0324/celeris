//! `celerisctl skills import <dir>`（ADR-0122 D1）— repo に写した skill を KB へ取り込む管理系。
//!
//! **DB を開かない**（`celerisctl knowledge` と同じく KB を直接読み書きする）。書き込みは
//! `task_ops::knowledge::skills_put`（`PUT /api/v1/skills/{name}` と同じ関数）だけで、mount はしない
//! （mount は `POST /api/v1/org/{id}/skills`）。daemon は dispatch のたびに KB を読むので再起動は要らない。
//!
//! ```text
//! celerisctl skills import <dir> [--name <name>]... [--root …] [--config …]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_ops::knowledge as ops;

use crate::commands::knowledge::{RootArgs, root_of};
use crate::error::CliError;
use crate::outln;

/// frontmatter に無ければ足す出典（ADR-0056 D3 の `source:`）。
const IMPORT_SOURCE: &str = "celerisctl";

#[derive(Subcommand, Debug)]
pub enum SkillsCommand {
    /// `<dir>` の skill（`SKILL.md` と UTF-8 の付属ファイル）を KB の `skills/<name>/` に取り込む。
    /// `<dir>` が `SKILL.md` を持てばその 1 件、持たなければ直下の各ディレクトリ。冪等。mount はしない。
    Import(ImportArgs),
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// skill 1 件のディレクトリか、skill のディレクトリを並べた親（`config/skills`）。
    pub dir: PathBuf,
    /// 取り込む skill の名前（ディレクトリ名）。繰り返すとその名前だけに絞る。
    #[arg(long = "name")]
    pub names: Vec<String>,
    #[command(flatten)]
    pub root: RootArgs,
}

pub fn run(command: SkillsCommand) -> Result<ExitCode, CliError> {
    match command {
        SkillsCommand::Import(args) => run_import(&args),
    }
}

fn run_import(args: &ImportArgs) -> Result<ExitCode, CliError> {
    let root = root_of(&args.root);
    let reports = ops::skills_import_dir(&root, &args.dir, &args.names, Some(IMPORT_SOURCE))
        .map_err(|e| CliError::msg(e.to_string()))?;
    if reports.is_empty() {
        return Err(CliError::msg(format!(
            "{} に SKILL.md を持つ skill がありません",
            args.dir.display()
        )));
    }
    for report in &reports {
        outln!(
            "imported {} ({} attached file(s))",
            report.path,
            report.files
        );
        for skipped in &report.skipped {
            outln!(
                "  skipped ({}): {}/{}",
                skipped.reason.as_str(),
                report.name,
                skipped.path
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
