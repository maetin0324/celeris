use super::*;
use crate::test_support::write_executable;

/// 成功経路のテストで使う待ち時間。偽 ssh は即座に応答するので、成功時は
/// この値まで待たない。高負荷の評価器で 5 秒の timeout を踏んだため長めに取る。
const LOAD_TOLERANT_WAIT: Duration = Duration::from_secs(30);

async fn wait_until_process_gone(pid: u32) {
    for _ in 0..150 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("process {pid} is still alive");
}

fn fake_ssh(dir: &Path, name: &str, script: &str) -> Vec<String> {
    let path = dir.join(name);
    write_executable(&path, script);
    vec![path.to_string_lossy().into_owned()]
}

/// 偽 ssh の共通の骨組み: 引数を 1 つずつ見て `-M`/`-N`/`check`/`exit` の有無を判定してから分岐する
/// （`case " $* " in *" -M "*" -N "*)` のような部分文字列マッチだと、`-M -N` の間の空白 1 個を
/// 2 つの `*"..."` パターンで取り合えず必ず失敗する。トークンごとに見る方が確実）。
fn preamble() -> &'static str {
    "is_master=0\n\
         is_check=0\n\
         is_exit=0\n\
         for a in \"$@\"; do\n  \
           case \"$a\" in\n    \
             -M) is_master=$((is_master + 1)) ;;\n    \
             -N) is_master=$((is_master + 1)) ;;\n    \
             check) is_check=1 ;;\n    \
             exit) is_exit=1 ;;\n  \
           esac\n\
         done\n"
}

/// `-O check` に一致するかどうかだけを見る（ADR-0018 の判定＝`control_master_alive_blocking` の
/// 引数と同じなので、テストの偽 ssh もその形に合わせる）。
fn always_ok_script() -> String {
    format!(
        "#!/bin/sh\n{}if [ \"$is_check\" = 1 ]; then exit 0; fi\nexit 1\n",
        preamble()
    )
}

/// `-M -N` で master を張り続け（ブロックし続け）、`-O check` は常に失敗する偽 ssh。
/// master の pid を `$STATE/pid` に書く（結果が公開型に出てこないテストで使う）。
fn never_authenticates_script(state: &Path) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               echo $$ > \"$STATE/pid\"\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then exit 1; fi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

/// `-M -N` で master を張り、張ってから `{delay_ms}` ms 経つと `-O check` が通るようになる偽 ssh
/// （`auth = \"publickey\"` の接続がしばらくしてから確立する場合を模す）。
fn delayed_success_script(state: &Path, delay_ms: u64) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               echo $$ > \"$STATE/pid\"\n  \
               date +%s%N > \"$STATE/started\"\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ ! -f \"$STATE/started\" ]; then exit 1; fi\n  \
               started=$(cat \"$STATE/started\")\n  \
               now=$(date +%s%N)\n  \
               elapsed_ms=$(( (now - started) / 1000000 ))\n  \
               if [ \"$elapsed_ms\" -ge {delay_ms} ]; then exit 0; else exit 1; fi\nfi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

/// `SSH_ASKPASS` を呼び、返ってきたコードが `{expected_code}` と一致すれば以後 `-O check` が通る偽 ssh
/// （`auth = \"totp\"` を模す）。askpass は `sh` で読ませる: 直前に同じプロセスで書いた実行ファイルを
/// exec すると、並行テストの fork が書き込み fd を exec 前まで握っていて ETXTBSY で黙って失敗し、
/// プロンプトが来ないまま prompt_timeout に落ちることがある（高負荷の評価器で観測）。
fn askpass_script(state: &Path, prompt: &str, expected_code: &str) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               code=$(sh \"$SSH_ASKPASS\" \"{prompt}\")\n  \
               if [ \"$code\" = \"{expected_code}\" ]; then echo ok > \"$STATE/authed\"; fi\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ -f \"$STATE/authed\" ]; then exit 0; else exit 1; fi\nfi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

// ---- 1. 生きている master をそのまま借りる ----

#[tokio::test]
async fn connected_without_spawning_when_master_already_alive() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &always_ok_script());
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        Duration::from_secs(2),
        0,
        "yes",
    )
    .await;
    match result {
        Ok(ClusterConnectStart::Connected(master)) => {
            assert!(master.is_none(), "master を借りるだけ")
        }
        other => panic!("expected Connected(None), got {other:?}"),
    }
}

