use super::*;

// ---- ADR-0059 D2: `~` の展開（純粋関数） ----

#[test]
fn shell_remote_path_expands_only_a_leading_tilde() {
    assert_eq!(shell_remote_path("~"), "\"$HOME\"");
    assert_eq!(
        shell_remote_path("~/work/project"),
        "\"$HOME\"'/work/project'"
    );
    // `~user` は展開しない（celeris は他ユーザの home を知らない。ADR-0039 D5 と同じ方針）。
    assert_eq!(shell_remote_path("~user/x"), "'~user/x'");
    // それ以外は従来どおり `shq`。
    assert_eq!(shell_remote_path("/work/x"), "'/work/x'");
    assert_eq!(shell_remote_path("relative/x"), "'relative/x'");
    assert_eq!(shell_remote_path("it's/quoted"), "'it'\\''s/quoted'");
}

// ---- ADR-0059 D6: `work_dir` からの相対解決（純粋関数） ----

#[test]
fn resolve_remote_dir_keeps_absolute_and_tilde_paths_untouched() {
    let work_dir = Some(Path::new("/work/NBB/rmaeda"));
    assert_eq!(
        resolve_remote_dir(Path::new("/scratch/x"), work_dir),
        Some(PathBuf::from("/scratch/x"))
    );
    assert_eq!(
        resolve_remote_dir(Path::new("~"), work_dir),
        Some(PathBuf::from("~"))
    );
    assert_eq!(
        resolve_remote_dir(Path::new("~/work"), work_dir),
        Some(PathBuf::from("~/work"))
    );
    // `work_dir` が無くても絶対・`~` はそのまま解決できる。
    assert_eq!(
        resolve_remote_dir(Path::new("/scratch/x"), None),
        Some(PathBuf::from("/scratch/x"))
    );
}

#[test]
fn resolve_remote_dir_resolves_empty_and_relative_paths_against_work_dir() {
    let work_dir = Some(Path::new("/work/NBB/rmaeda"));
    assert_eq!(
        resolve_remote_dir(Path::new(""), work_dir),
        Some(PathBuf::from("/work/NBB/rmaeda"))
    );
    assert_eq!(
        resolve_remote_dir(Path::new("benchfs"), work_dir),
        Some(PathBuf::from("/work/NBB/rmaeda/benchfs"))
    );
}

#[test]
fn resolve_remote_dir_is_none_when_there_is_no_work_dir_to_resolve_against() {
    assert_eq!(resolve_remote_dir(Path::new(""), None), None);
    assert_eq!(resolve_remote_dir(Path::new("benchfs"), None), None);
}

#[test]
fn remote_dir_is_resolved_matches_absolute_and_tilde_only() {
    assert!(remote_dir_is_resolved(Path::new("/work/x")));
    assert!(remote_dir_is_resolved(Path::new("~")));
    assert!(remote_dir_is_resolved(Path::new("~/work")));
    assert!(!remote_dir_is_resolved(Path::new("")));
    assert!(!remote_dir_is_resolved(Path::new("relative")));
}

// ---- ADR-0018 D5 / ADR-0059: `SyncMode::None` は同期を一切しない ----

#[tokio::test]
async fn sync_none_push_and_pull_never_run_rsync_or_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/x"));
    settings.sync = SyncMode::None;
    // 呼ばれたら即座にエラーで分かるように、実在しないプログラムを指す。
    settings.ssh_command = vec!["/nonexistent/ssh-should-not-run".into()];
    settings.rsync_command = vec!["/nonexistent/rsync-should-not-run".into()];
    let ws = SshWorkspace::new(dir.path(), settings);
    ws.push()
        .await
        .expect("push is a no-op under SyncMode::None");
    ws.pull()
        .await
        .expect("pull is a no-op under SyncMode::None");
}

// ---- ADR-0059 D3: worktree 準備の exit 65 を型で区別する ----

