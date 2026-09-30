use super::*;

fn stub(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("codex_stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    path
}

fn account_dir(root: &Path, id: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).expect("account dir");
    dir
}

// app-server の要求を確認して返す。推論コマンドを起こしたら失敗する。
fn check_stub(dir: &Path, response: &str) -> PathBuf {
    stub(
        dir,
        &format!(
            r#"
[ "$1" = app-server ] || exit 9
read -r init
case "$init" in *initialize*) ;; *) exit 10 ;; esac
echo '{{"id":1,"result":{{}}}}'
read -r initialized
case "$initialized" in *initialized*) ;; *) exit 11 ;; esac
read -r account
case "$account" in *account/read*) ;; *) exit 12 ;; esac
echo '{{"id":2,"result":{{"account":{{"type":"chatgpt"}}}}}}'
read -r limits
case "$limits" in *account/rateLimits/read*) ;; *) exit 13 ;; esac
{response}
"#
        ),
    )
}

#[tokio::test]
async fn check_account_codex_reads_limits_without_inference() {
    let dir = tempfile::tempdir().unwrap();
    let command = check_stub(
        dir.path(),
        r#"echo '{"method":"account/updated","params":{}}'
echo '{"id":3,"result":{"rateLimits":{"primary":{"usedPercent":14,"windowDurationMins":300,"resetsAt":2000000000},"secondary":{"usedPercent":24,"windowDurationMins":10080,"resetsAt":2000600000}}}}'"#,
    );
    let acct = account_dir(dir.path(), "a");
    let check = check_account_codex(
        command.to_str().unwrap(),
        &acct,
        Duration::from_secs(5),
        &[],
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::Ok);
    let obs = check.observation.unwrap();
    assert_eq!(obs.five_hour.unwrap().utilization, 0.14);
    assert_eq!(obs.five_hour.unwrap().resets_at, 2000000000);
    assert_eq!(obs.seven_day.unwrap().utilization, 0.24);
}

#[tokio::test]
async fn check_account_codex_failures_are_bounded_and_sanitized() {
    let dir = tempfile::tempdir().unwrap();
    let acct = account_dir(dir.path(), "a");
    for (response, expected) in [
        (
            r#"echo '{"id":3,"error":{"message":"401 Unauthorized secret-token"}}'"#,
            AccountCheckResult::AuthFailed,
        ),
        (
            r#"echo '{"id":3,"error":{"message":"usage limit"}}'"#,
            AccountCheckResult::Throttled,
        ),
        ("echo invalid-json", AccountCheckResult::SpawnFailed),
        ("exit 0", AccountCheckResult::SpawnFailed),
        ("sleep 30", AccountCheckResult::SpawnFailed),
    ] {
        let command = check_stub(dir.path(), response);
        let check = check_account_codex(
            command.to_str().unwrap(),
            &acct,
            Duration::from_millis(300),
            &[],
        )
        .await;
        assert_eq!(check.result, expected, "{response}");
        assert!(!check.detail.unwrap().contains("secret-token"));
    }
}

#[tokio::test]
async fn check_account_codex_handles_logged_out_and_api_key_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let acct = account_dir(dir.path(), "a");
    for (account, expected) in [
        ("null", AccountCheckResult::AuthFailed),
        (r#"{"type":"apiKey"}"#, AccountCheckResult::Ok),
    ] {
        let command = stub(
            dir.path(),
            &format!(
                r#"read -r init
echo '{{"id":1,"result":{{}}}}'
read -r initialized
read -r account
echo '{{"id":2,"result":{{"account":{account}}}}}'
# 次の要求は無い。親が終了させる。
sleep 30"#
            ),
        );
        let check = check_account_codex(
            command.to_str().unwrap(),
            &acct,
            Duration::from_secs(2),
            &[],
        )
        .await;
        assert_eq!(check.result, expected);
        assert!(check.observation.is_none());
    }
}