// ---- 2. publickey、少し待ってから繋がる ----

#[tokio::test]
async fn publickey_connects_after_a_delay() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &delayed_success_script(state.path(), 300),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await;
    match result {
        Ok(ClusterConnectStart::Connected(Some(master))) => {
            let pid = master.child.id();
            drop(master);
            // ADR-0060（Phase 103）: 接続が成立した後は、`ClusterMaster` を drop しても殺さない
            // （celeris の終了・再起動で繋がっていた master を道連れにしないため）。ここでは
            // 偽 ssh のプロセスがまだ生きていることを確かめてから、テストの後始末として直接 kill する。
            if let Some(pid) = pid {
                tokio::time::sleep(Duration::from_millis(200)).await;
                assert!(
                    Path::new(&format!("/proc/{pid}")).exists(),
                    "connected master should survive a plain drop"
                );
                let _ =
                    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL);
                wait_until_process_gone(pid).await;
            }
        }
        other => panic!("expected Connected(Some(_)), got {other:?}"),
    }
}

// ---- 3. publickey、connect_timeout で Failed、子が残らない ----

#[tokio::test]
async fn publickey_times_out_and_leaves_no_child() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &never_authenticates_script(state.path()));
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(300),
        Duration::from_secs(5),
        0,
        "yes",
    )
    .await;
    match result {
        Err(ClusterConnectError::Failed(_)) => {}
        other => panic!("expected Failed, got {other:?}"),
    }
    let pid_file = state.path().join("pid");
    assert!(pid_file.is_file(), "master は一度は起動した");
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until_process_gone(pid).await;
}

// ---- 4. totp、プロンプトが出る ----

#[tokio::test]
async fn totp_needs_code_reports_the_exact_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &askpass_script(state.path(), prompt, "123456"),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    match result {
        ClusterConnectStart::NeedsCode {
            prompt: got,
            session,
        } => {
            assert_eq!(got, prompt);
            session.cancel().await;
        }
        other => panic!("expected NeedsCode, got {other:?}"),
    }
}

// ---- 5. totp、正しいコードで繋がる ----

/// 偽 ssh が「認証が済んだら**自分を切り離して終了する**」（`ControlPersist` があるときの本物の挙動）。
///
/// `-O check` は**前面の子が消えてから**しか成功しないようにしてある。こうしないと
/// 「認証済みファイルを書いてから exit するまでの隙間」で普通の成功経路を通ってしまい、
/// 肝心の「子の終了を観測した後」の分岐を踏まないテストになる（実際に一度そうなった）。
fn daemonizing_askpass_script(state: &Path, prompt: &str, expected_code: &str) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               echo $$ > \"$STATE/masterpid\"\n  \
               code=$(sh \"$SSH_ASKPASS\" \"{prompt}\")\n  \
               if [ \"$code\" = \"{expected_code}\" ]; then echo ok > \"$STATE/authed\"; fi\n  \
               exit 0\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               [ -f \"$STATE/authed\" ] || exit 1\n  \
               if [ -f \"$STATE/masterpid\" ] && kill -0 \"$(cat \"$STATE/masterpid\")\" 2>/dev/null; then exit 1; fi\n  \
               exit 0\nfi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