#[tokio::test]
async fn ensure_worktree_maps_exit_65_to_not_a_git_repository_and_exit_66_to_remote() {
    let dir = tempfile::tempdir().unwrap();
    for (exit_code, matches_not_a_git_repo) in [(65, true), (66, false), (1, false)] {
        let stub = dir.path().join(format!("stub-{exit_code}.sh"));
        // ETXTBSY 対策（ADR-0010 D10）: テストプロセス自身が書き込み fd を持たないよう別プロセスで書く。
        crate::test_support::write_executable(&stub, &format!("#!/bin/sh\nexit {exit_code}\n"));
        let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/proj"));
        settings.sync = SyncMode::Worktree;
        settings.task_id = "01TESTTASK".into();
        settings.ssh_command = vec![stub.to_string_lossy().into_owned()];
        let mirror = dir.path().join(format!("mirror-{exit_code}"));
        let ws = SshWorkspace::new(&mirror, settings);
        let err = ws.ensure_worktree().await.expect_err("stub always fails");
        assert_eq!(
            matches!(err, WorkspaceError::NotAGitRepository(_)),
            matches_not_a_git_repo,
            "exit {exit_code}: {err:?}"
        );
    }
}

// ---- ADR-0059 D5: `.taskd/remote-exec` -> `.celeris/remote-exec` ----

