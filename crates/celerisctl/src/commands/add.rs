//! `celerisctl add` — DESIGN.md §5.9 / ADR-0004 D4 / ADR-0010 D4（P-17, P-19）。
//!
//! CLI 引数の解析（フラグ → `CriterionSpec` の固定順: accept → cmd → artifact → reviewer）と
//! 出力整形だけをここで行う。判断と検証（受け入れ条件の必須化、`depends_on` の検証、`Task` の
//! 組み立て）は `task-ops::add`（ADR-0013 D7）に移した。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use task_core::{GenreSpec, RoleSpec, TaskId, TaskKind, TaskStore, Tier};
use task_ops::add::{CriterionSpec, NewTaskSpec, create_task_with_roles};
use time::OffsetDateTime;

use crate::error::{CliError, parse_task_id};
use crate::outln;

#[derive(Args, Debug)]
pub struct AddArgs {
    #[arg(long)]
    pub title: String,

    #[arg(long)]
    pub objective: String,

    /// 人間が確認する受け入れ条件（`Check::Human`）。複数回指定できる。
    #[arg(long = "accept")]
    pub accept: Vec<String>,

    /// コマンドが exit 0 で終わることを条件にする（`Check::Command`）。複数回指定できる。
    #[arg(long = "check-cmd")]
    pub check_cmd: Vec<String>,

    /// 成果物が存在することを条件にする（`Check::ArtifactExists`）。複数回指定できる。
    #[arg(long = "check-artifact")]
    pub check_artifact: Vec<String>,

    /// レビュアー（別 LLM 実行）による判定を条件にする（`Check::Reviewer`）。複数回指定できる。
    #[arg(long = "check-reviewer")]
    pub check_reviewer: Vec<String>,

    #[arg(long, value_enum, default_value = "execute")]
    pub kind: KindArg,

    /// 省略時は役割（--role）の既定 → standard（ADR-0016 D1）。
    #[arg(long, value_enum)]
    pub tier: Option<TierArg>,

    #[arg(long, default_value_t = 0)]
    pub priority: i32,

    /// 親タスクの ID（省略可）。
    #[arg(long)]
    pub parent: Option<String>,

    /// 依存する先行タスクの ID。複数回指定できる。存在しない、または failed/cancelled ならエラー。
    #[arg(long = "depends-on")]
    pub depends_on: Vec<String>,

    /// 省略時は役割（--role）の既定 → 10（ADR-0016 D1）。
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// 省略時は役割（--role）の既定 → 600（ADR-0016 D1）。
    #[arg(long)]
    pub max_wall_secs: Option<u64>,

    #[arg(long, default_value_t = 2)]
    pub max_retries: u32,

    /// ADR-0016 D1: 役割名（自由記述）。`--config` があれば `[[roles]]` の既定（tier / adapter / 予算）を
    /// 埋め、run 時に指示文が前置きされる。`--config` が無いときは名前だけ保存する。
    #[arg(long)]
    pub role: Option<String>,

    /// ADR-0027 D1: 分野名（自由記述）。`--config` があれば `[[genres]]` の既定（`default_role` の役割の
    /// tier / adapter / 予算）を埋め、知らない分野や `--role` との不整合（`role` がその分野の `roles` に
    /// 無い）は起動時にエラーにする。`--config` が無いときは名前だけ保存する（検証しない）。
    #[arg(long)]
    pub genre: Option<String>,

    /// ADR-0016 D3: 委譲した子が全て終端になった後に集約 run を 1 回行い、`artifacts/summary.md` を書かせる。
    #[arg(long, default_value_t = false)]
    pub aggregate: bool,

    /// `[[roles]]` を読む `config.toml`。`--role` の既定をここから解決する。
    /// P-54: 省略時は環境変数 `CELERIS_CONFIG` を見る。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: Option<PathBuf>,

