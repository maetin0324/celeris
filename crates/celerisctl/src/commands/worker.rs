//! `celerisctl worker run` — DESIGN.md §5.9 のデバッグ用コマンド（ADR-0012 D4）。
//!
//! デーモンとディスパッチャを経由せず、1 タスクを 1 つの route（harness・LLM source・model・
//! account）で 1 回だけ実行する。**状態は変えない**: リースを取らず、遷移もイベントの追記もしない
//! （DB は読むだけ）。レビューも行わない。`context.prior_review` / `context.answers` は
//! ディスパッチャと同じ派生関数（`task_ops::derive::{prior_review_from_events,
//! answers_from_events}`）で events から組み立て、ワーカープロトコルの型
//! （`task_worker::{PriorReview, Answer}`）へ写す（ADR-0013 D7）。
//!
//! route はディスパッチャと同じ純関数 `task_dispatch::route_select` で決める（ADR 2026-10-11
//! llm-source-harness-selection）。`--harness` はタスクの harness 欄の代わり、`--model` は model の
//! 指定、`--source` / `--account` は候補を絞るだけ。admission の状態は宣言とログイン済み account の
//! 走査から組み、実行中の数・cooldown は見ない（読むだけ）。実行器は `[harnesses.<id>]` の宣言から
//! 組んだアダプタに route を当てたもの（`task_worker::route::route_adapter`）。
//! 作業ディレクトリは `--workspace` があればそれ、無ければタスクの `WorkspaceSpec::Local`
//! （相対なら task の root 基準。root は `Config::workspace_roots` の resolver が解く）。タスクが `running`/`reviewing` のときは、デーモンの
//! run と作業ディレクトリを取り合うため `--workspace` 指定なしでは拒否する。
//!
//! `--cluster <id>`（ADR-0018 受け入れ 10）: `[[clusters]] id` を指定すると、そのクラスタ側の
//! ディレクトリに対して 1 回 run する（デーモンを介さない動作確認）。DB はここでも変えない。
//! 多重接続（`ControlMaster`）が無ければ何も実行せず exit 4 で理由を返す。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use celeris::Config;
use clap::{Args, Subcommand};
use task_core::llm_route::{AccountId, ModelId, Registry, SourceId};
use task_core::{ArtifactRef, Event, Status, Task, TaskId, TaskStore, WorkspaceSpec};
use task_dispatch::{ClusterSpec, RouteInput, RouteState};
use task_ops::derive::{AnswerNote, ReviewNote, answers_from_events, prior_review_from_events};
use task_worker::{
    AdapterError, Answer, EventSink, LocalWorkspace, PROTOCOL_VERSION, PriorReview,
    ProviderFailure, RunContext, RunLimits, RunOutcome, RunRequest, SshWorkspace, Terminal,
    WorkerAdapter, WorkerMessage, Workspace, WorkspaceError, remote_exec_instructions,
};

use task_worker::protocol::SelectedLlmRoute;

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum WorkerCommand {
    /// 1 タスクを 1 つの route（harness・source・model・account）で 1 回だけ実行する。
    Run(WorkerRunArgs),
}

#[derive(Args, Debug)]
pub struct WorkerRunArgs {
    /// `config.toml`。
    /// P-54: 省略時は環境変数 `CELERIS_CONFIG` を見る。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: PathBuf,

    /// 実行するタスクの ID。
    #[arg(long)]
    pub task: String,

    /// 使う harness（`[harnesses.<id>]`）。省略時はタスクの harness 欄と `[harness_selection]` で決める。
    #[arg(long)]
    pub harness: Option<String>,

    /// 使う LLM source（`[llm_sources.<id>]`）。`route_defaults` の候補をこの source に絞る。
    #[arg(long)]
    pub source: Option<String>,

    /// 使う model。宣言された候補のうちこの model を出すものに絞る。
    #[arg(long)]
    pub model: Option<String>,

