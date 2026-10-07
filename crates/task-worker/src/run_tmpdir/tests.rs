use super::*;

#[test]
fn run_tmpdir_env_points_to_run_dir() {
    let ws = tempfile::tempdir().expect("tempdir");
    let tmp = RunTmpDir::create(ws.path(), "run-1").expect("create");
    let expected = ws.path().join("runs").join("run-1").join("tmp");
    assert_eq!(tmp.path(), expected);
    assert!(expected.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&expected)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    let env = tmp.env();
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["TMPDIR", "TMP", "TEMP"]);
    assert!(env.iter().all(|(_, v)| Path::new(v) == expected.as_path()));
}

#[test]
fn run_tmpdir_remove_keeps_run_records() {
    let ws = tempfile::tempdir().expect("tempdir");
    let tmp = RunTmpDir::create(ws.path(), "run-2").expect("create");
    std::fs::create_dir_all(tmp.path().join("rw/.git")).expect("copy");
    std::fs::write(tmp.path().join("rw/big.bin"), b"x").expect("write");
    let run_dir = ws.path().join("runs").join("run-2");
    std::fs::write(run_dir.join("result.json"), b"{}").expect("result");
    tmp.remove().expect("remove");
    assert!(!run_dir.join("tmp").exists());
    assert!(run_dir.join("result.json").is_file());
}

#[test]
fn run_tmpdir_removed_on_drop_when_the_run_is_abandoned() {
    let ws = tempfile::tempdir().expect("tempdir");
    let path = {
        let tmp = RunTmpDir::create(ws.path(), "run-3").expect("create");
        std::fs::write(tmp.path().join("leftover"), b"x").expect("write");
        tmp.path().to_path_buf()
    };
    assert!(!path.exists());
}

#[test]
fn run_tmpdir_create_clears_a_previous_attempt() {
    let ws = tempfile::tempdir().expect("tempdir");
    let stale = run_tmp_path(ws.path(), "run-4");
    std::fs::create_dir_all(&stale).expect("stale");
    std::fs::write(stale.join("old"), b"x").expect("write");
    let tmp = RunTmpDir::create(ws.path(), "run-4").expect("create");
    assert_eq!(std::fs::read_dir(tmp.path()).expect("read").count(), 0);
}

#[test]
fn run_tmpdir_sweep_stale_removes_only_terminal_runs() {
    let ws = tempfile::tempdir().expect("tempdir");
    for id in ["live", "dead"] {
        let p = run_tmp_path(ws.path(), id);
        std::fs::create_dir_all(&p).expect("mk");
        std::fs::write(p.join("f"), b"x").expect("write");
        std::fs::write(p.parent().expect("parent").join("result.json"), b"{}").expect("result");
    }
    let removed = sweep_stale(ws.path(), |id| id == "live");
    assert_eq!(removed, vec![run_tmp_path(ws.path(), "dead")]);
    assert!(run_tmp_path(ws.path(), "live").is_dir());
    assert!(ws.path().join("runs/dead/result.json").is_file());
    assert!(sweep_stale(&ws.path().join("missing"), |_| false).is_empty());
}

#[test]
fn run_tmpdir_preamble_forbids_tmp_copies() {
    let out = crate::preamble::render(&crate::protocol::RunContext::default(), "artifacts");
    assert!(out.contains("## 一時 file の置き場所"), "{out}");
    for needle in [
        "リポジトリの写し",
        "pnpm store の写し",
        "ビルド出力",
        "`/tmp` に置かず",
        "`$TMPDIR`",
        "`artifacts/`",
    ] {
        assert!(out.contains(needle), "missing {needle}: {out}");
    }
    assert_eq!(out.matches("## 一時 file の置き場所").count(), 1);
}
