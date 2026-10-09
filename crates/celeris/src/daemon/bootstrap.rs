//! 起動時の組み立て: DB の置き場所の検査、`Dispatcher` の構築、組織図と定期実行 job の種まき。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use task_core::{SqliteStore, StoreOptions, TaskStore};
use task_dispatch::{Dispatcher, StaticPolicy};
use task_worker::db_guard::{
    DaemonPaths, DbGuard, GuardDecision, PRODUCTION_IN_WORKER_RUN, ProtectedSet, ResolvedPath,
    WorkerRunMarker,
};
use time::OffsetDateTime;

use super::adapters::{build_adapters, effective_models};
use super::clusters::{
    ClusterMasters, TunnelForwardChildren, TunnelForwardRegistry, cluster_connector,
    cluster_control_persist, cluster_keepalive_secs, master_launchers, tunnel_forward_ensurer,
    tunnel_listener_probe, tunnel_probe,
};
use crate::config::ConfigError;
use crate::{Config, DaemonError};

/// ADR-0013 D5 の前提（DB はローカルディスク）を破っている場合に警告するための、ネットワーク FS の一覧。
pub(crate) const NETWORK_FILESYSTEMS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "9p",
    "afs",
    "ceph",
    "lustre",
    "gpfs",
    "beegfs",
    "glusterfs",
];

/// `/proc/self/mountinfo` の内容から、`target` を含む最長一致のマウント点のファイルシステム種別を返す。
pub(crate) fn filesystem_type_in(mountinfo: &str, target: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let Some(mount_point) = before.split_whitespace().nth(4) else {
            continue;
        };
        let Some(fstype) = after.split_whitespace().next() else {
            continue;
        };
        // 同じマウント点の行が複数ある場合（autofs → nfs4 など）は後の行が有効なので `>=` で上書きする。
        if target.starts_with(mount_point)
            && best
                .as_ref()
                .is_none_or(|(len, _)| mount_point.len() >= *len)
        {
            best = Some((mount_point.len(), fstype.to_string()));
        }
    }
    best.map(|(_, fstype)| fstype)
}

/// ADR-0015 D3: DB がネットワーク FS 上なら警告する（起動は止めない。判定できない環境では何もしない）。
pub(crate) fn warn_if_db_on_network_filesystem(db: &Path) {
    let dir = db.parent().unwrap_or(Path::new("."));
    let Ok(target) = dir.canonicalize() else {
        return;
    };
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return;
    };
    let Some(fstype) = filesystem_type_in(&mountinfo, &target) else {
        return;
    };
    if NETWORK_FILESYSTEMS.contains(&fstype.as_str()) || fstype.starts_with("fuse.") {
        tracing::warn!(
            db = %db.display(),
            filesystem = %fstype,
            "the database is on a network filesystem; SQLite WAL needs a local disk (ADR-0013 D5). \
             Expect stalls, `database is locked` and possible corruption"
        );
    }
}