    /// 作業ディレクトリ。省略時はタスクの workspace（相対なら task の root 基準）。
    /// `--cluster` と併用時は「クラスタ側」のパス（`celerisctl add --cluster` と同じ意味）。
    #[arg(long)]
    pub workspace: Option<PathBuf>,

    /// クラスタ ID（`[[clusters]] id`）。指定すると、クラスタ側のディレクトリに対して 1 回だけ
    /// run する（pull → run → push。ADR-0018 D1/D4）。デーモンを介さない動作確認で、DB は変えない。
    /// 多重接続（`ControlMaster`）が無ければ何も実行せず exit 4 で理由を返す。
    #[arg(long)]
    pub cluster: Option<String>,

    /// 使う account（OAuth source ならログイン済みの account ディレクトリ、それ以外は source に宣言した
    /// account）。省略時は宣言順・ログイン済みの最初の account。
    #[arg(long)]
    pub account: Option<String>,
}

/// `--cluster` を解決した先（ADR-0018 受け入れ 10）。
#[derive(Debug)]
struct ClusterTarget {
    spec: ClusterSpec,
    /// クラスタ側の作業ディレクトリ。
    remote_path: PathBuf,
    /// 手元の写し（デーモンと同じ `<task の root>/<task_id>`）。
    mirror_dir: PathBuf,
    /// タスクの `WorkspaceSpec::Remote{cluster}` と `--cluster` が食い違うときの警告（呼び出し側が stderr に出す）。
    warning: Option<String>,
}

/// `--cluster <id>` の解決（純関数）。`config.cluster_specs()` に無ければエラー。クラスタ側パスは
/// `--workspace`（あれば）かタスクの `WorkspaceSpec::Remote{path}`。`Local` タスクに `--workspace` が
/// 無ければエラー。手元の写しはデーモンと共有するため、タスクが `running`/`reviewing` なら
/// `--workspace` の有無に関係なく拒否する。
fn resolve_cluster_target(
    store: &dyn TaskStore,
    config: &Config,
    task: &Task,
    cluster_id: &str,
    workspace_arg: Option<&Path>,
) -> Result<ClusterTarget, CliError> {
    let mut specs = config.cluster_specs();
    let spec = specs
        .remove(cluster_id)
        .ok_or_else(|| CliError::msg(format!("cluster not found in config: {cluster_id}")))?;

    if matches!(task.status, Status::Running | Status::Reviewing) {
        return Err(CliError::msg(format!(
            "task {} is {:?}; stop celeris or wait before `worker run --cluster`",
            task.id, task.status
        )));
    }

    let mut warning = None;
    let remote_path = if let Some(path) = workspace_arg {
        if let WorkspaceSpec::Remote { cluster, .. } = &task.workspace
            && cluster != cluster_id
        {
            warning = Some(format!(
                "task {} workspace targets cluster {cluster:?}; overriding with --cluster {cluster_id:?}",
                task.id
            ));
        }
        path.to_path_buf()
    } else {
        match &task.workspace {
            WorkspaceSpec::Remote { cluster, path, .. } => {
                if cluster != cluster_id {
                    warning = Some(format!(
                        "task {} workspace targets cluster {cluster:?}; overriding with --cluster {cluster_id:?}",
                        task.id
                    ));
                }
                path.clone()
            }
            WorkspaceSpec::Local { .. } => {
                return Err(CliError::msg(format!(
                    "task {} has a local workspace; pass --workspace <cluster-side path> with --cluster",
                    task.id
                )));
            }
        }
    };

    let mirror_dir = config.workspace_roots().task_dir(store, &task.id)?;
    Ok(ClusterTarget {
        spec,
        remote_path,
        mirror_dir,
        warning,
    })
}

