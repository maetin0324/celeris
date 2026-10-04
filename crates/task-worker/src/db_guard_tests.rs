//! ADR-0095: 実プロセスで、namespace の中から DB が書けず、兄弟は書けることを確かめる。

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use task_core::{SqliteStore, TaskStore};

use super::*;

/// 本番の形: `<root>/lib/{celeris.sqlite3(+wal,shm), workspaces/t1, scratch/}`。DB は daemon と同じく
/// 開いたまま（WAL の `-wal` / `-shm` が残る）にするため `SqliteStore` を返して持たせる。
struct Fixture {
    _tmp: tempfile::TempDir,
    lib: PathBuf,
    db: PathBuf,
    ws: PathBuf,
    scratch: PathBuf,
    store: SqliteStore,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    let ws = lib.join("workspaces").join("t1");
    let scratch = lib.join("scratch");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    let db = lib.join("celeris.sqlite3");
    let store = SqliteStore::open(&db).unwrap();
    assert!(lib.join("celeris.sqlite3-wal").exists());
    assert!(lib.join("celeris.sqlite3-shm").exists());
    Fixture {
        _tmp: tmp,
        lib,
        db,
        ws,
        scratch,
        store,
    }
}

fn guard(f: &Fixture) -> DbGuard {
    DbGuard::new(&f.db)
        .unwrap()
        .with_ssh_shadow_root(Some(f._tmp.path().join("shadow")))
        // 実 HOME に依存しない（付記 D-a の 3 は専用の試験で HOME を注入して見る）。
        .with_home(None)
}

/// `script` を namespace の中の `sh -c` で走らせ、(exit 0 か, stdout+stderr) を返す。
fn run_guarded(g: &DbGuard, cwd: &Path, script: &str) -> (bool, String) {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script).current_dir(cwd);
    apply_std(&mut cmd, g).unwrap();
    let out = cmd.output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn q(p: &Path) -> String {
    format!("'{}'", p.display())
}

fn userns_available() -> bool {
    if crate::test_support::skip_unless_userns_tests() {
        return false;
    }
    let available = Command::new("unshare")
        .args(["-Ur", "true"])
        .output()
        .is_ok_and(|out| out.status.success());
    assert!(
        available,
        "user namespace is required for db_guard tests (CELERIS_USERNS_TESTS=1)"
    );
    true
}

#[test]
fn user_systemd_bus_address_is_removed_even_without_a_guard() {
    let mut cmd = tokio::process::Command::new("true");
    cmd.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/test/bus");
    let cmd = launch(cmd, None);
    assert!(
        cmd.as_std()
            .get_envs()
            .find(|(name, _)| *name == "DBUS_SESSION_BUS_ADDRESS")
            .is_some_and(|(_, value)| value.is_none())
    );
}

#[test]
fn user_systemd_bus_is_hidden_from_launched_process() {
    if !userns_available() {
        return;
    }
    let f = fixture();
    let runtime = f._tmp.path().join("runtime");
    std::fs::create_dir_all(runtime.join("systemd")).unwrap();
    let _bus = UnixListener::bind(runtime.join("bus")).unwrap();
    let _private = UnixListener::bind(runtime.join("systemd/private")).unwrap();
    // install（プロセス全体）は使わない: 並行する他の試験の spawn にこの tempdir のガードが掛かってしまう。
    let g = guard(&f);

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("test -z \"${DBUS_SESSION_BUS_ADDRESS+x}\" && test ! -S \"/run/user/$(id -u)/bus\" && test ! -S \"$XDG_RUNTIME_DIR/bus\" && test -f \"$XDG_RUNTIME_DIR/bus\" && test -d \"$XDG_RUNTIME_DIR/systemd\" && test -z \"$(ls -A \"$XDG_RUNTIME_DIR/systemd\")\" && test ! -e \"$XDG_RUNTIME_DIR/systemd/private\"")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/definitely-not-a-bus");
    let mut cmd = launch_with(cmd, None, Some(&g));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(async { cmd.output().await }).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        runtime.join("bus").exists(),
        "parent namespace was modified"
    );
    assert!(
        runtime.join("systemd/private").exists(),
        "parent namespace was modified"
    );

    // Keep this an end-to-end check without consulting or invoking the host manager. The
    // namespace above is backed by a fake runtime containing listening sockets, so success
    // would prove that the launched process escaped the masked paths.
    for (program, args) in [
        ("systemctl", &["--user", "show-environment"][..]),
        ("systemd-run", &["--user", "--scope", "true"][..]),
    ] {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).env("XDG_RUNTIME_DIR", &runtime);
        let mut cmd = launch_with(cmd, None, Some(&g));
        let out = rt.block_on(async { cmd.output().await }).unwrap();
        assert!(
            !out.status.success(),
            "{program} --user unexpectedly succeeded in the masked namespace"
        );
    }
}