#[tokio::test]
async fn write_remote_exec_helper_writes_celeris_and_cleans_up_the_old_taskd_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    let mirror = dir.path().join("mirror");
    tokio::fs::create_dir_all(mirror.join(".taskd"))
        .await
        .unwrap();
    tokio::fs::write(mirror.join(".taskd").join("remote-exec"), "old wrapper")
        .await
        .unwrap();

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("~/work/proj"));
    settings.sync = SyncMode::None;
    let ws = SshWorkspace::new(&mirror, settings);
    let path = ws.write_remote_exec_helper().await.expect("write helper");
    assert_eq!(path, mirror.join(".celeris").join("remote-exec"));
    assert!(path.is_file());
    assert!(
        !mirror.join(".taskd").join("remote-exec").exists(),
        "the old wrapper must be removed"
    );

    // ADR-0059 D2: `~/work/proj` は `.celeris/remote-exec` の生成スクリプトの中で `"$HOME"` に展開される。
    let script = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(script.contains("\"$HOME\"'/work/proj'"), "{script}");
    // ADR-0062 Phase 108: `$HOME/.ssh/config` があるときだけ `-F` を足す条件分岐を持つ。
    assert!(
        script.contains(r#"if [ -f "$HOME/.ssh/config" ]; then"#),
        "{script}"
    );
    assert!(script.contains(r#"-F "$HOME/.ssh/config""#), "{script}");
}

/// ADR-0062 Phase 108: 生成した `.celeris/remote-exec` は実行時に `$HOME/.ssh/config` があれば
/// `-F` で明示的に読み、無ければ何も足さない（システムの `/etc/ssh/ssh_config` に触れない）。
/// codex サンドボックス内で `/etc/ssh/ssh_config.d/...` の Include 先が拒否される問題への対応
/// （本番 2026-09-23）。
#[tokio::test]
async fn the_wrapper_only_adds_dash_f_when_the_users_ssh_config_exists() {
    let dir = tempfile::tempdir().unwrap();
    let mirror = dir.path().join("mirror");
    tokio::fs::create_dir_all(&mirror).await.unwrap();

    // 引数をそのままファイルに記録するだけの偽 ssh。
    let log = dir.path().join("argv.log");
    let fake_ssh = dir.path().join("ssh");
    crate::test_support::write_executable(
        &fake_ssh,
        &format!("#!/bin/sh\necho \"$@\" > {log:?}\nexit 0\n"),
    );

    let mut settings = SshSettings::new("pegasus", "pegasus", PathBuf::from("/work/proj"));
    settings.sync = SyncMode::None;
    settings.ssh_command = vec![fake_ssh.to_string_lossy().into_owned()];
    let ws = SshWorkspace::new(&mirror, settings);
    let path = ws.write_remote_exec_helper().await.expect("write helper");
    // Tokio のファイル書き込みと並行テストの exec が競合しないよう、
    // 生成した内容を別プロセスで書いた実行ファイルから確認する。
    let wrapper = dir.path().join("remote-exec-test");
    let script = tokio::fs::read_to_string(&path).await.unwrap();
    crate::test_support::write_executable(&wrapper, &script);

    // `$HOME/.ssh/config` が無ければ `-F` を付けない。
    let home_without = dir.path().join("home-without");
    tokio::fs::create_dir_all(&home_without).await.unwrap();
    let status = tokio::process::Command::new(&wrapper)
        .arg("true")
        .env("HOME", &home_without)
        .status()
        .await
        .expect("run wrapper");
    assert!(status.success());
    let argv = tokio::fs::read_to_string(&log).await.unwrap();
    assert!(!argv.contains("-F"), "{argv}");

    // `$HOME/.ssh/config` があれば `-F "$HOME/.ssh/config"` を付ける。
    let home_with = dir.path().join("home-with");
    tokio::fs::create_dir_all(home_with.join(".ssh"))
        .await
        .unwrap();
    tokio::fs::write(home_with.join(".ssh").join("config"), "Host pegasus\n")
        .await
        .unwrap();
    let status = tokio::process::Command::new(&wrapper)
        .arg("true")
        .env("HOME", &home_with)
        .status()
        .await
        .expect("run wrapper");
    assert!(status.success());
    let argv = tokio::fs::read_to_string(&log).await.unwrap();
    assert!(
        argv.contains(&format!("-F {}/.ssh/config", home_with.display())),
        "{argv}"
    );
}

// ---- ADR-0018 D3 / ADR-0059 D5: `.celeris/` は同期から常に除外し、`.taskd/` も後方互換で残す ----

#[test]
fn sync_always_excluded_keeps_the_legacy_taskd_alongside_celeris() {
    assert!(SYNC_ALWAYS_EXCLUDED.contains(&".taskd/"));
    assert!(SYNC_ALWAYS_EXCLUDED.contains(&".celeris/"));
}

// ---- ADR-0079 R5b-fix2: run 後の push・成果物を pull で消さない・reviewer への指示 ----

#[test]
fn pull_protects_artifacts_and_excludes_the_admin_dirs_while_push_still_sends_artifacts() {
    assert!(SYNC_PULL_PROTECTED.contains(&"artifacts/"));
    assert!(SYNC_ALWAYS_EXCLUDED.contains(&".taskd/"));
    let ws = SshWorkspace::new("/tmp/mirror", SshSettings::new("c", "h", "/work/x"));
    let pull = ws.pull_args();
    assert!(pull.contains(&"--delete".to_string()), "{pull:?}");
    assert!(
        pull.contains(&"--filter=P artifacts/".to_string()),
        "{pull:?}"
    );
    for dir in [".taskd/", ".celeris/", "runs/", "inputs/"] {
        assert!(
            pull.windows(2).any(|w| w[0] == "--exclude" && w[1] == dir),
            "pull must exclude {dir}: {pull:?}"
        );
    }
    let push = ws.push_args();
    assert!(!push.contains(&"--delete".to_string()), "{push:?}");
    assert!(
        !push
            .windows(2)
            .any(|w| w[0] == "--exclude" && w[1] == "artifacts/"),
        "artifacts are pushed (cluster-side checks may read them): {push:?}"
    );
    // push は手元 → クラスタ、pull はクラスタ → 手元。
    assert_eq!(push.last().map(String::as_str), Some("h:/work/x/"));
    assert_eq!(pull.last().map(String::as_str), Some("/tmp/mirror/"));
}

/// ssh の代わりに、受け取ったリモートコマンドを手元の `sh` で実行するだけの偽物（外部に出ない）。
/// `ssh -o BatchMode=yes <host> <cmd...>` の形で呼ばれる（rsync の `-e` からも、`run_ssh` からも）。
fn local_ssh(dir: &Path) -> Vec<String> {
    let path = dir.join("fake-ssh");
    crate::test_support::write_executable(
        &path,
        "#!/bin/sh\nwhile [ \"$1\" = \"-o\" ]; do shift 2; done\nshift\nexec sh -c \"$*\"\n",
    );
    vec![path.to_string_lossy().into_owned()]
}

/// 本物の rsync を包み、呼ばれた向き（最後の引数が手元なら pull）を記録する。`fail` なら rsync を呼ばずに 23 で落ちる。
fn logging_rsync(dir: &Path, name: &str, local: &Path, log: &Path, fail: bool) -> Vec<String> {
    let path = dir.join(name);
    let run = if fail {
        "exit 23".to_string()
    } else {
        "exec rsync \"$@\"".to_string()
    };
    crate::test_support::write_executable(
        &path,
        &format!(
            "#!/bin/sh\nfor a; do last=$a; done\nif [ \"$last\" = {local:?} ]; then echo pull >> {log:?}; else echo push >> {log:?}; fi\n{run}\n",
            local = format!("{}/", local.display()),
        ),
    );
    vec![path.to_string_lossy().into_owned()]
}

fn log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn rsync_available() -> bool {
    std::process::Command::new("rsync")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 本番 2026-09-29 の再現: reviewer の検査しか無い task は `exec` で push しないので、次の run の
/// prepare（`--delete` 付きの pull）が worker の編集と成果物を消していた。run 後の push（1 回）で
/// 編集はクラスタに届き、成果物は pull でも消えない。
#[tokio::test]
async fn push_after_run_runs_once_and_the_next_pull_keeps_edits_and_artifacts() {
    if !rsync_available() {
        eprintln!("rsync is not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let local = tmp.path().join("mirror");
    std::fs::create_dir_all(remote.join("src")).unwrap();
    std::fs::write(remote.join("src/lib.rs"), "old\n").unwrap();
    let log = tmp.path().join("rsync.log");
    let mut settings = SshSettings::new("c", "h", &remote);
    settings.ssh_command = local_ssh(tmp.path());
    settings.rsync_command = logging_rsync(tmp.path(), "rsync-ok", &local, &log, false);
    let ws = SshWorkspace::new(&local, settings);

    // run の前（prepare）: クラスタの内容を取り込む。
    ws.pull().await.expect("initial pull");
    assert_eq!(
        std::fs::read_to_string(local.join("src/lib.rs")).unwrap(),
        "old\n"
    );
    // worker の run: ソースを直し、成果物を書く（クラスタ側には無い）。
    std::fs::write(local.join("src/lib.rs"), "edited\n").unwrap();
    std::fs::create_dir_all(local.join("artifacts")).unwrap();
    std::fs::write(local.join("artifacts/experiment-plan.md"), "plan\n").unwrap();
    // run の後: push はちょうど 1 回。
    ws.push_after_run().await.expect("push after run");
    assert_eq!(log_lines(&log), vec!["pull", "push"]);
    assert!(!ws.push_pending(), "a successful push clears the marker");
    assert_eq!(
        std::fs::read_to_string(remote.join("src/lib.rs")).unwrap(),
        "edited\n"
    );

    // クラスタ側から成果物が消えても（人が片付けた等）、次の pull は手元の成果物を消さない。
    std::fs::remove_dir_all(remote.join("artifacts")).unwrap();
    ws.pull().await.expect("next prepare pull");
    assert_eq!(log_lines(&log), vec!["pull", "push", "pull"]);
    assert_eq!(
        std::fs::read_to_string(local.join("src/lib.rs")).unwrap(),
        "edited\n"
    );
    assert_eq!(
        std::fs::read_to_string(local.join("artifacts/experiment-plan.md")).unwrap(),
        "plan\n"
    );
}

/// push が落ちたら印が残り、エラーとして返る（黙らない）。印がある間の pull は `--delete` の前に push を
/// やり直し、それも落ちたら pull しない（手元の編集を消さない）。繋がれば push → pull の順で進む。
#[tokio::test]
async fn failed_push_is_reported_and_blocks_the_deleting_pull_until_a_push_succeeds() {
    if !rsync_available() {
        eprintln!("rsync is not installed; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote");
    let local = tmp.path().join("mirror");
    std::fs::create_dir_all(&remote).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("new.rs"), "worker edit\n").unwrap();
    let log = tmp.path().join("rsync.log");
    let mut failing = SshSettings::new("c", "h", &remote);
    failing.ssh_command = local_ssh(tmp.path());
    failing.rsync_command = logging_rsync(tmp.path(), "rsync-fail", &local, &log, true);
    let ws = SshWorkspace::new(&local, failing.clone());

    let err = ws.push_after_run().await.expect_err("push fails");
    assert!(matches!(err, WorkspaceError::Remote(_)), "{err:?}");
    assert!(ws.push_pending());
    ws.pull()
        .await
        .expect_err("pull must not run while the push is pending");
    assert_eq!(
        log_lines(&log),
        vec!["push", "push"],
        "no pull was attempted"
    );
    assert!(local.join("new.rs").is_file(), "the local edit survives");

    let mut ok = failing;
    ok.rsync_command = logging_rsync(tmp.path(), "rsync-ok", &local, &log, false);
    let ws = SshWorkspace::new(&local, ok);
    ws.pull().await.expect("push then pull");
    assert_eq!(log_lines(&log), vec!["push", "push", "push", "pull"]);
    assert!(!ws.push_pending());
    assert_eq!(
        std::fs::read_to_string(remote.join("new.rs")).unwrap(),
        "worker edit\n"
    );
    assert!(local.join("new.rs").is_file());
}

#[tokio::test]
async fn push_after_run_is_a_no_op_under_sync_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = SshSettings::new("c", "h", PathBuf::from("/work/x"));
    settings.sync = SyncMode::None;
    settings.ssh_command = vec!["/nonexistent/ssh-should-not-run".into()];
    settings.rsync_command = vec!["/nonexistent/rsync-should-not-run".into()];
    let ws = SshWorkspace::new(dir.path(), settings);
    ws.push_after_run().await.expect("no-op");
    assert!(!ws.push_pending());
}

#[test]
fn worker_and_reviewer_instructions_share_the_remote_exec_usage() {
    let mut settings = SshSettings::new(
        "sirius",
        "sirius",
        "/work/NBB/rmaeda/workspace/rust/benchfs",
    );
    settings.sync = SyncMode::Worktree;
    settings.task_id = "01TASK".into();
    let worker = remote_exec_instructions(&settings);
    let reviewer = remote_exec_reviewer_instructions(&settings);
    for text in [&worker, &reviewer] {
        assert!(text.contains(REMOTE_EXEC_USAGE), "{text}");
        assert!(text.contains(&remote_exec_head(&settings)), "{text}");
        assert!(text.contains("ブランチ `celeris/01TASK`"), "{text}");
    }
    assert!(worker.contains("run の後にクラスタへ同期され"));
    // ADR-0090 D5: worker の指示文の最後はクラスタ job の wait の段落（reviewer には無い）。
    assert!(
        worker.contains("元のリポジトリの作業ツリーは触らないでください。\n長く走るクラスタ job")
    );
    assert!(worker.ends_with(&cluster_job_wait_instructions(&settings)));
    assert!(
        worker.contains("\"type\": \"wait\", \"kind\": \"cluster_job\", \"cluster\": \"sirius\"")
    );
    assert!(worker.contains("job が Q / R の間に完了を申告しない"));
    assert!(!reviewer.contains("長く走るクラスタ job"));
    assert!(reviewer.contains("`git status`"));
    assert!(!reviewer.contains("元のリポジトリの作業ツリーは触らないでください"));
}

/// ADR-0090 D2: master 越しの 1 コマンドは stdout / stderr を先頭から丸ごと返し、exit 255 は接続の失敗。
#[test]
fn remote_command_returns_the_whole_output_and_treats_255_as_a_connection_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ok = fake_ssh_probe(
        dir.path(),
        "#!/bin/sh\nprintf 'Job Id: 1.pbs\\n    job_state = F\\n'\necho 'qstat: Unknown Job Id 2.pbs' >&2\nexit 153\n",
    );
    let out = run_remote_command_blocking(&ok, "sirius", "qstat -xf 1 2", Duration::from_secs(10))
        .expect("ran");
    assert_eq!(out.exit, Some(153));
    assert!(out.stdout.contains("job_state = F"), "{out:?}");
    assert!(out.stderr.contains("Unknown Job Id 2.pbs"), "{out:?}");
    let dir = tempfile::tempdir().unwrap();
    let down = fake_ssh_probe(
        dir.path(),
        "#!/bin/sh\necho 'mux_client: no master' >&2\nexit 255\n",
    );
    let err = run_remote_command_blocking(&down, "sirius", "qstat -xf 1", Duration::from_secs(10))
        .expect_err("255 is a connection failure");
    assert!(err.contains("exit 255"), "{err}");
}

// ---- ADR-0062 A（Phase 107）: 実通信 probe（`ssh -o BatchMode=yes <host> -- true`） ----

fn fake_ssh_probe(dir: &Path, script: &str) -> Vec<String> {
    let path = dir.join("ssh");
    crate::test_support::write_executable(&path, script);
    vec![path.to_string_lossy().into_owned()]
}

#[test]
fn command_probe_succeeds_when_the_remote_command_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh_probe(dir.path(), "#!/bin/sh\nexit 0\n");
    assert!(control_master_command_probe_blocking(
        &ssh,
        "cluster-host",
        Duration::from_secs(2)
    ));
}

#[test]
fn command_probe_fails_when_the_remote_command_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = fake_ssh_probe(dir.path(), "#!/bin/sh\nexit 255\n");
    assert!(!control_master_command_probe_blocking(
        &ssh,
        "cluster-host",
        Duration::from_secs(2)
    ));
}

/// NAT の idle timeout で TCP が黙って死んだ状況を模す: ssh が応答せずハングし続ける。
/// `timeout` を超えたら kill されて `false` になる（ハングしたまま残らない）。
#[test]
fn command_probe_times_out_and_kills_a_hanging_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let script = format!(
        "#!/bin/sh\necho $$ > {state:?}/pid\nwhile true; do sleep 3600; done\n",
        state = state.path()
    );
    let ssh = fake_ssh_probe(dir.path(), &script);
    let start = std::time::Instant::now();
    assert!(!control_master_command_probe_blocking(
        &ssh,
        "cluster-host",
        Duration::from_millis(300)
    ));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "probe must not block past the timeout"
    );
    let pid: u32 = std::fs::read_to_string(state.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    for _ in 0..150 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("hanging ssh process {pid} was not killed");
}