/// Admission state read from the declarations only: OAuth sources offer their logged-in account
/// directories (narrowed to the declared accounts, if any), other sources their declared accounts.
/// `--source` / `--account` narrow it. In-flight runs and cooldowns are not consulted.
fn route_state(config: &Config, registry: &Registry, args: &WorkerRunArgs) -> RouteState {
    let mut state = RouteState::default();
    for (source_id, spec) in &registry.sources {
        if args
            .source
            .as_deref()
            .is_some_and(|s| s != source_id.as_ref())
        {
            continue;
        }
        let mut accounts: Vec<AccountId> = spec.accounts.clone();
        if let Some(adapter) = task_dispatch::account_adapter_for_source_kind(spec.kind.as_ref()) {
            let logged_in: Vec<AccountId> = config
                .accounts
                .as_ref()
                .and_then(|a| a.root_for(adapter))
                .map(|root| task_dispatch::scan_accounts(root, adapter))
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d.logged_in)
                .map(|d| AccountId::from(d.id.as_str()))
                .collect();
            accounts = if accounts.is_empty() {
                logged_in
            } else {
                accounts
                    .into_iter()
                    .filter(|a| logged_in.contains(a))
                    .collect()
            };
        }
        if let Some(account) = &args.account {
            accounts.retain(|a| a.as_ref() == account);
        }
        if !accounts.is_empty() {
            state.accounts.available.insert(source_id.clone(), accounts);
        }
    }
    state
}

/// The route this manual run uses: the dispatcher's pure selection over the declared registry.
fn select_worker_route(
    config: &Config,
    registry: &Registry,
    task: &Task,
    args: &WorkerRunArgs,
) -> Result<SelectedLlmRoute, CliError> {
    if let Some(source) = &args.source
        && !registry
            .sources
            .contains_key(&SourceId::from(source.as_str()))
    {
        return Err(CliError::msg(format!(
            "llm source not found in config: {source}"
        )));
    }
    if let Some(account) = &args.account
        && !task_dispatch::valid_account_id(account)
    {
        return Err(CliError::msg(format!("invalid --account id: {account}")));
    }
    let mut harness = task_dispatch::harness_request_for(task);
    if let Some(h) = &args.harness {
        harness.harness = Some(h.as_str().into());
    }
    let input = RouteInput {
        task_id: task.id.to_string(),
        harness,
        model_hint: args.model.as_deref().map(ModelId::from),
        lane: None,
        cos: false,
    };
    let state = route_state(config, registry, args);
    let (outcome, trace) = task_dispatch::route_select(&input, registry, &state);
    let pick = outcome.map_err(|failure| {
        let rejected: Vec<String> = trace
            .rejections
            .iter()
            .map(|r| {
                format!(
                    "{}/{}: {:?}",
                    r.candidate.source, r.candidate.model, r.reason
                )
            })
            .collect();
        CliError::msg(format!(
            "no route for task {}: {} [{}]",
            task.id,
            failure.reason(),
            rejected.join(", ")
        ))
    })?;
    let root = registry
        .sources
        .get(&pick.candidate.source)
        .and_then(|s| task_dispatch::account_adapter_for_source_kind(s.kind.as_ref()))
        .and_then(|a| config.accounts.as_ref()?.root_for(a).cloned());
    task_dispatch::selected_llm_route(registry, &pick, root.as_deref())
        .map_err(|e| CliError::msg(format!("cannot launch route: {e}")))
}

/// A credential reference's value from `[secrets] dir` (never from the protocol payload).
fn read_secret(config: &Config, name: &str) -> Option<String> {
    let path = config.secrets.as_ref()?.dir.join(name);
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.trim_end_matches(['\n', '\r']).to_string())
}

/// The adapter that runs `route`: `[harnesses.<id>]`'s adapter with the route applied.
fn route_launch_adapter(
    config: &Config,
    route: &SelectedLlmRoute,
) -> Result<std::sync::Arc<dyn WorkerAdapter>, CliError> {
    let adapters = celeris::build_harness_adapters(config)
        .map_err(|e| CliError::msg(format!("invalid harness declarations: {e}")))?;
    task_worker::route::route_adapter(&adapters, route, &|name| read_secret(config, name))
        .map_err(|e| CliError::msg(format!("cannot launch route: {e}")))
}

