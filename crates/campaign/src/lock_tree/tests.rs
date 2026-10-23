use super::*;
use tempfile::TempDir;

fn run(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init(dir: &Path, files: &[(&str, &str)]) -> String {
    fs::create_dir_all(dir).expect("mkdir");
    run(dir, &["init", "-q"]);
    for (p, c) in files {
        let f = dir.join(p);
        fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
        fs::write(f, c).expect("write");
    }
    run(dir, &["add", "-A"]);
    run(dir, &["commit", "-q", "-m", "init"]);
    run(dir, &["rev-parse", "HEAD"])
}

struct Fx {
    _t: TempDir,
    main: PathBuf,
    sub: PathBuf,
    other: PathBuf,
    exp: LockedInitial,
}

fn fixture() -> Fx {
    let t = TempDir::new().expect("tmp");
    let sub = t.path().join("sub-src");
    let sub_commit = init(&sub, &[("lib.rs", "pub fn a() {}\n"), ("d/x.txt", "x")]);
    let other = t.path().join("other-src");
    init(&other, &[("lib.rs", "pub fn b() {}\n")]);
    let main = t.path().join("main-src");
    init(&main, &[("README.md", "hi\n"), ("src/a.rs", "fn a() {}\n")]);
    fs::write(main.join("run.sh"), "#!/bin/sh\n").expect("write");
    fs::set_permissions(main.join("run.sh"), fs::Permissions::from_mode(0o755)).expect("chmod");
    run(&main, &["add", "run.sh"]);
    run(
        &main,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{sub_commit},vendor/sub"),
        ],
    );
    fs::write(
        main.join(".gitmodules"),
        "[submodule \"vendor/sub\"]\n\tpath = vendor/sub\n\turl = ../sub-src\n",
    )
    .expect("write");
    run(&main, &["add", ".gitmodules"]);
    run(&main, &["commit", "-q", "-m", "with sub"]);
    let sub_c = sub.clone();
    let exp = derive_initial(&main, "HEAD", &|p| {
        (p == "vendor/sub").then(|| sub_c.clone())
    })
    .expect("derive");
    Fx {
        _t: t,
        main,
        sub,
        other,
        exp,
    }
}

/// git metadata の無い展開（`git archive` 相当）を作る。submodule は sub の同 commit の内容。
fn extract(fx: &Fx, dst: &Path) {
    fs::create_dir_all(dst).expect("mkdir");
    for (repo, sub_dir) in [(&fx.main, None), (&fx.sub, Some("vendor/sub"))] {
        let target = match sub_dir {
            Some(s) => dst.join(s),
            None => dst.to_path_buf(),
        };
        fs::create_dir_all(&target).expect("mkdir");
        let tar = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["archive", "HEAD"])
            .output()
            .expect("archive");
        assert!(tar.status.success());
        let mut x = Command::new("tar")
            .args(["-x", "-C"])
            .arg(&target)
            .stdin(Stdio::piped())
            .spawn()
            .expect("tar");
        x.stdin
            .take()
            .expect("stdin")
            .write_all(&tar.stdout)
            .expect("write");
        assert!(x.wait().expect("wait").success());
    }
}

#[test]
fn git_and_metadata_free_archive_give_the_same_hash() {
    let fx = fixture();
    assert_eq!(fx.exp.submodules.len(), 1);
    assert_eq!(fx.exp.submodules[0].path, "vendor/sub");
    let t = TempDir::new().expect("tmp");
    let a = t.path().join("a");
    extract(&fx, &a);
    assert!(!a.join(".git").exists());
    assert_eq!(
        archive_content_sha256(&a, &["vendor/sub".into()]).expect("hash"),
        fx.exp.content_sha256
    );
    assert_eq!(
        archive_content_sha256(&a.join("vendor/sub"), &[]).expect("hash"),
        fx.exp.submodules[0].content_sha256
    );
    verify_archive(&fx.exp, &a).expect("archive matches");
    verify_repo(&fx.exp, &fx.main).expect("repo matches");
    assert_eq!(
        git_content_sha256(&fx.main, "HEAD").expect("hash"),
        fx.exp.content_sha256
    );
}

#[test]
fn exec_bit_is_part_of_the_proof() {
    let fx = fixture();
    let t = TempDir::new().expect("tmp");
    let a = t.path().join("a");
    extract(&fx, &a);
    fs::set_permissions(a.join("run.sh"), fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::ContentMismatch(_))
    ));
}