/// スナップショットの `hostname`: `/proc/sys/kernel/hostname`（Linux）→ `HOSTNAME` → `"unknown"`。
pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// ADR-0095 D5: worker の run（adapter・check）から DB のディレクトリを読み取り専用にするガードを
/// プロセス全体に入れる。入れる前に namespace 付きのプロセスを 1 回起動して DB が書けないことを確かめ、
/// 確かめられなければ**起動しない**（黙って保護なしで走らない）。`build_dispatcher`（テストから広く
/// 呼ばれる）ではなく `run` だけが呼ぶ。DB は `build_dispatcher` が開いて作った後なので存在する。
///
/// ADR-0126 A: 本番 config から守る対象 P を作り、worker run の印と起動する daemon の path で判定する
/// （[`worker_db_guard_decision`]・[`worker_db_guard_action`]）。worker run の中で本番 DB・本番 token に
/// 当たれば probe をせずに拒否する（probe の成否に関わらず起動しない。fail closed）。本番に当たらない
/// daemon は probe をせずに guard を外す（免除。付記2 の 2）。本番に当たる daemon は従来どおり probe する。
///
/// ADR-0126 付記2 の 1: `[db] worker_read_only = false`（opt-out）は worker run の**外でだけ**効く。
/// 印がある中では opt-out でも本番一致を拒否する（[`worker_db_guard_opt_out_action`]）。
pub(crate) fn install_worker_db_guard(config: &Config) -> Result<(), DaemonError> {
    let marker = task_worker::db_guard::detect_worker_run_marker();
    if !config.db.worker_read_only {
        let action = if marker_present(&marker) {
            worker_db_guard_opt_out_action(
                &worker_db_guard_protected_set(passwd_home().as_deref()),
                &worker_db_guard_daemon_paths(config),
                &marker,
            )
        } else {
            WorkerDbGuardAction::Exempt
        };
        if let WorkerDbGuardAction::Refuse(message) = action {
            tracing::error!(db = %config.db.path.display(), "{message}");
            return Err(DaemonError::DbGuard(message));
        }
        tracing::warn!(
            db = %config.db.path.display(),
            "[db] worker_read_only = false: worker runs can write the database and reach the user systemd bus (ADR-0095 D5/D-a opt-out)"
        );
        task_worker::db_guard::install(None);
        return Ok(());
    }
    let decision = worker_db_guard_decision(
        &worker_db_guard_protected_set(passwd_home().as_deref()),
        &worker_db_guard_daemon_paths(config),
        &marker,
    );
    let mut guard = enforce_worker_db_guard(config, &decision, probe_worker_db_guard)?;
    if guard.is_none() && !marker_present(&marker) {
        // 付記2 の 2: 本番に当たらない daemon は userns を要求しない。ただし worker run の外（印なし）で
        // userns が使えるなら guard を入れる（失敗しても起動は止めない）。
        guard = match probe_worker_db_guard(config) {
            Ok(guard) => Some(guard),
            Err(e) => {
                tracing::warn!(error = %e, "worker db guard unavailable for a non-production daemon; running without it (ADR-0126 addendum 2)");
                None
            }
        };
    }
    task_worker::db_guard::install(guard);
    Ok(())
}

/// ADR-0126 付記2 の 1: `[db] worker_read_only = false` のときの動作（純関数。試験から userns なしで呼ぶ）。
/// 印が無ければ opt-out が効く（`Exempt` = guard なし）。印がある中で本番に当たれば `Refuse`。P が
/// 決まらない（本番 config が読めない）ときも、opt-out では probe で止める道が無いので `Refuse`（fail closed）。
/// `Probe` は返さない。
pub(crate) fn worker_db_guard_opt_out_action(
    protected: &ProtectedSet,
    daemon: &DaemonPaths,
    marker: &WorkerRunMarker,
) -> WorkerDbGuardAction {
    if !marker_present(marker) {
        return WorkerDbGuardAction::Exempt;
    }
    match worker_db_guard_action(&worker_db_guard_decision(protected, daemon, marker)) {
        WorkerDbGuardAction::Exempt => WorkerDbGuardAction::Exempt,
        refuse @ WorkerDbGuardAction::Refuse(_) => refuse,
        WorkerDbGuardAction::Probe => {
            WorkerDbGuardAction::Refuse(opt_out_unknown_message(protected))
        }
    }
}

fn opt_out_unknown_message(protected: &ProtectedSet) -> String {
    format!(
        "worker db guard: [db] worker_read_only = false inside a worker run, but whether this daemon uses \
         the production DB/token cannot be determined ({}; ADR-0126 addendum 2)",
        protected.unknown_reasons().join("; ")
    )
}

/// ADR-0126 A2: 判定から決まる動作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerDbGuardAction {
    /// worker run の中で本番を使う daemon。probe せずにこの文言で起動を止める（token の値は含めない）。
    Refuse(String),
    /// 本番 DB・本番 token に当たらない daemon（ADR-0126 付記2 の 2）、または worker run の外の opt-out。
    /// probe せず guard を入れない。
    Exempt,
    /// 従来どおり userns の probe をしてから guard を入れる。
    Probe,
}