pub fn run_run(store: &dyn TaskStore, args: WorkerRunArgs) -> Result<ExitCode, CliError> {
    let task_id = crate::error::parse_task_id(&args.task)?;
    let config = Config::load(&args.config).map_err(|e| {
        CliError::msg(format!(
            "failed to load config {}: {e}",
            args.config.display()
        ))
    })?;

    let task = store.get(task_id)?.ok_or_else(|| {
        CliError::msg(format!(
            "task not found: {task_id} (check that --db points at the same db as `db` in {})",
            args.config.display()
        ))
    })?;
    let events = store.events_for(task_id)?;

    // Manual adapter probes do not resolve an administrator profile or persist lifecycle.
    // Never silently run a capability-bearing task without its supervisor.
    if task_core::browser::requests_browser(&task.skills) {
        return Err(CliError::msg(
            "browser capability requires dispatcher execution with an administrator profile; manual worker run is unsupported",
        ));
    }

    let registry = config
        .llm_route_registry()
        .map_err(|e| CliError::msg(format!("invalid llm route declarations: {e}")))?;
    let route = select_worker_route(&config, &registry, &task, &args)?;
    let launch = route_launch_adapter(&config, &route)?;
    let adapter = launch.as_ref();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::msg(format!("failed to start tokio runtime: {e}")))?;

    if let Some(cluster_id) = &args.cluster {
        let target =
            resolve_cluster_target(store, &config, &task, cluster_id, args.workspace.as_deref())?;
        if let Some(warning) = &target.warning {
            eprintln!("warning: {warning}");
        }
        return rt.block_on(execute_on_cluster(
            &task, &events, target, &config, &route, adapter,
        ));
    }

    if args.workspace.is_none() && matches!(task.status, Status::Running | Status::Reviewing) {
        return Err(CliError::msg(format!(
            "task {task_id} is {:?}; pass --workspace to avoid racing the daemon's own run",
            task.status
        )));
    }

    let workspace_dir = match &args.workspace {
        Some(dir) => dir.clone(),
        None => match &task.workspace {
            WorkspaceSpec::Local { path, .. } if path.is_relative() => config
                .workspace_roots()
                .task_root(store, &task.id)?
                .join(path),
            WorkspaceSpec::Local { path, .. } => path.clone(),
            WorkspaceSpec::Remote { .. } => {
                return Err(CliError::msg(
                    "task workspace is remote; pass --workspace to run it locally",
                ));
            }
        },
    };

    rt.block_on(execute(
        &task,
        &events,
        workspace_dir,
        &config,
        &route,
        adapter,
    ))
}

