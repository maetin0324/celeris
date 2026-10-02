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

/// `export NAME=value` の行（dispatcher と同じ `cargo_env`）と、G3-fix1: 与えない sccache の族を外す
/// `unset NAME ...` の 1 行（dispatcher が run・checks の子プロセスで `env_remove` するのと同じ key）。
pub fn render_env(settings: &ScratchSettings, owner: &Owner) -> String {
    render_cargo_env(scratch::cargo_child_env(settings, owner))
}

fn render_cargo_env(env: scratch::CargoEnv) -> String {
    let mut text = String::new();
    if !env.remove.is_empty() {
        // key は `sccache_env_removals` が shell の名前だけに絞っている。
        text.push_str(&format!("unset {}\n", env.remove.join(" ")));
    }
    text.push_str(&render_exports(env.set));
    text
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
        && let Ok((store, _)) = task_core::SqliteStore::open_client(&path)
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
    // ADR-0129 (1): sccache / cache server の欄は `build_status` が `None` にする（Celeris の外）。
    let status = with_lookup(cfg, cli_db, |lookup, has_db| {
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
#[path = "scratch_tests.rs"]
mod tests;
