//! クラスタの ssh master・liveness hook・tunnel の配線（ADR-0032 / ADR-0062）。

use std::collections::HashMap;
use std::sync::Arc;

use task_dispatch::Dispatcher;

use crate::{Config, control_path};

/// ADR-0032 D2: celeris が張った ssh master を保持する場所。`ClusterMaster` を落とすと接続も切れるので、
/// **接続を生かしておきたい間はここに置く**（`DELETE /clusters/{id}/connect` はここから取り除く）。
/// 人が `cluster-login.sh` で張った master はこのマップに載らない（celeris の持ち物ではないため）。
pub type ClusterMasters =
    Arc<std::sync::Mutex<HashMap<String, task_worker::cluster_login::ClusterMaster>>>;

/// ADR-0062 A（Phase 107）: クラスタ id ごとの keepalive の秒数（`[[clusters]] keepalive_secs`）。
/// `[[clusters]]` は `POST /reload` で変わらないので、起動時に一度だけ表を作れば十分。
pub(crate) fn cluster_keepalive_secs(config: &Config) -> Arc<HashMap<String, u64>> {
    Arc::new(
        config
            .clusters
            .iter()
            .map(|c| (c.id.clone(), c.keepalive_secs))
            .collect(),
    )
}

/// ADR-0078 D1: クラスタ id ごとの `ControlPersist` の値（`[[clusters]] control_persist`）。
pub(crate) fn cluster_control_persist(config: &Config) -> Arc<HashMap<String, String>> {
    Arc::new(
        config
            .clusters
            .iter()
            .map(|c| (c.id.clone(), c.control_persist.clone()))
            .collect(),
    )
}

/// ADR-0060（Phase 103）: `[[clusters]] master_launcher` を、クラスタ id ごとに実際の起こし方へ解決する。
/// `[[clusters]]` は `POST /reload` で変わらない（起動時に再読込しない）ので、起動時に一度だけ計算すれば
/// 十分（`cluster_connector` の呼び出しごとに `systemd-run` の PATH 検索をやり直さない）。
pub(crate) fn master_launchers(
    config: &Config,
) -> Arc<HashMap<String, task_worker::cluster_login::MasterLauncher>> {
    let has_systemd_run = task_worker::cluster_login::systemd_run_on_path();
    let has_xdg_runtime_dir = task_worker::cluster_login::xdg_runtime_dir_is_set();
    Arc::new(
        config
            .clusters
            .iter()
            .map(|c| {
                let launcher = task_worker::cluster_login::resolve_master_launcher(
                    &c.master_launcher,
                    has_systemd_run,
                    has_xdg_runtime_dir,
                );
                (c.id.clone(), launcher)
            })
            .collect(),
    )
}