async fn execute(
    task: &Task,
    events: &[(u64, Event)],
    workspace_dir: PathBuf,
    config: &Config,
    route: &SelectedLlmRoute,
    adapter: &dyn WorkerAdapter,
) -> Result<ExitCode, CliError> {
    let prepared = LocalWorkspace::new(workspace_dir)
        .prepare(task)
        .await
        .map_err(|e| CliError::msg(format!("failed to prepare workspace: {e}")))?;

    let run_id = TaskId::new().to_string();
    // ADR-0036 D1: 成果物ディレクトリはタスクごと（共有 workspace の子は `.taskd/artifacts/<task_id>/`）。
    let artifacts_dir = task_core::artifacts::artifacts_dir_for(task, &prepared);
    let req = RunRequest {
        llm_route: Some(route.clone()),
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: task.clone(),
        workspace: prepared.clone(),
        // `celerisctl worker run` は人が指定した（か DB の）ディレクトリでそのまま動かす（ADR-0041 D1 の worktree は
        // ディスパッチャが用意するもので、手動 run では切らない）。
        work_dir: None,
        artifacts_dir,
        context: RunContext {
            prior_review: to_prior_review(prior_review_from_events(events)),
            inputs: task.inputs.clone(),
            answers: to_answers(answers_from_events(events)),
            review: None,
            role: None,
            children: Vec::new(),
            available_genres: Vec::new(),
            // `celerisctl worker run` は DB を変えない手動実行なので、役職・記憶・やり取り・組織図は渡さない
            // （ADR-0033 D4 / D6: 記憶の追記はディスパッチャの仕事）。
            ..RunContext::default()
        },
    };

    outln!(
        "worker run: task={} harness={} source={} model={} account={} workspace={} run_id={run_id}",
        task.id,
        route.harness_id,
        route.source_id,
        route.model_id,
        route.account_id,
        prepared.display(),
    );

    let limits = RunLimits {
        wall_clock: Duration::from_secs(task.budget.max_wall_secs),
        idle_timeout: Duration::from_secs(config.idle_timeout_secs),
        kill_grace: Duration::from_secs(config.kill_grace_secs),
    };

    let sink = PrintSink;
    // 監査の指摘: シグナルの既定動作で celerisctl が死ぬと drop が走らず、アダプタの子プロセス（別プロセスグループなので端末の
    // Ctrl-C も届かない）が kill されずに作業ディレクトリを編集し続ける。SIGINT / SIGTERM を受けたら run の future を drop して
    // 子を kill（`kill_on_drop`）し、exit 130 で終わる。
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| CliError::msg(format!("failed to install SIGTERM handler: {e}")))?;
    let finished = tokio::select! {
        result = adapter.run(req, &run_id, limits, &sink) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
        _ = sigterm.recv() => None,
    };
    let Some(result) = finished else {
        eprintln!("worker run interrupted; the worker process was killed");
        return Ok(ExitCode::from(130));
    };
    let (message, exit) = normalize_outcome(result);
    let json = serde_json::to_string(&message)
        .map_err(|e| CliError::msg(format!("failed to encode result: {e}")))?;
    outln!("result: {json}");
    Ok(ExitCode::from(exit))
}

