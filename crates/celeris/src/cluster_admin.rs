//! ADR-0032 D4 / D5: クラスタへの接続を GUI から張る（`POST /clusters/{id}/connect` の受け手）。
//!
//! `accounts_admin` と同じ作りにしてある。違いは扱うものが**アカウントの認証情報ではなく ssh の多重接続**で、
//! 人から受け取るのが URL ではなく**検証コード（TOTP）**という点。
//!
//! **秘密の規律（ADR-0032 D4）**: 検証コードはこのモジュールを素通りするだけで、ログにも応答にも
//! `Debug` にも出さない。プロンプト文字列は応答には返すがログには出さない（ユーザ名・ホスト名が入るため）。

use std::collections::HashMap;
use std::sync::Arc;

use task_api::{ClusterAdminError, ClusterConnectCodeOutcome, ClusterConnectStartOutcome};
use task_worker::cluster_login::{
    ClusterConnectSession, ClusterConnectStart, disconnect, start_connect,
};
use tokio::sync::{Mutex, oneshot};

use crate::ClusterMasters;
use crate::config::{ClusterConfig, Config};

/// プロンプトが出るまで待つ上限。人が押した直後なので、`start_connect` の中で ssh の起動も含む。
const PROMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// コードを流してから接続が成立するまで待つ上限。
const CODE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// 鍵だけで張るときの上限（人が待っているので、ディスパッチャの自動接続より長く取ってよい）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// 進行中のセッションを捨てるまでの時間（ADR-0032 D4）。TOTP の有効期間より十分長く、放置は掃除する。
pub const SESSION_EXPIRY: std::time::Duration = std::time::Duration::from_secs(300);

/// 進行中の接続セッション（クラスタ id ごとに高々 1 つ）。プロセスの生存期間だけ持ち、DB には書かない。
pub type ClusterConnectSessions = Arc<Mutex<HashMap<String, ClusterConnectSession>>>;

/// ディスパッチャに `connect_pending` を反映してもらうための知らせ（`AccountAdminEvent` と同じ役目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConnectPending {
    pub id: String,
    pub pending: bool,
}

fn find_cluster<'a>(config: &'a Config, id: &str) -> Result<&'a ClusterConfig, ClusterAdminError> {
    config
        .clusters
        .iter()
        .find(|c| c.id == id)
        .ok_or(ClusterAdminError::NotFound)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// ADR-0032 D5: `POST /clusters/{id}/connect`。