/// ADR-0032 D3: `auth = "publickey"` のクラスタを、ディスパッチの直前に 1 回だけ自分で張る。
///
/// **時間の設計**: これはディスパッチループの中から同期で呼ばれる（`control_master_alive_blocking` と
/// 同じ立場）。`-O check` は 1 秒で返るが接続はもっとかかるので、長く待つと tick 全体が止まる。
/// そこで **`AUTO_CONNECT_TIMEOUT` を短く（8 秒）**切る。鍵だけの接続は実測で 1 秒未満なので
/// （ADR-0032 §1 の fern03）、これで足りる。間に合わなければその tick は cooldown に落ち、
/// 次の機会に再試行される（人を待たせるより tick を止めない方を優先する）。
pub(crate) fn cluster_connector(
    masters: ClusterMasters,
    launchers: Arc<HashMap<String, task_worker::cluster_login::MasterLauncher>>,
    keepalives: Arc<HashMap<String, u64>>,
    persists: Arc<HashMap<String, String>>,
) -> task_dispatch::dispatcher::ClusterConnector {
    /// 自動接続に使う上限。ディスパッチループを止めないために短くしてある（上の説明）。
    const AUTO_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

    Arc::new(move |cluster_id: &str, host: &str| {
        let host = host.to_string();
        let cluster_id = cluster_id.to_string();
        // ADR-0060（Phase 103）: master_launcher は `[[clusters]]` 由来（再読込では変わらない）なので、
        // 起動時に一度だけ解決して渡された表から引く。無ければ安全側（従来どおり inline）。
        let launcher = launchers
            .get(&cluster_id)
            .cloned()
            .unwrap_or(task_worker::cluster_login::MasterLauncher::Inline);
        // ADR-0062 A（Phase 107）。
        let keepalive_secs = keepalives.get(&cluster_id).copied().unwrap_or(0);
        // ADR-0078 D1: 表に無ければ既定の "yes"。
        let control_persist = persists
            .get(&cluster_id)
            .cloned()
            .unwrap_or_else(|| "yes".to_string());
        // Phase 66b（本番 2026-09-21 の観測）: このクロージャは非同期の `start_connect` を専用ランタイムで
        // `block_on` する。呼び出し元（`task_dispatch::dispatcher::run_cluster_hooks_off_async`）は
        // tokio の文脈を持たない OS スレッドへ逃がしてから呼ぶ契約になっているが、万一これが破られて
        // tokio ランタイムのワーカースレッドから直接呼ばれると、下の `Builder::new_current_thread().build()`
        // 後の `.block_on()` が「Cannot start a runtime from within a runtime」で panic する
        // （`crates/celeris/src/lib.rs:621` で実際に panic した）。ここで一度だけ確かめ、破られていたら
        // panic ではなくエラーを返す（呼び出し側は cooldown に落とすだけで、デーモンは死なない）。
        if tokio::runtime::Handle::try_current().is_ok() {
            tracing::error!(
                cluster = %cluster_id,
                host = %host,
                "cluster connect must not be called from an async context (Phase 66b guard; \
                 this is a bug in the caller, not in ssh/network)"
            );
            return Err("cluster connect must not be called from an async context".to_string());
        }
        // ディスパッチループは同期なので、非同期の `start_connect` を専用ランタイムで回す。
        // `Handle::current().block_on` は同じランタイムのワーカースレッドを塞いでパニックしうるため使わない。
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("could not build a runtime for the cluster connect: {e}"))?;
        let outcome = runtime.block_on(task_worker::cluster_login::start_connect(
            &["ssh".to_string()],
            &host,
            &cluster_id,
            &launcher,
            false, // publickey のみ。人の入力は要らない（要るクラスタはここに来ない）
            AUTO_CONNECT_TIMEOUT,
            AUTO_CONNECT_TIMEOUT,
            keepalive_secs,
            &control_persist,
        ));
        match outcome {
            Ok(task_worker::cluster_login::ClusterConnectStart::Connected(master)) => {
                // `master` を落とすと接続も切れるので、生かしておく場所へ移す（`None` は人が張った master）。
                if let Some(master) = master {
                    match masters.lock() {
                        Ok(mut held) => {
                            held.insert(cluster_id.clone(), master);
                        }
                        // 保持できないなら接続を維持できない。master はここで drop されて切れる。
                        Err(_) => return Err("the cluster master registry is poisoned".to_string()),
                    }
                }
                tracing::info!(cluster = %cluster_id, host = %host, "cluster: auto-connected (publickey)");
                Ok(())
            }
            // `interactive = false` では起こらないが、型のうえではありうる。
            Ok(task_worker::cluster_login::ClusterConnectStart::NeedsCode { session, .. }) => {
                runtime.block_on(session.cancel());
                Err(
                    "the host asked for a verification code; set auth = \"totp\" for this cluster"
                        .to_string(),
                )
            }
            Err(e) => Err(format!("{e}")),
        }
    })
}

/// ADR-0078 D2: クラスタごとの `ControlPath` 検査を背景スレッドで 1 回走らせる（`ssh -G` は通信しない）。
/// `auth = "manual"` のクラスタも人が張る master を借りるので、全クラスタを対象にする。
pub(crate) fn spawn_control_path_inspection(config: &Config) {
    let targets: Vec<(String, String)> = config
        .clusters
        .iter()
        .map(|c| (c.id.clone(), c.host.clone()))
        .collect();
    if targets.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("control-path-inspect".to_string())
        .spawn(move || {
            for (id, host) in targets {
                control_path::inspect_cluster_control_path(&id, &host);
            }
        });
    if let Err(e) = spawned {
        tracing::debug!("control_path: could not spawn the inspection thread: {e}");
    }
}