// ---- Phase R6-3: クラスタの worktree は git submodule を初期化する ----

/// テスト用の git（人の設定・対話的な認証に引きずられない。ローカルパスの submodule を許す）。
fn test_git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `sub`（1 ファイル）を submodule `lib/sub` に持つ `proj` を作る。`with_submodule = false` なら submodule 無し。
fn project_repo(root: &Path, with_submodule: bool) -> PathBuf {
    let sub = root.join("sub");
    let proj = root.join("proj");
    for dir in [&sub, &proj] {
        std::fs::create_dir_all(dir).unwrap();
        test_git(dir, &["init", "-q", "-b", "main"]);
    }
    std::fs::write(sub.join("lib.rs"), "pub fn sub() {}\n").unwrap();
    test_git(&sub, &["add", "-A"]);
    test_git(&sub, &["commit", "-q", "-m", "sub"]);
    std::fs::write(proj.join("README.md"), "proj\n").unwrap();
    test_git(&proj, &["add", "-A"]);
    if with_submodule {
        let url = sub.to_string_lossy().into_owned();
        test_git(&proj, &["submodule", "add", "-q", &url, "lib/sub"]);
    }
    test_git(&proj, &["commit", "-q", "-m", "proj"]);
    proj
}

/// `local_ssh` と同じ偽 ssh だが、ローカルパスの submodule の clone を許す（git ≥ 2.38.1 の既定は拒否）。
fn local_ssh_allowing_file_protocol(dir: &Path) -> Vec<String> {
    let path = dir.join("fake-ssh-git");
    crate::test_support::write_executable(
        &path,
        "#!/bin/sh\nwhile [ \"$1\" = \"-o\" ]; do shift 2; done\nshift\n\
         export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.file.allow GIT_CONFIG_VALUE_0=always GIT_TERMINAL_PROMPT=0\n\
         exec sh -c \"$*\"\n",
    );
    vec![path.to_string_lossy().into_owned()]
}