/// `--cluster <id>`: クラスタ側の作業ディレクトリに対して 1 回だけ run する（ADR-0018 受け入れ 10）。
/// `SshWorkspace` を経由すること以外は `execute` と同じ（DB は変えない。リースも取らない）。
async fn execute_on_cluster(
    task: &Task,
    events: &[(u64, Event)],
    target: ClusterTarget,
    config: &Config,
    route: &SelectedLlmRoute,
    adapter: &dyn WorkerAdapter,
) -> Result<ExitCode, CliError> {
    // ADR-0059 D1: タスクの `WorkspaceSpec::Remote.mode`（`--workspace` で上書きしたパスでも、
    // クラスタ側の同期方針はタスクに書いてある `mode` にそのまま従う）。
    let settings =
        target
            .spec
            .ssh_settings(&target.remote_path, task.id, task.workspace.remote_mode());
    let ws = SshWorkspace::new(&target.mirror_dir, settings.clone());

    if !ws.control_master_alive().await {
        return unreachable_result(format!(
            "no ssh ControlMaster connection to {} (host {}); run scripts/cluster-login.sh {}",
            target.spec.id, target.spec.host, target.spec.host
        ));
    }

    let prepared = match ws.prepare(task).await {
        Ok(p) => p,
        Err(WorkspaceError::Unreachable(msg)) => return unreachable_result(msg),
        Err(e) => return Err(CliError::msg(format!("failed to prepare workspace: {e}"))),
    };
    ws.write_remote_exec_helper()
        .await
        .map_err(|e| CliError::msg(format!("failed to write .celeris/remote-exec: {e}")))?;

    // ADR-0018 D3: DB のタスクは変えない。渡す写しの `objective` だけにラッパの使い方を足す。
    let mut run_task = task.clone();
    run_task
        .objective
        .push_str(&remote_exec_instructions(&settings));

    let run_id = TaskId::new().to_string();
    // ADR-0036 D1: Remote の写しは `<root>/<task_id>` でタスクごとなので `<写し>/artifacts`。
    let artifacts_dir = task_core::artifacts::artifacts_dir_for(&run_task, &prepared);
    let req = RunRequest {
        llm_route: Some(route.clone()),
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: run_task,
        workspace: prepared.clone(),
        // `celerisctl worker run` は人が指定した（か DB の）ディレクトリでそのまま動かす（ADR-0041 D1 の worktree は
        // ディスパッチャが用意するもので、手動 run では切らない）。
        work_dir: None,
        artifacts_dir,
        context: RunContext {
            prior_review: to_prior_review(prior_review_from_events(events)),
            inputs: task.inputs.clone(),
            answers: to_answers(answers_from_events(events)),
            review: None,
            role: None,
            children: Vec::new(),
            available_genres: Vec::new(),
            // `celerisctl worker run` は DB を変えない手動実行なので、役職・記憶・やり取り・組織図は渡さない
            // （ADR-0033 D4 / D6: 記憶の追記はディスパッチャの仕事）。
            ..RunContext::default()
        },
    };

    outln!(
        "worker run: task={} harness={} source={} model={} account={} workspace={} run_id={run_id} cluster={} remote={}:{}",
        task.id,
        route.harness_id,
        route.source_id,
        route.model_id,
        route.account_id,
        prepared.display(),
        target.spec.id,
        target.spec.host,
        target.remote_path.display(),
    );

    let limits = RunLimits {
        wall_clock: Duration::from_secs(task.budget.max_wall_secs),
        idle_timeout: Duration::from_secs(config.idle_timeout_secs),
        kill_grace: Duration::from_secs(config.kill_grace_secs),
    };

    let sink = PrintSink;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| CliError::msg(format!("failed to install SIGTERM handler: {e}")))?;
    let finished = tokio::select! {
        result = adapter.run(req, &run_id, limits, &sink) => Some(result),
        _ = tokio::signal::ctrl_c() => None,
        _ = sigterm.recv() => None,
    };
    let Some(result) = finished else {
        eprintln!("worker run interrupted; the worker process was killed");
        return Ok(ExitCode::from(130));
    };

    // ADR-0018 D4: run が終わった（アダプタ自体が Ok を返した）ら、手元の編集をクラスタへ push する。
    if result.is_ok() {
        match ws.push().await {
            Ok(()) => outln!(
                "progress: pushed to cluster {}:{}",
                target.spec.id,
                target.remote_path.display()
            ),
            Err(WorkspaceError::Unreachable(msg)) => return unreachable_result(msg),
            Err(e) => return Err(CliError::msg(format!("failed to push to cluster: {e}"))),
        }
    }

    let (message, exit) = normalize_outcome(result);
    let json = serde_json::to_string(&message)
        .map_err(|e| CliError::msg(format!("failed to encode result: {e}")))?;
    outln!("result: {json}");
    Ok(ExitCode::from(exit))
}

/// ssh / rsync 自体が失敗した（多重接続が無い等）ときの表示（ADR-0018 D5: 供給側失敗と同じ形）。
/// アダプタは起動しない／起動できなかったので、`WorkerMessage::Error{retryable: true}` を出して exit 4。
fn unreachable_result(message: String) -> Result<ExitCode, CliError> {
    let msg = WorkerMessage::Error {
        message,
        retryable: true,
        provider_failure: None,
        usage: None,
    };
    let json = serde_json::to_string(&msg)
        .map_err(|e| CliError::msg(format!("failed to encode result: {e}")))?;
    outln!("result: {json}");
    Ok(ExitCode::from(4))
}

/// `task_ops::derive::ReviewNote` をワーカープロトコルの `task_worker::PriorReview` に写す
/// （ADR-0013 D7: task-ops は task_worker に依存しないため、この写像は呼び出し側で行う）。
fn to_prior_review(notes: Vec<ReviewNote>) -> Vec<PriorReview> {
    notes
        .into_iter()
        .map(|n| PriorReview {
            criterion: n.criterion,
            pass: n.pass,
            reason: n.reason,
        })
        .collect()
}

