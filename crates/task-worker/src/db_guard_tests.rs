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
    let available = Command::new("unshare")
        .args(["-Ur", "true"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !available {
        assert_ne!(
            std::env::var("CELERIS_DB_GUARD_TESTS").as_deref(),
            Ok("require"),
            "user namespace is required for db_guard tests"
        );
        eprintln!("skip: unprivileged user namespace is unavailable");
    }
    available
}

struct InstalledGuard;

impl Drop for InstalledGuard {
    fn drop(&mut self) {
        install(None);
    }
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
    install(Some(guard(&f)));
    let _reset = InstalledGuard;

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("test -z \"${DBUS_SESSION_BUS_ADDRESS+x}\" && test ! -S \"/run/user/$(id -u)/bus\" && test ! -S \"$XDG_RUNTIME_DIR/bus\" && test -f \"$XDG_RUNTIME_DIR/bus\" && test -d \"$XDG_RUNTIME_DIR/systemd\" && test -z \"$(ls -A \"$XDG_RUNTIME_DIR/systemd\")\" && test ! -e \"$XDG_RUNTIME_DIR/systemd/private\"")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/definitely-not-a-bus");
    let mut cmd = launch(cmd, None);
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
        let mut cmd = launch(cmd, None);
        let out = rt.block_on(async { cmd.output().await }).unwrap();
        assert!(
            !out.status.success(),
            "{program} --user unexpectedly succeeded in the masked namespace"
        );
    }
}

#[test]
fn the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable() {
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
    let f = fixture();
    let g = guard(&f);
    if Command::new("unshare").arg("--help").output().is_err() {
        eprintln!("skip: unshare(1) is not installed");
        return;
    }
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