/// ADR-0062 A（Phase 107）: 実 ssh を打つフック（実通信 probe・celeris 保持 master の終了検出）を
/// 配線する（ADR-0078 D3-3: probe の失敗で `-O exit` しないので、片付けのフックは外した）。**本番の起動経路からだけ**呼ぶこと
/// （`build_dispatcher` は `accounts_admin`/`lib.rs` の多数のテストから呼ばれ、そこでは
/// `cluster_liveness_probe` を偽物にすり替えるだけで済ませているため、実 ssh を打つこの 3 つを
/// そこに混ぜると CLAUDE.md の「テストで外部ネットワークに出ない」を破る）。
pub fn wire_cluster_liveness_hooks(dispatcher: &mut Dispatcher, masters: ClusterMasters) {
    dispatcher.set_cluster_command_probe(Arc::new(
        |ssh_command: &[String], host: &str, timeout: std::time::Duration| {
            task_worker::ssh::control_master_command_probe_blocking(ssh_command, host, timeout)
        },
    ));
    dispatcher.set_cluster_master_watcher(cluster_master_watcher(masters));
}

/// ADR-0062 A（Phase 107）: `ClusterMasters` から、明示的な切断を経ずに終了した master を集める
/// フック。`try_wait_exit` は非破壊（ブロックしない）なので tick から直接呼んでよい。終了を見つけたら
/// マップから取り除く（同じ終了を二度返さないため。`ClusterMaster::kill` と同じくもう保持する意味が無い）。
pub(crate) fn cluster_master_watcher(
    masters: ClusterMasters,
) -> task_dispatch::dispatcher::ClusterMasterWatcher {
    Arc::new(move || {
        let mut exited = Vec::new();
        let Ok(mut held) = masters.lock() else {
            return exited;
        };
        let ids: Vec<String> = held.keys().cloned().collect();
        for id in ids {
            let Some(master) = held.get_mut(&id) else {
                continue;
            };
            if let Some(exit_code) = master.try_wait_exit() {
                let stderr_tail = master.stderr_tail(300);
                held.remove(&id);
                exited.push(task_dispatch::dispatcher::ClusterMasterExit {
                    cluster: id,
                    exit_code,
                    stderr_tail,
                });
            }
        }
        exited
    })
}

/// ADR-0053 D3（Phase 66）: celeris が spawn したフォールバックの `ssh -N -L` 子を保持する場所。
/// `-O forward` が届かないとき（bnode150 が pegasus から直接届かない等）だけここに増える。
/// Drop で全部落とす（celeris の終了とともに閉じる）。**master 本体（`ClusterMaster`）とは違い**、
/// この子は ADR-0060 の対象外（celeris の cgroup の中のまま。滅多に使わないフォールバック経路なので、
/// このフェーズでは触っていない）。
#[derive(Default)]
pub(crate) struct TunnelForwardRegistry(HashMap<String, std::process::Child>);

