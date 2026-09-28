//! ADR-0075 D2 / D6 / D7（Phase G1）: `celerisctl scratch {status,gc,lease,touch,release,env}`。
//!
//! - `lease` / `touch` / `release` / `env`: 外部の owner（`release-<sha12>` の release ゲート、`agent-<name>` の実装エージェント）の
//!   lease。daemon の GC と同じ `.lock` で直列化する。**DB は開かない**。
//! - `status [--json]` / `gc [--dry-run]`: pool を走査して分類する（DB があれば daemon と同じ規則。無ければ daemon 由来の owner は
//!   消さない）。`status --json` は `GET /api/v1/metrics/scratch` と同じ `celeris.scratch-status/1`。
//!
//! `env` が出す値は dispatcher と同じ関数（`task_worker::scratch::cargo_env`）で組む（D4 の「env は一か所で組む」）。
//! Phase G2: `env` には `[scratch.cargo]` と、sccache の server が応答すれば sccache 系も入る。`env --server` は
//! `celeris-sccache.service` が server を起こすときの env（`SCCACHE_DIR` など、client と同じ値）。`status` は sccache の
//! 配線の状態と `sccache --show-stats` の要約（hit / miss / サイズ）も出す。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use celeris::Config;
use clap::{Args, Subcommand};
use task_dispatch::scratch_gc::{self, Measured};
use task_worker::scratch::{self, AdoptCandidate, AllocateRequest, Owner, ScratchSettings};

use crate::error::CliError;
use crate::outln;

#[derive(Debug, Subcommand)]
pub enum ScratchCommand {
    /// pool の使用量・owner ごとの分類・legacy の残り（`--json` は `GET /api/v1/metrics/scratch` と同じ schema）。
    Status(StatusArgs),
    /// daemon と同じ `plan_gc` で回収する（daemon が止まっている間の道具。`--dry-run` は一覧だけ）。
    Gc(GcArgs),
    /// 外部の owner の lease を作る（あれば touch）。`CARGO_TARGET_DIR` のパスを標準出力に出す。
    Lease(LeaseArgs),
    /// lease の mtime を今にする（長いビルドの前に）。
    Touch(OwnerArgs),
    /// lease に `released_at` を書く（P3 に落とす）。
    Release(OwnerArgs),
    /// dispatcher と同じ env を `export NAME=value` で出す（`--repo` を渡せば lease も作る）。`--server` は
    /// sccache の server の env（`celeris-sccache.service` 用。owner は要らない）。
    Env(EnvArgs),
}

#[derive(Debug, Args)]
pub struct ConfigArg {
    /// `config.toml`。省略時は `CELERIS_CONFIG`。
    #[arg(long, env = "CELERIS_CONFIG")]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub config: ConfigArg,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct GcArgs {
    #[command(flatten)]
    pub config: ConfigArg,
    #[arg(long)]
    pub dry_run: bool,
}

/// owner は位置引数でも `--owner` でもよい。
#[derive(Debug, Args)]
pub struct OwnerArgs {
    #[command(flatten)]
    pub config: ConfigArg,
    /// `release-<sha12>` / `agent-<name>`（`task-*` は daemon の owner）。
    #[arg(value_name = "OWNER")]
    pub owner_pos: Option<String>,
    #[arg(long = "owner")]
    pub owner: Option<String>,
}

#[derive(Debug, Args)]
pub struct LeaseArgs {
    #[command(flatten)]
    pub owner: OwnerArgs,
    /// ビルドするリポジトリ（`repo_key` の元。adopt の候補選びにも使う）。
    #[arg(long)]
    pub repo: PathBuf,
    /// ビルドする作業ツリー（checkout 時刻 = `<worktree>/.git` の mtime を adopt の安全条件に使う）。省略時は
    /// `--repo` が worktree（`.git` がファイル）ならそれ、違えば adopt しない。
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    /// base commit（adopt の距離に使う）。
    #[arg(long)]
    pub base: Option<String>,
    /// この lease の TTL（秒）。省略時は `[scratch] external_lease_ttl_secs`。
    #[arg(long)]
    pub ttl: Option<u64>,
}

#[derive(Debug, Args)]
pub struct EnvArgs {
    #[command(flatten)]
    pub owner: OwnerArgs,
    /// 渡せば lease も作る（`lease` と同じ）。
    #[arg(long)]
    pub repo: Option<PathBuf>,
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    #[arg(long)]
    pub base: Option<String>,
    #[arg(long)]
    pub ttl: Option<u64>,
    /// sccache の server の env（`SCCACHE_DIR` / `SCCACHE_CACHE_SIZE` / `SCCACHE_SERVER_PORT` / `SCCACHE_IDLE_TIMEOUT` と
    /// 本物のバイナリ `CELERIS_SCCACHE_BIN`）を出す。sccache が無効・バイナリが無ければ失敗する。
    #[arg(long, conflicts_with_all = ["repo", "worktree", "base", "ttl"])]
    pub server: bool,
}

fn owner_of(args: &OwnerArgs) -> Result<Owner, CliError> {
    let raw = match (&args.owner_pos, &args.owner) {
        (Some(a), Some(b)) if a != b => {
            return Err(CliError::msg(format!("owner given twice: {a} / {b}")));
        }
        (Some(a), _) | (None, Some(a)) => a.clone(),
        (None, None) => return Err(CliError::msg("owner is required (e.g. agent-<name>)")),
    };
    Owner::parse(&raw).map_err(CliError::msg)
}