/// ADR-0126 A2: `GuardDecision` → 動作（純関数）。`RefuseProduction { inside_worker_run: true }` は
/// probe に進まず必ず `Refuse`。worker run の外の本番 daemon（本番そのもの）は従来どおり `Probe`。
pub(crate) fn worker_db_guard_action(decision: &GuardDecision) -> WorkerDbGuardAction {
    match decision {
        GuardDecision::Exempt => WorkerDbGuardAction::Exempt,
        GuardDecision::RefuseProduction {
            reason,
            inside_worker_run: true,
        } => WorkerDbGuardAction::Refuse(format!("{PRODUCTION_IN_WORKER_RUN}: {reason}")),
        GuardDecision::RefuseProduction {
            inside_worker_run: false,
            ..
        }
        | GuardDecision::RequireUserns { .. } => WorkerDbGuardAction::Probe,
    }
}

/// 判定に従って guard を決める。`probe` は `Probe` のときだけ呼ぶ（試験が呼ばれないことを確かめる）。
/// 返り値は `db_guard::install` に渡す（`None` = guard なし）。
pub(crate) fn enforce_worker_db_guard(
    config: &Config,
    decision: &GuardDecision,
    probe: impl FnOnce(&Config) -> Result<DbGuard, DaemonError>,
) -> Result<Option<DbGuard>, DaemonError> {
    match worker_db_guard_action(decision) {
        WorkerDbGuardAction::Refuse(message) => {
            tracing::error!(db = %config.db.path.display(), "{message}");
            Err(DaemonError::DbGuard(message))
        }
        WorkerDbGuardAction::Exempt => {
            tracing::warn!(
                db = %config.db.path.display(),
                "the daemon does not use the production DB/token; worker db guard not installed (ADR-0126)"
            );
            Ok(None)
        }
        WorkerDbGuardAction::Probe => {
            if let GuardDecision::RequireUserns { reason } = decision {
                tracing::debug!(%reason, "worker db guard: not exempt; probing user namespaces (ADR-0126)");
            }
            probe(config).map(Some)
        }
    }
}

/// ADR-0095 D5: namespace 付きのプロセスで DB が書けないことを確かめた guard を返す。
fn probe_worker_db_guard(config: &Config) -> Result<DbGuard, DaemonError> {
    let guard = DbGuard::new(&config.db.path)
        .map_err(|e| DaemonError::DbGuard(e.to_string()))?
        .with_releases_dir(config.selfdeploy.releases_dir.clone())
        .with_hot_mount(config.storage.hot_mount.clone());
    task_worker::db_guard::probe(&guard).map_err(|e| {
        DaemonError::DbGuard(format!(
            "cannot make {} read-only for worker runs ({e}; ADR-0095). Worker runs must not be able to \
             write the database: enable unprivileged user namespaces for this user, or set \
             [db] worker_read_only = false to run without the guard (not recommended)",
            guard.db_path().display()
        ))
    })?;
    tracing::info!(
        db = %guard.db_path().display(),
        dir = %guard.dir().display(),
        "worker runs see the db directory read-only (ADR-0095)"
    );
    Ok(guard)
}

/// ADR-0126 A2: DB を開く（`build_dispatcher`）前の拒否。worker run の中で本番 DB・本番 token に当たる
/// daemon は DB に触れる前に止める。付記2 の 1: `[db] worker_read_only = false` でも止める（opt-out は
/// worker run の外でだけ効く）。残りは DB を作った後の [`install_worker_db_guard`] が判定する。
pub(crate) fn refuse_production_db_in_worker_run(config: &Config) -> Result<(), DaemonError> {
    let marker = task_worker::db_guard::detect_worker_run_marker();
    match worker_db_guard_precheck(passwd_home().as_deref(), config, &marker) {
        WorkerDbGuardAction::Refuse(message) => {
            tracing::error!(db = %config.db.path.display(), "{message}");
            Err(DaemonError::DbGuard(message))
        }
        WorkerDbGuardAction::Exempt | WorkerDbGuardAction::Probe => Ok(()),
    }
}