/// **実機の回帰（2026-09-17、sirius）**: `~/.ssh/config` に `ControlPersist` があると、ssh は認証が済んだ
/// 時点で自分をバックグラウンドへ切り離す（master は PPID 1 になり、こちらの子は終了する）。
/// 「子が終了した＝失敗」と決めつけていたため、**接続できているのに失敗を返していた**
/// （人間の報告「一回接続に失敗しましたという表記が出てから接続に成功しています」。
/// 実際には 1 回目で繋がっていて、失敗表示だけが誤りだった）。子の終了後に必ず `-O check` を見る。
#[tokio::test]
async fn totp_succeeds_when_ssh_backgrounds_itself_after_authenticating() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &daemonizing_askpass_script(state.path(), prompt, "123456"),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    match session.submit_code("123456", LOAD_TOLERANT_WAIT).await {
        // 保持する子は無い（ssh が切り離した）が、**接続は成功している**。
        Ok(master) => assert!(
            master.is_none(),
            "the child exited, so there is nothing to hold"
        ),
        Err(e) => panic!("expected success even though the child exited, got {e:?}"),
    }
}

/// 切り離されても**間違ったコードなら失敗のまま**（上の修正で失敗を握りつぶしていないこと）。
#[tokio::test]
async fn totp_still_fails_when_the_code_is_wrong_and_ssh_exits() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &daemonizing_askpass_script(state.path(), prompt, "123456"),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    assert!(
        session
            .submit_code("000000", Duration::from_secs(2))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn totp_submit_code_connects_with_the_right_code() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &askpass_script(state.path(), prompt, "123456"),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    let master = session.submit_code("123456", LOAD_TOLERANT_WAIT).await;
    match master {
        Ok(_master) => {}
        Err(e) => panic!("expected Ok(ClusterMaster), got {e:?}"),
    }
}

// ---- 6. 不正なコードは ssh に渡らない ----

#[tokio::test]
async fn totp_rejects_invalid_codes_without_delivering_them() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";

    for bad in ["", "12\n34"] {
        let ssh = fake_ssh(
            dir.path(),
            "ssh",
            &askpass_script(state.path(), prompt, "123456"),
        );
        let result = start_connect(
            &ssh,
            "cluster-host",
            "c1",
            &MasterLauncher::Inline,
            true,
            LOAD_TOLERANT_WAIT,
            LOAD_TOLERANT_WAIT,
            0,
            "yes",
        )
        .await
        .unwrap();
        let ClusterConnectStart::NeedsCode { session, .. } = result else {
            panic!("expected NeedsCode");
        };
        let outcome = session.submit_code(bad, Duration::from_secs(1)).await;
        assert!(
            matches!(outcome, Err(ClusterConnectError::InvalidCode)),
            "{outcome:?}"
        );
    }
    // 何もコードが渡っていないので、askpass のスクリプトは一度も `authed` を書いていない。
    assert!(!state.path().join("authed").exists(), "ssh に何も渡らない");
}

// ---- 7. cancel で子プロセスと一時ディレクトリが消える ----

#[tokio::test]
async fn cancel_kills_the_child_and_removes_the_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &askpass_script(state.path(), prompt, "123456"),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    let pid = session.child.as_ref().and_then(|c| c.id());
    let session_dir = session.dir.clone();
    assert!(session_dir.is_dir());
    session.cancel().await;
    assert!(!session_dir.exists(), "一時ディレクトリも消える");
    if let Some(pid) = pid {
        wait_until_process_gone(pid).await;
    }
}

// ---- 8. totp、プロンプトが来ないまま prompt_timeout ----

#[tokio::test]
async fn totp_times_out_when_no_prompt_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &never_authenticates_script(state.path()));
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        Duration::from_millis(300),
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await;
    assert!(
        matches!(result, Err(ClusterConnectError::Timeout)),
        "{result:?}"
    );
    let pid_file = state.path().join("pid");
    assert!(pid_file.is_file(), "master は一度は起動した");
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until_process_gone(pid).await;
}

// ---- disconnect ----