///
/// `auth = "manual"` は「celeris は接続を張らない」ままなので `NotSupported`（409）。
/// `"publickey"` はコード無しで張る。`"totp"` はプロンプトを拾って `needs_code` を返す。
pub fn spawn_connect_start(
    config: &Config,
    sessions: ClusterConnectSessions,
    masters: ClusterMasters,
    id: String,
    events: tokio::sync::mpsc::Sender<ClusterConnectPending>,
    reply: oneshot::Sender<Result<ClusterConnectStartOutcome, ClusterAdminError>>,
) {
    let cluster = match find_cluster(config, &id) {
        Ok(c) => c,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    if cluster.auth == "manual" {
        let _ = reply.send(Err(ClusterAdminError::NotSupported));
        return;
    }
    let host = cluster.host.clone();
    let interactive = cluster.auth == "totp";
    let keepalive_secs = cluster.keepalive_secs;
    // ADR-0078 D1。
    let control_persist = cluster.control_persist.clone();
    // ADR-0060（Phase 103）: master の起こし方を解決する（環境の判定は同期・軽いのでここで済ませる）。
    let launcher = task_worker::cluster_login::resolve_master_launcher(
        &cluster.master_launcher,
        task_worker::cluster_login::systemd_run_on_path(),
        task_worker::cluster_login::xdg_runtime_dir_is_set(),
    );

    tokio::spawn(async move {
        // 押し直しに備えて、古いセッションは先に畳む（アカウントのログインと同じ）。
        if let Some(old) = sessions.lock().await.remove(&id) {
            old.cancel().await;
        }
        let outcome = start_connect(
            &["ssh".to_string()],
            &host,
            &id,
            &launcher,
            interactive,
            PROMPT_TIMEOUT,
            CONNECT_TIMEOUT,
            keepalive_secs,
            &control_persist,
        )
        .await;
        match outcome {
            Ok(ClusterConnectStart::Connected(master)) => {
                hold_master(&masters, &id, master);
                tracing::info!(who = "admin", op = "cluster_connect", cluster = %id, "cluster: connected");
                let _ = reply.send(Ok(ClusterConnectStartOutcome {
                    kind: "connected".to_string(),
                    prompt: None,
                    expires_at_unix: None,
                }));
            }
            Ok(ClusterConnectStart::NeedsCode { prompt, session }) => {
                sessions.lock().await.insert(id.clone(), session);
                let _ = events
                    .send(ClusterConnectPending {
                        id: id.clone(),
                        pending: true,
                    })
                    .await;
                // プロンプトの中身はログに出さない（D4）。
                tracing::info!(
                    who = "admin",
                    op = "cluster_connect",
                    cluster = %id,
                    "cluster: waiting for a verification code"
                );
                // ADR-0078 D5: TOTP のプロンプトを人へ中継した回数を journal で数えるための固定の文言
                // （コードもプロンプトの中身も書かない）。
                tracing::info!(cluster = %id, "cluster totp prompt relayed");
                let _ = reply.send(Ok(ClusterConnectStartOutcome {
                    kind: "needs_code".to_string(),
                    prompt: Some(prompt),
                    expires_at_unix: Some(unix_now() + SESSION_EXPIRY.as_secs() as i64),
                }));
            }
            Err(e) => {
                tracing::warn!(who = "admin", op = "cluster_connect", cluster = %id, error = %e, "cluster: connect failed");
                let _ = reply.send(Err(ClusterAdminError::Failed(e.to_string())));
            }
        }
    });
}

/// ADR-0032 D5: `POST /clusters/{id}/connect/code`。**コードはここを素通りするだけ**。
pub fn spawn_connect_code(
    config: &Config,
    sessions: ClusterConnectSessions,
    masters: ClusterMasters,
    id: String,
    code: String,
    events: tokio::sync::mpsc::Sender<ClusterConnectPending>,
    reply: oneshot::Sender<Result<ClusterConnectCodeOutcome, ClusterAdminError>>,
) {
    if let Err(e) = find_cluster(config, &id) {
        let _ = reply.send(Err(e));
        return;
    }
    tokio::spawn(async move {
        let Some(session) = sessions.lock().await.remove(&id) else {
            let _ = reply.send(Err(ClusterAdminError::NotStarted));
            return;
        };
        let result = session.submit_code(&code, CODE_WAIT).await;
        // 成否にかかわらずセッションは終わったので pending を降ろす。
        let _ = events
            .send(ClusterConnectPending {
                id: id.clone(),
                pending: false,
            })
            .await;
        match result {
            // `None` は失敗ではない（ssh が `ControlPersist` で master を切り離した場合。保持する子が無いだけ）。
            Ok(master) => {
                hold_master(&masters, &id, master);
                tracing::info!(who = "admin", op = "cluster_connect_code", cluster = %id, "cluster: connected");
                let _ = reply.send(Ok(ClusterConnectCodeOutcome {
                    ok: true,
                    detail: None,
                }));
            }
            Err(task_worker::cluster_login::ClusterConnectError::InvalidCode) => {
                // コードそのものは出さない。
                let _ = reply.send(Err(ClusterAdminError::InvalidCode));
            }
            Err(e) => {
                tracing::warn!(who = "admin", op = "cluster_connect_code", cluster = %id, error = %e, "cluster: connect failed");
                let _ = reply.send(Ok(ClusterConnectCodeOutcome {
                    ok: false,
                    detail: Some(e.to_string()),
                }));
            }
        }
    });
}

/// ADR-0032 D5: `DELETE /clusters/{id}/connect`。進行中のセッションを取り消し、celeris が持っている
/// master があれば落とす。人が張った master は `ssh -O exit` で落とす。
pub fn spawn_connect_cancel(
    config: &Config,
    sessions: ClusterConnectSessions,
    masters: ClusterMasters,
    id: String,
    events: tokio::sync::mpsc::Sender<ClusterConnectPending>,
    reply: oneshot::Sender<Result<(), ClusterAdminError>>,
) {
    let cluster = match find_cluster(config, &id) {
        Ok(c) => c,
        Err(e) => {
            let _ = reply.send(Err(e));
            return;
        }
    };
    let host = cluster.host.clone();
    tokio::spawn(async move {
        if let Some(session) = sessions.lock().await.remove(&id) {
            session.cancel().await;
            let _ = events
                .send(ClusterConnectPending {
                    id: id.clone(),
                    pending: false,
                })
                .await;
        }
        // celeris が保持している master を落とす（ADR-0060: 明示的な切断なので、`ClusterMaster::kill`
        // でプロセスグループごと SIGKILL する。通常の Drop はもう殺さない）。
        drop_master(&masters, &id).await;
        // 人が張った master が残っているかもしれないので、こちらも閉じる。
        let result = disconnect(&["ssh".to_string()], &host).await;
        tracing::info!(who = "admin", op = "cluster_disconnect", cluster = %id, "cluster: disconnect requested");
        match result {
            Ok(()) => {
                let _ = reply.send(Ok(()));
            }
            // 既に接続が無い場合も「切れている」という要求は満たされているので成功にする。
            Err(_) => {
                let _ = reply.send(Ok(()));
            }
        }
    });
}

/// `ClusterMaster` を落とすと接続も切れるので、生かしておきたい間は登録簿に置く（ADR-0032 D2）。
fn hold_master(
    masters: &ClusterMasters,
    id: &str,
    master: Option<task_worker::cluster_login::ClusterMaster>,
) {
    let Some(master) = master else {
        // 人が張った master を見つけた場合。celeris の持ち物ではないので登録しない。
        return;
    };
    match masters.lock() {
        Ok(mut held) => {
            held.insert(id.to_string(), master);
        }
        Err(_) => {
            tracing::warn!(cluster = %id, "cluster: master registry is poisoned; the connection will close")
        }
    }
}

/// ADR-0060: 登録簿から取り除くだけでは master は死なない（通常の Drop はもう殺さない）。
/// 明示的な切断（`DELETE /clusters/{id}/connect`）だからここで `ClusterMaster::kill` を呼ぶ。
async fn drop_master(masters: &ClusterMasters, id: &str) {
    let master = match masters.lock() {
        Ok(mut held) => held.remove(id),
        Err(_) => {
            tracing::warn!(cluster = %id, "cluster: master registry is poisoned");
            None
        }
    };
    if let Some(master) = master {
        master.kill().await;
    }
}

/// 放置されたセッションを畳む。**`accounts_admin::expire_stale_logins` と同じ規約（B1）**:
/// この関数はチャネルに送らない。掃除した id を返し、呼び出し側（`tick_loop`）が Dispatcher に反映する。
pub async fn expire_stale_cluster_sessions(
    sessions: &ClusterConnectSessions,
    expiry: std::time::Duration,
) -> Vec<String> {
    let mut expired = Vec::new();
    let mut guard = sessions.lock().await;
    let stale: Vec<String> = guard
        .iter()
        .filter(|(_, s)| s.started_at().elapsed() >= expiry)
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        if let Some(session) = guard.remove(&id) {
            session.cancel().await;
            tracing::info!(cluster = %id, "cluster: connect session expired");
            expired.push(id);
        }
    }
    expired
}

#[cfg(test)]
#[path = "cluster_admin/tests.rs"]
mod tests;