/// [`refuse_production_db_in_worker_run`] の判定部分（試験から userns なしで呼ぶ）。`Refuse` 以外は
/// 「ここでは止めない」の意味しかない（`Probe` を返す）。`worker_read_only` の値に関わらず判定する。
///
/// DB がまだ無いときは判定関数が DB の path を解けないので、(a) 印の path（読み取り専用。そこに試験用 DB
/// は作れない）の配下か、(b) DB を置くディレクトリ（存在する祖先まで解く）と token を、DB の代わりに
/// 本番と重ならない `/dev/null` を置いた [`worker_db_guard_decision`] で見る。
pub(crate) fn worker_db_guard_precheck(
    home: Option<&Path>,
    config: &Config,
    marker: &WorkerRunMarker,
) -> WorkerDbGuardAction {
    if !marker_present(marker) {
        return WorkerDbGuardAction::Probe;
    }
    let mut daemon = worker_db_guard_daemon_paths(config);
    if std::fs::symlink_metadata(&daemon.db).is_err() {
        let resolved = ResolvedPath::resolve_lenient(&daemon.db);
        if let WorkerRunMarker::ReadOnly(guarded) = marker {
            let under = resolved
                .as_ref()
                .map(|db| db.path.starts_with(guarded))
                // 解けない path は判定できないので止める（fail closed）。
                .unwrap_or(true);
            if under {
                return WorkerDbGuardAction::Refuse(format!(
                    "{PRODUCTION_IN_WORKER_RUN}: db {} is under the guarded directory {}",
                    daemon.db.display(),
                    guarded.display()
                ));
            }
        }
        let Some(dir) = resolved
            .ok()
            .and_then(|db| db.path.parent().map(Path::to_path_buf))
        else {
            return WorkerDbGuardAction::Refuse(format!(
                "{PRODUCTION_IN_WORKER_RUN}: cannot resolve the daemon db {}",
                daemon.db.display()
            ));
        };
        daemon = DaemonPaths {
            db: PathBuf::from("/dev/null"),
            state_dir: Some(dir),
            token_file: daemon.token_file,
        };
    }
    let protected = worker_db_guard_protected_set(home);
    let decision = worker_db_guard_decision(&protected, &daemon, marker);
    match worker_db_guard_action(&decision) {
        refuse @ WorkerDbGuardAction::Refuse(_) => refuse,
        // opt-out では DB を作った後の probe で止める道が無いので、P が決まらなければここで止める。
        _ if !config.db.worker_read_only && !protected.unknown_reasons().is_empty() => {
            WorkerDbGuardAction::Refuse(opt_out_unknown_message(&protected))
        }
        _ => WorkerDbGuardAction::Probe,
    }
}

fn marker_present(marker: &WorkerRunMarker) -> bool {
    !matches!(marker, WorkerRunMarker::Absent)
}

/// ADR-0126 A1-2: `getpwuid(getuid())` の home（`$HOME` は使わない。試験が書き換える）。
fn passwd_home() -> Option<PathBuf> {
    task_worker::db_guard::production_config_path()
        .and_then(|config| config.ancestors().nth(3).map(Path::to_path_buf))
}