#[tokio::test]
async fn disconnect_runs_o_exit_batch_mode() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &format!(
            "#!/bin/sh\n{}if [ \"$is_exit\" = 1 ]; then exit 0; fi\nexit 1\n",
            preamble()
        ),
    );
    disconnect(&ssh, "cluster-host")
        .await
        .expect("disconnect ok");
}

#[tokio::test]
async fn disconnect_surfaces_stderr_without_a_code() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        "#!/bin/sh\necho 'no such control socket' >&2\nexit 1\n",
    );
    let err = disconnect(&ssh, "cluster-host")
        .await
        .expect_err("disconnect fails");
    match err {
        ClusterConnectError::Failed(detail) => {
            assert!(detail.contains("control socket"), "{detail}")
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

// ---- ADR-0060: master_launcher ----

// (a) systemd-run 指定で argv が組み立てどおりになる。
#[test]
fn launch_master_command_wraps_with_systemd_run() {
    let launcher = MasterLauncher::SystemdRun {
        program: "systemd-run".to_string(),
    };
    let args = vec!["-M".to_string(), "-N".to_string(), "pegasus".to_string()];
    let (program, full_args) = launch_master_command(&launcher, "ssh", &args, "pegasus");
    assert_eq!(program, "systemd-run");
    assert_eq!(
        &full_args[..3],
        &[
            "--user".to_string(),
            "--scope".to_string(),
            "--quiet".to_string()
        ]
    );
    assert_eq!(full_args[3], "--unit");
    assert!(
        full_args[4].starts_with("celeris-ssh-master-pegasus-"),
        "{}",
        full_args[4]
    );
    assert_eq!(full_args[5], "--description");
    assert_eq!(full_args[6], "celeris ssh master (pegasus)");
    assert_eq!(full_args[7], "--");
    assert_eq!(
        &full_args[8..],
        &[
            "ssh".to_string(),
            "-M".to_string(),
            "-N".to_string(),
            "pegasus".to_string()
        ]
    );
}

/// クラスタ id に systemd のユニット名として危険な文字が入っていても安全な文字に落とす。
#[test]
fn launch_master_command_sanitizes_the_cluster_id_in_the_unit_name() {
    let launcher = MasterLauncher::systemd_run();
    let (_, full_args) = launch_master_command(&launcher, "ssh", &[], "weird id/../x");
    assert!(
        full_args[4].starts_with("celeris-ssh-master-weird_id____x-"),
        "{}",
        full_args[4]
    );
}

// (b) inline は従来と同じ argv のまま。
#[test]
fn launch_master_command_inline_is_unchanged() {
    let args = vec!["-M".to_string(), "-N".to_string(), "pegasus".to_string()];
    let (program, full_args) =
        launch_master_command(&MasterLauncher::Inline, "ssh", &args, "pegasus");
    assert_eq!(program, "ssh");
    assert_eq!(full_args, args);
}

// (c) auto の判定は「systemd-run が PATH にあり、かつ XDG_RUNTIME_DIR が設定されているとき」だけ。
#[test]
fn resolve_master_launcher_auto_needs_both_conditions() {
    assert!(matches!(
        resolve_master_launcher("auto", true, true),
        MasterLauncher::SystemdRun { .. }
    ));
    assert!(matches!(
        resolve_master_launcher("auto", false, true),
        MasterLauncher::Inline
    ));
    assert!(matches!(
        resolve_master_launcher("auto", true, false),
        MasterLauncher::Inline
    ));
    assert!(matches!(
        resolve_master_launcher("auto", false, false),
        MasterLauncher::Inline
    ));
}

#[test]
fn resolve_master_launcher_explicit_values_ignore_the_environment() {
    assert!(matches!(
        resolve_master_launcher("systemd-run", false, false),
        MasterLauncher::SystemdRun { .. }
    ));
    assert!(matches!(
        resolve_master_launcher("inline", true, true),
        MasterLauncher::Inline
    ));
}

#[test]
fn path_has_executable_finds_an_executable_file_in_one_of_the_path_dirs() {
    let empty_dir = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let path_env = std::env::join_paths([empty_dir.path(), bin_dir.path()])
        .unwrap()
        .into_string()
        .unwrap();
    assert!(!path_has_executable(&path_env, "systemd-run"));

    write_executable(&bin_dir.path().join("systemd-run"), "#!/bin/sh\nexit 0\n");
    assert!(path_has_executable(&path_env, "systemd-run"));
}

#[test]
fn path_has_executable_ignores_non_executable_files() {
    let bin_dir = tempfile::tempdir().unwrap();
    std::fs::write(bin_dir.path().join("systemd-run"), "not executable").unwrap();
    let path_env = bin_dir.path().to_string_lossy().into_owned();
    assert!(!path_has_executable(&path_env, "systemd-run"));
}

/// (e) 偽 `systemd-run`（`--` の後を `exec` するだけ）を経由しても、`Command::envs` で渡した
/// `SSH_ASKPASS` 等の環境変数が末端の ssh まで届く（`--scope` は呼び出し元の環境を継ぐ、という
/// ADR-0060 の前提を実プロセスで確かめる）。
#[tokio::test]
async fn fake_systemd_run_execs_ssh_and_preserves_the_askpass_env() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &askpass_script(state.path(), prompt, "123456"),
    );
    let fake_systemd_run = dir.path().join("systemd-run");
    write_executable(
        &fake_systemd_run,
        "#!/bin/sh\n\
             while [ $# -gt 0 ]; do\n  \
               if [ \"$1\" = \"--\" ]; then\n    \
                 shift\n    \
                 exec \"$@\"\n  \
               fi\n  \
               shift\n\
             done\n\
             exit 1\n",
    );
    let launcher = MasterLauncher::SystemdRun {
        program: fake_systemd_run.to_string_lossy().into_owned(),
    };
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &launcher,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    let master = session.submit_code("123456", LOAD_TOLERANT_WAIT).await;
    match master {
        Ok(_master) => {}
        Err(e) => {
            panic!("expected Ok(ClusterMaster) through the fake systemd-run wrapper, got {e:?}")
        }
    }
}

