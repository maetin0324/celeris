//! `celerisctl` — DESIGN.md §5.9 の CLI。ADR-0004 参照。

mod commands;
mod error;
mod output;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use task_core::{ClientAccess, SCHEMA_VERSION, SqliteStore};

use commands::accept::{self, AcceptArgs};
use commands::add::{self, AddArgs};
use commands::browser::{self as browser_cmd, BrowserCommand};
use commands::build_cache::{self, BuildCacheCommand};
use commands::cancel::{self, CancelArgs};
use commands::config::{self as config_cmd, ConfigCommand};
use commands::cos_ops::{self, ApiRequestArgs};
use commands::cron::{self as cron_cmd, CronCommand};
use commands::curation::{self as curation_cmd, CurationCommand};
use commands::db::{self as db_cmd, DbCommand};
use commands::execution::{
    self as execution_cmd, ExecutionCommand, ExecutionPlanCommand, PhaseGateActionArg, TreeCommand,
};
use commands::gate::{self, AnswerArgs, ApproveArgs, RejectArgs};
use commands::knowledge::{self, KnowledgeCommand};
use commands::mcp::{self, McpCommand};
use commands::models::{self as models_cmd, ModelsCommand};
use commands::org::{self as org_cmd, OrgCommand};
use commands::plan::{self, PlanArgs};
use commands::plan_lint;
use commands::projects::{self, ProjectsCommand};
use commands::query::{self, LogArgs, LsArgs, ShowArgs};
use commands::release::{self as release_cmd, ReleaseCommand};
use commands::replay::{self, ReplayArgs};
use commands::rereview::{self, RereviewArgs};
use commands::retry::{self, RetryArgs};
use commands::routing::{self as routing_cmd, RoutingCommand};
use commands::scratch::{self as scratch_cmd, ScratchCommand};
use commands::skills::{self as skills_cmd, SkillsCommand};
use commands::target_sweep::{self as target_cmd, TargetCommand};
use commands::worker::{self, WorkerCommand};
use commands::workspace::{self, WorkspaceCommand};
use error::CliError;