/// 付記 D-a の 3: `$HOME` の `.config/systemd`・`.local/celeris/releases`・`.config/celeris` は namespace の中で
/// 読み取り専用（EROFS）になり、外では同じ dir に書ける。HOME は tempdir を注入する（実 HOME に依存しない）。
#[test]
fn host_config_read_only_inside_namespace() {
    if !userns_available() {
        return;
    }
    let f = fixture();
    let home = f._tmp.path().join("home");
    let dirs: Vec<PathBuf> = HOST_CONFIG_DIRS.iter().map(|rel| home.join(rel)).collect();
    for dir in &dirs {
        std::fs::create_dir_all(dir).unwrap();
    }
    // `.config` 自体は対象外で、namespace の中でも書ける。
    let g = guard(&f).with_home(Some(home.clone()));
    let mut script = String::new();
    for (i, dir) in dirs.iter().enumerate() {
        script.push_str(&format!(
            "if touch {d}/inside 2>err{i}; then echo WRITABLE-{i}; \
             elif grep -q 'Read-only file system' err{i}; then echo erofs-{i}; \
             else cat err{i}; fi; ",
            d = q(dir)
        ));
    }
    script.push_str(&format!(
        "touch {}/outside-ro && echo config-ok; touch ok && echo ws-ok",
        q(&home.join(".config"))
    ));
    let (ok, out) = run_guarded(&g, &f.ws, &script);
    assert!(ok, "{out}");
    for i in 0..dirs.len() {
        assert!(!out.contains(&format!("WRITABLE-{i}")), "{i} in:\n{out}");
        assert!(
            out.contains(&format!("erofs-{i}")),
            "{i} missing in:\n{out}"
        );
    }
    for good in ["config-ok", "ws-ok"] {
        assert!(out.contains(good), "{good} missing in:\n{out}");
    }
    for dir in &dirs {
        assert!(!dir.join("inside").exists(), "{}", dir.display());
        // namespace の外では同じ dir に書ける。
        std::fs::write(dir.join("outside"), "x").unwrap();
    }
}

/// 付記 D-a の 3: 存在しない path には何もしない（spawn は通常どおり）。
#[test]
fn host_config_read_only_skips_missing_paths() {
    let f = fixture();
    let home = f._tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/celeris")).unwrap();
    let g = guard(&f).with_home(Some(home.clone()));
    assert_eq!(
        g.host_config_read_only_paths().unwrap(),
        vec![std::fs::canonicalize(home.join(".config/celeris")).unwrap()]
    );
    assert!(
        guard(&f)
            .with_home(Some(f._tmp.path().join("absent-home")))
            .host_config_read_only_paths()
            .unwrap()
            .is_empty()
    );
}