#[test]
fn missing_swapped_added_and_modified_submodules_are_rejected() {
    let fx = fixture();
    let t = TempDir::new().expect("tmp");
    let reset = |n: &str| {
        let a = t.path().join(n);
        extract(&fx, &a);
        a
    };
    // 欠落
    let a = reset("missing");
    fs::remove_dir_all(a.join("vendor/sub")).expect("rm");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 空の dir（未 init の checkout）
    let a = reset("empty");
    fs::remove_dir_all(a.join("vendor/sub")).expect("rm");
    fs::create_dir_all(a.join("vendor/sub")).expect("mkdir");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 差替え（別 repo の内容）
    let a = reset("swapped");
    fs::write(a.join("vendor/sub/lib.rs"), "pub fn b() {}\n").expect("write");
    fs::remove_dir_all(a.join("vendor/sub/d")).expect("rm");
    assert!(
        fs::read_to_string(fx.other.join("lib.rs"))
            .expect("read")
            .contains("b()")
    );
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 修正版（内容 1 byte 違い）
    let a = reset("modified");
    fs::write(a.join("vendor/sub/d/x.txt"), "y").expect("write");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 本体の 1 byte 違い
    let a = reset("main-modified");
    fs::write(a.join("README.md"), "hj\n").expect("write");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::ContentMismatch(_))
    ));
    // 追加（余分な submodule 風 dir / file）
    let a = reset("added");
    fs::create_dir_all(a.join("vendor/extra")).expect("mkdir");
    fs::write(a.join("vendor/extra/lib.rs"), "pub fn b() {}\n").expect("write");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::ContentMismatch(_))
    ));
    let a = reset("added-in-sub");
    fs::write(a.join("vendor/sub/new.rs"), "").expect("write");
    assert!(matches!(
        verify_archive(&fx.exp, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // 期待側に無い submodule を持つ lock（gitlink の commit 差替え）
    let mut wrong = fx.exp.clone();
    wrong.submodules[0].content_sha256 = derive_initial(&fx.other, "HEAD", &|_| None)
        .expect("derive")
        .content_sha256;
    let a = reset("ok");
    assert!(matches!(
        verify_archive(&wrong, &a),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
}

#[test]
fn repo_with_other_commit_or_gitlink_is_not_adopted_as_expectation() {
    let fx = fixture();
    // 同じ repo に commit を足すと、固定 commit の期待値は変わらないが HEAD を期待にはしない。
    fs::write(fx.main.join("README.md"), "changed\n").expect("write");
    run(&fx.main, &["commit", "-q", "-am", "edit"]);
    verify_repo(&fx.exp, &fx.main).expect("pinned commit still verifies");
    let moved = derive_initial(&fx.main, "HEAD", &|_| Some(fx.sub.clone())).expect("derive");
    assert_ne!(moved.content_sha256, fx.exp.content_sha256);
    // 別 repo は拒否。
    assert!(matches!(
        verify_repo(&fx.exp, &fx.other),
        Err(TreeProofError::CommitMismatch(_))
    ));
    // tree を偽る lock。
    let mut forged = fx.exp.clone();
    forged.tree = "0".repeat(40);
    assert!(matches!(
        verify_repo(&forged, &fx.main),
        Err(TreeProofError::TreeMismatch(_))
    ));
    // gitlink の commit を差し替えた lock。
    let mut forged = fx.exp.clone();
    forged.submodules[0].commit = "1".repeat(40);
    assert!(matches!(
        verify_repo(&forged, &fx.main),
        Err(TreeProofError::SubmoduleMismatch(_))
    ));
    // submodule 実体が無ければ導出は失敗（成功扱いにしない）。
    assert!(matches!(
        derive_initial(&fx.main, &fx.exp.commit, &|_| None),
        Err(TreeProofError::Unavailable(_))
    ));
}

#[test]
fn presence_of_git_metadata_is_not_a_reason_to_reject() {
    let fx = fixture();
    let t = TempDir::new().expect("tmp");
    let a = t.path().join("a");
    extract(&fx, &a);
    verify_archive(&fx.exp, &a).expect("without metadata");
    // root の .git dir と submodule の .git file が付いても同じ内容なら通る。
    fs::create_dir_all(a.join(".git/objects")).expect("mkdir");
    fs::write(a.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write");
    fs::write(
        a.join("vendor/sub/.git"),
        "gitdir: ../../.git/modules/vendor/sub\n",
    )
    .expect("write");
    verify_archive(&fx.exp, &a).expect("with metadata");
    // 実 clone（作業ツリー + .git）も通る。submodule は内容を差し込む。
    let c = t.path().join("clone");
    run(
        t.path(),
        &[
            "clone",
            "-q",
            fx.main.to_str().expect("utf8"),
            c.to_str().expect("utf8"),
        ],
    );
    run(&c, &["checkout", "-q", &fx.exp.commit]);
    assert!(c.join(".git").exists());
    fs::create_dir_all(c.join("vendor/sub")).expect("mkdir");
    copy_into(&fx.sub, &c.join("vendor/sub"));
    verify_archive(&fx.exp, &c).expect("clone checkout");
    // metadata があっても内容が違えば内容で拒否される（理由は metadata ではない）。
    fs::write(c.join("README.md"), "x").expect("write");
    assert!(matches!(
        verify_archive(&fx.exp, &c),
        Err(TreeProofError::ContentMismatch(_))
    ));
}

fn copy_into(src: &Path, dst: &Path) {
    for e in fs::read_dir(src).expect("read") {
        let p = e.expect("entry").path();
        let name = p.file_name().expect("name");
        if name == ".git" {
            continue;
        }
        if p.is_dir() {
            fs::create_dir_all(dst.join(name)).expect("mkdir");
            copy_into(&p, &dst.join(name));
        } else {
            fs::copy(&p, dst.join(name)).expect("copy");
        }
    }
}
