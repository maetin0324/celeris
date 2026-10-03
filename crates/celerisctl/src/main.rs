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
use commands::cron::{self as cron_cmd, CronCommand};
use commands::db::{self as db_cmd, DbCommand};
use commands::execution::{self as execution_cmd, ExecutionCommand, TreeCommand};
use commands::gate::{self, AnswerArgs, ApproveArgs, RejectArgs};
use commands::knowledge::{self, KnowledgeCommand};
use commands::mcp::{self, McpCommand};
use commands::org::{self as org_cmd, OrgCommand};
use commands::plan::{self, PlanArgs};
use commands::plan_lint;
use commands::projects::{self, ProjectsCommand};
use commands::query::{self, LogArgs, LsArgs, ShowArgs};
use commands::replay::{self, ReplayArgs};
use commands::rereview::{self, RereviewArgs};
use commands::retry::{self, RetryArgs};
use commands::routing::{self as routing_cmd, RoutingCommand};
use commands::scratch::{self as scratch_cmd, ScratchCommand};
use commands::skills::{self as skills_cmd, SkillsCommand};
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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// ADR-0075（Phase G1）: scratch pool（`status [--json]` / `gc [--dry-run]` / 外部 lease の `lease` / `touch` /
    /// `release` / `env`）。`lease` 系は DB を開かない。`status` / `gc` は DB があれば読む（daemon と同じ分類）。
    Scratch {
        #[command(subcommand)]
        command: ScratchCommand,
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
    /// ADR-0069 Phase 118 D3: `routing show`。tier → 実行モデル/effort の表。DB には触らない。
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
        Command::DocsMaintenance { .. } => unreachable!("handled before store open"),
        Command::Cron { .. } => unreachable!("handled before store open"),
        Command::BuildCache { .. } => unreachable!("handled before store open"),
        Command::Browser { .. } => unreachable!("handled before store open"),
        Command::Scratch { .. } => unreachable!("handled before store open"),
        Command::Org { command } => org_cmd::run(store, db_path, command),
        // `Config` は DB を開く前に処理される（`main` を見よ）。
        Command::Config { command } => config_cmd::run(command),
        // `Routing` も同じ（設定ファイルだけを読む。`main` を見よ）。
        Command::Routing { command } => routing_cmd::run(command),
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Command::Scratch { command } = cli.command {
        return match scratch_cmd::run(cli.db, command) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if let Command::Browser { command } = cli.command {
        return match browser_cmd::run(command) {
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
    // ADR-0069 Phase 118 D3: `routing show` も設定ファイルしか読まない（DB を開かない）。
    if let Command::Routing { command } = cli.command {
        return match routing_cmd::run(command) {
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