/// 接続成立後は、明示的な `ClusterMaster::kill` を呼ばない限り master は生きたまま
/// （ADR-0060: 通常の Drop はもう殺さない）。`kill` を呼べば確実に落ちる。
#[tokio::test]
async fn cluster_master_kill_terminates_the_child_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(
        dir.path(),
        "ssh",
        &delayed_success_script(state.path(), 100),
    );
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::Connected(Some(master)) = result else {
        panic!("expected Connected(Some(_))");
    };
    let pid = master.child.id().expect("pid");
    master.kill().await;
    wait_until_process_gone(pid).await;
}

// ---- ADR-0062 A: keepalive ----

/// 偽 ssh の argv を丸ごと `$STATE/argv` に書き出す（`-M`/`-N` の判定用の `preamble()` は使わず、
/// 生の `"$@"` をそのまま記録する）。`-O check` は常に成功（`always_ok_script` と同じ判定）。
fn argv_recording_script(state: &Path) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               printf '%s\\n' \"$@\" > \"$STATE/argv\"\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ -f \"$STATE/argv\" ]; then exit 0; else exit 1; fi\nfi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

#[tokio::test]
async fn keepalive_args_are_added_to_the_master_argv_when_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &argv_recording_script(state.path()));
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        Duration::from_secs(2),
        15,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::Connected(Some(master)) = result else {
        panic!("expected Connected(Some(_))");
    };
    let pid = master.child.id().expect("pid");
    master.kill().await;
    wait_until_process_gone(pid).await;
    let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
    assert!(argv.contains("ServerAliveInterval=15"), "{argv}");
    assert!(argv.contains("ServerAliveCountMax=3"), "{argv}");
    assert!(argv.contains("TCPKeepAlive=yes"), "{argv}");
}