/// ADR-0136: 本番 config を使わず、解決済みの hot mount と releases を直接注入する。
/// 旧 home の保護対象は設定済み path が増えても残す。
#[test]
fn worker_db_guard_protected_set_includes_configured_hot_paths_and_legacy_home() {
    let f = fixture();
    let home = f._tmp.path().join("home");
    for rel in HOST_CONFIG_DIRS {
        std::fs::create_dir_all(home.join(rel)).unwrap();
    }
    let hot = f._tmp.path().join("local");
    let releases = hot.join("celeris/state/releases");
    std::fs::create_dir_all(&releases).unwrap();
    let alias = f._tmp.path().join("releases-alias");
    std::os::unix::fs::symlink(&releases, &alias).unwrap();
    let guard = guard(&f)
        .with_home(Some(home.clone()))
        .with_hot_mount(Some(hot.clone()))
        .with_releases_dir(alias);

    let protected = guard.host_config_read_only_paths().unwrap();
    for rel in HOST_CONFIG_DIRS {
        assert!(protected.contains(&std::fs::canonicalize(home.join(rel)).unwrap()));
    }
    assert!(protected.contains(&std::fs::canonicalize(hot).unwrap()));
    assert!(protected.contains(&std::fs::canonicalize(releases).unwrap()));
    assert_eq!(protected.len(), HOST_CONFIG_DIRS.len() + 2);
}