#[tokio::test]
async fn check_account_codex_spawn_failure_for_nonexistent_command() {
    let dir = tempfile::tempdir().unwrap();
    let acct = account_dir(dir.path(), "a");
    let check = check_account_codex("/nonexistent/codex", &acct, Duration::from_secs(1), &[]).await;
    assert_eq!(check.result, AccountCheckResult::SpawnFailed);
}

// ---- device login ----

/// ADR-0025 D5 の実測どおり、ANSI で色付けされた URL とコードを出す（標準入力は使わず、コード入力完了後に
/// exit 0 する）。
fn login_stub(dir: &Path, ok: bool) -> PathBuf {
    let exit_code = if ok { 0 } else { 1 };
    let write_auth_json = if ok {
        r#"printf '%s' '{}' > "$CODEX_HOME/auth.json""#
    } else {
        "true"
    };
    stub(
        dir,
        &format!(
            "printf '\\033[1mVisit\\033[0m https://auth.openai.com/codex/device and enter code:\\n'\n\
                 printf '\\033[32mABCD-EFGHI\\033[0m\\n'\n\
                 sleep 0.2\n\
                 {write_auth_json}\n\
                 exit {exit_code}\n"
        ),
    )
}

#[tokio::test]
async fn start_login_codex_extracts_url_and_code_without_ansi() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path(), true);
    let acct = account_dir(dir.path(), "a");
    let session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    assert_eq!(session.url, "https://auth.openai.com/codex/device");
    assert_eq!(session.user_code, "ABCD-EFGHI");
    let result = session.wait(Duration::from_secs(5)).await;
    assert_eq!(result.result, LoginOutcome::Ok);
    assert!(acct.join("auth.json").is_file());
}

/// 回帰: 子が URL とコードを出して**すぐに終了**しても、終了検知の前にパイプに残った出力を読み切ってから
/// 探すので「URL を出す前に終了した」と誤判定しない（accounts_admin の codex ログインテストが CI 負荷下で
/// `ProcessExited` になった競合）。読み取りタスクが追いつく前に終了させるため、遅延なしの stub を繰り返す。
#[tokio::test]
async fn start_login_codex_finds_url_and_code_even_when_the_process_exits_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"printf '%s\n' 'https://auth.openai.com/codex/device' 'ABCD-EFGHI'
printf '%s' '{}' > "$CODEX_HOME/auth.json"
exit 0
"#,
    );
    for i in 0..20 {
        let acct = account_dir(dir.path(), &format!("a{i}"));
        let session = start_login_codex(
            command.to_str().unwrap(),
            &acct,
            &[],
            Duration::from_secs(5),
        )
        .await
        .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        assert_eq!(session.url, "https://auth.openai.com/codex/device");
        assert_eq!(session.user_code, "ABCD-EFGHI");
        let result = session.wait(Duration::from_secs(5)).await;
        assert_eq!(result.result, LoginOutcome::Ok);
        assert!(acct.join("auth.json").is_file());
    }
}

/// 上の回帰の裏: 出力を読み切っても URL が無ければ、従来どおり `ProcessExited`（`Timeout` ではない）。
#[tokio::test]
async fn start_login_codex_still_reports_process_exited_when_no_url_was_printed() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        "printf 'codex 0.1.0\nno device flow here\n'
exit 0
",
    );
    let acct = account_dir(dir.path(), "a");
    let err = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect_err("must fail");
    assert!(matches!(err, LoginError::ProcessExited), "{err}");
}

#[tokio::test]
async fn wait_fails_when_process_exits_nonzero_even_without_auth_json() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path(), false);
    let acct = account_dir(dir.path(), "a");
    let session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session.wait(Duration::from_secs(5)).await;
    assert_eq!(result.result, LoginOutcome::Failed);
    assert!(!acct.join("auth.json").is_file());
}

/// D5: exit 0 でも `auth.json` が無ければ失敗として扱う（成功の判定は両方の条件を要る）。
#[tokio::test]
async fn wait_fails_when_exit_zero_but_auth_json_missing() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"printf 'https://auth.openai.com/codex/device\n'
printf 'ABCD-EFGHI\n'
exit 0
"#,
    );
    let acct = account_dir(dir.path(), "a");
    let session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session.wait(Duration::from_secs(5)).await;
    assert_eq!(result.result, LoginOutcome::Failed);
}