impl Drop for TunnelForwardRegistry {
    fn drop(&mut self) {
        for (_, mut child) in self.0.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(crate) type TunnelForwardChildren = Arc<std::sync::Mutex<TunnelForwardRegistry>>;

/// ADR-0053 D3: `[[clusters.forwards]]` を(再)確立するフック。
///
/// 1. まず master に `-O forward -L <listen>:<target> <host>` を頼む（軽い。master が `target` に
///    届けば十分。ADR-0053 D3 の本筋）。
/// 2. 届かなければ（`-O forward` が失敗）、master 上で別プロセスとして `ssh -N -L <listen>:<target> <host>`
///    を張る（ADR-0053 D3「bnode150 が直接届かないなら master 上で ssh -N -L を起こす」フォールバック）。
///    この子プロセスは `TunnelForwardChildren` に保持し、生きている間は二重に起こさない。
pub(crate) fn tunnel_forward_ensurer(
    children: TunnelForwardChildren,
) -> task_dispatch::dispatcher::TunnelForwardEnsurer {
    Arc::new(move |host: &str, listen: &str, target: &str| {
        let key = format!("{host}\u{0}{listen}\u{0}{target}");
        {
            let mut guard = children
                .lock()
                .map_err(|_| "the tunnel forward registry is poisoned".to_string())?;
            if let Some(child) = guard.0.get_mut(&key) {
                if matches!(child.try_wait(), Ok(None)) {
                    // まだ立ち上げ中／生きている。二重に起こさない。
                    return Ok(());
                }
                guard.0.remove(&key);
            }
        }
        let spec = format!("{listen}:{target}");
        let status = std::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-O", "forward", "-L", &spec, host])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(status, Ok(s) if s.success()) {
            tracing::info!(host, listen, target, "tunnel: forward added via -O forward");
            return Ok(());
        }
        tracing::warn!(
            host,
            listen,
            target,
            "tunnel: -O forward failed; falling back to a separate ssh -N -L"
        );
        let mut cmd = std::process::Command::new("ssh");
        cmd.args(["-o", "BatchMode=yes", "-N", "-L", &spec, host])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd
            .spawn()
            .map_err(|e| format!("could not start the fallback `ssh -N -L`: {e}"))?;
        tracing::info!(
            host,
            listen,
            target,
            "tunnel: forward added via a fallback ssh -N -L child"
        );
        let mut guard = children
            .lock()
            .map_err(|_| "the tunnel forward registry is poisoned".to_string())?;
        guard.0.insert(key, child);
        Ok(())
    })
}

/// ADR-0053 Phase 85: target（先方）の健康 probe の上限。`task_worker::PROBE_TIMEOUT`（3 秒。
/// `[knowledge.langmem]` 等の到達性検査と共有の既定値）より短くしてある。
///
/// ADR-0066 D3（Phase 110b）: この probe は、この forward を初めて観測する tick からは同期に
/// （`refresh_cluster_tunnels` が 1 回だけ種を蒔く）、それ以降は task-dispatch 側の専用スレッド
/// （tick とは無縁）から呼ばれる。同期に呼ばれるのは forward ごとに実質 1 回だけになったが、
/// タイムアウトは変えていない（専用スレッドから呼ばれるときも、先方が無応答なら早めに諦めて次の
/// forward・次の周回に進みたいため）。旧実装（tick が毎回 2〜3 秒級の probe を同期で待っていた）の
/// 経緯は Phase 85 の追記のとおり。
pub(crate) const TUNNEL_TARGET_PROBE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(2);

/// ADR-0053 D3 / Phase 85 / ADR-0066 D3: forward の target（先方）の健康を `GET http://<listen>/v1/models`
/// で見る（既存の probe をそのまま使うが、タイムアウトは短くしてある）。配線（`set_tunnel_probe`）は
/// 変わらない。呼び出し元（同期の種蒔きか、専用スレッドか）は task-dispatch 側が決める。
pub(crate) fn tunnel_probe() -> task_dispatch::dispatcher::TunnelProbe {
    Arc::new(|listen: &str| {
        let base = format!("http://{listen}/v1");
        // Qwen 側の中継はトンネルの向こう（bnode150）そのものであり、celeris の llm-proxy を経由しない
        // ので bearer は要らない（ADR-0052 の knowledge probe と同じ Qwen エンドポイントに対する既定）。
        match task_worker::probe_models(&base, TUNNEL_TARGET_PROBE_TIMEOUT, None) {
            task_worker::Reachability::Ok => Ok(()),
            task_worker::Reachability::Unreachable { reason }
            | task_worker::Reachability::Unknown { reason } => Err(reason),
        }
    })
}

/// ADR-0053 Phase 85: forward の**リスナー**（`-O forward`/`ssh -N -L` が手元の `listen` で実際に
/// 待ち受けているか）を見る。ssh は起こさず、`listen` への軽い TCP connect だけで判定する
/// （届けば「リスナーは有る」。中身の健康は見ない＝`tunnel_probe` の役目と分ける）。
pub(crate) fn tunnel_listener_probe() -> task_dispatch::dispatcher::TunnelListenerProbe {
    /// TCP connect 自体の上限。ローカルの loopback アドレスへの接続なので短くてよい。
    const LISTENER_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
    Arc::new(|listen: &str| {
        let Ok(addr) = listen.parse::<std::net::SocketAddr>() else {
            tracing::warn!(
                listen,
                "tunnel: could not parse the forward's listen address"
            );
            return false;
        };
        std::net::TcpStream::connect_timeout(&addr, LISTENER_CONNECT_TIMEOUT).is_ok()
    })
}