#[test]
fn worker_db_guard_refuses_missing_configured_releases() {
    let f = fixture();
    let releases_guard = guard(&f).with_releases_dir(f._tmp.path().join("missing-releases"));
    assert_eq!(
        releases_guard
            .host_config_read_only_paths()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    let guard = guard(&f).with_hot_mount(Some(f._tmp.path().join("missing-hot-mount")));
    assert_eq!(
        guard.host_config_read_only_paths().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

/// 付記 D-a の 3 / D5: 存在を確かめられない（EACCES）なら spawn の準備が失敗する（保護なしで起動しない）。
#[test]
fn host_config_read_only_failure_refuses_the_spawn() {
    use std::os::unix::fs::PermissionsExt;
    if nix::unistd::geteuid().is_root() {
        eprintln!("skip: root ignores directory permissions");
        return;
    }
    let f = fixture();
    let home = f._tmp.path().join("home");
    let config = home.join(".config");
    std::fs::create_dir_all(config.join("celeris")).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
    let g = guard(&f).with_home(Some(home));
    let mut std_cmd = Command::new("true");
    let prepared = apply_std(&mut std_cmd, &g);
    let mut cmd = tokio::process::Command::new("true");
    apply(&mut cmd, &g);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(prepared.is_err(), "spawn preparation should fail");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let res = rt.block_on(async { cmd.status().await });
    assert!(res.is_err(), "spawn should be refused: {res:?}");
}

#[test]
fn the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable() {
    if !userns_available() {
        return;
    }
    let f = fixture();
    let g = guard(&f);
    let db = q(&f.db);
    let script = format!(
        "(echo x >> {db}) 2>/dev/null && echo DB-WRITABLE; \
         (true >> {db}-wal) 2>/dev/null && echo WAL-WRITABLE; \
         (true >> {db}-shm) 2>/dev/null && echo SHM-WRITABLE; \
         rm -f {db} 2>/dev/null && echo DB-REMOVED; \
         mv {db} {db}.moved 2>/dev/null && echo DB-MOVED; \
         (true > {db}-journal) 2>/dev/null && echo JOURNAL-CREATED; \
         touch {lib}/new-sibling 2>/dev/null && echo SIBLING-CREATED; \
         touch ../../via-relative-path 2>/dev/null && echo RELATIVE-WRITABLE; \
         ln {db} {ws}/hardlink 2>/dev/null && echo HARDLINKED; \
         test -w {db} && echo ACCESS-SAYS-WRITABLE; \
         touch ok && echo ws-ok; \
         touch {scratch}/x && echo scratch-ok; \
         cat {db} > /dev/null && echo db-readable",
        lib = q(&f.lib),
        ws = q(&f.ws),
        scratch = q(&f.scratch),
    );
    let (ok, out) = run_guarded(&g, &f.ws, &script);
    assert!(ok, "{out}");
    for bad in [
        "DB-WRITABLE",
        "WAL-WRITABLE",
        "SHM-WRITABLE",
        "DB-REMOVED",
        "DB-MOVED",
        "JOURNAL-CREATED",
        "SIBLING-CREATED",
        "RELATIVE-WRITABLE",
        "HARDLINKED",
        "ACCESS-SAYS-WRITABLE",
    ] {
        assert!(!out.contains(bad), "{bad} in:\n{out}");
    }
    for good in ["ws-ok", "scratch-ok", "db-readable"] {
        assert!(out.contains(good), "{good} missing in:\n{out}");
    }
    assert!(f.db.exists());
    assert!(!f.lib.join("new-sibling").exists());
    assert!(f.ws.join("ok").exists());
    // DB は壊れていない（daemon 役の接続でそのまま読める）。
    assert!(f.store.list(None).unwrap().is_empty());
    assert!(task_core::integrity_check(&f.db).unwrap());
}

#[test]
fn a_nested_user_namespace_cannot_undo_the_read_only_mount() {
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
    let f = fixture();
    let g = guard(&f);
    assert!(
        Command::new("unshare").arg("--help").output().is_ok(),
        "unshare(1) is required for this test (CELERIS_USERNS_TESTS=1)"
    );
    let script = format!(
        "unshare -Urm sh -c \"mount -o remount,bind,rw {lib} && echo REMOUNTED; \
         umount {lib} && echo UNMOUNTED; touch {lib}/escaped && echo ESCAPED\" 2>&1; \
         unshare -Ur true && echo nested-userns-ok",
        lib = q(&f.lib)
    );
    let (_, out) = run_guarded(&g, &f.ws, &script);
    assert!(!out.contains("REMOUNTED"), "{out}");
    assert!(!out.contains("UNMOUNTED"), "{out}");
    assert!(!out.contains("ESCAPED"), "{out}");
    // 入れ子の sandbox（codex の bwrap 等）が user namespace を作れる（pivot_root / chroot をしないため）。
    assert!(out.contains("nested-userns-ok"), "{out}");
    assert!(!f.lib.join("escaped").exists());
}

#[test]
fn the_guarded_process_has_no_capabilities_and_keeps_its_uid() {
    if !userns_available() {
        return;
    }
    let f = fixture();
    let g = guard(&f);
    let (ok, out) = run_guarded(
        &g,
        &f.ws,
        "grep CapEff /proc/self/status; id -u; cat /proc/self/uid_map",
    );
    assert!(ok, "{out}");
    assert!(out.contains("CapEff:\t0000000000000000"), "{out}");
    let uid = nix::unistd::getuid().as_raw().to_string();
    assert!(out.lines().any(|l| l.trim() == uid), "{out}");
}

#[test]
fn probe_confirms_the_db_is_not_writable_inside() {
    if !userns_available() {
        return;
    }
    let f = fixture();
    probe(&guard(&f)).unwrap();
}

#[test]
fn writable_children_skip_the_db_family_and_symlinks() {
    let f = fixture();
    std::fs::write(f.lib.join("notes.txt"), "x").unwrap();
    std::fs::write(f.lib.join("celeris.sqlite3-journal"), "").unwrap();
    std::os::unix::fs::symlink(&f.ws, f.lib.join("link")).unwrap();
    let names: Vec<String> = guard(&f)
        .writable_children()
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["notes.txt", "scratch", "workspaces"]);
}

#[test]
fn sync_ssh_shadow_copies_contents_and_drops_stale_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("ssh_config.d");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("20-a.conf"), "Host a\n").unwrap();
    let root = tmp.path().join("root");
    let shadow = sync_ssh_shadow(&src, &root).unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(shadow.join("20-a.conf")).unwrap(),
        "Host a\n"
    );
    std::fs::write(shadow.join("stale.conf"), "x").unwrap();
    std::fs::write(src.join("20-a.conf"), "Host b\n").unwrap();
    sync_ssh_shadow(&src, &root).unwrap();
    assert_eq!(
        std::fs::read_to_string(shadow.join("20-a.conf")).unwrap(),
        "Host b\n"
    );
    assert!(!shadow.join("stale.conf").exists());
    assert!(
        sync_ssh_shadow(&tmp.path().join("absent"), &root)
            .unwrap()
            .is_none()
    );
}