#[derive(Parser, Debug)]
#[command(
    name = "celerisctl",
    about = "celeris task control CLI (DESIGN.md §5.9)"
)]
struct Cli {
    /// SQLite データベースファイルのパス（ADR-0004 D5）。
    /// 優先順位: --db > 環境変数 CELERIS_DB > CELERIS_RUN_DB（worker の run の中で daemon が渡す。ADR-0098 D6）
    /// > ./celeris.sqlite3
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Reason recorded in the CoS operation audit event.
    #[arg(long, global = true)]
    reason: Option<String>,
    /// Reuse this key when retrying the same CoS operation.
    #[arg(long, global = true)]
    idempotency_key: Option<String>,
    /// Optimistic revision expected by the CoS operation.
    #[arg(long, global = true)]
    expected_revision: Option<String>,
    /// Override the configured API base URL (including /api/v1).
    #[arg(long, global = true)]
    api_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Send a registered domain mutation through the audited CoS operation API.
    ApiRequest(ApiRequestArgs),
    /// ADR 2026-10-04-release-notes: `notes`（リリースの説明を書く）/ `preview`（昇格の要約）。DB は開かない。
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    /// ADR-0075（Phase G1）: scratch pool（`status [--json]` / `gc [--dry-run]` / 外部 lease の `lease` / `touch` /
    /// `release` / `env`）。`lease` 系は DB を開かない。`status` / `gc` は DB があれば読む（daemon と同じ分類）。
    Scratch {
        #[command(subcommand)]
        command: ScratchCommand,
    },
    /// ADR 2026-10-07-build-tmp-hygiene D1.4: 共有 cargo target の掃除（`sweep [--root]... [--dry-run | --apply] [--json]`）。DB は開かない。
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// 共有 Cargo ビルドキャッシュの古い repo-key を列挙・削除する（DB は開かない）。
    /// ADR-0075: scratch へ移行済み。通常は `celerisctl scratch gc` を使う。
    BuildCache {
        #[command(subcommand)]
        command: BuildCacheCommand,
    },
    /// ADR-0080 D6: browser の本人（owner）session を GUI の control socket で確定する（DB は開かない）。
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// Read-only docs audit and human-approved reconciliation.
    DocsMaintenance {
        #[command(subcommand)]
        command: commands::docs_maintenance::DocsMaintenanceCommand,
    },
    Add(AddArgs),
    Plan(PlanArgs),
    /// ADR-0067 D5（Phase 111）: draft/ready の受け入れ条件を「human チェックには artifacts か知識ベースの
    /// 参照が要る」規則（D2）で点検する。読み取り専用（直しはしない）。
    PlanLint,
    Ls(LsArgs),
    Show(ShowArgs),
    Approve(ApproveArgs),
    /// ADR-0070 D2 追記（Phase 116）: `draft` を `ready` にする専用の道具（`approve` は
    /// `Approval` タスクの承認とも兼用でわかりにくいので、こちらは名前で意図を明確にする）。
    Accept(AcceptArgs),
    Reject(RejectArgs),
    Cancel(CancelArgs),
    /// ADR-0051 / ADR-0054 Phase 113 D3: 既存成果を再判定する（新しい実装runは起こさない）。
    /// `done`、または直前の遷移が `review_fail` だった `failed` からだけ。
    Rereview(RereviewArgs),
    /// ADR-0070 D2（Phase 116）: `failed`/`cancelled` を複製してやり直す（attempts は常に 0 から）。
    Retry(RetryArgs),
    Answer(AnswerArgs),
    Log(LogArgs),
    Replay(ReplayArgs),
    /// ADR-0047 D3（Phase 61）: 知識ベース（`init` / `search` / `get` / `record` / `reindex`）。
    /// **DB を開かない**ので、コンテナの中でも KB さえマウントされていれば動く。
    Knowledge {
        #[command(subcommand)]
        command: KnowledgeCommand,
    },
    /// ADR-0122 D1: repo に写した skill を KB へ取り込む（`skills import <dir>`）。**DB を開かない**。
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    /// ADR-0056 D1（Phase 78）: MCP クライアントの発行・一覧・失効（`client`）、stdio 橋（`stdio`）。
    /// `stdio` 以外は DB を直接開く。
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// `celerisctl worker run` 等（デバッグ用。ADR-0012 D4）。
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// ADR-0046 D3: 設定の変換（`config to-harnesses`）。DB には触らない。
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// ADR-0131 D5: cron jobs through the daemon API.
    Cron {
        #[command(subcommand)]
        command: CronCommand,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// ADR 2026-10-06 D5: model catalog through the daemon API (`models list|discover`).
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// ADR-0131 付記 D12: 日次知識整理の `curation-plan.json` を daemon と同じ規則で点検する
    /// （`curation validate`）。DB を開かず、ネットワークも使わない。
    Curation {
        #[command(subcommand)]
        command: CurationCommand,
    },
    /// ADR-0069 Phase 118 D3: `routing show`。tier → 実行モデル/effort の表。DB には触らない。
    /// Phase 4: `routing export`（明示の `--db` を読み取り専用で開く）/ `routing evaluate`（DB を開かない）。
    Routing {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// ADR-0046 D7: 組織の移行（`org migrate-v2`）。
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
    /// ADR-0054 D2（Phase 68）: 案件の一覧・詳細（`ls`/`show` のタスク版）。読み取り専用。
    /// ADR-0074 D3.3（Phase F4a (c)）: `projects plan approve|reject` だけは書き込み（`project` でも可）。
    #[command(visible_alias = "project")]
    Projects {
        #[command(subcommand)]
        command: ProjectsCommand,
    },
    /// ADR-0066 D2（Phase 110b）: `workspace prune`。
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// ADR-0064 D2/D3（Phase 110a）: `backup <dest>` / `integrity-check <path>`。
    /// `scripts/selfdeploy/relocate-db.sh` が `sqlite3` コマンドの無い環境で使う。**DB を通常の
    /// 経路（`--db`）では開かない**（`backup` は `--src` で任意の元ファイルを指定できる）。
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// ADR-0072 D14（Phase E2）: `execution plan set|show`。
    Execution {
        #[command(subcommand)]
        command: ExecutionCommand,
    },
    /// ADR-0079 D15（Phase R5b-prep）: `tree adopt`（既存の task を木の子として採用する）。
    Tree {
        #[command(subcommand)]
        command: TreeCommand,
    },
}

/// ADR-0095 D6: celerisctl は **migration をしない**（`SqliteStore::open_client`）。DB が無い・古い DB は
/// 開かずに失敗し（daemon が作り・migrate する）、新しい DB は読み取り専用で開いて警告する。
fn open_store(db_path: &Path) -> Result<SqliteStore, ExitCode> {
    match SqliteStore::open_client(db_path) {
        Ok((store, ClientAccess::ReadWrite)) => Ok(store),
        Ok((store, ClientAccess::ReadOnlyNewerSchema { found })) => {
            eprintln!(
                "warning: db {} has schema version {found}, newer than the {SCHEMA_VERSION} this \
                 celerisctl supports; opened read-only (reads work, writes are refused; ADR-0095)",
                db_path.display()
            );
            Ok(store)
        }
        Err(e) => {
            eprintln!("error: failed to open db {}: {e}", db_path.display());
            Err(ExitCode::FAILURE)
        }
    }
}

fn resolve_db_path(cli_db: Option<PathBuf>) -> PathBuf {
    cli_db
        .or_else(|| env::var_os("CELERIS_DB").map(PathBuf::from))
        .or_else(run_db)
        .unwrap_or_else(|| PathBuf::from("celeris.sqlite3"))
}

/// ADR-0098 D6: worker の run の中で daemon が渡す、その daemon の DB（`CELERIS_RUN_DB`）。
fn run_db() -> Option<PathBuf> {
    env::var_os(task_ops::followup::ENV_RUN_DB)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// ADR-0098 D6: `add` を後続の宣言にするか。run の中（`CELERIS_FOLLOWUPS_FILE` と `CELERIS_RUN_DB` がある）で、
/// 書き先の DB がその daemon の DB のときだけ。試験が一時 DB（`--db <tmp>`）に書く `add` はそのまま DB に書く。
fn followups_target(cli_db: Option<&Path>) -> Option<PathBuf> {
    let file = env::var_os(task_ops::followup::ENV_FOLLOWUPS_FILE)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)?;
    let run_db = run_db()?;
    let target = resolve_db_path(cli_db.map(Path::to_path_buf));
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    same(&target, &run_db).then_some(file)
}

fn dispatch(store: &SqliteStore, db_path: &Path, command: Command) -> Result<ExitCode, CliError> {
    match command {
        Command::ApiRequest(_) => unreachable!("handled before store open"),
        Command::DocsMaintenance { .. } => unreachable!("handled before store open"),
        Command::Cron { .. } => unreachable!("handled before store open"),
        Command::Models { .. } => unreachable!("handled before store open"),
        Command::Curation { .. } => unreachable!("handled before store open"),
        Command::BuildCache { .. } => unreachable!("handled before store open"),
        Command::Browser { .. } => unreachable!("handled before store open"),
        Command::Scratch { .. } => unreachable!("handled before store open"),
        Command::Target { .. } => unreachable!("handled before store open"),
        Command::Release { .. } => unreachable!("handled before store open"),
        Command::Org { command } => org_cmd::run(store, db_path, command),
        // `Config` は DB を開く前に処理される（`main` を見よ）。
        Command::Config { command } => config_cmd::run(command),
        // `Routing` も同じ（設定ファイルだけを読む。`main` を見よ）。
        Command::Routing { command } => routing_cmd::run(Some(db_path), command),
        Command::Add(args) => add::run(store, args),
        Command::Plan(args) => plan::run(store, args),
        Command::PlanLint => plan_lint::run(store),
        Command::Ls(args) => query::run_ls(store, args),
        Command::Show(args) => query::run_show(store, args),
        Command::Approve(args) => gate::run_approve(store, args),
        Command::Accept(args) => accept::run(store, args),
        Command::Reject(args) => gate::run_reject(store, args),
        Command::Cancel(args) => cancel::run(store, args),
        Command::Rereview(args) => rereview::run(store, args),
        Command::Retry(args) => retry::run(store, args),
        Command::Answer(args) => gate::run_answer(store, args),
        Command::Log(args) => query::run_log(store, args),
        Command::Replay(args) => replay::run(store, args),
        Command::Projects { command } => projects::run(store, command),
        Command::Workspace { command } => workspace::run(store, command),
        // `main` が先に処理する（DB を開かない場合があるため）。
        Command::Knowledge { .. } => unreachable!("handled before the store is opened"),
        Command::Mcp { .. } => unreachable!("handled before the store is opened"),
        Command::Skills { .. } => unreachable!("handled before the store is opened"),
        Command::Db { .. } => unreachable!("handled before the store is opened"),
        Command::Worker { command } => match command {
            WorkerCommand::Run(args) => worker::run_run(store, args),
        },
        Command::Execution { command } => execution_cmd::run(store, command),
        Command::Tree { command } => execution_cmd::run_tree(store, command),
    }
}

/// The registered CoS operation (method, domain path, body) of a mutating subcommand, if any
/// (ADR 2026-10-09-cos-operations-all-mutations D3; `config/skills/cos-operator/operations.md`).
fn cos_mapped(
    command: &Command,
) -> Result<Option<(&'static str, String, serde_json::Value)>, CliError> {
    use serde_json::json;
    let task_path = |id: &str, tail: &str| -> Result<String, CliError> {
        let id = error::parse_task_id(id)?;
        Ok(format!("/api/v1/tasks/{id}{tail}"))
    };
    Ok(match command {
        Command::Retry(args) => Some((
            "POST",
            task_path(&args.id, "/retry")?,
            json!({"accept": !args.draft}),
        )),
        Command::Approve(args) => Some((
            "POST",
            task_path(&args.id, "/approve")?,
            json!({"note": args.note}),
        )),
        Command::Reject(args) => Some((
            "POST",
            task_path(&args.id, "/reject")?,
            json!({"note": args.note}),
        )),
        Command::Accept(args) => Some(("POST", task_path(&args.id, "/accept")?, json!({}))),
        Command::Cancel(args) => Some(("POST", task_path(&args.id, "/cancel")?, json!({}))),
        Command::Rereview(args) => Some(("POST", task_path(&args.id, "/rereview")?, json!({}))),
        Command::Answer(args) => Some((
            "POST",
            task_path(&args.id, "/answer")?,
            json!({"answer": args.answer}),
        )),
        Command::Execution {
            command: ExecutionCommand::PhaseGate(args),
        } => {
            let action = match args.action {
                PhaseGateActionArg::Continue => "continue",
                PhaseGateActionArg::Replan => "replan",
                PhaseGateActionArg::Withdraw => "withdraw",
            };
            Some((
                "POST",
                task_path(&args.task_id, "/execution/phase-gate")?,
                json!({"action": action, "note": args.note}),
            ))
        }
        Command::Execution {
            command:
                ExecutionCommand::Plan {
                    command: ExecutionPlanCommand::Replan(args),
                },
        } => {
            let raw = execution_cmd::read_plan_file(&args.file)?;
            let body: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| CliError::msg(format!("plan file is not JSON: {e}")))?;
            Some(("PUT", task_path(&args.task_id, "/execution-plan")?, body))
        }
        Command::Execution {
            command:
                ExecutionCommand::Plan {
                    command: ExecutionPlanCommand::Set(args),
                },
        } => {
            let raw = execution_cmd::read_plan_file(&args.file)?;
            let body: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| CliError::msg(format!("plan file is not JSON: {e}")))?;
            Some(("POST", task_path(&args.task_id, "/execution-plan")?, body))
        }
        Command::Tree {
            command: execution_cmd::TreeCommand::Adopt(args),
        } => Some((
            "POST",
            task_path(&args.root, "/tree/adopt")?,
            json!({"task_id": args.task, "stage": args.stage, "unit_key": args.unit}),
        )),
        Command::Cron { command, .. }
            if !matches!(
                command,
                CronCommand::List | CronCommand::Show(_) | CronCommand::History(_)
            ) =>
        {
            let (method, path, body) = cron_cmd::request_of(command.clone())?;
            Some((
                method,
                format!("/api/v1{path}"),
                body.unwrap_or_else(|| json!({})),
            ))
        }
        Command::Models { command, .. } => {
            models_cmd::request_of(command)?.map(|(method, path, body)| {
                (
                    method,
                    format!("/api/v1{path}"),
                    body.unwrap_or(serde_json::Value::Null),
                )
            })
        }
        // The API replay is the plain check; `--check`/`--apply` rebuild indexes in the local DB
        // and have no API operation.
        Command::Replay(args) if !args.check && !args.apply => {
            Some(("POST", "/api/v1/replay".to_string(), json!({})))
        }
        _ => None,
    })
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let cos_options = cos_ops::Options {
        reason: cli.reason.take(),
        idempotency_key: cli.idempotency_key.take(),
        expected_revision: cli.expected_revision.take(),
        api_url: cli.api_url.take(),
    };
    if let Command::ApiRequest(args) = cli.command {
        return match cos_ops::run(args, &cos_options) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(credential) = cos_ops::credential() {
        if let Command::Add(args) = cli.command {
            let result = (|| {
                let body = add::cos_body(args)?;
                let api = cos_ops::api_config(cos_options.api_url.as_deref())?;
                cos_ops::send(
                    &api,
                    "POST",
                    "/api/v1/tasks",
                    body,
                    &cos_options,
                    Some(&credential),
                )
            })();
            return match result {
                Ok(value) => {
                    println!("{value}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        // ADR 2026-10-07 cos-live-fixes D2: CoS の `knowledge record` は KB を直接書かず、
        // `POST /api/v1/knowledge/inbox` を監査つきの operation として呼ぶ。
        if let Command::Knowledge {
            command: KnowledgeCommand::Record(args),
        } = &cli.command
        {
            return match knowledge::run_record_cos(args, &cos_options, &credential) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        // ADR 2026-10-09-cos-operations-all-mutations D3: subcommands whose domain request is a
        // registered CoS operation go through `/cos/operations` with the run credential.
        match cos_mapped(&cli.command) {
            Ok(Some((method, path, body))) => {
                let result = cos_ops::api_config(cos_options.api_url.as_deref()).and_then(|api| {
                    cos_ops::send(&api, method, &path, body, &cos_options, Some(&credential))
                });
                return match result {
                    Ok(value) => {
                        println!("{value}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                };
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
        let read_only = matches!(
            &cli.command,
            Command::Ls(_)
                | Command::Show(_)
                | Command::Log(_)
                | Command::PlanLint
                | Command::Projects { .. }
                | Command::Routing { .. }
                | Command::Config { .. }
                | Command::Models {
                    command: ModelsCommand::List(_),
                    ..
                }
                | Command::Knowledge {
                    command: KnowledgeCommand::Search(_) | KnowledgeCommand::Get(_)
                }
                | Command::Cron {
                    command: CronCommand::List | CronCommand::Show(_) | CronCommand::History(_),
                    ..
                }
        );
        if !read_only {
            eprintln!(
                "error: this command has no audited CoS operation mapping; use api-request with a registered domain path"
            );
            return ExitCode::FAILURE;
        }
    }
    if let Command::Scratch { command } = cli.command {
        return match scratch_cmd::run(cli.db, command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Target { command } = cli.command {
        return match target_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Release { command } = cli.command {
        return match release_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Browser { command } = cli.command {
        return match browser_cmd::run(command, cos_options.api_url.as_deref()) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::BuildCache { command } = cli.command {
        return match build_cache::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::DocsMaintenance { command } = cli.command {
        return match commands::docs_maintenance::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0046 D3: `config to-harnesses` は設定ファイルしか読まない（DB を開かない）。
    if let Command::Config { command } = cli.command {
        return match config_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Cron { command, config } = cli.command {
        return match cron_cmd::run(config, command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {}", error::render(&e));
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Models { command, config } = cli.command {
        return match models_cmd::run(config, command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {}", error::render(&e));
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0131 付記 D12: `curation validate` はファイルだけを読む（DB を開かない）。
    if let Command::Curation { command } = cli.command {
        return match curation_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {}", error::render(&e));
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0069 Phase 118 D3: `routing show` も設定ファイルしか読まない（DB を開かない）。
    // Phase 4 の `routing export` は明示の `--db` だけを task-ops の読み取り専用接続で開く
    // （`CELERIS_DB` 等の既定は解決しない。store も開かない＝migration しない）。
    if let Command::Routing { command } = cli.command {
        return match routing_cmd::run(cli.db.as_deref(), command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0098 D6: worker の run の中で daemon の DB に向けた `add` は DB を開かず、その run の後続の宣言になる
    // （run の終わりに daemon が run の task の案件で作る）。
    let followups_file = followups_target(cli.db.as_deref());
    if let Some(path) = followups_file.as_deref()
        && let Command::Add(args) = cli.command
    {
        return match add::queue(args, path) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {}", error::render(&e));
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0047 D3: 知識ベースの道具は **DB を開かない**（ワーカーのコンテナには DB が無い）。
    // ADR-0052 D3 の例外は `knowledge rerun` だけ（管理系。`org migrate-v2` と同じく DB を直接開く）。
    if let Command::Knowledge { command } = cli.command {
        if !command.needs_db() {
            return match knowledge::run(command) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        let db_path = resolve_db_path(cli.db);
        let store = match open_store(&db_path) {
            Ok(s) => s,
            Err(code) => return code,
        };
        return match knowledge::run_with_store(&store, command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {}", error::render(&e));
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0122 D1: `skills import` も KB だけを読み書きする（DB を開かない）。
    if let Command::Skills { command } = cli.command {
        return match skills_cmd::run(command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    // ADR-0064 D2/D3: `db backup`/`db integrity-check` は通常の `--db` の store を開かず、
    // 専用の接続で直接処理する（`backup` の既定の元は `--db` の解決規則と同じ）。
    if let Command::Db { command } = cli.command {
        return match command {
            DbCommand::Backup(args) => {
                let src = args.src.clone().unwrap_or_else(|| resolve_db_path(cli.db));
                match db_cmd::run_backup(args, src) {
                    Ok(code) => code,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            DbCommand::IntegrityCheck(args) => match db_cmd::run_integrity_check(args) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
        };
    }
    // ADR-0056 D1: `mcp stdio` は DB を開かない（手元の HTTP に橋を架けるだけ）。
    // Phase 101: `mcp call` も同じ（DB は開かない）。
    // `mcp client …` は `knowledge rerun` と同じ管理系（DB を直接開く）。
    if let Command::Mcp { command } = cli.command {
        return match command {
            McpCommand::Stdio(args) => match mcp::run_stdio(args) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            McpCommand::Call(args) => match mcp::run_call(args) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            },
            McpCommand::Client { command } => {
                let db_path = resolve_db_path(cli.db);
                let store = match open_store(&db_path) {
                    Ok(s) => s,
                    Err(code) => return code,
                };
                match mcp::run_client(&store, command) {
                    Ok(code) => code,
                    Err(e) => {
                        eprintln!("error: {}", error::render(&e));
                        ExitCode::FAILURE
                    }
                }
            }
        };
    }
    let db_path = resolve_db_path(cli.db);

    let store = match open_store(&db_path) {
        Ok(s) => s,
        Err(code) => return code,
    };

    match dispatch(&store, &db_path, cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", error::render(&e));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod cos_mapping_tests {
    use super::*;

    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(std::iter::once("celerisctl").chain(args.iter().copied()))
            .expect("parse")
            .command
    }

    /// ADR 2026-10-09-cos-operations-all-mutations D3: subcommands of registered operations map to
    /// the same domain request the API registry allows; others stay unmapped (refused under CoS).
    #[test]
    fn cos_mapped_subcommands_match_registered_operations() {
        let id = task_core::TaskId::new();
        let (method, path, body) = cos_mapped(&parse(&["retry", &id.to_string(), "--draft"]))
            .expect("map")
            .expect("mapped");
        assert_eq!(method, "POST");
        assert_eq!(path, format!("/api/v1/tasks/{id}/retry"));
        assert_eq!(body, serde_json::json!({"accept": false}));
        let (_, path, body) = cos_mapped(&parse(&["answer", &id.to_string(), "はい"]))
            .expect("map")
            .expect("mapped");
        assert_eq!(path, format!("/api/v1/tasks/{id}/answer"));
        assert_eq!(body["answer"], "はい");
        let (_, path, body) = cos_mapped(&parse(&[
            "execution",
            "phase-gate",
            &id.to_string(),
            "replan",
            "--note",
            "直す",
        ]))
        .expect("map")
        .expect("mapped");
        assert_eq!(path, format!("/api/v1/tasks/{id}/execution/phase-gate"));
        assert_eq!(
            body,
            serde_json::json!({"action": "replan", "note": "直す"})
        );
        let (method, path, body) = cos_mapped(&parse(&[
            "execution",
            "plan",
            "set",
            &id.to_string(),
            "--file",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../task-api/tests/fixtures/browser-plan.json"
            ),
        ]))
        .expect("map")
        .expect("mapped");
        assert_eq!(method, "POST");
        assert_eq!(path, format!("/api/v1/tasks/{id}/execution-plan"));
        assert_eq!(body["schema"], "celeris.execution-plan/3");
        let (method, path, body) = cos_mapped(&parse(&[
            "tree",
            "adopt",
            &id.to_string(),
            "--task",
            &task_core::TaskId::new().to_string(),
            "--stage",
            "s1",
            "--unit",
            "u1",
        ]))
        .expect("map")
        .expect("mapped");
        assert_eq!(method, "POST");
        assert_eq!(path, format!("/api/v1/tasks/{id}/tree/adopt"));
        assert_eq!(body["stage"], "s1");
        assert_eq!(body["unit_key"], "u1");
        let (method, path, body) = cos_mapped(&parse(&["cron", "pause", "daily"]))
            .expect("map")
            .expect("mapped");
        assert_eq!(
            (method, path.as_str(), body),
            (
                "POST",
                "/api/v1/cron-jobs/daily/pause",
                serde_json::json!({})
            )
        );
        let (method, path, body) = cos_mapped(&parse(&[
            "cron",
            "update",
            "daily",
            "--schedule",
            "0 5 * * *",
        ]))
        .expect("map")
        .expect("mapped");
        assert_eq!(
            (method, path.as_str()),
            ("PATCH", "/api/v1/cron-jobs/daily")
        );
        assert_eq!(body, serde_json::json!({"schedule": "0 5 * * *"}));
        assert!(
            cos_mapped(&parse(&["cron", "list"]))
                .expect("map")
                .is_none()
        );
        assert!(
            cos_mapped(&parse(&["knowledge", "reindex"]))
                .expect("map")
                .is_none()
        );
        // The task gates go through task.approve/reject/accept/cancel.
        for (verb, tail) in [
            ("approve", "/approve"),
            ("reject", "/reject"),
            ("accept", "/accept"),
            ("cancel", "/cancel"),
            ("rereview", "/rereview"),
        ] {
            let (method, path, _) = cos_mapped(&parse(&[verb, &id.to_string()]))
                .expect("map")
                .expect("mapped");
            assert_eq!(method, "POST");
            assert_eq!(path, format!("/api/v1/tasks/{id}{tail}"));
        }
        // ops-admin-config: model catalog and the API replay go through their operations.
        let (method, path, body) =
            cos_mapped(&parse(&["models", "discover", "--source", "opencode-go"]))
                .expect("map")
                .expect("mapped");
        assert_eq!(
            (method, path.as_str()),
            ("POST", "/api/v1/llm/models/discover")
        );
        assert_eq!(body, serde_json::json!({"source": "opencode-go"}));
        let (method, path, body) = cos_mapped(&parse(&[
            "models",
            "assign",
            "opencode-go",
            "cheap",
            "glm-5",
        ]))
        .expect("map")
        .expect("mapped");
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/api/v1/llm/models/assignments/opencode-go/cheap")
        );
        assert_eq!(body["model_id"], "glm-5");
        let (method, path, _) = cos_mapped(&parse(&["models", "unassign", "opencode-go", "cheap"]))
            .expect("map")
            .expect("mapped");
        assert_eq!(
            (method, path.as_str()),
            ("DELETE", "/api/v1/llm/models/assignments/opencode-go/cheap")
        );
        assert!(
            cos_mapped(&parse(&["models", "list"]))
                .expect("map")
                .is_none()
        );
        let (method, path, _) = cos_mapped(&parse(&["replay"]))
            .expect("map")
            .expect("mapped");
        assert_eq!((method, path.as_str()), ("POST", "/api/v1/replay"));
        assert!(
            cos_mapped(&parse(&["replay", "--apply"]))
                .expect("map")
                .is_none()
        );
    }

    /// Rows of `config/skills/cos-operator/operations.md` (pinned to the API `ALLOWED` by the task-api
    /// test `cos_operator_skill_table_matches_allowed`), as (method, path pattern).
    fn skill_allowed_rows() -> Vec<(String, String)> {
        let text = include_str!("../../../config/skills/cos-operator/operations.md");
        let table = text
            .split("## 除外する操作")
            .next()
            .expect("operations table");
        table
            .lines()
            .filter_map(|line| {
                let cell = line.split('|').nth(2)?.trim().trim_matches('`');
                let (method, path) = cell.split_once(' ')?;
                (matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
                    && path.starts_with("/api/v1/"))
                .then(|| (method.to_string(), path.to_string()))
            })
            .collect()
    }

    fn matches_row(pattern: &str, path: &str) -> bool {
        let p: Vec<_> = pattern.split('/').collect();
        let s: Vec<_> = path.split('/').collect();
        p.len() == s.len()
            && p.iter()
                .zip(&s)
                .all(|(p, s)| (p.starts_with('<') && p.ends_with('>') && !s.is_empty()) || p == s)
    }

    /// ops-closeout (ADR 2026-10-09-cos-operations-all-mutations D3): every mutating subcommand whose
    /// API route is ALLOWED is wrapped into `/cos/operations` under a CoS credential, and the
    /// wrapped request is a registered row. Local-only commands stay unmapped (refused under CoS).
    #[test]
    fn cos_mapped_table_wraps_every_allowed_subcommand() {
        let id = task_core::TaskId::new().to_string();
        let child = task_core::TaskId::new().to_string();
        let plan = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../task-api/tests/fixtures/browser-plan.json"
        );
        let t = |tail: &str| format!("/api/v1/tasks/{id}{tail}");
        let table: Vec<(Vec<&str>, &str, String)> = vec![
            (vec!["approve", &id], "POST", t("/approve")),
            (vec!["reject", &id], "POST", t("/reject")),
            (vec!["accept", &id], "POST", t("/accept")),
            (vec!["cancel", &id], "POST", t("/cancel")),
            (vec!["rereview", &id], "POST", t("/rereview")),
            (vec!["retry", &id], "POST", t("/retry")),
            (vec!["answer", &id, "はい"], "POST", t("/answer")),
            (
                vec!["execution", "phase-gate", &id, "continue"],
                "POST",
                t("/execution/phase-gate"),
            ),
            (
                vec!["execution", "plan", "set", &id, "--file", plan],
                "POST",
                t("/execution-plan"),
            ),
            (
                vec!["execution", "plan", "replan", &id, "--file", plan],
                "PUT",
                t("/execution-plan"),
            ),
            (
                vec![
                    "tree", "adopt", &id, "--task", &child, "--stage", "s1", "--unit", "u1",
                ],
                "POST",
                t("/tree/adopt"),
            ),
            (
                vec!["cron", "pause", "daily"],
                "POST",
                "/api/v1/cron-jobs/daily/pause".into(),
            ),
            (
                vec!["cron", "resume", "daily"],
                "POST",
                "/api/v1/cron-jobs/daily/resume".into(),
            ),
            (
                vec!["cron", "run", "daily"],
                "POST",
                "/api/v1/cron-jobs/daily/run".into(),
            ),
            (
                vec!["cron", "update", "daily", "--schedule", "0 5 * * *"],
                "PATCH",
                "/api/v1/cron-jobs/daily".into(),
            ),
            (
                vec![
                    "cron",
                    "create",
                    "--name",
                    "daily",
                    "--schedule",
                    "0 5 * * *",
                    "--timezone",
                    "Asia/Tokyo",
                    "--template",
                    "{}",
                ],
                "POST",
                "/api/v1/cron-jobs".into(),
            ),
            (
                vec!["models", "discover", "--source", "opencode-go"],
                "POST",
                "/api/v1/llm/models/discover".into(),
            ),
            (
                vec!["models", "assign", "opencode-go", "cheap", "glm-5"],
                "PUT",
                "/api/v1/llm/models/assignments/opencode-go/cheap".into(),
            ),
            (
                vec!["models", "unassign", "opencode-go", "cheap"],
                "DELETE",
                "/api/v1/llm/models/assignments/opencode-go/cheap".into(),
            ),
            (vec!["replay"], "POST", "/api/v1/replay".into()),
        ];
        let rows = skill_allowed_rows();
        assert!(rows.len() > 50, "operations.md table not parsed: {rows:?}");
        for (argv, method, path) in &table {
            let (got_method, got_path, _) = cos_mapped(&parse(argv))
                .unwrap_or_else(|e| panic!("{argv:?}: {e}"))
                .unwrap_or_else(|| panic!("{argv:?} is not wrapped into /cos/operations"));
            assert_eq!(
                (got_method, got_path.as_str()),
                (*method, path.as_str()),
                "{argv:?}"
            );
            assert!(
                rows.iter()
                    .any(|(m, p)| m == method && matches_row(p, path)),
                "{argv:?}: {method} {path} is not an ALLOWED row"
            );
        }
        for argv in [
            vec!["cron", "list"],
            vec!["models", "list"],
            vec!["replay", "--apply"],
            vec!["knowledge", "reindex"],
        ] {
            assert!(
                cos_mapped(&parse(&argv)).expect("map").is_none(),
                "{argv:?}"
            );
        }
    }
}
