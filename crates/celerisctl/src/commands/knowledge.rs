//! `celerisctl knowledge`（ADR-0047 D3。Phase 61）— **ワーカーからの共通アクセス**。
//!
//! このサブコマンドだけは **DB を開かない**（`~/.local/share/celeris/knowledge` を直接読み書きする）。コンテナの中でも
//! KB を同じパスにマウントすればそのまま動く（ADR-0043 D3 / ADR-0047 D3）。
//!
//! ```text
//! celerisctl knowledge init    [--root …]
//! celerisctl knowledge search <語> [--scope …] [--limit N] [--json]
//! celerisctl knowledge get    <path> [--json]
//! celerisctl knowledge record --title … --scope … [--tags a,b] --source task:<id> [--confidence …] [--path …] < body.md
//! celerisctl knowledge reindex
//! celerisctl knowledge migrate-human-sources [--apply] [--json]   # ADR-0047 付記 H4。既定は dry-run
//! celerisctl knowledge rerun   <task_id> [--db …]     # 管理系。**DB を開く**（ADR-0052 D3）
//! ```
//!
//! 根の決め方（ADR-0047 D3）: `--root` > `CELERIS_KNOWLEDGE_ROOT` > `[knowledge] root` > `~/.local/share/celeris/knowledge`。
//! 設定ファイルが読めない環境（コンテナの中）では黙って次の候補に落ちる。
//!
//! 例外は `rerun` だけ（ADR-0052 D3）。これは人が使う**管理系**で、`org migrate-v2` と同じように
//! DB を直接開く（celerisctl は HTTP の口を持たず、他の全サブコマンドも DB を直接見る）。やることは
//! `knowledge_runs` の 1 行を `failed` / `retried_at = NULL` に戻すだけで、**新しい run を作るのは
//! デーモンの次の tick**（`celeris::knowledge_maint::retry_failed`）。デーモンを止める必要は無い。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};
use task_core::knowledge::{self as kb, Confidence};
use task_core::{KnowledgeRunStore, SqliteStore, TaskId};
use task_ops::knowledge::{self as ops, RecordError, RecordRequest};

use crate::error::CliError;
use crate::outln;

/// 設定ファイルの既定（`celeris` と同じ）。
const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug)]
pub enum KnowledgeCommand {
    /// 知識ベースを用意する（git init・骨組み・雛形・索引。**冪等**）。
    Init(RootArgs),
    /// 索引（tags / title）と本文の全文一致で探す。
    Search(SearchArgs),
    /// ページ 1 枚を読む（front matter 付き）。
    Get(GetArgs),
    /// 候補を `_inbox/` に書く（正本には直接書かない。本文は標準入力）。
    Record(RecordArgs),
    /// `index.json` を作り直す。
    Reindex(RootArgs),
    /// 知識整理 run をもう一度やらせる（ADR-0052 D3。**DB を開く**管理系）。
    Rerun(RerunArgs),
    /// 旧形の `sources: human` を git 履歴から『人が書いた』『人の指示由来』に分ける
    /// （ADR-0047 付記 2026-10-04 H4。既定は dry-run。`--apply` で 1 commit）。
    MigrateHumanSources(MigrateHumanSourcesArgs),
}

#[derive(Args, Debug)]
pub struct MigrateHumanSourcesArgs {
    /// 書き換えて commit する（無ければ dry-run で何も書かない）。
    #[arg(long)]
    pub apply: bool,
    /// JSON で出す。
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub root: RootArgs,
}

impl KnowledgeCommand {
    /// このサブコマンドが DB を要るか（`rerun` だけ）。`main` が store を開くかどうかを決める。
    pub fn needs_db(&self) -> bool {
        matches!(self, KnowledgeCommand::Rerun(_))
    }
}

#[derive(Args, Debug)]
pub struct RerunArgs {
    /// 知識整理をやり直したい**元のタスク**の id（`knowledge_runs.task_id`。run 自身の id ではない）。
    pub task_id: String,
}

/// どのサブコマンドにもある根の指定。
#[derive(Args, Debug, Default)]
pub struct RootArgs {
    /// 知識ベースの根（既定: `CELERIS_KNOWLEDGE_ROOT` → `[knowledge] root` → `~/.local/share/celeris/knowledge`）。
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// 設定ファイル（`[knowledge] root` を読むためだけ。読めなければ無視する）。
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// 探す語（空白で区切ると複数の語。大文字小文字は区別しない）。
    pub query: Vec<String>,
    /// KB の相対パスの接頭辞か front matter の `scope`（`user` / `environment/clusters` / `project:<slug>`）。
    #[arg(long)]
    pub scope: Option<String>,
    /// 出す件数（既定 10）。
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
    /// JSON で出す。
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub root: RootArgs,
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// KB の根からの相対パス（`environment/clusters/pegasus.md`）。
    pub path: String,
    /// JSON で出す（`path` / `title` / `tags` / `scope` / `sources` / `body`）。
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub root: RootArgs,
}