/// D4: namespace の中でも ssh が `/etc/ssh/ssh_config.d` の Include を "Bad owner" で拒否しない
/// （`ssh -G` は設定を解釈して表示するだけで接続しない）。ssh が無いホストでは見ない。
#[test]
fn ssh_config_includes_still_parse_inside_the_namespace() {
    if !userns_available() {
        return;
    }
    if !Path::new(SSH_CONFIG_D).is_dir() || Command::new("ssh").arg("-V").output().is_err() {
        eprintln!("skip: no ssh or no {SSH_CONFIG_D}");
        return;
    }
    let f = fixture();
    let (ok, out) = run_guarded(&guard(&f), &f.ws, "ssh -G example.invalid >/dev/null");
    assert!(ok, "ssh -G failed inside the namespace: {out}");
}

/// 守る DB が消えたガード（in-process の試験で先に終わった daemon の残り）は spawn を止めない。
#[test]
fn a_guard_whose_db_is_gone_does_not_block_spawns() {
    let f = fixture();
    let g = guard(&f);
    drop(f);
    let mut cmd = tokio::process::Command::new("true");
    apply(&mut cmd, &g);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let status = rt.block_on(async { cmd.status().await.unwrap() });
    assert!(status.success());
}

/// D2: worker の run のプロセスを spawn する全ての所が `db_guard::launch` を通る（`container::wrap` を直接
/// 呼ぶ所が残っていない）。新しい adapter を足したときにこの一覧も足す。
#[test]
fn every_worker_spawn_site_goes_through_launch() {
    let sites: [(&str, &str, usize); 10] = [
        ("subprocess.rs", include_str!("subprocess.rs"), 1),
        ("claude_code.rs", include_str!("claude_code.rs"), 1),
        ("codex.rs", include_str!("codex.rs"), 1),
        ("aider.rs", include_str!("aider.rs"), 1),
        ("acp.rs", include_str!("acp.rs"), 1),
        ("langmem.rs", include_str!("langmem.rs"), 1),
        (
            "local_deep_research.rs",
            include_str!("local_deep_research.rs"),
            1,
        ),
        ("paperqa.rs", include_str!("paperqa.rs"), 2),
        ("workspace.rs", include_str!("workspace.rs"), 1),
        ("container.rs", include_str!("container.rs"), 0),
    ];
    for (name, src, launches) in sites {
        assert_eq!(
            src.matches("crate::db_guard::launch(").count(),
            launches,
            "{name}"
        );
        assert!(
            !src.contains("crate::container::wrap("),
            "{name} bypasses db_guard::launch"
        );
    }
}

// ---------------------------------------------------------------------------
// ADR-0126 A5: 判定関数を userns・実 mount なしで一時 dir に対して直接呼ぶ。
// ---------------------------------------------------------------------------

const PROD_TOKEN: &str = "prod-secret-token-0123456789";

/// 本番の形 `<tmp>/prod/{lib/celeris.sqlite3, state, token}` と、試験 daemon 用の `<tmp>/test/`。
struct Prod {
    tmp: tempfile::TempDir,
    db: PathBuf,
    lib: PathBuf,
    state: PathBuf,
    token: PathBuf,
    test: PathBuf,
}

fn prod() -> Prod {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("prod").join("lib");
    let state = tmp.path().join("prod").join("state");
    let test = tmp.path().join("test");
    for d in [&lib, &state, &test] {
        std::fs::create_dir_all(d).unwrap();
    }
    let db = lib.join("celeris.sqlite3");
    std::fs::write(&db, b"db").unwrap();
    let token = tmp.path().join("prod").join("token");
    std::fs::write(&token, format!("{PROD_TOKEN}\n")).unwrap();
    Prod {
        tmp,
        db,
        lib,
        state,
        token,
        test,
    }
}