/// ADR-0126 A1: `home` の `.config/celeris/config.toml`（本番 config）から守る対象 P を作る。
/// 本番 config が無ければ P は空（印は判定側で足す）。home が決まらない・存在するのに読めない・
/// parse できないなら P は Unknown（免除しない）。相対 path は `Config::load` と同じく config の
/// ディレクトリ基準、`~` は `$HOME` ではなく `home` で展開する。
pub(crate) fn worker_db_guard_protected_set(home: Option<&Path>) -> ProtectedSet {
    let mut set = ProtectedSet::new();
    let Some(home) = home else {
        set.mark_unknown("cannot resolve the home directory of the current uid");
        return set;
    };
    let path = home.join(".config/celeris/config.toml");
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return set,
        Err(e) => {
            set.mark_unknown(format!("production config {}: {e}", path.display()));
            return set;
        }
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            set.mark_unknown(format!("production config {}: {e}", path.display()));
            return set;
        }
    };
    let value: toml::Table = match toml::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            set.mark_unknown(format!(
                "production config {} cannot be parsed: {e}",
                path.display()
            ));
            return set;
        }
    };
    let base = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.to_path_buf());
    let base = base.canonicalize().unwrap_or(base);
    let resolve = |raw: &str, expand: bool| {
        let p = PathBuf::from(raw);
        let p = if expand {
            task_core::expand_home(&p, Some(home))
        } else {
            p
        };
        if p.is_relative() { base.join(p) } else { p }
    };
    // `db` は文字列か `[db]` テーブル（ADR-0064 D1）。省略時は ADR-0045 D2 の既定。
    let db = match value.get("db") {
        None => Some("~/.local/celeris/celeris.sqlite3"),
        Some(toml::Value::String(s)) => Some(s.as_str()),
        Some(toml::Value::Table(t)) => match t.get("path") {
            None => Some("~/.local/celeris/celeris.sqlite3"),
            Some(toml::Value::String(s)) => Some(s.as_str()),
            Some(_) => None,
        },
        Some(_) => None,
    };
    match db {
        Some(db) => set.add_db_file(&resolve(db, true)),
        None => set.mark_unknown(format!("production config {}: invalid db", path.display())),
    }
    match value.get("state_dir") {
        None => {}
        Some(toml::Value::String(s)) => set.add_dir(&resolve(s, true)),
        Some(_) => set.mark_unknown(format!(
            "production config {}: invalid state_dir",
            path.display()
        )),
    }
    // `[api] token_file` は相対なら config のディレクトリ基準（`~` は展開しない。`ApiConfig::resolve_paths`）。
    match value.get("api") {
        None => {}
        Some(toml::Value::Table(api)) => match api.get("token_file") {
            None => {}
            Some(toml::Value::String(s)) => set.add_token_file(&resolve(s, false)),
            Some(_) => set.mark_unknown(format!(
                "production config {}: invalid [api] token_file",
                path.display()
            )),
        },
        Some(_) => set.mark_unknown(format!(
            "production config {}: invalid [api]",
            path.display()
        )),
    }
    set
}

/// ADR-0126 A2: 起動する daemon の path（`state_dir` は `Config` に無いので DB のディレクトリで代わる）。
pub(crate) fn worker_db_guard_daemon_paths(config: &Config) -> DaemonPaths {
    DaemonPaths {
        db: config.db.path.clone(),
        state_dir: None,
        token_file: config.api.token_file.clone(),
    }
}

/// ADR-0126 A2/A3: `task_worker::db_guard` の判定関数に渡す（試験から userns なしで呼ぶ）。
///
/// 付記2 の 2: 判定関数が `RequireUserns` を返しても、P が決まっていて（`unknown` が空）印が読み取り専用で
/// 裏付けられた場合でないなら（= 本番に当たらないことが確かめられた上で、印が無い・裏付けられないだけ）
/// `Exempt` にする。本番に当たる daemon（`RefuseProduction`）と P が決まらないときは変えない。
pub(crate) fn worker_db_guard_decision(
    protected: &ProtectedSet,
    daemon: &DaemonPaths,
    marker: &WorkerRunMarker,
) -> GuardDecision {
    match task_worker::db_guard::judge_worker_db_guard(protected, daemon, marker) {
        GuardDecision::RequireUserns { reason }
            if protected.unknown_reasons().is_empty()
                && !matches!(marker, WorkerRunMarker::ReadOnly(_)) =>
        {
            tracing::debug!(%reason, "worker db guard: not the production DB/token; exempt (ADR-0126 addendum 2)");
            GuardDecision::Exempt
        }
        decision => decision,
    }
}

/// 付記 E2（ADR 2026-10-05-browser-department-web-live-view、daemon 側）: この daemon が試験専用 loopback
/// 許可を申告した browser launcher を拒否するか（真 = 拒否）。本番判定は worker db guard と同じ P
/// （[`worker_db_guard_protected_set`]）を使う。
pub(crate) fn browser_launcher_refuses_test_loopback(config: &Config) -> bool {
    match launcher_test_loopback_refusal(
        passwd_home().as_deref(),
        config.source_path.as_deref(),
        &worker_db_guard_daemon_paths(config),
    ) {
        Some(reason) => {
            tracing::debug!(%reason, "browser launcher: test-only loopback egress is refused for this daemon");
            true
        }
        None => false,
    }
}