#[tokio::test]
async fn keepalive_secs_zero_omits_the_option() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &argv_recording_script(state.path()));
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        Duration::from_secs(2),
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::Connected(Some(master)) = result else {
        panic!("expected Connected(Some(_))");
    };
    let pid = master.child.id().expect("pid");
    master.kill().await;
    wait_until_process_gone(pid).await;
    let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
    assert!(!argv.contains("ServerAliveInterval"), "{argv}");
}

/// totp 経路でも同じ keepalive オプションが付く。
#[tokio::test]
async fn keepalive_args_are_added_on_the_totp_path_too() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    // askpass を経由しつつ argv も記録する。
    let script = format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               printf '%s\\n' \"$@\" > \"$STATE/argv\"\n  \
               code=$(sh \"$SSH_ASKPASS\" \"{prompt}\")\n  \
               if [ \"$code\" = \"123456\" ]; then echo ok > \"$STATE/authed\"; fi\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ -f \"$STATE/authed\" ]; then exit 0; else exit 1; fi\nfi\n\
             exit 1\n",
        preamble = preamble(),
        state = state.path(),
    );
    let ssh = fake_ssh(dir.path(), "ssh", &script);
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        20,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    let master = session
        .submit_code("123456", LOAD_TOLERANT_WAIT)
        .await
        .unwrap();
    let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
    assert!(argv.contains("ServerAliveInterval=20"), "{argv}");
    if let Some(master) = master {
        master.kill().await;
    }
}

/// ADR-0078 D1: argv 中で `-o ControlPersist=<want>` が `-M` より前にある。
fn assert_persist_before_master(argv: &str, want: &str) {
    let lines: Vec<&str> = argv.lines().collect();
    let opt = format!("ControlPersist={want}");
    let opt_at = lines
        .iter()
        .position(|l| *l == opt)
        .unwrap_or_else(|| panic!("{opt} missing: {argv}"));
    assert_eq!(lines[opt_at - 1], "-o", "{argv}");
    let m_at = lines
        .iter()
        .position(|l| *l == "-M")
        .unwrap_or_else(|| panic!("-M missing: {argv}"));
    assert!(opt_at < m_at, "{argv}");
}

#[test]
fn persist_args_is_pure() {
    assert_eq!(persist_args("yes"), vec!["-o", "ControlPersist=yes"]);
    assert_eq!(persist_args("28800"), vec!["-o", "ControlPersist=28800"]);
    assert!(persist_args("").is_empty());
}

/// ADR-0078 D1 の偽 ssh: `-M -N` は master を背景に切り離して即座に終わる（本物の `ControlPersist`
/// と同じ）。argv に `ControlPersist=yes` が無ければ、人の設定の `ControlPersist 10` を模して「mux の
/// クライアント（`-O check`）が 1 秒来ない」と master が自分で終わる。`-O check` は master が生きて
/// いれば通り、最後のクライアントの時刻を更新する。`$STATE/master_calls` に master を張った回数を書く。
fn persisting_master_script(state: &Path) -> String {
    format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             persist=no\n\
             for a in \"$@\"; do [ \"$a\" = ControlPersist=yes ] && persist=yes; done\n\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               echo x >> \"$STATE/master_calls\"\n  \
               date +%s%N > \"$STATE/last_client\"\n  \
               STATE=\"$STATE\" PERSIST=$persist nohup sh -c '\
                 echo $$ > \"$STATE/alive\"; \
                 while [ ! -f \"$STATE/stop\" ]; do \
                   if [ \"$PERSIST\" != yes ]; then \
                     last=$(cat \"$STATE/last_client\"); now=$(date +%s%N); \
                     if [ $(( (now - last) / 1000000 )) -gt 1000 ]; then break; fi; \
                   fi; \
                   sleep 0.05; \
                 done; \
                 rm -f \"$STATE/alive\"' </dev/null >/dev/null 2>&1 &\n  \
               while [ ! -f \"$STATE/alive\" ]; do sleep 0.01; done\n  \
               exit 0\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ -f \"$STATE/alive\" ] && kill -0 \"$(cat \"$STATE/alive\")\" 2>/dev/null; then\n    \
                 date +%s%N > \"$STATE/last_client\"; exit 0\n  \
               fi\n  \
               exit 1\nfi\n\
             exit 1\n",
        preamble = preamble(),
    )
}