fn protected(p: &Prod) -> ProtectedSet {
    let mut set = ProtectedSet::new();
    set.add_db_file(&p.db);
    set.add_dir(&p.state);
    set.add_token_file(&p.token);
    assert!(set.unknown_reasons().is_empty(), "{set:?}");
    set
}

/// guard の効いた worker run の中（印 = 本番 DB のディレクトリが読み取り専用と裏付けられた）。
fn inside(p: &Prod) -> WorkerRunMarker {
    WorkerRunMarker::ReadOnly(std::fs::canonicalize(&p.lib).unwrap())
}

fn test_db(p: &Prod) -> PathBuf {
    let db = p.test.join("celeris.sqlite3");
    std::fs::write(&db, b"test").unwrap();
    db
}

fn daemon(db: PathBuf) -> DaemonPaths {
    DaemonPaths {
        db,
        state_dir: None,
        token_file: None,
    }
}

#[track_caller]
fn assert_refused(d: GuardDecision) {
    match &d {
        GuardDecision::RefuseProduction {
            inside_worker_run: true,
            ..
        } => assert_eq!(d.refusal_message(), Some(PRODUCTION_IN_WORKER_RUN)),
        other => panic!("expected RefuseProduction inside a worker run, got {other:?}"),
    }
}

#[test]
fn worker_db_guard_refuses_production_db_path_inside_worker_run() {
    let p = prod();
    let d = judge_worker_db_guard(&protected(&p), &daemon(p.db.clone()), &inside(&p));
    assert_refused(d);
    assert!(PRODUCTION_IN_WORKER_RUN.contains("ADR-0126"));
}

#[test]
fn worker_db_guard_refuses_production_db_via_symlink() {
    let p = prod();
    let link = p.test.join("link.sqlite3");
    std::os::unix::fs::symlink(&p.db, &link).unwrap();
    assert_refused(judge_worker_db_guard(
        &protected(&p),
        &daemon(link),
        &inside(&p),
    ));
    // ディレクトリへの symlink 越しでも同じ。
    let dir_link = p.test.join("libdir");
    std::os::unix::fs::symlink(&p.lib, &dir_link).unwrap();
    let via_dir = dir_link.join("celeris.sqlite3");
    assert_refused(judge_worker_db_guard(
        &protected(&p),
        &daemon(via_dir),
        &inside(&p),
    ));
}

#[test]
fn worker_db_guard_refuses_production_db_via_relative_path() {
    let p = prod();
    // `..` を含む path。
    let dotdot = p
        .test
        .join("..")
        .join("prod")
        .join("lib")
        .join("celeris.sqlite3");
    assert_refused(judge_worker_db_guard(
        &protected(&p),
        &daemon(dotdot),
        &inside(&p),
    ));
    // cwd 基準の相対 path（cwd を変えずに `../` を積んで作る）。
    let cwd = std::env::current_dir().unwrap();
    let mut rel = PathBuf::new();
    for _ in cwd.components().skip(1) {
        rel.push("..");
    }
    let abs = std::fs::canonicalize(&p.db).unwrap();
    rel.push(abs.strip_prefix("/").unwrap());
    assert!(rel.is_relative());
    assert_refused(judge_worker_db_guard(
        &protected(&p),
        &daemon(rel),
        &inside(&p),
    ));
}

#[test]
fn worker_db_guard_refuses_production_db_via_hard_link() {
    let p = prod();
    let hard = p.test.join("hard.sqlite3");
    std::fs::hard_link(&p.db, &hard).unwrap();
    let d = judge_worker_db_guard(&protected(&p), &daemon(hard), &inside(&p));
    match &d {
        GuardDecision::RefuseProduction { reason, .. } => {
            assert!(reason.contains("production db"), "{reason}")
        }
        other => panic!("expected RefuseProduction, got {other:?}"),
    }
    assert_refused(d);
}