/// [`browser_launcher_refuses_test_loopback`] の判定（試験から userns なしで呼ぶ）。拒否するなら理由。
/// 次のどれかで拒否する（fail closed）: 本番 config（`home` の `.config/celeris/config.toml`）から P を
/// 決められない、daemon が読んだ config がその本番 config そのもの（正規化して比べる。正規化できなければ
/// 一致とみなす）、daemon の DB・DB のディレクトリ・token が本番に当たる（`judge_worker_db_guard` の
/// `RefuseProduction`。印は見ない）。
pub(crate) fn launcher_test_loopback_refusal(
    home: Option<&Path>,
    config_path: Option<&Path>,
    daemon: &DaemonPaths,
) -> Option<String> {
    let protected = worker_db_guard_protected_set(home);
    if !protected.unknown_reasons().is_empty() {
        return Some(format!(
            "the production config cannot be determined: {}",
            protected.unknown_reasons().join("; ")
        ));
    }
    if let (Some(home), Some(config_path)) = (home, config_path) {
        let production = home.join(".config/celeris/config.toml");
        if production.exists() {
            match (production.canonicalize(), config_path.canonicalize()) {
                (Ok(p), Ok(c)) if p != c => {}
                _ => {
                    return Some(format!(
                        "config {} is the production config",
                        config_path.display()
                    ));
                }
            }
        }
    }
    match task_worker::db_guard::judge_worker_db_guard(&protected, daemon, &WorkerRunMarker::Absent)
    {
        GuardDecision::RefuseProduction { reason, .. } => Some(reason),
        GuardDecision::Exempt | GuardDecision::RequireUserns { .. } => None,
    }
}

#[cfg(test)]
#[path = "bootstrap_worker_db_guard_tests.rs"]
mod worker_db_guard_tests;