/// `persisting_master_script` の背景の master を止め、消えるまで待つ（テストの後片付け）。
fn stop_persisting_master(state: &Path) {
    let _ = std::fs::write(state.join("stop"), "");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while state.join("alive").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// ADR-0078 D1 / §6: daemon の停止→起動（あるいはリリース切り替え・tick の停止）で `-O check` が
/// 途切れても、`ControlPersist=yes` で張った master は残る。新しい daemon の `start_connect` は既存の
/// master を見つけて借り（`Connected(None)`）、master を張り直さない（= TOTP を求めない）。
#[tokio::test]
async fn a_control_persist_yes_master_survives_a_daemon_restart_gap() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &persisting_master_script(state.path()));
    let first = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        LOAD_TOLERANT_WAIT,
        30,
        "yes",
    )
    .await;
    assert!(
        matches!(first, Ok(ClusterConnectStart::Connected(_))),
        "{first:?}"
    );
    // 旧 daemon が止まり、`-O check` が 1.5 秒途切れる（偽の idle 上限 1 秒より長い）。
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // 新 daemon の最初の `-O check` と接続。
    let alive = control_master_alive_blocking(&ssh, "cluster-host");
    let second = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        LOAD_TOLERANT_WAIT,
        30,
        "yes",
    )
    .await;
    let master_calls = std::fs::read_to_string(state.path().join("master_calls"))
        .unwrap_or_default()
        .lines()
        .count();
    stop_persisting_master(state.path());
    assert!(alive, "the master must survive the gap");
    assert!(
        matches!(second, Ok(ClusterConnectStart::Connected(None))),
        "{second:?}"
    );
    assert_eq!(master_calls, 1, "no new master (no TOTP) after the restart");
}

/// 対照（修正前の挙動）: `ControlPersist=yes` を渡さないと、人の設定の短い `ControlPersist` のまま
/// `-O check` の途切れで master が消える。消えるまでを読み切ってから判定する（固定の sleep で
/// 「消えたはず」と決めない）。
#[tokio::test]
async fn without_control_persist_yes_the_master_dies_in_the_restart_gap() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &persisting_master_script(state.path()));
    let first = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        LOAD_TOLERANT_WAIT,
        30,
        "",
    )
    .await;
    assert!(
        matches!(first, Ok(ClusterConnectStart::Connected(_))),
        "{first:?}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while state.path().join("alive").exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let gone = !state.path().join("alive").exists();
    let alive = control_master_alive_blocking(&ssh, "cluster-host");
    stop_persisting_master(state.path());
    assert!(gone, "the fake idle limit should have ended the master");
    assert!(!alive);
}

#[tokio::test]
async fn control_persist_yes_is_added_before_m_on_the_publickey_path() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ssh = fake_ssh(dir.path(), "ssh", &argv_recording_script(state.path()));
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        false,
        Duration::from_millis(200),
        Duration::from_secs(2),
        15,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::Connected(Some(master)) = result else {
        panic!("expected Connected(Some(_))");
    };
    let pid = master.child.id().expect("pid");
    master.kill().await;
    wait_until_process_gone(pid).await;
    let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
    assert_persist_before_master(&argv, "yes");
}