#[test]
fn worker_db_guard_refuses_production_state_dir_subpath() {
    let p = prod();
    // 試験 DB の state_dir が本番 state_dir の中。
    let mut paths = daemon(test_db(&p));
    paths.state_dir = Some(p.state.join("sub").join("deeper"));
    assert_refused(judge_worker_db_guard(&protected(&p), &paths, &inside(&p)));
    // 試験 DB 自体が本番 state_dir の中。
    let inner = p.state.join("t");
    std::fs::create_dir_all(&inner).unwrap();
    let db = inner.join("celeris.sqlite3");
    std::fs::write(&db, b"x").unwrap();
    assert_refused(judge_worker_db_guard(
        &protected(&p),
        &daemon(db),
        &inside(&p),
    ));
    // state_dir が本番 DB のディレクトリを含む。
    let mut paths = daemon(test_db(&p));
    paths.state_dir = Some(p.tmp.path().join("prod"));
    assert_refused(judge_worker_db_guard(&protected(&p), &paths, &inside(&p)));
}

#[test]
fn worker_db_guard_refuses_production_token_file_and_copied_token() {
    let p = prod();
    let mut same = daemon(test_db(&p));
    same.token_file = Some(p.token.clone());
    assert_refused(judge_worker_db_guard(&protected(&p), &same, &inside(&p)));

    // 内容の写し（末尾の改行の違いも同じ token とみなす）。
    let copy = p.test.join("token");
    std::fs::write(&copy, PROD_TOKEN).unwrap();
    let mut copied = daemon(test_db(&p));
    copied.token_file = Some(copy);
    let d = judge_worker_db_guard(&protected(&p), &copied, &inside(&p));
    match &d {
        GuardDecision::RefuseProduction { reason, .. } => {
            assert!(!reason.contains(PROD_TOKEN), "token leaked: {reason}")
        }
        other => panic!("expected RefuseProduction, got {other:?}"),
    }
    assert_refused(d);
    // P の Debug にも token の値は出ない。
    assert!(!format!("{:?}", protected(&p)).contains(PROD_TOKEN));

    // 本番 token が読めないなら、token_file を持つ試験 daemon は拒否する。
    let mut unreadable = protected(&p);
    unreadable.tokens.clear();
    unreadable.token_unreadable = true;
    let other = p.test.join("other-token");
    std::fs::write(&other, "test-token").unwrap();
    let mut with_token = daemon(test_db(&p));
    with_token.token_file = Some(other);
    assert_refused(judge_worker_db_guard(&unreadable, &with_token, &inside(&p)));
}

#[test]
fn worker_db_guard_refuses_when_production_config_is_unreadable() {
    let p = prod();
    // 本番 config が存在するのに読めない → P を決められない。試験 DB でも免除しない（fail closed）。
    let mut set = ProtectedSet::new();
    set.mark_unknown("production config: Permission denied");
    let d = judge_worker_db_guard(&set, &daemon(test_db(&p)), &inside(&p));
    assert!(
        matches!(&d, GuardDecision::RequireUserns { reason } if reason.contains("unknown")),
        "{d:?}"
    );
    // 印の path が解けないときも同じ。
    let gone = WorkerRunMarker::ReadOnly(p.tmp.path().join("missing"));
    let d = judge_worker_db_guard(&protected(&p), &daemon(test_db(&p)), &gone);
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
    // 試験 daemon の DB が解けないなら拒否。
    let d = judge_worker_db_guard(
        &protected(&p),
        &daemon(p.test.join("no-such.sqlite3")),
        &inside(&p),
    );
    assert_refused(d);
}