/// 設定から `Dispatcher` を組み立てる。`[accounts]`/`[secrets]`/`[memory]` があればディレクトリを 0700 で作る
/// （ADR-0024 D1、ADR-0030 D1、ADR-0033 D6）。
pub fn build_dispatcher(
    config: &Config,
    masters: ClusterMasters,
) -> Result<Dispatcher, DaemonError> {
    config.ensure_accounts_dir()?;
    config.ensure_secrets_dir()?;
    config.ensure_memory_dir()?;
    // ADR-0064 D1/D5: `[db]` の `busy_timeout_ms` を使い、デーモンの書き込み接続は
    // `background_checkpoint` を立てる（別の背景 tick が `PRAGMA wal_checkpoint(PASSIVE)` を打つ）。
    let sqlite = Arc::new(SqliteStore::open_with(
        &config.db.path,
        StoreOptions {
            busy_timeout: config.db.busy_timeout(),
            background_checkpoint: true,
            ..StoreOptions::default()
        },
    )?);
    // External CoS mutations can outlive the process that initiated them. Never replay an
    // uncertain side effect: surface old pending receipts for human remediation on startup.
    const COS_EXTERNAL_PENDING_GRACE_SECS: i64 = 30 * 60;
    sqlite
        .cos_operation_remediate_stale(
            OffsetDateTime::now_utc() - time::Duration::seconds(COS_EXTERNAL_PENDING_GRACE_SECS),
        )
        .map_err(|error| crate::DaemonError::CosOperationRecovery(error.to_string()))?;
    let store: Arc<dyn TaskStore> = sqlite.clone();
    seed_org_if_empty(store.as_ref(), config)?;
    seed_cron_if_empty(store.as_ref(), config, OffsetDateTime::now_utc())?;
    let policy = StaticPolicy::new(
        config.provider_specs(),
        std::time::Duration::from_secs(config.error_cooldown_secs),
    );
    let mut dispatcher = Dispatcher::new(
        store.clone(),
        Box::new(policy),
        effective_models(config),
        build_adapters(config),
        config.account_pool_providers(),
        config.dispatch_config(),
    );
    // ADR 2026-10-07-build-tmp-hygiene D1.4: cron の保守 executor（`target_sweep`）の roots と上限。
    dispatcher.set_target_sweep(config.target_sweep_params());
    // 付記 A2・A3（2026-10-09）: scratch の target と終わった task の作業場所の target も掃除する。
    dispatcher.set_target_sweep_scope(config.target_sweep_scope());
    // ADR 2026-10-07-build-tmp-hygiene D4: ディスク使用率の監視（tick の段 `disk_watch`）。
    dispatcher.set_disk_watch(config.disk_watch_entries());
    dispatcher.set_cos_chat_launch(
        sqlite.clone(),
        super::cos_launch::build_cos_chat_launch(config, &config.db.path, config.api.listen),
    );
    // ADR 2026-10-06 model-role-assignments D2: DB の割り当てを同じ store から読む（run 起動のたびに解決）。
    dispatcher.set_role_assignment_reader(sqlite);
    // 付記「モデルごとの複数役割」: shadow / enforce の役割メンバーの順位づけに routing catalog の品質を使う。
    if let Some(shared) = config.routing_catalog_state.as_ref() {
        dispatcher.set_routing_model_profiles(Arc::new(SharedCatalogProfiles(Arc::clone(shared))));
    }
    // ADR 2026-10-06 D3: `account_pool = "opencode-go"` のように pool の adapter を明示した行。
    dispatcher.set_account_pool_adapters(config.account_pool_adapters());
    // ADR-0132 付記 L1/L2: cheap lane で先に試すローカルの行（`[execution] cheap_local_first = false` なら空）。
    dispatcher.set_local_providers(config.local_cheap_providers());
    // ADR 2026-10-07: coding の既定ハーネス解決（`adapter_policy = "model_family"` のハーネス）。
    dispatcher.set_coding_harness_default(config.coding_harness_default());
    // ADR 2026-10-04 Phase 2: `[model_routing]` の mode・観測 TTL・窓の reserve_value（既定 legacy）。
    if let Some(runtime) = &config.model_routing.runtime {
        dispatcher.set_dispatch_routing(runtime.dispatch_settings());
        // Phase 4: `[model_routing.shadow]`（既定 off）。listener は後で llm-proxy が足す。
        dispatcher.set_routing_shadow(runtime.shadow.clone());
    }
    dispatcher.set_cluster_connector(cluster_connector(
        masters.clone(),
        master_launchers(config),
        cluster_keepalive_secs(config),
        cluster_control_persist(config),
    ));
    // ADR-0062 A（Phase 107）: master 越しの実通信 probe・死んだ接続の片付け・celeris 保持 master の
    // 終了検出は、ここ（テストからも広く呼ばれる `build_dispatcher`）では配線しない。実 ssh を打つ
    // フックなので、テストの偽 `cluster_liveness_probe`（`alive = true`）だけでは防げず、本物の
    // `ssh -o BatchMode=yes <host> -- true` が飛んでしまう（CLAUDE.md: テストで外部ネットワークに
    // 出ない）。本番の起動経路（`wire_cluster_liveness_hooks`）だけで配線する。
    // ADR-0053 D3（Phase 66）: `[[clusters.forwards]]`（Qwen トンネル等）の(再)確立と生存監視。
    // 設定に forward が無ければ `refresh_cluster_tunnels` 自体が早期に戻るので、挿しても無害。
    let tunnel_forward_children: TunnelForwardChildren =
        Arc::new(std::sync::Mutex::new(TunnelForwardRegistry::default()));
    dispatcher.set_tunnel_forward_ensurer(tunnel_forward_ensurer(tunnel_forward_children));
    // ADR-0053 Phase 85: listener（手元の待ち受け。軽い TCP connect）と target の健康
    // （`/v1/models`。バックオフされる）を別のフックで挿す。
    dispatcher.set_tunnel_listener_probe(tunnel_listener_probe());
    dispatcher.set_tunnel_probe(tunnel_probe());
    // ADR-0043 D3（Phase 56）: 起動時に 1 度だけコンテナ runtime を調べる（`podman info` → `docker info`）。
    // 結果はログと `GET /daemon` の `containers` に出る。使えなければ `run = container` のタスクは
    // dispatch されず `blocked`（「コンテナ runtime が使えません」）になる。
    dispatcher.detect_container_runtime();
    Ok(dispatcher)
}

