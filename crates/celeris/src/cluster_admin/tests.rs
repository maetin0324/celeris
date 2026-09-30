use super::*;

fn config_with(auth: &str) -> Config {
    let text = format!(
        "[[clusters]]\nid = \"c1\"\nhost = \"h1\"\nauth = {auth:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
    );
    let cfg: Config = toml::from_str(&text).expect("config");
    cfg.validate().expect("valid config");
    cfg
}

fn channels() -> (
    tokio::sync::mpsc::Sender<ClusterConnectPending>,
    tokio::sync::mpsc::Receiver<ClusterConnectPending>,
) {
    tokio::sync::mpsc::channel(4)
}

/// ADR-0032 D5: 知らない id は 404 相当。**ssh は起動しない**（`reply` が即座に返る）。
#[tokio::test]
async fn unknown_cluster_is_not_found_for_all_three() {
    let config = config_with("totp");
    let sessions: ClusterConnectSessions = Default::default();
    let masters: ClusterMasters = Default::default();
    let (tx, _rx) = channels();

    let (reply, rx) = oneshot::channel();
    spawn_connect_start(
        &config,
        sessions.clone(),
        masters.clone(),
        "nope".into(),
        tx.clone(),
        reply,
    );
    assert!(matches!(rx.await, Ok(Err(ClusterAdminError::NotFound))));

    let (reply, rx) = oneshot::channel();
    spawn_connect_code(
        &config,
        sessions.clone(),
        masters.clone(),
        "nope".into(),
        "1".into(),
        tx.clone(),
        reply,
    );
    assert!(matches!(rx.await, Ok(Err(ClusterAdminError::NotFound))));

    let (reply, rx) = oneshot::channel();
    spawn_connect_cancel(&config, sessions, masters, "nope".into(), tx, reply);
    assert!(matches!(rx.await, Ok(Err(ClusterAdminError::NotFound))));
}

/// ADR-0032 D1/D5: `auth = "manual"` は「celeris は接続を張らない」ままなので 409 相当。
#[tokio::test]
async fn manual_cluster_refuses_to_connect() {
    let config = config_with("manual");
    let (tx, _rx) = channels();
    let (reply, rx) = oneshot::channel();
    spawn_connect_start(
        &config,
        Default::default(),
        Default::default(),
        "c1".into(),
        tx,
        reply,
    );
    assert!(matches!(rx.await, Ok(Err(ClusterAdminError::NotSupported))));
}

/// 進行中のセッションが無いのにコードを送ったら 409 相当。
#[tokio::test]
async fn code_without_a_session_is_not_started() {
    let config = config_with("totp");
    let (tx, _rx) = channels();
    let (reply, rx) = oneshot::channel();
    spawn_connect_code(
        &config,
        Default::default(),
        Default::default(),
        "c1".into(),
        "123456".into(),
        tx,
        reply,
    );
    assert!(matches!(rx.await, Ok(Err(ClusterAdminError::NotStarted))));
}

/// セッションが無くても取り消しは成功する（切れている＝要求は満たされている）。
#[tokio::test]
async fn cancel_without_a_session_succeeds() {
    let config = config_with("totp");
    let (tx, _rx) = channels();
    let (reply, rx) = oneshot::channel();
    spawn_connect_cancel(
        &config,
        Default::default(),
        Default::default(),
        "c1".into(),
        tx,
        reply,
    );
    assert!(matches!(rx.await, Ok(Ok(()))));
}

/// 掃除の関数はチャネルに送らない（B1 の規約）。空なら何も返さない。
#[tokio::test]
async fn expire_returns_ids_and_does_not_send_on_a_channel() {
    let sessions: ClusterConnectSessions = Default::default();
    let expired = expire_stale_cluster_sessions(&sessions, std::time::Duration::from_secs(0)).await;
    assert!(expired.is_empty());
}