fn worktree_workspace(tmp: &Path, proj: &Path, ssh: Vec<String>) -> SshWorkspace {
    let mut settings = SshSettings::new("sirius", "h", proj);
    settings.sync = SyncMode::Worktree;
    settings.task_id = "01R63TASK".into();
    settings.ssh_command = ssh;
    SshWorkspace::new(tmp.join("mirror"), settings)
}

/// 本番 2026-09-29（task 01M3Q25DSD895DGMGPWD752G3G、sirius の BenchFS）: `git worktree add` の直後は
/// submodule のディレクトリが空だった。準備で `submodule update --init --recursive` し、進行を 1 行残す。
/// 2 回目（再利用）は落ちず、初期化済みなので何もしない（行も増えない）。
#[tokio::test]
async fn ensure_worktree_initialises_submodules_and_is_idempotent_on_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project_repo(tmp.path(), true);
    let ws = worktree_workspace(
        tmp.path(),
        &proj,
        local_ssh_allowing_file_protocol(tmp.path()),
    );
    let wt = ws.effective_remote_dir();

    ws.ensure_worktree().await.expect("first prepare");
    assert_eq!(
        std::fs::read_to_string(wt.join("lib/sub/lib.rs")).unwrap(),
        "pub fn sub() {}\n",
        "the submodule is populated in the fresh worktree"
    );
    let notes = ws.take_progress_notes();
    assert_eq!(
        notes,
        vec![format!(
            "initialised 1 submodules in {} on cluster sirius",
            wt.display()
        )]
    );

    ws.ensure_worktree().await.expect("reuse does not fail");
    assert!(wt.join("lib/sub/lib.rs").is_file());
    assert!(
        ws.take_progress_notes().is_empty(),
        "already initialised: nothing to do on reuse"
    );

    // 人が submodule を deinit しても（空のディレクトリに戻る）、次の準備で戻る。
    test_git(&wt, &["submodule", "deinit", "-q", "--all", "--force"]);
    assert!(!wt.join("lib/sub/lib.rs").exists());
    ws.ensure_worktree().await.expect("re-init on reuse");
    assert!(wt.join("lib/sub/lib.rs").is_file());
    assert_eq!(ws.take_progress_notes().len(), 1);
}