#[derive(Args, Debug)]
pub struct RecordArgs {
    #[arg(long)]
    pub title: String,
    /// `user` / `environment` / `project:<slug>` / `experience`。
    #[arg(long)]
    pub scope: String,
    /// `--tags a,b` か `--tags a --tags b`。
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    /// **1 件以上必須**（`task:<id>` / `message:<id>` / `human:instruction` / `url:<…>`）。
    /// 人の指示・発言に由来する事実は `human:instruction`（`human` と `human:authored` は `human:instruction` に直る）。
    #[arg(long = "source", value_delimiter = ',')]
    pub sources: Vec<String>,
    /// `high` / `medium` / `low`。
    #[arg(long)]
    pub confidence: Option<String>,
    /// 取り込む先の KB 相対パス（省略なら人が accept するときに `scope` と題名から決まる）。
    #[arg(long)]
    pub path: Option<String>,
    /// JSON で出す。
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub root: RootArgs,
}

/// ADR-0047 D3 の根の決め方。設定ファイルは**読めたら使う**（無くてもエラーにしない）。
pub(crate) fn root_of(args: &RootArgs) -> PathBuf {
    let configured = configured_root(args.config.as_deref());
    ops::resolve_root(args.root.as_deref(), configured.as_deref())
}

fn configured_root(explicit: Option<&Path>) -> Option<PathBuf> {
    let path = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CELERIS_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let path = task_core::expand_home(&path, task_core::home_dir().as_deref());
    // 読めない・壊れている設定は「無い」と同じ（コンテナの中には設定が無い）。
    celeris::Config::load(&path).ok().map(|c| c.knowledge.root)
}

pub fn run(command: KnowledgeCommand) -> Result<ExitCode, CliError> {
    match command {
        KnowledgeCommand::Init(args) => run_init(&args),
        KnowledgeCommand::Search(args) => run_search(&args),
        KnowledgeCommand::Get(args) => run_get(&args),
        KnowledgeCommand::Record(args) => run_record(&args),
        KnowledgeCommand::Reindex(args) => run_reindex(&args),
        KnowledgeCommand::MigrateHumanSources(args) => run_migrate_human_sources(&args),
        KnowledgeCommand::Rerun(_) => Err(CliError::msg(
            "knowledge rerun は DB を開く（`KnowledgeCommand::needs_db` を見て store 付きで呼ぶこと）",
        )),
    }
}

/// `rerun` だけは store が要る（ADR-0052 D3）。
pub fn run_with_store(
    store: &SqliteStore,
    command: KnowledgeCommand,
) -> Result<ExitCode, CliError> {
    match command {
        KnowledgeCommand::Rerun(args) => run_rerun(store, &args),
        other => run(other),
    }
}

/// ADR-0052 D3: `knowledge_runs` の 1 行を「まだやり直していない失敗」に戻す。
/// 新しい run タスクを作るのは**デーモンの次の tick**（`knowledge_maint::retry_failed`）。
fn run_rerun(store: &SqliteStore, args: &RerunArgs) -> Result<ExitCode, CliError> {
    let task_id = args
        .task_id
        .parse::<TaskId>()
        .map_err(|e| CliError::msg(format!("invalid task id {}: {e}", args.task_id)))?;
    let Some(before) = store
        .knowledge_run_get(task_id)
        .map_err(|e| CliError::msg(e.to_string()))?
    else {
        return Err(CliError::msg(format!(
            "knowledge run not found for task {task_id}（このタスクからは知識整理 run が起きていない）"
        )));
    };
    if !store
        .knowledge_run_reset(task_id)
        .map_err(|e| CliError::msg(e.to_string()))?
    {
        return Err(CliError::msg(format!(
            "could not reset the knowledge run for task {task_id}"
        )));
    }
    outln!(
        "reset knowledge run for task {task_id}（直前: state={} run_task={}）",
        before.state.as_str(),
        before.run_task_id
    );
    outln!(
        "次の tick で celeris が新しい知識整理 run を 1 回だけ作ります（デーモンの再起動は不要）。"
    );
    Ok(ExitCode::SUCCESS)
}

fn run_init(args: &RootArgs) -> Result<ExitCode, CliError> {
    let root = root_of(args);
    let outcome = ops::init(&root).map_err(CliError::msg)?;
    if outcome.created {
        outln!("created {}", root.display());
    } else {
        outln!("ok {}", root.display());
    }
    for path in &outcome.added {
        outln!("  + {path}");
    }
    if !outcome.created && outcome.added.is_empty() {
        outln!("  （変更なし）");
    }
    Ok(ExitCode::SUCCESS)
}

fn run_reindex(args: &RootArgs) -> Result<ExitCode, CliError> {
    let root = root_of(args);
    require_kb(&root)?;
    let index = ops::reindex(&root).map_err(CliError::msg)?;
    outln!("{} pages ({})", index.items.len(), index.generated_at);
    Ok(ExitCode::SUCCESS)
}