#[tokio::test]
async fn control_persist_yes_is_added_before_m_on_the_totp_path_too() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let prompt = "(rmaeda@130.158.241.2) Verification code: ";
    let script = format!(
        "#!/bin/sh\nSTATE={state:?}\n{preamble}\
             if [ \"$is_master\" -ge 2 ]; then\n  \
               printf '%s\\n' \"$@\" > \"$STATE/argv\"\n  \
               code=$(sh \"$SSH_ASKPASS\" \"{prompt}\")\n  \
               if [ \"$code\" = \"123456\" ]; then echo ok > \"$STATE/authed\"; fi\n  \
               while kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; done\nfi\n\
             if [ \"$is_check\" = 1 ]; then\n  \
               if [ -f \"$STATE/authed\" ]; then exit 0; else exit 1; fi\nfi\n\
             exit 1\n",
        preamble = preamble(),
        state = state.path(),
    );
    let ssh = fake_ssh(dir.path(), "ssh", &script);
    let result = start_connect(
        &ssh,
        "cluster-host",
        "c1",
        &MasterLauncher::Inline,
        true,
        LOAD_TOLERANT_WAIT,
        LOAD_TOLERANT_WAIT,
        0,
        "yes",
    )
    .await
    .unwrap();
    let ClusterConnectStart::NeedsCode { session, .. } = result else {
        panic!("expected NeedsCode");
    };
    let master = session
        .submit_code("123456", LOAD_TOLERANT_WAIT)
        .await
        .unwrap();
    let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
    assert_persist_before_master(&argv, "yes");
    if let Some(master) = master {
        master.kill().await;
    }
}

#[tokio::test]
async fn control_persist_uses_the_configured_seconds_and_empty_omits_it() {
    for (value, expect_present) in [("28800", true), ("", false)] {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let ssh = fake_ssh(dir.path(), "ssh", &argv_recording_script(state.path()));
        let result = start_connect(
            &ssh,
            "cluster-host",
            "c1",
            &MasterLauncher::Inline,
            false,
            Duration::from_millis(200),
            Duration::from_secs(2),
            0,
            value,
        )
        .await
        .unwrap();
        let ClusterConnectStart::Connected(Some(master)) = result else {
            panic!("expected Connected(Some(_))");
        };
        let pid = master.child.id().expect("pid");
        master.kill().await;
        wait_until_process_gone(pid).await;
        let argv = std::fs::read_to_string(state.path().join("argv")).unwrap();
        if expect_present {
            assert_persist_before_master(&argv, value);
        } else {
            assert!(!argv.contains("ControlPersist"), "{argv}");
        }
    }
}

/// ADR-0062 A: master が明示的な切断を経ずに自分で終了したとき、`try_wait_exit` が exit code を
/// 拾い、`stderr_tail` が stderr の末尾（`max_bytes` を超えない）を返す。
#[tokio::test]
async fn try_wait_exit_and_stderr_tail_report_the_masters_own_death() {
    let dir = tempfile::tempdir().unwrap();
    let script = "#!/bin/sh\necho 'Broken pipe, master exiting' 1>&2\nexit 7\n";
    let ssh = fake_ssh(dir.path(), "ssh", script);
    // `-M -N` の張り自体がすぐ終了して exit 7 を返す（`never_authenticates_script` と違い、
    // ブロックしない）。`poll_until_connected_or_timeout` は子の終了を見て `-O check` を試すが、
    // このスクリプトは check にも常に失敗するので `ChildExited` → `Failed` になる。ここでは
    // `spawn_master` の戻り値を直接使うため、`start_connect` は経由せず低レベルの挙動を確認する。
    let (mut child, stderr_buf, err_task) = spawn_master(
        &ssh[0],
        &["-M".into(), "-N".into(), "cluster-host".into()],
        &[],
    )
    .unwrap();
    let status = child.wait().await.unwrap();
    assert_eq!(status.code(), Some(7));
    join_with_timeout(err_task, READER_JOIN_TIMEOUT).await;
    let mut master = ClusterMaster { child, stderr_buf };
    assert_eq!(master.try_wait_exit(), Some(Some(7)));
    let tail = master.stderr_tail(300);
    assert!(tail.contains("Broken pipe"), "{tail}");
    let short = master.stderr_tail(5);
    assert!(short.len() <= 5, "{short:?}");
}