/// `task_ops::derive::AnswerNote` をワーカープロトコルの `task_worker::Answer` に写す。
fn to_answers(notes: Vec<AnswerNote>) -> Vec<Answer> {
    notes
        .into_iter()
        .map(|n| Answer {
            question: n.question,
            answer: n.answer,
        })
        .collect()
}

/// アダプタの結果を、出す `WorkerMessage` と exit code に正規化する（ADR-0012 D4 の表）。
fn normalize_outcome(result: Result<RunOutcome, AdapterError>) -> (WorkerMessage, u8) {
    match result {
        Ok(outcome) => match outcome.terminal {
            Terminal::Done {
                summary,
                evidence,
                usage,
            } => (
                WorkerMessage::Done {
                    summary,
                    evidence,
                    usage,
                },
                0,
            ),
            Terminal::Question { text } => (WorkerMessage::Question { text }, 3),
            Terminal::Error {
                message, retryable, ..
            } => (
                WorkerMessage::Error {
                    message,
                    retryable,
                    provider_failure: None,
                    usage: None,
                },
                4,
            ),
            // ADR-0072 D7/D9（Phase E1）: `celerisctl worker run` は単発の CLI 呼び出しで、
            // continuation の対象外（daemon の仕組みなので）。`done` と同じ exit 0 で終わる
            // （yield は「自分から区切った」正常終了）。
            Terminal::Yielded { checkpoint, usage } => {
                (WorkerMessage::Yielded { checkpoint, usage }, 0)
            }
            // 予算切れは `error{retryable:true}` と同じ exit code（4）にする。
            Terminal::BudgetExhausted {
                kind,
                message,
                usage,
            } => (
                WorkerMessage::BudgetExhausted {
                    kind,
                    message,
                    usage,
                },
                4,
            ),
            // ADR-0090 D1: 単発の CLI 呼び出しは job を待たない（daemon の仕組み）。wait の行を出して exit 0。
            Terminal::Waiting {
                request,
                checkpoint,
                usage,
            } => (
                WorkerMessage::Wait {
                    kind: task_core::cluster_job::WAIT_KIND_CLUSTER_JOB.to_string(),
                    cluster: request.cluster,
                    scheduler: Some(request.scheduler),
                    jobs: request.jobs,
                    poll_secs: request.poll_secs,
                    timeout_secs: request.timeout_secs,
                    checkpoint,
                    summary: request.summary,
                    usage,
                },
                0,
            ),
        },
        Err(e) => {
            let provider_failure = match &e {
                AdapterError::Throttled { retry_after } => Some(ProviderFailure::Throttled {
                    retry_after_secs: retry_after.as_secs(),
                }),
                AdapterError::AuthFailed(_) => Some(ProviderFailure::AuthFailed),
                AdapterError::Exhausted(_) => Some(ProviderFailure::Exhausted),
                _ => None,
            };
            (
                WorkerMessage::Error {
                    message: format!("adapter: {e}"),
                    retryable: true,
                    provider_failure,
                    usage: None,
                },
                4,
            )
        }
    }
}

/// `progress` / `artifact` を逐次標準出力に出すシンク（ADR-0012 D4）。ストアには何も書かない。
#[derive(Debug, Default, Clone, Copy)]
struct PrintSink;

impl EventSink for PrintSink {
    fn progress(&self, msg: &str) {
        outln!("progress: {msg}");
    }

    fn artifact(&self, artifact: &ArtifactRef) {
        outln!(
            "artifact: {} {} sha256={}",
            artifact.name,
            artifact.path,
            artifact.sha256
        );
    }

    /// ADR-0016 D2: `worker run` は DB を変えないので、提案は表示するだけで子タスクは挿入しない。
    fn delegate(&self, tasks: &[task_core::DelegateTask]) {
        let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
        outln!(
            "delegate: {} task(s) proposed (not inserted; worker run does not write the DB): {}",
            tasks.len(),
            titles.join(", ")
        );
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
