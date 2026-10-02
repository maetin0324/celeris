//! `celerisctl ls` / `celerisctl show` / `celerisctl log` — DESIGN.md §5.9。読み取り専用コマンド。
//!
//! `run_ls` は `store.list(status)` の結果をフラット、または `--tree` で `parent_id` に
//! よる親子インデント表示する。`run_show` はタスクの詳細と `events_for` の要約を表示する。
//! `run_log` は `events_for` を `seq` 順に表示し、`--follow` は 500ms 間隔でポーリングする
//! （Phase 2 の時点ではイベントを継続的に追記するディスパッチャがまだ無いため、`--follow` は
//! Phase 3 以降で実用になる）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, ValueEnum};
use task_core::{Check, Status, Task, TaskId, TaskStore};
use task_ops::view::{self, ViewContext};
use time::OffsetDateTime;

use crate::error::CliError;
use crate::outln;

#[derive(Args, Debug)]
pub struct LsArgs {
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,

    #[arg(long)]
    pub tree: bool,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    pub id: String,

    /// `TaskDetail` を JSON で出力する（`GET /api/v1/tasks/{id}` と同一。`docs/api/v1/gui-api.md` §3.5）。
    #[arg(long)]
    pub json: bool,

    /// `--json` のときの `ViewContext.workspace_root`。省略時は `--config` の値、それも無ければ `./workspaces`
    /// （`task_detail_json` の doc を参照）。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,

    /// `--json` を `GET /api/v1/tasks/{id}` と完全に同じにするための `config.toml`（P-54: 省略時は環境変数 `CELERIS_CONFIG`）。
    /// これがあると `workspace_root` / リトライの待ち / `max_requeues` が設定の値になり、`[[clusters]]` が分かるので
    /// `sync = "worktree"` のタスクに `worktree` が出る（ADR-0019 D2）。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct LogArgs {
    pub id: String,

    #[arg(long)]
    pub follow: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusArg {
    Draft,
    Ready,
    Running,
    Blocked,
    Reviewing,
    Done,
    Failed,
    Cancelled,
}

impl From<StatusArg> for Status {
    fn from(value: StatusArg) -> Self {
        match value {
            StatusArg::Draft => Status::Draft,
            StatusArg::Ready => Status::Ready,
            StatusArg::Running => Status::Running,
            StatusArg::Blocked => Status::Blocked,
            StatusArg::Reviewing => Status::Reviewing,
            StatusArg::Done => Status::Done,
            StatusArg::Failed => Status::Failed,
            StatusArg::Cancelled => Status::Cancelled,
        }
    }
}

fn print_task_line(task: &Task, indent: usize) {
    let prefix = "  ".repeat(indent);
    let mut line = format!(
        "{prefix}{} {:?} {:?} {}",
        task.id, task.status, task.kind, task.title
    );
    // ADR-0016 D1 / ADR-0027 D1: role/genre がある行にだけ `[role=.. genre=..]` を付ける（無い方が普通の celerisctl の使い方なので、
    // 常に `(none)` を書いて行を汚さない）。
    match (&task.role, &task.genre) {
        (None, None) => {}
        (role, genre) => {
            let role = role.as_deref().unwrap_or("-");
            let genre = genre.as_deref().unwrap_or("-");
            line.push_str(&format!(" [role={role} genre={genre}]"));
        }
    }
    outln!("{line}");
}

fn print_tree(tasks: &[Task]) {
    let mut children: HashMap<TaskId, Vec<&Task>> = HashMap::new();
    let mut roots: Vec<&Task> = Vec::new();
    for task in tasks {
        match task.parent_id {
            Some(parent) => children.entry(parent).or_default().push(task),
            None => roots.push(task),
        }
    }
    for root in roots {
        print_task_line(root, 0);
        if let Some(kids) = children.get(&root.id) {
            for kid in kids {
                print_task_line(kid, 1);
            }
        }
    }
}

fn check_kind_name(check: &Check) -> &'static str {
    match check {
        Check::Command { .. } => "command",
        Check::ArtifactExists { .. } => "artifact_exists",
        Check::KnowledgePage { .. } => "knowledge_page",
        Check::Reviewer => "reviewer",
        Check::Human => "human",
    }
}

fn print_events(store: &dyn TaskStore, id: TaskId) -> Result<(), CliError> {
    let events = store.events_for(id)?;
    for (seq, event) in events {
        outln!("[{seq}] {event:?}");
    }
    Ok(())
}

