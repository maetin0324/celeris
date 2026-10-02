//! 起動時の組み立て: DB の置き場所の検査、`Dispatcher` の構築、組織図の種まき。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use task_core::{SqliteStore, StoreOptions, TaskStore};
use task_dispatch::{Dispatcher, StaticPolicy};
use task_worker::db_guard::{DaemonPaths, GuardDecision, ProtectedSet, WorkerRunMarker};
use time::OffsetDateTime;

use super::adapters::{build_adapters, effective_models};
use super::clusters::{
    ClusterMasters, TunnelForwardChildren, TunnelForwardRegistry, cluster_connector,
    cluster_control_persist, cluster_keepalive_secs, master_launchers, tunnel_forward_ensurer,
    tunnel_listener_probe, tunnel_probe,
};
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
/// 確かめられなければ**起動しない**（黙って保護なしで走らない）。`[db] worker_read_only = false` なら外す。
/// `build_dispatcher`（テストから広く呼ばれる）ではなく `run` だけが呼ぶ。DB は `build_dispatcher` が
/// 開いて作った後なので存在する。
///
/// ADR-0126 A: `worker_read_only = true` のときは、本番 config から守る対象 P を作り、worker run の印と
/// 起動する daemon の path で判定する。試験用の一時 DB を guard の効いた worker run の中で使うときだけ
/// probe をせずに guard を外す（免除）。それ以外は従来どおり probe する。
pub(crate) fn install_worker_db_guard(config: &Config) -> Result<(), DaemonError> {
    if !config.db.worker_read_only {
        tracing::warn!(
            db = %config.db.path.display(),
            "[db] worker_read_only = false: worker runs can write the database and reach the user systemd bus (ADR-0095 D5/D-a opt-out)"
        );
        task_worker::db_guard::install(None);
        return Ok(());
    }
    let home = passwd_home();
    let decision = worker_db_guard_decision(
        &worker_db_guard_protected_set(home.as_deref()),
        &worker_db_guard_daemon_paths(config),
        &task_worker::db_guard::detect_worker_run_marker(),
    );
    match &decision {
        GuardDecision::Exempt => {
            tracing::warn!(
                db = %config.db.path.display(),
                "test DB inside a guarded worker run; worker db guard not installed (ADR-0126)"
            );
            task_worker::db_guard::install(None);
            return Ok(());
        }
        GuardDecision::RefuseProduction { reason, .. } => {
            tracing::info!(%reason, "worker db guard: the daemon uses production paths; probing user namespaces (ADR-0126)");
        }
        GuardDecision::RequireUserns { reason } => {
            tracing::debug!(%reason, "worker db guard: not exempt; probing user namespaces (ADR-0126)");
        }
    }
    let guard = task_worker::db_guard::DbGuard::new(&config.db.path)
        .map_err(|e| DaemonError::DbGuard(e.to_string()))?;
    task_worker::db_guard::probe(&guard).map_err(|e| {
        let message = format!(
            "cannot make {} read-only for worker runs ({e}; ADR-0095). Worker runs must not be able to \
             write the database: enable unprivileged user namespaces for this user, or set \
             [db] worker_read_only = false to run without the guard (not recommended)",
            guard.db_path().display()
        );
        DaemonError::DbGuard(match (decision.refusal_message(), &decision) {
            (Some(refusal), GuardDecision::RefuseProduction { reason, .. }) => {
                format!("{refusal}: {reason}. {message}")
            }
            _ => message,
        })
    })?;
    tracing::info!(
        db = %guard.db_path().display(),
        dir = %guard.dir().display(),
        "worker runs see the db directory read-only (ADR-0095)"
    );
    task_worker::db_guard::install(Some(guard));
    Ok(())
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
pub(crate) fn worker_db_guard_decision(
    protected: &ProtectedSet,
    daemon: &DaemonPaths,
    marker: &WorkerRunMarker,
) -> GuardDecision {
    task_worker::db_guard::judge_worker_db_guard(protected, daemon, marker)
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
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_with(
        &config.db.path,
        StoreOptions {
            busy_timeout: config.db.busy_timeout(),
            background_checkpoint: true,
            ..StoreOptions::default()
        },
    )?);
    seed_org_if_empty(store.as_ref(), config)?;
    let policy = StaticPolicy::new(
        config.provider_specs(),
        std::time::Duration::from_secs(config.error_cooldown_secs),
    );
    let mut dispatcher = Dispatcher::new(
        store,
        Box::new(policy),
        effective_models(config),
        build_adapters(config),
        config.account_pool_providers(),
        config.dispatch_config(),
    );
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