#[tokio::test]
async fn cancel_kills_the_login_process() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"printf 'https://auth.openai.com/codex/device\n'
printf 'ABCD-EFGHI\n'
sleep 30
"#,
    );
    let acct = account_dir(dir.path(), "a");
    let session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let pid = session.child.as_ref().and_then(|c| c.id()).expect("pid");
    session.cancel();
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process {pid} is still alive after cancel()");
}

/// ADR-0025 D5: `try_finished` はブロックせず、まだ実行中なら `None`、終了していれば結果を返す
/// （tick ごとのポーリング用。celeris の実装がこちらを使う）。
#[tokio::test]
async fn try_finished_polls_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path(), true);
    let acct = account_dir(dir.path(), "a");
    let mut session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    // Not finished yet (the stub sleeps 0.2s before exiting).
    assert_eq!(session.try_finished(), None);
    for _ in 0..100 {
        if let Some(result) = session.try_finished() {
            assert_eq!(result.result, LoginOutcome::Ok);
            assert!(acct.join("auth.json").is_file());
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("session never finished");
}

/// ADR-0025 D5: 15 分の上限（テストでは短い値を注入する）を超えると `wait` は打ち切って `Failed` を返す。
#[tokio::test]
async fn wait_expires_and_kills_the_process_after_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"printf 'https://auth.openai.com/codex/device\n'
printf 'ABCD-EFGHI\n'
sleep 30
"#,
    );
    let acct = account_dir(dir.path(), "a");
    let session = start_login_codex(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let pid = session.child.as_ref().and_then(|c| c.id()).expect("pid");
    let result = session.wait(Duration::from_millis(200)).await;
    assert_eq!(result.result, LoginOutcome::Failed);
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process {pid} is still alive after wait() timed out");
}

// ---- extract_device_code ----

#[test]
fn extract_device_code_ignores_the_url_and_finds_the_code() {
    let text = b"Visit https://auth.openai.com/codex/device and enter\nABCD-EFGHI\n";
    assert_eq!(extract_device_code(text).as_deref(), Some("ABCD-EFGHI"));
}

/// 実機（codex-cli 0.154.0）の出力そのまま。バナーの `command-line` を拾ってはいけない（人からの報告で発覚）。
#[test]
fn extract_device_code_skips_the_banner_and_takes_the_standalone_code_line() {
    let text = concat!(
        "\n  Welcome to Codex [v0.154.0]\n",
        "  OpenAI's command-line coding agent\n\n",
        "Follow these steps to sign in with ChatGPT using device code authorization:\n\n",
        "1. Open this link in your browser and sign in to your account\n",
        "   https://auth.openai.com/codex/device\n\n",
        "2. Enter this one-time code (expires in 15 minutes)\n",
        "   QWER-1TYUI\n\n",
        "Continue only if you started this login in Codex.\n",
    )
    .as_bytes();
    assert_eq!(extract_device_code(text).as_deref(), Some("QWER-1TYUI"));
}

/// 文中の小文字の「英数字-英数字」（`command-line` / `one-time` / パス）は候補にしない。
#[test]
fn extract_device_code_ignores_hyphenated_words_in_prose() {
    for text in [
        &b"OpenAI's command-line coding agent\n"[..],
        &b"2. Enter this one-time code (expires in 15 minutes)\n"[..],
        &b"codex_home: /tmp/claude-1001/-home-rmaeda/workspace-agent\n"[..],
    ] {
        assert_eq!(
            extract_device_code(text),
            None,
            "should not match: {}",
            String::from_utf8_lossy(text)
        );
    }
}

#[test]
fn extract_device_code_none_when_absent() {
    let text = b"Visit https://auth.openai.com/codex/device and enter the code shown on the page\n";
    assert_eq!(extract_device_code(text), None);
}
