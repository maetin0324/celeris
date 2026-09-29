use super::*;

fn stub(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("claude_stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    path
}

fn account_dir(root: &Path, id: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).expect("account dir");
    dir
}

// ---- check_account ----

#[tokio::test]
async fn check_account_ok_path_records_observation() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"echo '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1789605600,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.14,"resetsAt":1789605600},"seven_day":{"utilization":0.24,"resetsAt":1790031600}}}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'
"#,
    );
    let acct = account_dir(dir.path(), "a");
    let check = check_account(
        command.to_str().unwrap(),
        &acct,
        "haiku",
        Duration::from_secs(5),
        &[],
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::Ok);
    assert_eq!(check.detail.as_deref(), Some("ok"));
    let obs = check.observation.expect("observation");
    assert_eq!(obs.five_hour.map(|w| w.utilization), Some(0.14));
    assert_eq!(obs.seven_day.map(|w| w.utilization), Some(0.24));
}

#[tokio::test]
async fn check_account_auth_failure_path() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        r#"echo '{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}'"#,
    );
    let acct = account_dir(dir.path(), "a");
    let check = check_account(
        command.to_str().unwrap(),
        &acct,
        "haiku",
        Duration::from_secs(5),
        &[],
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::AuthFailed);
    assert!(check.detail.unwrap().contains("login"));
}

#[tokio::test]
async fn check_account_spawn_failure_for_nonexistent_command() {
    let dir = tempfile::tempdir().unwrap();
    let acct = account_dir(dir.path(), "a");
    let missing = dir.path().join("does-not-exist");
    let check = check_account(
        missing.to_str().unwrap(),
        &acct,
        "haiku",
        Duration::from_secs(5),
        &[],
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::SpawnFailed);
}

// ---- login ----

fn login_stub(dir: &Path) -> PathBuf {
    stub(
        dir,
        r#"printf 'Opening browser to sign in\xe2\x80\xa6\n'
printf "If the browser didn't open, visit: \033]8;;https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc\007https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc\033]8;;\007\n"
printf 'Paste code here if prompted > '
read -r code
if [ "$code" = "good-code" ]; then
  printf '%s' '{}' > "$CLAUDE_SECURESTORAGE_CONFIG_DIR/.credentials.json"
  echo 'Login successful.'
  exit 0
else
  echo 'Login failed: Request failed with status code 400'
  exit 1
fi
"#,
    )
}

#[tokio::test]
async fn start_login_extracts_the_exact_oauth_url_without_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    assert_eq!(
        session.url,
        "https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc"
    );
    session.cancel();
}

#[tokio::test]
async fn submit_code_ok_with_the_correct_code() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session
        .submit_code("good-code", Duration::from_secs(5))
        .await;
    assert_eq!(result.result, LoginOutcome::Ok);
    assert!(acct.join(".credentials.json").is_file());
}

#[tokio::test]
async fn submit_code_failed_with_the_wrong_code_detail_has_no_code() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session
        .submit_code("bad-code", Duration::from_secs(5))
        .await;
    assert_eq!(result.result, LoginOutcome::Failed);
    let detail = result.detail.expect("detail");
    assert!(detail.contains("400"), "{detail}");
    assert!(!detail.contains("bad-code"), "{detail}");
    assert!(!acct.join(".credentials.json").is_file());
}

/// N6: 改行・制御文字を含むコードは stdin に書かずに拒否する（`detail` にもコードは出さない）。
#[tokio::test]
async fn submit_code_rejects_control_characters_without_echoing_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session
        .submit_code("good-code\nrm -rf /", Duration::from_secs(5))
        .await;
    assert_eq!(result.result, LoginOutcome::Failed);
    let detail = result.detail.expect("detail");
    assert!(!detail.contains("good-code"), "{detail}");
    assert!(!detail.contains("rm -rf"), "{detail}");
    assert!(!acct.join(".credentials.json").is_file());
}

/// N6: 空白だけのコードも拒否する。
/// 回帰（codex 側と同じ競合）: URL を出してすぐ終了する子でも、パイプを読み切ってから探すので URL が取れる。
#[tokio::test]
async fn start_login_finds_the_url_even_when_the_process_exits_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let command = stub(
        dir.path(),
        "printf 'visit: https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc\n'
exit 0
",
    );
    for i in 0..20 {
        let acct = account_dir(dir.path(), &format!("a{i}"));
        let session = start_login(
            command.to_str().unwrap(),
            &acct,
            &[],
            Duration::from_secs(5),
        )
        .await
        .unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        assert_eq!(
            session.url,
            "https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=abc"
        );
        session.cancel();
    }
}

#[tokio::test]
async fn submit_code_rejects_whitespace_only_code() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let result = session.submit_code("   ", Duration::from_secs(5)).await;
    assert_eq!(result.result, LoginOutcome::Failed);
    assert!(!acct.join(".credentials.json").is_file());
}

#[tokio::test]
async fn cancel_kills_the_login_process() {
    let dir = tempfile::tempdir().unwrap();
    let command = login_stub(dir.path());
    let acct = account_dir(dir.path(), "a");
    let session = start_login(
        command.to_str().unwrap(),
        &acct,
        &[],
        Duration::from_secs(5),
    )
    .await
    .expect("session");
    let pid = session.child.as_ref().and_then(|c| c.id()).expect("pid");
    session.cancel();
    // The child should exit shortly after being killed.
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process {pid} is still alive after cancel()");
}