fn load(config: &Path) -> Result<Config, CliError> {
    Config::load(config).map_err(|e| CliError::msg(format!("config: {e}")))
}

/// 有効な scratch の設定（無効なら理由つきで失敗。release.sh は失敗を見て従来の target に戻る）。
fn active_settings(cfg: &Config) -> Result<ScratchSettings, CliError> {
    let s = cfg.scratch_settings();
    if !s.enabled {
        return Err(CliError::msg(format!(
            "scratch is disabled{}",
            s.disabled_reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        )));
    }
    Ok(s)
}

fn shell_quote(v: &str) -> String {
    if !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '='))
    {
        v.to_string()
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

fn render_exports(env: Vec<(String, String)>) -> String {
    env.into_iter()
        .map(|(k, v)| format!("export {k}={}\n", shell_quote(&v)))
        .collect()
}

/// `export NAME=value` の行（dispatcher と同じ `cargo_env`）。
pub fn render_env(settings: &ScratchSettings, owner: &Owner) -> String {
    render_exports(scratch::cargo_env(settings, owner))
}

/// `env --server`: sccache の server の env と本物のバイナリ。server は起こさない。Phase G3: cache server
/// （`celeris cache-server`）が `/healthz` に応答すれば webdav（`SCCACHE_WEBDAV_*`）、でなければ G2 の local disk
/// （`SCCACHE_DIR`。U5: backend が応答しないと sccache の server は起動に失敗するため）。選んだ方を
/// `<scratch>/bin/sccache-server.mode` に書く（dispatcher は webdav のときだけ cache server の応答も確かめる）。
pub fn render_server_env(settings: &ScratchSettings) -> Result<String, CliError> {
    // unit の起動順（After=celeris-scratch-cache.service）では listen の前に来うるので、少しだけ待つ。
    let cache_up = |port: u16| {
        (0..5).any(|i| {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            scratch::cache_server_healthy(port)
        })
    };
    render_server_env_with(settings, cache_up)
}

/// `render_server_env` の本体（`cache_up` はテストで差し替える）。
pub fn render_server_env_with(
    settings: &ScratchSettings,
    cache_up: impl Fn(u16) -> bool,
) -> Result<String, CliError> {
    let state = scratch::resolve_sccache_with(settings, |_| true, |_| true);
    if let Some(reason) = state.reason() {
        return Err(CliError::msg(format!(
            "sccache is {}: {reason}",
            state.label()
        )));
    }
    let backend = scratch::choose_sccache_backend(settings, cache_up);
    if let Err(e) = scratch::write_sccache_mode(&settings.pool(), backend.mode()) {
        return Err(CliError::msg(format!(
            "could not record the sccache backend: {e}"
        )));
    }
    let mut env = scratch::sccache_server_env_for(settings, &backend);
    env.push((
        "CELERIS_SCCACHE_BIN".to_string(),
        settings.sccache.binary.display().to_string(),
    ));
    let note = match &backend {
        scratch::SccacheBackend::Webdav { endpoint, .. } => {
            format!("# celeris: sccache backend = webdav {endpoint} (ADR-0075 D5)\n")
        }
        scratch::SccacheBackend::Disk { reason } => format!(
            "# celeris: sccache backend = disk {}{}\n",
            settings.pool().l1_dir().display(),
            reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ),
    };
    Ok(format!("{note}{}", render_exports(env)))
}

/// 外部の owner の lease を取る（adopt は同じ repo の P3 の外部 owner の target から。DB は見ない）。
fn take_lease(
    settings: &ScratchSettings,
    owner: &Owner,
    repo: &Path,
    worktree: Option<&Path>,
    base: Option<String>,
    ttl: Option<u64>,
) -> Result<scratch::Allocation, CliError> {
    if owner.kind().is_daemon_owned() {
        return Err(CliError::msg(format!(
            "{owner} is a daemon owner; external leases are release-<sha12> or agent-<name>"
        )));
    }
    let now = SystemTime::now();
    let scan = scratch_gc::scan(settings, &[], &scratch::NoDb, false, &HashMap::new(), now);
    // ADR-0075 D3 の安全条件: 引き継ぐ側の checkout 時刻（worktree の `.git` の mtime）より前に最後に書かれた
    // target だけを引き継ぐ。checkout 時刻が分からない（`--repo` が worktree でなく `--worktree` も無い）なら adopt しない。
    let tree = worktree.unwrap_or(repo);
    let checkout = std::fs::symlink_metadata(tree.join(".git"))
        .ok()
        .filter(|m| m.file_type().is_file())
        .and_then(|_| scratch::checkout_time(tree));
    let candidates: Vec<AdoptCandidate> = scan
        .candidates
        .into_iter()
        .filter(|c| !c.owner.kind().is_daemon_owned())
        .collect();
    let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let base_for_distance = base.clone();
    let repo_for_distance = repo.clone();
    let distance = move |c: &AdoptCandidate| match (&base_for_distance, &c.base_commit) {
        (Some(a), Some(b)) => scratch_gc::commit_distance(&repo_for_distance, a, b),
        _ => None,
    };
    let alloc = scratch::allocate(
        &settings.pool(),
        &AllocateRequest {
            owner,
            repo_path: &repo,
            base_commit: base,
            work_unit_key: None,
            checkout,
            candidates: &candidates,
            distance: &distance,
            adopt: settings.adopt,
            max_distance: settings.adopt_max_distance,
        },
    )
    .map_err(|e| CliError::msg(format!("lease {owner}: {e}")))?;
    if ttl.is_some() {
        scratch::set_ttl(&settings.pool(), owner, ttl)
            .map_err(|e| CliError::msg(format!("lease {owner}: {e}")))?;
    }
    if let Some(from) = &alloc.adopted_from {
        eprintln!("scratch: {owner} adopted the target of {from}");
    }
    Ok(alloc)
}

fn db_path(cli_db: Option<PathBuf>, cfg: &Config) -> PathBuf {
    cli_db
        .or_else(|| std::env::var_os("CELERIS_DB").map(PathBuf::from))
        .unwrap_or_else(|| cfg.db.path.clone())
}

/// status / gc の測定（CLI は daemon の測定 cache を持たないので、legacy と lease の無いものだけその場で測る）。
fn measure_unleased(settings: &ScratchSettings, legacy: &[PathBuf]) -> HashMap<PathBuf, Measured> {
    let now = SystemTime::now();
    let scan = scratch_gc::scan(
        settings,
        legacy,
        &scratch::NoDb,
        false,
        &HashMap::new(),
        now,
    );
    let mut out = HashMap::new();
    let mut targets: Vec<PathBuf> = legacy.to_vec();
    for row in &scan.owners {
        if row.lease.is_none() && row.has_target {
            targets.push(row.path.clone());
        }
    }
    for p in targets {
        if let Ok((bytes, last_write)) = scratch::measure_tree(&p) {
            out.insert(
                p,
                Measured {
                    bytes,
                    last_write,
                    measured_at: now,
                },
            );
        }
    }
    out
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / scratch::GIB as f64)
}

