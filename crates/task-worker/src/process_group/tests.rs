use super::*;

/// このモジュールの実プロセスを使う非同期テストを直列化する。
/// `reap_finished_children` のテストは process 全体を対象とするため、
/// 別の integration-test binary で実行する。
/// `tokio::sync::Mutex` を使うのは、await をまたいでガードを持ち続ける `#[tokio::test]` が
/// `std::sync::Mutex` だと clippy（`await_holding_lock`）に落ちるため。
fn child_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 非同期テスト（`#[tokio::test]`）用。
async fn serialize_child_process_tests() -> tokio::sync::MutexGuard<'static, ()> {
    child_test_lock().lock().await
}

#[test]
fn registration_is_visible_until_it_is_dropped() {
    let run_id = format!("reg-{}", std::process::id());
    {
        let _guard = ProcessGroup::register(&run_id, Some(4_000_000));
        assert_eq!(pgid_of(&run_id), Some(4_000_000));
    }
    assert_eq!(pgid_of(&run_id), None);
}

/// Phase 119 D2: `sweep(pid, grace)` はそのプロセスグループへ SIGTERM を送る（登録テーブルは
/// 経由しない、`child.id()` から直接渡された pid で動く）。孤児が居なければ `killpg` が
/// `ESRCH` を返すだけの no-op であることも確認する（`spawn_group_reaper` の余計なスレッドを
/// 立てない）。
#[tokio::test]
async fn sweep_signals_the_group_and_is_a_no_op_when_nothing_is_left() {
    let _serial = serialize_child_process_tests().await;
    let mut command = tokio::process::Command::new("sleep");
    command.arg("300").kill_on_drop(true);
    command.process_group(0);
    let mut child = command.spawn().unwrap_or_else(|e| panic!("spawn: {e}"));
    let pid = child.id().expect("child pid");

    sweep(pid, Duration::from_secs(10));
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap_or_else(|_| panic!("the process should have exited after sweep's SIGTERM"))
        .unwrap_or_else(|e| panic!("wait: {e}"));
    assert!(
        !status.success(),
        "sleep should have been signalled, not exited on its own"
    );

    // 掃除対象が既に居ない（既に reap 済みの pid）なら no-op。
    assert!(!signal_group(pid as i32, Signal::SIGTERM));
}

/// ADR-0070 D5（Phase 116）: 登録された偽の子プロセスが生きている間は `group_alive` が `true`、
/// プロセスが死んでも登録がまだ残っていれば（`Dispatcher::reclaim_expired_leases` が
/// `self.running` を消す前に見る、まさにこの状況）`false` になる。未登録の run は常に `false`。
#[tokio::test]
async fn group_alive_reflects_whether_the_registered_process_is_still_running() {
    let _serial = serialize_child_process_tests().await;
    assert!(!group_alive("no-such-run"));

    let mut command = tokio::process::Command::new("sleep");
    command.arg("300").kill_on_drop(true);
    command.process_group(0);
    let mut child = command.spawn().unwrap_or_else(|e| panic!("spawn: {e}"));
    let pid = child.id().expect("child pid");
    let run_id = format!("alive-{}", std::process::id());
    let guard = ProcessGroup::register(&run_id, Some(pid));
    assert!(group_alive(&run_id), "just-spawned child should be alive");

    // 登録はそのままに、プロセスだけを直接殺す（`kill_tree` は先に登録を消してしまうので使わない）。
    signal::kill(nix::unistd::Pid::from_raw(pid as i32), Signal::SIGKILL)
        .expect("kill the fake child");
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    assert!(
        !group_alive(&run_id),
        "a dead process should not be reported as alive even while still registered"
    );
    drop(guard);
    assert!(
        !group_alive(&run_id),
        "dropping the guard also unregisters it"
    );
}

#[test]
fn a_child_without_a_pid_registers_nothing() {
    let run_id = format!("nopid-{}", std::process::id());
    let _guard = ProcessGroup::register(&run_id, None);
    assert_eq!(pgid_of(&run_id), None);
}

#[test]
fn killing_an_unknown_run_is_a_no_op() {
    assert!(!kill_tree("no-such-run", Duration::from_millis(1)));
}

