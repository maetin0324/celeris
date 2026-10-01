use super::*;

fn stub_command(dir: &std::path::Path, script: &str) -> String {
    use std::io::Write;
    let path = dir.join("claude_stub.sh");
    // ETXTBSY 対策（ADR-0010 D10）: テストプロセス自身が書き込み fd を持つと、並行するテストの fork に
    // 継承されて exec が `Text file busy` で失敗しうる。task-worker の `test_support` と同じく別プロセスで書く。
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("#!/bin/sh\n{script}\n").as_bytes())
        .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    path.to_string_lossy().into_owned()
}

fn config_with_accounts(claude_dir: PathBuf, command: String) -> Config {
    let text = format!(
        "[accounts]\nclaude_dir = {claude_dir:?}\n[adapters.claude_code]\ncommand = {command:?}\n[[providers]]\nid = \"x\"\nadapter = \"claude-code\"\n"
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    cfg.validate().unwrap();
    cfg
}

fn config_with_codex_accounts(codex_dir: PathBuf, command: String) -> Config {
    let text = format!(
        "[accounts]\ncodex_dir = {codex_dir:?}\n[adapters.codex]\ncommand = {command:?}\n[[providers]]\nid = \"x\"\nadapter = \"codex\"\n"
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    cfg.validate().unwrap();
    cfg
}

#[tokio::test]
async fn spawn_check_reports_ok_and_records_observation_via_event() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'"#,
    );
    let config = config_with_accounts(acct, command);
    let (tx, mut rx) = mpsc::channel(4);
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_check(
        &config,
        AccountAdapter::ClaudeCode,
        "a".to_string(),
        tx,
        reply_tx,
    );
    let outcome = reply_rx.await.unwrap().unwrap();
    assert_eq!(outcome.result, task_api::ProviderCheckResult::Ok);
    let event = rx.recv().await.unwrap();
    match event {
        AccountAdminEvent::Checked {
            adapter,
            id,
            result,
            ..
        } => {
            assert_eq!(adapter, AccountAdapter::ClaudeCode);
            assert_eq!(id, "a");
            assert_eq!(result, "ok");
        }
        _ => panic!("expected Checked event"),
    }
}

#[tokio::test]
async fn spawn_check_missing_account_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&acct).unwrap();
    let config = config_with_accounts(acct, "claude".into());
    let (tx, _rx) = mpsc::channel(4);
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_check(
        &config,
        AccountAdapter::ClaudeCode,
        "missing".to_string(),
        tx,
        reply_tx,
    );
    assert!(matches!(
        reply_rx.await.unwrap(),
        Err(AccountAdminError::NotFound)
    ));
}

/// ADR-0025 D4: codex の確認は `[accounts].check_model` を使わず、`check_account_codex` に委譲する。
#[tokio::test]
async fn spawn_check_codex_reports_ok_and_records_observation() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"read -r init
echo '{"id":1,"result":{}}'
read -r initialized
read -r account
echo '{"id":2,"result":{"account":{"type":"chatgpt"}}}'
read -r limits
echo '{"id":3,"result":{"rateLimits":{"primary":{"usedPercent":25,"windowDurationMins":300,"resetsAt":2000000000}}}}'"#,
    );
    let config = config_with_codex_accounts(acct, command);
    let (tx, mut rx) = mpsc::channel(4);
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_check(
        &config,
        AccountAdapter::Codex,
        "a".to_string(),
        tx,
        reply_tx,
    );
    let outcome = reply_rx.await.unwrap().unwrap();
    assert_eq!(outcome.result, task_api::ProviderCheckResult::Ok);
    let event = rx.recv().await.unwrap();
    match event {
        AccountAdminEvent::Checked {
            adapter,
            id,
            result,
            ..
        } => {
            assert_eq!(adapter, AccountAdapter::Codex);
            assert_eq!(id, "a");
            assert_eq!(result, "ok");
        }
        _ => panic!("expected Checked event"),
    }
}

/// ADR-0025 D4: 未ログイン（401）なら `spawn_check` の結果は `auth_failed`。
#[tokio::test]
async fn spawn_check_codex_auth_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"read -r init
echo '{"id":1,"error":{"message":"401 Unauthorized"}}'"#,
    );
    let config = config_with_codex_accounts(acct, command);
    let (tx, _rx) = mpsc::channel(4);
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_check(
        &config,
        AccountAdapter::Codex,
        "a".to_string(),
        tx,
        reply_tx,
    );
    let outcome = reply_rx.await.unwrap().unwrap();
    assert_eq!(outcome.result, task_api::ProviderCheckResult::AuthFailed);
}