#[tokio::test]
async fn ensure_worktree_skips_the_submodule_step_without_gitmodules() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project_repo(tmp.path(), false);
    let ws = worktree_workspace(tmp.path(), &proj, local_ssh(tmp.path()));
    ws.ensure_worktree().await.expect("prepare");
    let wt = ws.effective_remote_dir();
    assert!(wt.join("README.md").is_file());
    assert!(!wt.join(".gitmodules").exists());
    assert!(ws.take_progress_notes().is_empty());
}

/// Phase R7-4: submodule の初期化に失敗しても（ここでは submodule の元を消した）、準備は通る。
/// 失敗は submodule の path とクラスタ・worktree を名指しした警告の進行の行になる（黙らない）。
#[tokio::test]
async fn a_failed_submodule_init_is_a_warning_note_not_a_prepare_error() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project_repo(tmp.path(), true);
    std::fs::remove_dir_all(tmp.path().join("sub")).unwrap();
    let ws = worktree_workspace(
        tmp.path(),
        &proj,
        local_ssh_allowing_file_protocol(tmp.path()),
    );
    ws.ensure_worktree()
        .await
        .expect("a broken submodule does not stop the prepare");
    let wt = ws.effective_remote_dir();
    assert!(
        wt.join("README.md").is_file(),
        "the worktree itself is fine"
    );
    let notes = ws.take_progress_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let note = &notes[0];
    assert!(
        note.starts_with("submodule lib/sub could not be initialised: "),
        "{note}"
    );
    assert!(note.contains("sirius"), "{note}");
    assert!(note.contains(&wt.to_string_lossy().into_owned()), "{note}");
}