/// ADR-0033 D1: 組織図の種を蒔く。**`org_nodes` が空のときだけ**書き、それ以外は何もしない
/// （以後の編集は GUI → API → DB。設定は再読込しない）。蒔いた件数を返す。
pub fn seed_org_if_empty(store: &dyn TaskStore, config: &Config) -> Result<usize, DaemonError> {
    if config.org.is_empty() {
        return Ok(0);
    }
    if !store.org_list()?.is_empty() {
        tracing::debug!(
            "org: org_nodes is not empty; the config seed is not applied (the DB wins)"
        );
        return Ok(0);
    }
    let now = OffsetDateTime::now_utc();
    let nodes = config.org_nodes(now);
    // 監査 D-4: 1 トランザクションで蒔く。途中の 1 件が不正でも部分的に書かれた組織が残らない
    // （残ると次回起動時は `org_list` が空でなくなり、二度と補完されない）。
    store.org_seed(&nodes)?;
    tracing::info!(
        count = nodes.len(),
        "org: seeded the organization from the config"
    );
    Ok(nodes.len())
}

/// ADR-0131 D6 / 付記 D10: 定期実行 job の種を蒔く。**`cron_jobs` が空のときだけ**書き、それ以外は
/// 何もしない（DB が正。2 回目以降の起動で重複も上書きもしない）。検証は API の cron job 作成と同じ
/// `task_ops::cron_jobs::validate_job` / `create_job` を通し、不正な種は起動時の設定エラーにする。
/// 全件を先に検証してから書くので、途中の 1 件が不正でも一部だけ入った状態は残らない。蒔いた件数を返す。
pub fn seed_cron_if_empty(
    store: &dyn TaskStore,
    config: &Config,
    now: OffsetDateTime,
) -> Result<usize, DaemonError> {
    if config.cron.seed.is_empty() {
        return Ok(0);
    }
    if !store.cron_job_list()?.is_empty() {
        tracing::debug!(
            "cron: cron_jobs is not empty; the config seed is not applied (the DB wins)"
        );
        return Ok(0);
    }
    let roles = config.role_specs();
    let genres = config.genre_specs();
    let ctx = task_ops::cron_jobs::CronFireContext {
        roles: &roles,
        genres: &genres,
        ..Default::default()
    };
    let invalid = |name: &str, e: task_ops::OpsError| {
        DaemonError::Config(ConfigError::Invalid(format!("[[cron.seed]] {name:?}: {e}")))
    };
    for seed in &config.cron.seed {
        let new = seed.to_new_job();
        let probe = task_core::CronJob {
            id: task_core::CronJobId::new(),
            name: new.name,
            enabled: new.enabled,
            schedule: new.schedule,
            timezone: new.timezone,
            overlap: new.overlap,
            catch_up: new.catch_up,
            template: new.template,
            next_fire_at: None,
            created_at: now,
            updated_at: now,
        };
        task_ops::cron_jobs::validate_job(store, &ctx, &probe, now)
            .map_err(|e| invalid(&seed.name, e))?;
    }
    for seed in &config.cron.seed {
        task_ops::cron_jobs::create_job(store, &ctx, seed.to_new_job(), now)
            .map_err(|e| invalid(&seed.name, e))?;
    }
    tracing::info!(
        count = config.cron.seed.len(),
        "cron: seeded the cron jobs from the config"
    );
    Ok(config.cron.seed.len())
}

/// 付記「モデルごとの複数役割」: 共有の routing catalog snapshot から model profile を読む（dispatcher 用）。
struct SharedCatalogProfiles(Arc<std::sync::RwLock<Arc<crate::config::RoutingCatalog>>>);

impl task_dispatch::RoutingModelProfiles for SharedCatalogProfiles {
    fn model_profiles(&self) -> Vec<task_core::model_router::profiles::ModelProfile> {
        self.0
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .models
            .clone()
    }
}