fn run_migrate_human_sources(args: &MigrateHumanSourcesArgs) -> Result<ExitCode, CliError> {
    use ops::HumanSourceClass as C;
    let root = root_of(&args.root);
    require_kb(&root)?;
    let report = ops::migrate_human_sources(&root, args.apply).map_err(CliError::msg)?;
    if args.json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| CliError::msg(format!("failed to render json: {e}")))?;
        outln!("{json}");
        return Ok(ExitCode::SUCCESS);
    }
    for (class, label) in [
        (C::Authored, "人が書いた（human:authored）"),
        (C::Instruction, "人の指示由来（human:instruction）"),
        (C::Undetermined, "判別できない（human のまま保護）"),
    ] {
        outln!("## {label}: {} 件", report.count(class));
        for e in report.entries.iter().filter(|e| e.class == class) {
            outln!("- {} — {}", e.path, e.reason);
        }
    }
    match (&report.sha, report.applied) {
        (Some(sha), _) => outln!("applied: {sha}"),
        (None, true) => outln!("applied: 書き換えなし"),
        (None, false) => outln!("dry-run（何も書いていない。--apply で書き換える）"),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_search(args: &SearchArgs) -> Result<ExitCode, CliError> {
    let root = root_of(&args.root);
    require_kb(&root)?;
    let query = args.query.join(" ");
    let hits = ops::search(&root, &query, args.scope.as_deref(), args.limit);
    if args.json {
        let json = serde_json::to_string_pretty(&hits)
            .map_err(|e| CliError::msg(format!("failed to render json: {e}")))?;
        outln!("{json}");
        return Ok(ExitCode::SUCCESS);
    }
    if hits.is_empty() {
        outln!("（見つかりません）");
        return Ok(ExitCode::SUCCESS);
    }
    for hit in &hits {
        let tags = if hit.item.tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", hit.item.tags.join(", "))
        };
        let scope = hit.item.scope.as_deref().unwrap_or("-");
        let updated = hit.item.updated.as_deref().unwrap_or("-");
        outln!(
            "{}\t{}{}\t{}\t{}",
            hit.item.path,
            hit.item.title,
            tags,
            scope,
            updated
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_get(args: &GetArgs) -> Result<ExitCode, CliError> {
    let root = root_of(&args.root);
    let path = kb::page_path(&args.path).map_err(|e| CliError::msg(e.to_string()))?;
    let raw = ops::read_page(&root, &path)
        .ok_or_else(|| CliError::msg(format!("knowledge page not found: {path}")))?;
    if args.json {
        let (front, body) = kb::front_matter(&raw);
        let value = serde_json::json!({
            "path": path,
            "title": kb::title_of(&raw, &path),
            "tags": front.tags,
            "scope": front.scope,
            "sources": front.sources,
            "updated": front.updated,
            "confidence": front.confidence,
            "body": body,
        });
        let json = serde_json::to_string_pretty(&value)
            .map_err(|e| CliError::msg(format!("failed to render json: {e}")))?;
        outln!("{json}");
    } else {
        // `front matter 付き`（ADR-0047 D3）。末尾の改行は 1 つにする。
        outln!("{}", raw.trim_end_matches('\n'));
    }
    Ok(ExitCode::SUCCESS)
}

fn run_record(args: &RecordArgs) -> Result<ExitCode, CliError> {
    let root = root_of(&args.root);
    require_kb(&root)?;
    let confidence = match args.confidence.as_deref() {
        Some(raw) => Some(raw.parse::<Confidence>().map_err(CliError::msg)?),
        None => None,
    };
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .map_err(|e| CliError::msg(format!("failed to read body from stdin: {e}")))?;
    let request = RecordRequest {
        title: args.title.clone(),
        scope: args.scope.clone(),
        tags: args.tags.clone(),
        sources: args.sources.clone(),
        confidence,
        body,
        path: args.path.clone(),
        op: None,
    };
    match ops::record(&root, &request) {
        Ok(outcome) => {
            if args.json {
                let value = serde_json::json!({
                    "id": outcome.id,
                    "path": outcome.path,
                    "sha": outcome.sha,
                    "target": outcome.target,
                    "op": outcome.op.map(|o| o.as_str()),
                });
                let json = serde_json::to_string_pretty(&value)
                    .map_err(|e| CliError::msg(format!("failed to render json: {e}")))?;
                outln!("{json}");
            } else {
                // Phase K-1: 取り込み先（置き場のガードを通したもの）と、既にあるページへの追記かを出す。
                let how = match outcome.op {
                    Some(op) => format!("{} へ {op}", outcome.target),
                    None => outcome.target.clone(),
                };
                outln!(
                    "recorded {} → {how}（人が確認してから正本に入ります）",
                    outcome.path
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        // ADR-0047 D4: 秘密は保存しない。何が当たったかを stderr に出して落とす。
        Err(e @ RecordError::Secret(_)) => Err(CliError::msg(e.to_string())),
        Err(e) => Err(CliError::msg(e.to_string())),
    }
}

fn require_kb(root: &Path) -> Result<(), CliError> {
    if ops::exists(root) {
        Ok(())
    } else {
        Err(CliError::msg(format!(
            "{} に知識ベースがありません（`celerisctl knowledge init` で用意する）",
            root.display()
        )))
    }
}

#[cfg(test)]
#[path = "knowledge_tests.rs"]
mod tests;