pub fn run_ls(store: &dyn TaskStore, args: LsArgs) -> Result<ExitCode, CliError> {
    let status = args.status.map(Status::from);
    let tasks = store.list(status)?;
    if args.tree {
        print_tree(&tasks);
    } else {
        for task in &tasks {
            print_task_line(task, 0);
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run_show(store: &dyn TaskStore, args: ShowArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;

    if args.json {
        return run_show_json(store, id, args.workspace_root, args.config.as_ref());
    }

    let task = store
        .get(id)?
        .ok_or_else(|| CliError::Message(format!("task not found: {}", args.id)))?;

    outln!("id: {}", task.id);
    outln!("status: {:?}", task.status);
    outln!("kind: {:?}", task.kind);
    outln!("title: {}", task.title);
    outln!("objective: {}", task.objective);
    outln!("priority: {}", task.priority);
    outln!("role: {}", task.role.as_deref().unwrap_or("(none)"));
    outln!("genre: {}", task.genre.as_deref().unwrap_or("(none)"));
    outln!("attempts: {}", task.attempts);
    outln!("budget: {:?}", task.budget);
    outln!("lease: {:?}", task.lease);
    match task.parent_id {
        Some(parent) => outln!("parent_id: {parent}"),
        None => outln!("parent_id: (none)"),
    }
    let depends_on: Vec<String> = task.depends_on.iter().map(TaskId::to_string).collect();
    outln!("depends_on: [{}]", depends_on.join(", "));
    outln!("acceptance:");
    for criterion in &task.acceptance {
        outln!(
            "  - {} ({})",
            criterion.text,
            check_kind_name(&criterion.check)
        );
    }

    outln!("events:");
    print_events(store, id)?;

    Ok(ExitCode::SUCCESS)
}

/// `celerisctl show --json <id>` の中身。`task_ops::view::task_detail` の結果を JSON 文字列に
/// する（`GET /api/v1/tasks/{id}` と同一。`docs/api/v1/gui-api.md` §3.5）。出力そのものをテストしやすい
/// よう `run_show_json` から分離してある。
///
/// `--config`（または `CELERIS_CONFIG`）があれば、`ViewContext` はその `config.toml` から作る
/// （= API と同じ値。`[[clusters]]` も分かるので `worktree` が出る）。無ければ次の既定値で作る
/// （`config.toml` の対応する既定値と同じ。`celeris::config` の `default_workspace_root` /
/// `default_retry_backoff_base_secs` / `default_retry_backoff_max_secs` / `default_max_requeues`）:
/// - `workspace_root`: `./workspaces`（`--workspace-root` で上書き可）
/// - `retry_backoff`: base 10 秒 / max 300 秒
/// - `max_requeues`: 5
/// - `clusters`: 空（`worktree` は `null` になる）
fn task_detail_json(
    store: &dyn TaskStore,
    id: TaskId,
    workspace_root: Option<PathBuf>,
    config: Option<&PathBuf>,
) -> Result<String, CliError> {
    let config = match config {
        Some(path) => Some(celeris::Config::load(path).map_err(|e| {
            CliError::msg(format!("failed to load config {}: {e}", path.display()))
        })?),
        None => None,
    };
    let ctx = match &config {
        Some(c) => ViewContext {
            workspace_root: workspace_root.unwrap_or_else(|| c.workspace_root.clone()),
            retry_backoff_base: Duration::from_secs(c.retry_backoff_base_secs),
            retry_backoff_max: Duration::from_secs(c.retry_backoff_max_secs),
            max_requeues: c.max_requeues,
            clusters: c.cluster_view_infos(),
        },
        None => ViewContext {
            workspace_root: workspace_root.unwrap_or_else(|| PathBuf::from("./workspaces")),
            retry_backoff_base: Duration::from_secs(10),
            retry_backoff_max: Duration::from_secs(300),
            max_requeues: 5,
            clusters: Default::default(),
        },
    };
    let detail = view::task_detail(store, id, &ctx, OffsetDateTime::now_utc())?;
    // `GET /api/v1/tasks/{id}` と同じ compact な直列化（docs/api/v1/gui-api.md §3.5）。整形は `jq` 等で行う。
    serde_json::to_string(&detail)
        .map_err(|e| CliError::msg(format!("failed to encode task detail: {e}")))
}

fn run_show_json(
    store: &dyn TaskStore,
    id: TaskId,
    workspace_root: Option<PathBuf>,
    config: Option<&PathBuf>,
) -> Result<ExitCode, CliError> {
    let json = task_detail_json(store, id, workspace_root, config)?;
    outln!("{json}");
    Ok(ExitCode::SUCCESS)
}

pub fn run_log(store: &dyn TaskStore, args: LogArgs) -> Result<ExitCode, CliError> {
    let id = crate::error::parse_task_id(&args.id)?;

    if !args.follow {
        print_events(store, id)?;
        return Ok(ExitCode::SUCCESS);
    }

    // Phase 2 の時点ではディスパッチャ等、別プロセスがタスクのイベントを継続的に
    // 追記する仕組みはまだ存在しない。そのため --follow は「今後追記されるイベント
    // を待ち受ける」だけの単純なポーリングループとして実装しておき、Phase 3 以降で
    // ディスパッチャが実装された際にそのまま使えるようにする。
    let mut last_seq: Option<u64> = None;
    loop {
        let events = store.events_for(id)?;
        for (seq, event) in &events {
            if last_seq.is_none_or(|last| *seq > last) {
                outln!("[{seq}] {event:?}");
                last_seq = Some(*seq);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