#[test]
fn worker_db_guard_refuses_test_db_when_marker_is_not_read_only() {
    let p = prod();
    // 書ける dir を指す偽の印は裏付けられない。
    let marker = verify_worker_run_marker(Some(p.lib.clone().into_os_string()));
    assert!(
        matches!(&marker, WorkerRunMarker::Unverified { .. }),
        "{marker:?}"
    );
    // 書き込み試行の痕跡を残さない。
    assert_eq!(std::fs::read_dir(&p.lib).unwrap().count(), 1);
    let d = judge_worker_db_guard(&protected(&p), &daemon(test_db(&p)), &marker);
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
    // 相対 path の印も裏付けない。
    assert!(matches!(
        verify_worker_run_marker(Some("relative/dir".into())),
        WorkerRunMarker::Unverified { .. }
    ));
}

#[test]
fn worker_db_guard_allows_test_db_inside_worker_run() {
    let p = prod();
    let mut paths = daemon(test_db(&p));
    paths.state_dir = Some(p.test.join("state"));
    assert_eq!(
        judge_worker_db_guard(&protected(&p), &paths, &inside(&p)),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_db_guard_allows_test_db_with_test_token() {
    let p = prod();
    let token = p.test.join("token");
    std::fs::write(&token, "test-only-token\n").unwrap();
    let mut paths = daemon(test_db(&p));
    paths.token_file = Some(token);
    assert_eq!(
        judge_worker_db_guard(&protected(&p), &paths, &inside(&p)),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_db_guard_requires_userns_for_test_db_outside_worker_run() {
    let p = prod();
    assert_eq!(verify_worker_run_marker(None), WorkerRunMarker::Absent);
    assert_eq!(
        verify_worker_run_marker(Some(OsString::new())),
        WorkerRunMarker::Absent
    );
    let d = judge_worker_db_guard(
        &protected(&p),
        &daemon(test_db(&p)),
        &WorkerRunMarker::Absent,
    );
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
    // worker run の外で本番 DB を使うなら拒否側だが、worker run の文言は足さない。
    let d = judge_worker_db_guard(
        &protected(&p),
        &daemon(p.db.clone()),
        &WorkerRunMarker::Absent,
    );
    assert!(
        matches!(
            d,
            GuardDecision::RefuseProduction {
                inside_worker_run: false,
                ..
            }
        ),
        "{d:?}"
    );
    assert_eq!(d.refusal_message(), None);
}

#[test]
fn worker_db_guard_sibling_dir_is_not_production() {
    // `/var/lib/celeris` と `/var/lib/celeris2` の型: 文字列の前方一致ではなく component で比べる。
    let tmp = tempfile::tempdir().unwrap();
    let prod_dir = tmp.path().join("celeris");
    let sibling = tmp.path().join("celeris2");
    std::fs::create_dir_all(&prod_dir).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let prod_db = prod_dir.join("celeris.sqlite3");
    std::fs::write(&prod_db, b"db").unwrap();
    let sib_db = sibling.join("celeris.sqlite3");
    std::fs::write(&sib_db, b"test").unwrap();
    let mut set = ProtectedSet::new();
    set.add_db_file(&prod_db);
    let marker = WorkerRunMarker::ReadOnly(std::fs::canonicalize(&prod_dir).unwrap());
    let mut paths = daemon(sib_db);
    paths.state_dir = Some(tmp.path().join("celeris2-state"));
    assert_eq!(
        judge_worker_db_guard(&set, &paths, &marker),
        GuardDecision::Exempt
    );
}

#[test]
fn apply_std_sets_the_worker_run_marker_to_the_guarded_dir() {
    let f = fixture();
    let g = guard(&f);
    let mut cmd = Command::new("true");
    apply_std(&mut cmd, &g).unwrap();
    let marker = cmd
        .get_envs()
        .find(|(k, _)| *k == WORKER_DB_GUARD_ENV)
        .and_then(|(_, v)| v.map(PathBuf::from));
    assert_eq!(marker.as_deref(), Some(g.dir()));
    drop(f.store);
}