/// 本番 2026-09-30（task 01M3PAZ4XG4QN1T8S98VNA6ABV、sirius の BenchFS）: `ior_integration/ior` の固定 commit が
/// remote に無く（`not our ref`）、`submodule update --init --recursive` 1 回の失敗で準備ごと落ちた。
/// R7-4: submodule は 1 つずつ初期化する。良い方は入り、悪い方は `fatal:` の行つきの警告になる。
#[tokio::test]
async fn one_unfetchable_submodule_does_not_stop_the_other_or_the_prepare() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project_repo(tmp.path(), true);
    // 2 つ目の submodule `ior`: remote に無い commit（submodule の中だけの commit）を固定する。
    let bad = tmp.path().join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    test_git(&bad, &["init", "-q", "-b", "main"]);
    std::fs::write(bad.join("ior.c"), "int main(){}\n").unwrap();
    test_git(&bad, &["add", "-A"]);
    test_git(&bad, &["commit", "-q", "-m", "ior"]);
    let url = bad.to_string_lossy().into_owned();
    test_git(&proj, &["submodule", "add", "-q", &url, "ior"]);
    std::fs::write(proj.join("ior/local.c"), "// unpushed\n").unwrap();
    test_git(&proj.join("ior"), &["add", "-A"]);
    test_git(&proj.join("ior"), &["commit", "-q", "-m", "local only"]);
    test_git(&proj, &["add", "-A"]);
    test_git(&proj, &["commit", "-q", "-m", "pin an unpushed ior commit"]);

    let ws = worktree_workspace(
        tmp.path(),
        &proj,
        local_ssh_allowing_file_protocol(tmp.path()),
    );
    let wt = ws.effective_remote_dir();
    ws.ensure_worktree()
        .await
        .expect("one broken submodule does not stop the prepare");
    assert_eq!(
        std::fs::read_to_string(wt.join("lib/sub/lib.rs")).unwrap(),
        "pub fn sub() {}\n",
        "the good submodule is populated"
    );
    assert!(!wt.join("ior/local.c").exists());
    let notes = ws.take_progress_notes();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(
        notes[0],
        format!(
            "initialised 1 submodules in {} on cluster sirius",
            wt.display()
        )
    );
    assert!(
        notes[1].starts_with("submodule ior could not be initialised: fatal:"),
        "{}",
        notes[1]
    );
    assert!(notes[1].contains("sirius"), "{}", notes[1]);

    // 再利用でも落ちない。失敗した ior は clone までは済んで `submodule status` の行頭が `-` でなくなるので、
    // 再利用では試し直さない（R6-3 と同じく、`-` の無い worktree には触らない = 人・agent の submodule の
    // commit を巻き戻さない）。警告は最初の準備の 1 回だけ。
    ws.ensure_worktree().await.expect("reuse");
    assert!(wt.join("lib/sub/lib.rs").is_file());
    assert!(ws.take_progress_notes().is_empty());
}