    /// ワークスペースのローカルパス。省略時は `<task_id>`（相対パス、P-19）。
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    /// ADR-0018: クラスタ（`[[clusters]] id`）でコマンドを実行する。`--workspace` にはクラスタ側の
    /// 作業ディレクトリ（既存プロジェクトでよい）を絶対パスで指定する。
    #[arg(long)]
    pub cluster: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindArg {
    Plan,
    Execute,
    Review,
    Approval,
}

impl From<KindArg> for TaskKind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::Plan => TaskKind::Plan,
            KindArg::Execute => TaskKind::Execute,
            KindArg::Review => TaskKind::Review,
            KindArg::Approval => TaskKind::Approval,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierArg {
    Frontier,
    Standard,
    Cheap,
}

impl From<TierArg> for Tier {
    fn from(value: TierArg) -> Self {
        match value {
            TierArg::Frontier => Tier::Frontier,
            TierArg::Standard => Tier::Standard,
            TierArg::Cheap => Tier::Cheap,
        }
    }
}

/// `--accept`/`--check-cmd`/`--check-artifact`/`--check-reviewer` から `CriterionSpec` を
/// 固定順（accept → cmd → artifact → reviewer）で組み立てる。並び順・必須チェック自体は
/// `task_ops::add::create_task` が行う。
fn build_criteria(args: &mut AddArgs) -> Vec<CriterionSpec> {
    let mut acceptance = Vec::new();
    acceptance.extend(
        std::mem::take(&mut args.accept)
            .into_iter()
            .map(|text| CriterionSpec::Human { text }),
    );
    acceptance.extend(std::mem::take(&mut args.check_cmd).into_iter().map(|cmd| {
        CriterionSpec::Command {
            cmd,
            expect_exit: 0,
        }
    }));
    acceptance.extend(
        std::mem::take(&mut args.check_artifact)
            .into_iter()
            .map(|name| CriterionSpec::ArtifactExists { name }),
    );
    acceptance.extend(
        std::mem::take(&mut args.check_reviewer)
            .into_iter()
            .map(|text| CriterionSpec::Reviewer { text }),
    );
    acceptance
}

/// `--config` があれば `[[roles]]` と `[[genres]]` を読む。無ければ両方とも空（役割名／分野名だけ保存する。
/// `--role`/`--genre` が指定されているのに `--config` が無ければ警告する）。
fn load_roles_and_genres(
    config: Option<&PathBuf>,
    role: Option<&str>,
    genre: Option<&str>,
) -> Result<(Vec<RoleSpec>, Vec<GenreSpec>), CliError> {
    match config {
        Some(path) => {
            let config = celeris::Config::load(path).map_err(|e| {
                CliError::msg(format!("failed to load config {}: {e}", path.display()))
            })?;
            Ok((config.role_specs(), config.genre_specs()))
        }
        None => {
            if role.is_some() {
                eprintln!(
                    "warning: --role given without --config; role defaults and instructions are not applied at creation"
                );
            }
            if genre.is_some() {
                eprintln!(
                    "warning: --genre given without --config; genre defaults and validation are not applied at creation"
                );
            }
            Ok((Vec::new(), Vec::new()))
        }
    }
}

pub fn run(store: &dyn TaskStore, args: AddArgs) -> Result<ExitCode, CliError> {
    let (roles, genres) = load_roles_and_genres(
        args.config.as_ref(),
        args.role.as_deref(),
        args.genre.as_deref(),
    )?;
    let spec = build_spec(args)?;
    let task = create_task_with_roles(store, spec, &roles, &genres, OffsetDateTime::now_utc())?;

    outln!("{}", task.id);
    Ok(ExitCode::SUCCESS)
}

/// ADR-0098 D6: worker の run の中で daemon の DB（`CELERIS_RUN_DB`）に向けた `add` は **DB を開かず**、spec を
/// その run の `followups.json` に追記する。run の終わりに daemon が run の task の案件・リポジトリで `draft` として作る。
/// `--parent` / `--workspace` / `--cluster` は daemon が使わない欄なのでここで断る（D3-4）。`--role` / `--genre` は
/// 名前のまま渡し、daemon の `[[roles]]` / `[[genres]]` で解決する（`--config` は読まない）。
pub fn queue(args: AddArgs, followups_file: &Path) -> Result<ExitCode, CliError> {
    let refused: Vec<&str> = [
        ("--parent", args.parent.is_some()),
        ("--workspace", args.workspace.is_some()),
        ("--cluster", args.cluster.is_some()),
    ]
    .into_iter()
    .filter_map(|(flag, given)| given.then_some(flag))
    .collect();
    if !refused.is_empty() {
        return Err(CliError::msg(format!(
            "{} cannot be used inside a worker run: the follow-up is an independent task in this run's \
             project and its workspace follows the project's repositories (ADR-0098 D3). For a child task \
             write delegate.json instead",
            refused.join(", ")
        )));
    }
    let spec = build_spec(args)?;
    let title = spec.title.clone();
    let n = task_ops::followup::append_to_file(followups_file, &spec)?;
    outln!(
        "queued follow-up #{n} {title:?} in {} — celeris creates it as a draft in this run's project \
         (with its repositories) when the run ends (ADR-0098)",
        followups_file.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn build_spec(mut args: AddArgs) -> Result<NewTaskSpec, CliError> {
    let acceptance = build_criteria(&mut args);

    let parent = match &args.parent {
        Some(s) => Some(parse_task_id(s)?),
        None => None,
    };

    let depends_on: Vec<TaskId> = args
        .depends_on
        .iter()
        .map(|s| parse_task_id(s))
        .collect::<Result<_, _>>()?;

    let spec = NewTaskSpec {
        repos: Vec::new(),
        title: args.title,
        objective: args.objective,
        acceptance,
        kind: args.kind.into(),
        tier: args.tier.map(Into::into),
        priority: Some(task_ops::add::PriorityInput::Number(args.priority)),
        parent,
        depends_on,
        max_turns: args.max_turns,
        max_wall_secs: args.max_wall_secs,
        max_retries: args.max_retries,
        role: args.role,
        genre: args.genre,
        aggregate: args.aggregate,
        // ADR-0033 D2: 案件・途中目標・担当は GUI（API）から付ける。`celerisctl add` は引数を増やさない。
        project_id: None,
        milestone_id: None,
        assignee: None,
        workspace: args.workspace,
        cluster: args.cluster,
        // ADR-0059 D1: `celerisctl add` は引数を増やさない（`workspace_mode` は GUI / API から）。
        workspace_mode: None,
        adapter: None,
        // ADR-0044 D1/D3: `celerisctl add` は引数を増やさない（ラベル・種類・初期状態は GUI から）。
        labels: Vec::new(),
        category: None,
        // ADR-0046 D2/D4: skills と mode も GUI から（`celerisctl add` は引数を増やさない）。
        skills: Vec::new(),
        mode: None,
        status: None,
        features: None,
        execution: None,
        // ADR-0074 D2.1（Phase F3 途中確認）: GUI・API から（`celerisctl add` は引数を増やさない）。
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: task_ops::add::SpecProvenance::default(),
    };
    Ok(spec)
}

#[cfg(test)]
#[path = "add_tests.rs"]
mod tests;