pub fn run(cli_db: Option<PathBuf>, command: ScratchCommand) -> Result<ExitCode, CliError> {
    match command {
        ScratchCommand::Lease(args) => {
            let cfg = load(&args.owner.config.config)?;
            let settings = active_settings(&cfg)?;
            let owner = owner_of(&args.owner)?;
            let alloc = take_lease(
                &settings,
                &owner,
                &args.repo,
                args.worktree.as_deref(),
                args.base,
                args.ttl,
            )?;
            outln!("{}", alloc.target_dir.display());
            Ok(ExitCode::SUCCESS)
        }
        ScratchCommand::Touch(args) => {
            let cfg = load(&args.config.config)?;
            let settings = active_settings(&cfg)?;
            let owner = owner_of(&args)?;
            if scratch::touch(&settings.pool(), &owner)
                .map_err(|e| CliError::msg(format!("touch {owner}: {e}")))?
            {
                Ok(ExitCode::SUCCESS)
            } else {
                Err(CliError::msg(format!("no lease for {owner}")))
            }
        }
        ScratchCommand::Release(args) => {
            let cfg = load(&args.config.config)?;
            let settings = active_settings(&cfg)?;
            let owner = owner_of(&args)?;
            if scratch::release(&settings.pool(), &owner)
                .map_err(|e| CliError::msg(format!("release {owner}: {e}")))?
            {
                outln!("released {owner}");
            } else {
                outln!("no lease for {owner}");
            }
            Ok(ExitCode::SUCCESS)
        }
        ScratchCommand::Env(args) => {
            let cfg = load(&args.owner.config.config)?;
            let settings = active_settings(&cfg)?;
            if args.server {
                let text = render_server_env(&settings)?;
                outln!("{}", text.trim_end());
                return Ok(ExitCode::SUCCESS);
            }
            let owner = owner_of(&args.owner)?;
            if let Some(repo) = &args.repo {
                take_lease(
                    &settings,
                    &owner,
                    repo,
                    args.worktree.as_deref(),
                    args.base,
                    args.ttl,
                )?;
            }
            let text = render_env(&settings, &owner);
            outln!("{}", text.trim_end());
            Ok(ExitCode::SUCCESS)
        }
        ScratchCommand::Status(args) => {
            let cfg = load(&args.config.config)?;
            let status = status_of(&cfg, cli_db)?;
            if args.json {
                let body = serde_json::to_string_pretty(&status)
                    .map_err(|e| CliError::msg(format!("json: {e}")))?;
                outln!("{body}");
            } else {
                print_status(&status);
            }
            Ok(ExitCode::SUCCESS)
        }
        ScratchCommand::Gc(args) => {
            let cfg = load(&args.config.config)?;
            let settings = active_settings(&cfg)?;
            let text = gc(&cfg, &settings, cli_db, args.dry_run)?;
            outln!("{}", text.trim_end());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn with_lookup<T>(
    cfg: &Config,
    cli_db: Option<PathBuf>,
    f: impl FnOnce(&dyn scratch::StatusLookup, bool) -> T,
) -> T {
    let path = db_path(cli_db, cfg);
    if path.exists()
        && let Ok(store) = task_core::SqliteStore::open(&path)
    {
        return f(&scratch::StoreLookup(&store), true);
    }
    f(&scratch::NoDb, false)
}

/// `status --json` の中身（`GET /api/v1/metrics/scratch` と同じ型）。daemon の直近の GC の記録は持たないので `last_gc` は `None`。
pub fn status_of(
    cfg: &Config,
    cli_db: Option<PathBuf>,
) -> Result<task_ops::daemon::ScratchStatus, CliError> {
    let settings = cfg.scratch_settings();
    let now = SystemTime::now();
    if !settings.enabled {
        return Ok(scratch_gc::disabled_status(&settings, now));
    }
    let legacy = scratch_gc::legacy_paths(
        &cfg.workspace.build_cache_dir,
        Some(&cfg.selfdeploy.releases_dir),
    );
    let sizes = measure_unleased(&settings, &legacy);
    let min_free = cfg.dispatch.min_free_disk_mb.saturating_mul(1024 * 1024);
    let mut status = with_lookup(cfg, cli_db, |lookup, has_db| {
        let run = scratch_gc::run_gc(
            &settings,
            &legacy,
            lookup,
            has_db,
            &sizes,
            min_free,
            false,
            &BTreeSet::new(),
            true,
        );
        scratch_gc::build_status(
            &settings, &run.scan, &run.plan, run.fs, min_free, &sizes, None, now,
        )
    });
    // ADR-0075 D6（Phase G2）: sccache の配線の状態と、server が居れば `--show-stats` の要約。
    let state = scratch::resolve_sccache(&settings, scratch::server_listening);
    let mut view = scratch_gc::sccache_view(&settings, &state);
    view.stats = scratch_gc::query_sccache_stats(&settings, &state);
    status.sccache = Some(view);
    // ADR-0075 D6（Phase G3）: cache server の状態と `/stats`（L1 / L2 の hit・使用量・flusher の遅延）。
    status.cache = Some(scratch_gc::cache_view(&settings, true));
    Ok(status)
}

fn gc(
    cfg: &Config,
    settings: &ScratchSettings,
    cli_db: Option<PathBuf>,
    dry_run: bool,
) -> Result<String, CliError> {
    let legacy = scratch_gc::legacy_paths(
        &cfg.workspace.build_cache_dir,
        Some(&cfg.selfdeploy.releases_dir),
    );
    let sizes = measure_unleased(settings, &legacy);
    let min_free = cfg.dispatch.min_free_disk_mb.saturating_mul(1024 * 1024);
    let run = with_lookup(cfg, cli_db, |lookup, has_db| {
        scratch_gc::run_gc(
            settings,
            &legacy,
            lookup,
            has_db,
            &sizes,
            min_free,
            false,
            &BTreeSet::new(),
            dry_run,
        )
    });
    let mut out = String::new();
    out.push_str(&format!(
        "pressure {} · targets {} · pinned {}\n",
        run.plan.pressure.as_str(),
        gb(run.plan.used_bytes),
        gb(run.plan.pinned_bytes)
    ));
    let picks = match &run.executed {
        Some(e) => e.removed.clone(),
        None => run.plan.selected.clone(),
    };
    for p in &picks {
        out.push_str(&format!(
            "{} {} ({}, {}, {})\n",
            if dry_run { "would remove" } else { "removed" },
            p.id,
            if p.seed { "seed" } else { p.class.label() },
            gb(p.estimated_bytes),
            p.why
        ));
    }
    if !dry_run {
        // daemon が止まっていても消えるよう、削除待ちはここで消す（同期）。
        let roots = scratch_gc::deleting_roots(&settings.pool(), &legacy);
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        scratch_gc::spawn_removal(roots.clone(), busy.clone());
        while busy.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    out.push_str(&format!("{} candidate(s)\n", picks.len()));
    Ok(out)
}

/// 小さな量も読める表示（KB / MB / GB）。
fn human(b: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if b < MIB {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else if b < scratch::GIB {
        format!("{:.1} MB", b as f64 / MIB as f64)
    } else {
        gb(b)
    }
}

fn pct(n: u64, d: u64) -> String {
    if d == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", n as f64 * 100.0 / d as f64)
    }
}

/// ADR-0075 D6（Phase G3）: cache server（L1 / L2）の行。
fn print_cache(c: &task_ops::daemon::ScratchCacheView) {
    outln!(
        "cache server {} ({}{}) · sccache backend {}",
        c.endpoint,
        c.state,
        c.reason
            .as_deref()
            .map(|r| format!(": {r}"))
            .unwrap_or_default(),
        c.sccache_mode.as_deref().unwrap_or("-")
    );
    let Some(st) = &c.stats else { return };
    outln!(
        "  gets {} · L1 hit {} · L2 hit {} · miss {} · promotes {} · puts {}",
        st.gets,
        pct(st.l1_hits, st.gets),
        pct(st.l2_hits, st.gets),
        pct(st.misses, st.gets),
        st.promotes,
        st.puts
    );
    outln!(
        "  L1 {} {} / {} ({} entries, evicted {})",
        st.l1_dir,
        human(st.l1_bytes),
        human(st.l1_max_bytes),
        st.l1_entries,
        st.l1_evicted
    );
    outln!(
        "  L2 {} ({}) {} / {} ({} entries, scanned {}) · errors {} · timeouts {} · corrupt {}",
        st.l2_dir.as_deref().unwrap_or("-"),
        st.l2_state,
        st.l2_bytes.map(human).unwrap_or_else(|| "-".to_string()),
        human(st.l2_max_bytes),
        st.l2_entries
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string()),
        st.l2_scanned_at.as_deref().unwrap_or("-"),
        st.l2_errors,
        st.l2_timeouts,
        st.l2_corrupt
    );
    if let Some(since) = &st.l2_degraded_since {
        outln!(
            "  L2 detached since {since} (retry at {}): {}",
            st.l2_retry_at.as_deref().unwrap_or("-"),
            st.l2_last_error.as_deref().unwrap_or("-")
        );
    }
    outln!(
        "  flush queue {} ({}, oldest {}) · last flush {} · written {} ({}) · skipped {} · dropped {} · cap {} MB/s",
        st.flush_queue_len,
        human(st.flush_queue_bytes),
        st.flush_oldest_age_secs
            .map(|s| format!("{s} s"))
            .unwrap_or_else(|| "-".to_string()),
        st.flush_last_at.as_deref().unwrap_or("-"),
        st.flush_written,
        human(st.flush_written_bytes),
        st.flush_skipped_existing,
        st.flush_dropped,
        st.flush_mbps
    );
    if let Some(at) = &st.l2_gc_last_at {
        outln!(
            "  L2 GC {at}: removed {} ({})",
            st.l2_gc_removed,
            human(st.l2_gc_removed_bytes)
        );
    }
}

fn print_status(s: &task_ops::daemon::ScratchStatus) {
    outln!(
        "scratch {} ({})",
        s.dir,
        if s.enabled { "enabled" } else { "disabled" }
    );
    if let Some(reason) = &s.disabled_reason {
        outln!("  reason: {reason}");
    }
    if !s.enabled {
        return;
    }
    if let (Some(total), Some(free)) = (s.fs_total_bytes, s.fs_free_bytes) {
        outln!("filesystem {} · free {}", gb(total), gb(free));
    }
    outln!(
        "targets {} / {} (pinned {}) · effective max {} (configured {}) · pressure {}",
        gb(s.targets_bytes),
        gb(s.targets_max_bytes),
        gb(s.pinned_bytes),
        gb(s.effective_max_bytes),
        gb(s.total_max_bytes),
        s.pressure
    );
    for o in &s.owners {
        outln!(
            "  {:<48} {:<5} {:>9} {} {} {}{}",
            o.owner,
            o.class,
            o.size_bytes.map(gb).unwrap_or_else(|| "-".to_string()),
            o.lease_mtime.as_deref().unwrap_or("-"),
            o.repo_key.as_deref().unwrap_or("-"),
            o.base_commit.as_deref().unwrap_or("-"),
            o.adopted_from
                .as_deref()
                .map(|f| format!(" (adopted from {f})"))
                .unwrap_or_default()
        );
    }
    if !s.legacy.is_empty() {
        outln!("legacy ({}):", s.legacy.len());
        for l in &s.legacy {
            outln!(
                "  {} {} {}",
                l.path,
                l.class,
                l.size_bytes.map(gb).unwrap_or_else(|| "-".to_string())
            );
        }
    }
    if let Some(c) = &s.sccache {
        outln!(
            "sccache L1 {} ({}{}) · {} · port {}",
            c.dir,
            c.state,
            c.reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default(),
            c.binary,
            c.port
        );
        if let Some(st) = &c.stats {
            let rate = |h: u64, m: u64| {
                if h + m == 0 {
                    "-".to_string()
                } else {
                    format!("{:.1}%", h as f64 * 100.0 / (h + m) as f64)
                }
            };
            outln!(
                "  hits {} / misses {} ({}) · rust {} / {} ({}) · size {} / {}",
                st.hits,
                st.misses,
                rate(st.hits, st.misses),
                st.rust_hits,
                st.rust_misses,
                rate(st.rust_hits, st.rust_misses),
                st.cache_size_bytes
                    .map(gb)
                    .unwrap_or_else(|| "-".to_string()),
                gb(c.max_bytes)
            );
        }
    }
    if let Some(c) = &s.cache {
        print_cache(c);
    }
    if let Some(g) = &s.last_gc {
        outln!(
            "last gc {} · {} removed · {}",
            g.at,
            g.removed.len(),
            gb(g.reclaimed_bytes)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tmp: &Path) -> PathBuf {
        let path = tmp.join("config.toml");
        std::fs::write(
            &path,
            format!(
                "db = \"{}\"\n[workspace]\nbuild_cache_dir = \"{}\"\n[scratch]\ndir = \"{}\"\n[scratch.l2]\ndir = \"{}\"\n[scratch.cache_server]\nport = 1\n[selfdeploy]\nreleases_dir = \"{}\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
                tmp.join("celeris.sqlite3").display(),
                tmp.join("build-cache").display(),
                tmp.join("scratch").display(),
                tmp.join("l2").display(),
                tmp.join("releases").display(),
            ),
        )
        .unwrap();
        path
    }

    fn owner_args(config: &Path, owner: &str) -> OwnerArgs {
        OwnerArgs {
            config: ConfigArg {
                config: config.to_path_buf(),
            },
            owner_pos: Some(owner.to_string()),
            owner: None,
        }
    }

    /// ADR-0075 §5 G1 受け入れ条件 6: `celerisctl scratch env` の出力と dispatcher が組む env が同じ。
    #[test]
    fn env_matches_the_dispatcher_env() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = config(tmp.path());
        let cfg = load(&cfg_path).unwrap();
        let settings = active_settings(&cfg).unwrap();
        // dispatcher の設定（`Config::dispatch_config().scratch`）と CLI の設定は同じ値。
        assert_eq!(cfg.dispatch_config().scratch, settings);
        let owner = Owner::parse("agent-a5caa712b0867e383").unwrap();
        let text = render_env(&settings, &owner);
        let parsed: Vec<(String, String)> = text
            .lines()
            .map(|l| {
                let (k, v) = l.strip_prefix("export ").unwrap().split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect();
        assert_eq!(parsed, task_worker::scratch::cargo_env(&settings, &owner));
        assert_eq!(
            parsed[0].1,
            tmp.path()
                .join("scratch/targets/agent-a5caa712b0867e383/target")
                .display()
                .to_string()
        );
        // `--repo` を渡せば lease も作る。
        run(
            None,
            ScratchCommand::Env(EnvArgs {
                owner: owner_args(&cfg_path, "agent-a5caa712b0867e383"),
                repo: Some(tmp.path().to_path_buf()),
                worktree: None,
                base: None,
                ttl: None,
                server: false,
            }),
        )
        .unwrap();
        assert!(settings.pool().lease_path(&owner).exists());
    }

    /// ADR-0075 §5 G3 受け入れ条件 6: `scratch status` は cache server の `/stats`（L1 / L2 の hit・promote・使用量・
    /// flusher の待ち行列と最終 flush 時刻・L2 の状態）を `cache` に入れる。cache server が居なければ `unavailable`。
    #[test]
    fn status_reads_the_cache_server_stats() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("cs/l2")).unwrap();
        let mut sc = scratch_cache::StoreConfig::new(
            tmp.path().join("cs/l1"),
            Some(tmp.path().join("cs/l2")),
        );
        sc.l2_gc_interval = std::time::Duration::ZERO;
        let store = scratch_cache::TieredStore::open(sc).unwrap();
        let k = "ab".repeat(32);
        store.put(&k, b"entry").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while store.queue_len() > 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(store.get(&k).unwrap().into_bytes().is_some());
        let (ptx, prx) = std::sync::mpsc::channel();
        let served = store.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                ptx.send(l.local_addr().unwrap().port()).unwrap();
                let app = scratch_cache::server::router(served, Default::default());
                let _ = scratch_cache::server::serve(l, app, std::future::pending()).await;
            });
        });
        let port = prx.recv().unwrap();
        let cfg_path = config(tmp.path());
        let text = std::fs::read_to_string(&cfg_path).unwrap().replace(
            "[scratch.cache_server]\nport = 1\n",
            &format!("[scratch.cache_server]\nport = {port}\n"),
        );
        std::fs::write(&cfg_path, text).unwrap();
        // 手元の sccache の server（本番の 4236）を拾わない。
        let mut text = std::fs::read_to_string(&cfg_path).unwrap();
        text.push_str("[scratch.sccache]\nport = 1\n");
        std::fs::write(&cfg_path, text).unwrap();
        let cfg = load(&cfg_path).unwrap();
        let status = status_of(&cfg, None).unwrap();
        let cache = status.cache.clone().unwrap();
        assert_eq!(cache.state, "ready", "{cache:?}");
        assert_eq!(cache.endpoint, format!("http://127.0.0.1:{port}"));
        let st = cache.stats.unwrap();
        assert_eq!((st.puts, st.gets, st.l1_hits), (1, 1, 1));
        assert_eq!(st.flush_written, 1);
        assert!(st.flush_last_at.is_some());
        assert_eq!(st.l2_state, "ok");
        assert_eq!(st.flush_queue_len, 0);
        print_status(&status);
        // 同じ形を JSON でも出す（`status --json` と `GET /metrics/scratch` は同じ型）。
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            json["cache"]["stats"]["schema"],
            "celeris.scratch-cache-stats/1"
        );
        // cache server が居なければ unavailable（config の既定の port 1 は閉じた特権 port）。
        let cfg_path = config(tmp.path());
        let mut text = std::fs::read_to_string(&cfg_path).unwrap();
        text.push_str("[scratch.sccache]\nport = 1\n");
        std::fs::write(&cfg_path, text).unwrap();
        let cfg = load(&cfg_path).unwrap();
        let cache = status_of(&cfg, None).unwrap().cache.unwrap();
        assert_eq!(cache.state, "unavailable");
        assert!(cache.stats.is_none());
        store.shutdown();
    }

    /// ADR-0075 §5 G3: `env --server` は cache server が応答すれば webdav（`SCCACHE_WEBDAV_*`、token、`SCCACHE_DIR` なし）、
    /// 応答しなければ G2 の local disk を選び、選んだ方を `<scratch>/bin/sccache-server.mode` に書く。dispatcher と
    /// `scratch env` は webdav のときだけ cache server の応答も確かめ、無ければ sccache 系を与えない。
    #[test]
    fn env_server_switches_to_webdav_when_the_cache_server_is_up() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("sccache-real");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg_path = config(tmp.path());
        let mut text = std::fs::read_to_string(&cfg_path).unwrap();
        text.push_str(&format!(
            "[scratch.sccache]\nbinary = \"{}\"\nport = 1\n",
            bin.display()
        ));
        std::fs::write(&cfg_path, text).unwrap();
        let settings = active_settings(&load(&cfg_path).unwrap()).unwrap();
        task_worker::scratch::ensure_token(&settings.cache_server.token_file).unwrap();
        let token = task_worker::scratch::read_token(&settings.cache_server.token_file).unwrap();
        assert_eq!(token.len(), 64);

        let server = render_server_env_with(&settings, |_| true).unwrap();
        assert!(
            server.starts_with("# celeris: sccache backend = webdav http://127.0.0.1:1 "),
            "{server}"
        );
        assert!(server.contains("export SCCACHE_WEBDAV_ENDPOINT=http://127.0.0.1:1\n"));
        assert!(server.contains("export SCCACHE_WEBDAV_KEY_PREFIX=sccache\n"));
        assert!(server.contains(&format!("export SCCACHE_WEBDAV_TOKEN={token}\n")));
        assert!(server.contains("export SCCACHE_SERVER_PORT=1\n"));
        assert!(!server.contains("SCCACHE_DIR"), "{server}");
        let pool = settings.pool();
        assert_eq!(
            task_worker::scratch::read_sccache_mode(&pool).as_deref(),
            Some("webdav")
        );
        // webdav の sccache に対して cache server が応答しなければ配線しない。
        let down = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| false);
        assert!(
            down.reason().unwrap_or_default().contains("cache server"),
            "{down:?}"
        );
        let up = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| true);
        assert!(up.wrapper().is_some(), "{up:?}");
        // client（run）の env は G2 のまま（webdav 系も token も入れない）。
        let env = task_worker::scratch::cargo_env_with(
            &settings,
            &Owner::parse("agent-g3").unwrap(),
            &up,
        );
        assert!(
            env.iter().all(|(k, _)| !k.starts_with("SCCACHE_WEBDAV")),
            "{env:?}"
        );

        // cache server が居なければ disk に戻り、記録も disk（cache server の有無に関係なく配線する）。
        let server = render_server_env_with(&settings, |_| false).unwrap();
        assert!(server.contains("export SCCACHE_DIR="), "{server}");
        assert!(!server.contains("WEBDAV"), "{server}");
        assert_eq!(
            task_worker::scratch::read_sccache_mode(&pool).as_deref(),
            Some("disk")
        );
        let disk = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| false);
        assert!(disk.wrapper().is_some(), "{disk:?}");
    }

    /// ADR-0075 §5 G2 受け入れ条件 2: server が応答するとき `scratch env` は dispatcher と同じ sccache 系を含み、
    /// `env --server` は server の env（client と同じ値）と本物のバイナリを出す。`status` に配線の状態が出る。
    #[test]
    fn env_includes_sccache_when_the_server_is_up() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("sccache-real");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                drop(conn);
            }
        });
        let cfg_path = config(tmp.path());
        let mut text = std::fs::read_to_string(&cfg_path).unwrap();
        text.push_str(&format!(
            "[scratch.sccache]\nbinary = \"{}\"\nport = {port}\n",
            bin.display()
        ));
        std::fs::write(&cfg_path, text).unwrap();
        let cfg = load(&cfg_path).unwrap();
        let settings = active_settings(&cfg).unwrap();
        assert_eq!(cfg.dispatch_config().scratch, settings);
        let owner = Owner::parse("agent-g2").unwrap();
        let env = task_worker::scratch::cargo_env(&settings, &owner);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "CARGO_TARGET_DIR",
                "CARGO_INCREMENTAL",
                "CARGO_PROFILE_DEV_DEBUG",
                "RUSTC_WRAPPER",
                "SCCACHE_DIR",
                "SCCACHE_CACHE_SIZE",
                "SCCACHE_SERVER_PORT",
                "SCCACHE_IDLE_TIMEOUT"
            ]
        );
        let rendered = render_env(&settings, &owner);
        let parsed: Vec<(String, String)> = rendered
            .lines()
            .map(|l| {
                let (k, v) = l.strip_prefix("export ").unwrap().split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect();
        assert_eq!(parsed, env);
        let server = render_server_env_with(&settings, |_| false).unwrap();
        assert!(
            server.starts_with("# celeris: sccache backend = disk "),
            "{server}"
        );
        assert!(server.contains(&format!(
            "export SCCACHE_DIR={}\n",
            tmp.path().join("scratch/sccache-l1").display()
        )));
        assert!(server.contains(&format!("export SCCACHE_SERVER_PORT={port}\n")));
        assert!(server.contains("export SCCACHE_CACHE_SIZE=40G\n"));
        assert!(server.contains(&format!("export CELERIS_SCCACHE_BIN={}\n", bin.display())));
        assert!(!server.contains("RUSTC_WRAPPER"));
        let status = status_of(&cfg, None).unwrap();
        let view = status.sccache.unwrap();
        assert_eq!(view.state, "ready");
        assert_eq!(view.port, port);
        // 偽のバイナリは `--show-stats` に失敗するので統計は無い（status は落ちない）。
        assert!(view.stats.is_none());
        // server が居なければ sccache 系は消え、`status` は理由を出す（閉じた port は特権 port の 1。並行するテストと
        // 競合しない）。
        let text = std::fs::read_to_string(&cfg_path)
            .unwrap()
            .replace(&format!("port = {port}"), "port = 1");
        std::fs::write(&cfg_path, text).unwrap();
        let cfg = load(&cfg_path).unwrap();
        let settings = active_settings(&cfg).unwrap();
        let env = task_worker::scratch::cargo_env(&settings, &owner);
        assert!(!env.iter().any(|(k, _)| k == "RUSTC_WRAPPER"));
        let view = status_of(&cfg, None).unwrap().sccache.unwrap();
        assert_eq!(view.state, "unavailable");
        assert!(view.reason.unwrap().contains("no sccache server"));
        // バイナリが無ければ `env --server` は失敗する（unit は起動に失敗して気づける）。
        std::fs::remove_file(&bin).unwrap();
        assert!(render_server_env(&settings).is_err());
    }

    /// 外部 lease の作成 → touch → release（P3）→ 再 lease の往復。daemon の owner には lease を取らせない。
    #[test]
    fn lease_touch_release_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = config(tmp.path());
        let cfg = load(&cfg_path).unwrap();
        let settings = active_settings(&cfg).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // release.sh の `.build/<sha12>`（`.git` がファイルの worktree）。checkout は今。
        let tree = tmp.path().join("build");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), "gitdir: /x\n").unwrap();
        let lease_args = |owner: &str, ttl: Option<u64>| LeaseArgs {
            owner: owner_args(&cfg_path, owner),
            repo: repo.clone(),
            worktree: Some(tree.clone()),
            base: Some("abc".into()),
            ttl,
        };
        run(
            None,
            ScratchCommand::Lease(lease_args("release-0123456789ab", Some(60))),
        )
        .unwrap();
        let owner = Owner::parse("release-0123456789ab").unwrap();
        let pool = settings.pool();
        let lease = scratch::read_lease(&pool.lease_path(&owner))
            .unwrap()
            .unwrap();
        assert_eq!(lease.ttl_secs, Some(60));
        assert_eq!(lease.base_commit.as_deref(), Some("abc"));
        let status = status_of(&cfg, None).unwrap();
        let row = |s: &task_ops::daemon::ScratchStatus| {
            s.owners
                .iter()
                .find(|o| o.owner == "release-0123456789ab")
                .unwrap()
                .class
                .clone()
        };
        assert_eq!(row(&status), "p0");
        let old = SystemTime::now() - std::time::Duration::from_secs(120);
        scratch::set_mtime(&pool.lease_path(&owner), old).unwrap();
        assert_eq!(row(&status_of(&cfg, None).unwrap()), "p3");
        run(
            None,
            ScratchCommand::Touch(owner_args(&cfg_path, "release-0123456789ab")),
        )
        .unwrap();
        assert_eq!(row(&status_of(&cfg, None).unwrap()), "p0");
        run(
            None,
            ScratchCommand::Release(owner_args(&cfg_path, "release-0123456789ab")),
        )
        .unwrap();
        assert_eq!(row(&status_of(&cfg, None).unwrap()), "p3");
        assert!(
            scratch::read_lease(&pool.lease_path(&owner))
                .unwrap()
                .unwrap()
                .released_at
                .is_some()
        );
        // 次の release は前の release の target を引き継ぐ（前の lease の最終書き込みが今より前）。
        std::fs::write(pool.target_dir(&owner).join("warm"), "x").unwrap();
        for p in [
            pool.target_dir(&owner).join("warm"),
            pool.target_dir(&owner),
            pool.lease_path(&owner),
        ] {
            std::fs::File::open(&p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        run(
            None,
            ScratchCommand::Lease(lease_args("release-ba9876543210", None)),
        )
        .unwrap();
        let next = Owner::parse("release-ba9876543210").unwrap();
        assert!(pool.target_dir(&next).join("warm").exists());
        assert_eq!(
            scratch::read_lease(&pool.lease_path(&next))
                .unwrap()
                .unwrap()
                .adopted_from
                .as_deref(),
            Some("release-0123456789ab")
        );
        // daemon の owner・touch の対象が無いときは失敗。
        assert!(run(None, ScratchCommand::Lease(lease_args("task-01ABC", None))).is_err());
        assert!(
            run(
                None,
                ScratchCommand::Touch(owner_args(&cfg_path, "agent-none"))
            )
            .is_err()
        );
    }

    /// `gc --dry-run` は一覧だけで何も消さない。`gc` は消す（daemon と同じ `plan_gc`）。legacy は 1h 以上 idle のものだけ。
    #[test]
    fn gc_dry_run_lists_without_removing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = config(tmp.path());
        let cfg = load(&cfg_path).unwrap();
        let settings = active_settings(&cfg).unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(
            None,
            ScratchCommand::Lease(LeaseArgs {
                owner: owner_args(&cfg_path, "release-0123456789ab"),
                repo: repo.clone(),
                worktree: None,
                base: None,
                ttl: None,
            }),
        )
        .unwrap();
        run(
            None,
            ScratchCommand::Release(owner_args(&cfg_path, "release-0123456789ab")),
        )
        .unwrap();
        // legacy: 古いもの（消える）と新しいもの（残る）。
        let old_legacy = tmp.path().join("build-cache/cargo/old-abc");
        let new_legacy = tmp.path().join("build-cache/cargo/agent-platform-g1");
        std::fs::create_dir_all(&old_legacy).unwrap();
        std::fs::create_dir_all(&new_legacy).unwrap();
        std::fs::write(old_legacy.join("f"), "x").unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(7200);
        for p in [old_legacy.join("f"), old_legacy.clone()] {
            std::fs::File::open(&p)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let owner = Owner::parse("release-0123456789ab").unwrap();
        let text = gc(&cfg, &settings, None, true).unwrap();
        assert!(text.contains("would remove release-0123456789ab"), "{text}");
        assert!(
            text.contains(&format!("would remove {}", old_legacy.display())),
            "{text}"
        );
        assert!(!text.contains("agent-platform-g1"), "{text}");
        assert!(settings.pool().target_dir(&owner).exists());
        assert!(old_legacy.exists());
        let text = gc(&cfg, &settings, None, false).unwrap();
        assert!(text.contains("removed release-0123456789ab"), "{text}");
        assert!(!settings.pool().target_dir(&owner).exists());
        assert!(!old_legacy.exists());
        assert!(new_legacy.exists());
        assert!(!scratch_gc::has_pending_deletes(
            &scratch_gc::deleting_roots(&settings.pool(), &[old_legacy])
        ));
    }

    #[test]
    fn disabled_scratch_makes_lease_fail_so_release_sh_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            "[scratch]\nenabled = false\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
        let err = run(
            None,
            ScratchCommand::Lease(LeaseArgs {
                owner: owner_args(&path, "release-0123456789ab"),
                repo: tmp.path().to_path_buf(),
                worktree: None,
                base: None,
                ttl: None,
            }),
        )
        .unwrap_err();
        assert!(err.to_string().contains("scratch is disabled"), "{err}");
        let status = status_of(&load(&path).unwrap(), None).unwrap();
        assert!(!status.enabled);
    }
}