#[tokio::test]
async fn login_start_code_cancel_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"printf "visit: \033]8;;https://claude.com/cai/oauth/authorize?x=1\007https://claude.com/cai/oauth/authorize?x=1\033]8;;\007\n"
printf 'code> '
read -r code
if [ "$code" = "good-code" ]; then
  printf '%s' '{}' > "$CLAUDE_SECURESTORAGE_CONFIG_DIR/.credentials.json"
  exit 0
fi
exit 1
"#,
    );
    let config = config_with_accounts(acct.clone(), command);
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let (tx, mut rx) = mpsc::channel(8);

    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_start(
        &config,
        sessions.clone(),
        codex_sessions.clone(),
        AccountAdapter::ClaudeCode,
        "a".to_string(),
        tx.clone(),
        reply_tx,
    );
    let started = reply_rx.await.unwrap().unwrap();
    assert_eq!(started.url, "https://claude.com/cai/oauth/authorize?x=1");
    assert_eq!(started.user_code, None);
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending { pending: true, .. })
    ));
    assert!(sessions.lock().await.contains_key("a"));

    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_code(
        sessions.clone(),
        "a".to_string(),
        "good-code".to_string(),
        tx.clone(),
        reply_tx,
    );
    let result = reply_rx.await.unwrap().unwrap();
    assert!(result.ok);
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending { pending: false, .. })
    ));
    assert!(acct.join("a").join(".credentials.json").is_file());
    assert!(!sessions.lock().await.contains_key("a"));

    // 進行中でないときの login/code は LoginNotStarted。
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_code(
        sessions.clone(),
        "a".to_string(),
        "x".to_string(),
        tx.clone(),
        reply_tx,
    );
    assert!(matches!(
        reply_rx.await.unwrap(),
        Err(AccountAdminError::LoginNotStarted)
    ));

    // cancel は進行中でなくても成功する（no-op）。
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_cancel(
        sessions.clone(),
        codex_sessions.clone(),
        AccountAdapter::ClaudeCode,
        "a".to_string(),
        tx,
        reply_tx,
    );
    assert!(reply_rx.await.unwrap().is_ok());
}

/// ADR-0025 D5: codex のログインは `login_start` が `user_code` を返し、`login/code` を使わない
/// （celeris 側はそのメッセージを扱わない。task-api が 409 を返す）。完了はポーリング（`poll_codex_logins`）で
/// 検知する。
#[tokio::test]
async fn codex_login_start_returns_user_code_and_poll_detects_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"printf 'https://auth.openai.com/codex/device\n'
printf 'ABCD-EFGHI\n'
sleep 0.2
printf '%s' '{}' > "$CODEX_HOME/auth.json"
exit 0
"#,
    );
    let config = config_with_codex_accounts(acct.clone(), command);
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let (tx, mut rx) = mpsc::channel(8);

    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_start(
        &config,
        sessions,
        codex_sessions.clone(),
        AccountAdapter::Codex,
        "a".to_string(),
        tx,
        reply_tx,
    );
    let started = reply_rx.await.unwrap().unwrap();
    assert_eq!(started.url, "https://auth.openai.com/codex/device");
    assert_eq!(started.user_code.as_deref(), Some("ABCD-EFGHI"));
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending {
            adapter: AccountAdapter::Codex,
            pending: true,
            ..
        })
    ));
    assert!(codex_sessions.lock().await.contains_key("a"));

    for _ in 0..100 {
        let finished = poll_codex_logins(&codex_sessions).await;
        if !finished.is_empty() {
            assert_eq!(finished, vec![("a".to_string(), true)]);
            assert!(!codex_sessions.lock().await.contains_key("a"));
            assert!(acct.join("a").join("auth.json").is_file());
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("codex login never finished");
}

/// N12 相当: `expire_stale_codex_logins` の期限は呼び出し側が注入できる。
#[tokio::test]
async fn expire_stale_codex_logins_cancels_sessions_older_than_the_injected_expiry() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"printf 'https://auth.openai.com/codex/device\n'
printf 'ABCD-EFGHI\n'
sleep 30
"#,
    );
    let config = config_with_codex_accounts(acct, command);
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let (tx, mut rx) = mpsc::channel(8);

    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_start(
        &config,
        sessions,
        codex_sessions.clone(),
        AccountAdapter::Codex,
        "a".to_string(),
        tx,
        reply_tx,
    );
    reply_rx.await.unwrap().unwrap();
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending { pending: true, .. })
    ));
    assert!(codex_sessions.lock().await.contains_key("a"));

    let expired = expire_stale_codex_logins(&codex_sessions, Duration::from_secs(600)).await;
    assert!(expired.is_empty());

    tokio::time::sleep(Duration::from_millis(20)).await;
    let expired = expire_stale_codex_logins(&codex_sessions, Duration::from_millis(10)).await;
    assert_eq!(expired, vec!["a".to_string()]);
    assert!(!codex_sessions.lock().await.contains_key("a"));
}