/// 実プロセスで一族ごと止まることを見る（孫まで）。
#[tokio::test]
async fn kill_tree_terminates_the_whole_group_including_grandchildren() {
    let _serial = serialize_child_process_tests().await;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!("sleep 300 & echo $! > {}; wait", pidfile.to_string_lossy());
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(&script)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.process_group(0);
    let mut child = command.spawn().unwrap_or_else(|e| panic!("spawn: {e}"));
    let run_id = format!("group-{}", std::process::id());
    let guard = ProcessGroup::register(&run_id, child.id());

    // 孫の pid が書かれるのを待つ（上限付き）。
    let grandchild = wait_for_pid(&pidfile).await;

    assert!(kill_tree(&run_id, Duration::from_millis(200)));
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    drop(guard);

    // 孫も居なくなる（SIGTERM で `sleep` は死ぬ）。
    for _ in 0..100 {
        if !pid_alive(grandchild) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the grandchild {grandchild} is still alive after kill_tree");
}

/// Phase 55/56 の合流（ADR-0044 P55-4 / ADR-0043 P56-7）: コンテナで走る run では、ホストの
/// プロセスグループへの 2 段と**同じ瞬間**に `<runtime> kill --signal TERM` →（`grace` 後）
/// `rm -f` がラベル越しに出る。偽の runtime に argv を記録させて確かめる。
#[cfg(unix)]
#[tokio::test]
async fn a_containerized_run_is_also_stopped_inside_the_container() {
    let _serial = serialize_child_process_tests().await;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let log = dir.path().join("argv.log");
    let runtime = write_fake_runtime(dir.path(), &log);

    // ホスト側: 孫を持つ本物のプロセスグループ（`<runtime> run` のクライアントに相当）。
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!("sleep 300 & echo $! > {}; wait", pidfile.to_string_lossy());
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(&script)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.process_group(0);
    let mut child = command.spawn().unwrap_or_else(|e| panic!("spawn: {e}"));
    let run_id = format!("container-{}", std::process::id());
    let guard = ProcessGroup::register(&run_id, child.id());
    let grandchild = wait_for_pid(&pidfile).await;

    let stopper: Arc<dyn ContainerStopper> = Arc::new(crate::container::ContainerStop {
        program: runtime.to_string_lossy().into_owned(),
        task_id: "01TASKCONTAINER".into(),
    });
    assert!(kill_tree_with(
        &run_id,
        Duration::from_millis(200),
        Some(stopper)
    ));

    // SIGTERM の段は同期に出ている。
    let first = read_lines(&log);
    assert_eq!(
        first,
        vec![
            "ps -aq --filter label=celeris.task=01TASKCONTAINER",
            "kill --signal TERM c0ffee111111",
        ],
        "the container did not get a SIGTERM at the same moment as the process group"
    );

    // `grace` の後に `rm -f`（別スレッド）。
    let lines = wait_for_lines(&log, 4).await;
    assert_eq!(
        lines,
        vec![
            "ps -aq --filter label=celeris.task=01TASKCONTAINER",
            "kill --signal TERM c0ffee111111",
            "ps -aq --filter label=celeris.task=01TASKCONTAINER",
            "rm -f c0ffee111111",
        ],
        "the container was not removed after the grace period"
    );

    // ホスト側は従来どおり（孫まで消える）。
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    drop(guard);
    for _ in 0..100 {
        if !pid_alive(grandchild) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the grandchild {grandchild} is still alive after kill_tree_with");
}

/// 登録が無くても（クライアントが先に死んでコンテナだけ残った）コンテナは止める。
#[cfg(unix)]
#[tokio::test]
async fn a_container_is_stopped_even_when_the_process_group_is_already_gone() {
    let _serial = serialize_child_process_tests().await;
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let log = dir.path().join("argv.log");
    let runtime = write_fake_runtime(dir.path(), &log);
    let stopper: Arc<dyn ContainerStopper> = Arc::new(crate::container::ContainerStop {
        program: runtime.to_string_lossy().into_owned(),
        task_id: "01TASKORPHAN".into(),
    });
    assert!(kill_tree_with(
        "no-such-run",
        Duration::from_millis(100),
        Some(stopper)
    ));
    let lines = wait_for_lines(&log, 4).await;
    assert_eq!(lines[1], "kill --signal TERM c0ffee111111");
    assert_eq!(lines[3], "rm -f c0ffee111111");
}

/// `ps -aq --filter …` に 1 件返し、呼ばれた argv を 1 行ずつ記録する偽の runtime。
#[cfg(unix)]
fn write_fake_runtime(dir: &std::path::Path, log: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-runtime");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nif [ \"$1\" = ps ]; then echo c0ffee111111; fi\nexit 0\n",
            log.to_string_lossy()
        ),
    )
    .unwrap_or_else(|e| panic!("write fake runtime: {e}"));
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

async fn wait_for_lines(path: &std::path::Path, want: usize) -> Vec<String> {
    for _ in 0..200 {
        let lines = read_lines(path);
        if lines.len() >= want {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the fake runtime recorded only {:?}", read_lines(path));
}

async fn wait_for_pid(path: &std::path::Path) -> i32 {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the grandchild never wrote its pid to {}", path.display());
}

fn pid_alive(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