/// N12: `expire_stale_logins` の期限は呼び出し側が注入できる（実際に 10 分待たずにテストできる）。
#[tokio::test]
async fn expire_stale_logins_cancels_sessions_older_than_the_injected_expiry() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    let command = stub_command(
        tmp.path(),
        r#"printf "visit: \033]8;;https://claude.com/cai/oauth/authorize?x=1\007https://claude.com/cai/oauth/authorize?x=1\033]8;;\007\n"
sleep 30
"#,
    );
    let config = config_with_accounts(acct.clone(), command);
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let (tx, mut rx) = mpsc::channel(8);

    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_start(
        &config,
        sessions.clone(),
        codex_sessions,
        AccountAdapter::ClaudeCode,
        "a".to_string(),
        tx,
        reply_tx,
    );
    reply_rx.await.unwrap().unwrap();
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending { pending: true, .. })
    ));
    assert!(sessions.lock().await.contains_key("a"));

    // 期限内なら何も打ち切らない。
    let expired = expire_stale_logins(&sessions, Duration::from_secs(600)).await;
    assert!(expired.is_empty());
    assert!(sessions.lock().await.contains_key("a"));

    // 短い expiry を注入すれば、実際に 10 分待たなくても打ち切りを確認できる。
    tokio::time::sleep(Duration::from_millis(20)).await;
    let expired = expire_stale_logins(&sessions, Duration::from_millis(10)).await;
    assert_eq!(expired, vec!["a".to_string()]);
    assert!(!sessions.lock().await.contains_key("a"));
}

#[tokio::test]
async fn usage_poll_skips_logged_out_and_logging_in_accounts_and_throttles_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("codex-accounts");
    for id in ["a", "logged-out"] {
        std::fs::create_dir_all(acct.join(id)).unwrap();
    }
    std::fs::write(acct.join("a/auth.json"), "{}").unwrap();
    let command = stub_command(
        tmp.path(),
        r#"read -r init
echo '{"id":1,"result":{}}'
read -r initialized
read -r account
echo '{"id":2,"result":{"account":{"type":"apiKey"}}}'"#,
    );
    let mut config = config_with_codex_accounts(acct, command);
    config.db.path = tmp.path().join("db");
    config.workspace_root = tmp.path().join("ws");
    let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
    let mut checks = UsageChecks::default();
    let (tx, mut rx) = mpsc::channel(8);
    dispatcher.set_account_login_pending(AccountAdapter::Codex, "a", true);
    checks.poll(&config, &dispatcher, tx.clone());
    assert!(checks.last.is_empty());
    dispatcher.set_account_login_pending(AccountAdapter::Codex, "a", false);
    checks.poll(&config, &dispatcher, tx.clone());
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, AccountAdminEvent::Checked { adapter: AccountAdapter::Codex, result, .. } if result == "ok")
    );
    assert_eq!(checks.last.len(), 1);
    assert!(checks.last.contains_key("a"));
    checks.poll(&config, &dispatcher, tx.clone());
    assert!(rx.try_recv().is_err());
    checks.last.insert(
        "a".into(),
        std::time::Instant::now() - Duration::from_secs(301),
    );
    checks.poll(&config, &dispatcher, tx);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .is_some()
    );
}

// ---- S2+S8: remove_account ----

fn config_for_dispatcher(tmp: &std::path::Path, claude_dir: PathBuf) -> Config {
    let mut config = config_with_accounts(claude_dir, "claude".into());
    config.db.path = tmp.join("celeris.db");
    config.workspace_root = tmp.join("ws");
    config
}

#[tokio::test]
async fn remove_account_moves_the_directory_cancels_login_and_clears_the_book() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(acct.join("a")).unwrap();
    std::fs::write(acct.join("a").join(".credentials.json"), "{}").unwrap();
    // 既存の観測値がある帳簿を用意しておき、削除後に消えることを確かめる。
    {
        let mut book = task_dispatch::AccountBook::load(&acct.join(".celeris-usage.json"));
        book.record_check(
            "a",
            task_dispatch::AccountCheckRecord {
                at: 1,
                result: "ok".into(),
                detail: None,
            },
        );
        book.save().unwrap();
    }
    let config = config_for_dispatcher(tmp.path(), acct.clone());
    let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    // 進行中のログインがあれば、削除で止められる。
    let login_command = stub_command(
        tmp.path(),
        r#"printf "visit: \033]8;;https://claude.com/cai/oauth/authorize?x=1\007https://claude.com/cai/oauth/authorize?x=1\033]8;;\007\n"
sleep 30
"#,
    );
    let login_config = config_with_accounts(acct.clone(), login_command);
    let (tx, mut rx) = mpsc::channel(8);
    let (reply_tx, reply_rx) = oneshot::channel();
    spawn_login_start(
        &login_config,
        sessions.clone(),
        codex_sessions.clone(),
        AccountAdapter::ClaudeCode,
        "a".to_string(),
        tx.clone(),
        reply_tx,
    );
    reply_rx.await.unwrap().unwrap();
    assert!(matches!(
        rx.recv().await,
        Some(AccountAdminEvent::LoginPending { pending: true, .. })
    ));

    let result = remove_account(
        &config,
        &mut dispatcher,
        &sessions,
        &codex_sessions,
        AccountAdapter::ClaudeCode,
        "a",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(!acct.join("a").exists());
    assert!(!sessions.lock().await.contains_key("a"));
    let removed: Vec<_> = std::fs::read_dir(acct.join(".removed")).unwrap().collect();
    assert_eq!(removed.len(), 1);
    let moved = removed.into_iter().next().unwrap().unwrap().path();
    assert!(
        moved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("a-")
    );
    assert!(
        moved.join(".credentials.json").is_file(),
        "credentials are not deleted, just moved"
    );

    let reloaded = task_dispatch::AccountBook::load(&acct.join(".celeris-usage.json"));
    assert!(
        reloaded.state("a").is_none(),
        "account book entry should be cleared"
    );
}

#[tokio::test]
async fn remove_account_missing_directory_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let acct = tmp.path().join("claude-accounts");
    std::fs::create_dir_all(&acct).unwrap();
    let config = config_for_dispatcher(tmp.path(), acct);
    let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let result = remove_account(
        &config,
        &mut dispatcher,
        &sessions,
        &codex_sessions,
        AccountAdapter::ClaudeCode,
        "missing",
    )
    .await;
    assert!(matches!(result, Err(AccountAdminError::NotFound)));
}

/// ADR-0025 D1: codex の `remove_account` は codex の根ディレクトリだけを見る（claude と混同しない）。
#[tokio::test]
async fn remove_account_codex_uses_the_codex_root() {
    let tmp = tempfile::tempdir().unwrap();
    let codex_dir = tmp.path().join("codex-accounts");
    std::fs::create_dir_all(codex_dir.join("a")).unwrap();
    std::fs::write(codex_dir.join("a").join("auth.json"), "{}").unwrap();
    let mut config = config_with_codex_accounts(codex_dir.clone(), "codex".into());
    config.db.path = tmp.path().join("celeris.db");
    config.workspace_root = tmp.path().join("ws");
    let mut dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
    let sessions = new_sessions();
    let codex_sessions = new_codex_sessions();
    let result = remove_account(
        &config,
        &mut dispatcher,
        &sessions,
        &codex_sessions,
        AccountAdapter::Codex,
        "a",
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(!codex_dir.join("a").exists());
    let removed: Vec<_> = std::fs::read_dir(codex_dir.join(".removed"))
        .unwrap()
        .collect();
    assert_eq!(removed.len(), 1);
}
