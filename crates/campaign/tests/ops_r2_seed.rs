//! ops-r2-seed.sh（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-seed）。A・B が同じ種（同じ sha256）から始まり、
//! 種は .git なしの非圧縮 tar で、漏洩検査が tar の中身を読めることを確かめる。network には出ない。
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn seed(args: &[&std::ffi::OsStr]) -> Output {
    Command::new("sh")
        .arg(repo_root().join("scripts/campaign/ops-r2-seed.sh"))
        .args(args)
        .output()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(s.success(), "git {args:?}");
}

const CANARY: &str = "OPSR2-SEED-CANARY-7f3a91";

fn make(root: &Path) -> (PathBuf, String) {
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.c"), format!("int x; /* {CANARY} */\n")).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "seed"]);
    let tar = root.join("benchfs.tar");
    let o = seed(&[
        "make".as_ref(),
        "--repo".as_ref(),
        repo.as_os_str(),
        "--rev".as_ref(),
        "HEAD".as_ref(),
        "--out".as_ref(),
        tar.as_os_str(),
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    (tar, String::from_utf8_lossy(&o.stdout).trim().to_string())
}

#[test]
fn a_and_b_get_the_same_seed_without_git_and_unpack_it() {
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (tar, sha) = make(d.path());
    assert_eq!(sha.len(), 64);
    let mut placed = Vec::new();
    for arm in ["a", "b"] {
        let state = d.path().join(arm);
        let o = seed(&[
            "place".as_ref(),
            "--seed".as_ref(),
            tar.as_os_str(),
            "--sha256".as_ref(),
            sha.as_ref(),
            "--state".as_ref(),
            state.as_os_str(),
        ]);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        placed.push(std::fs::read(state.join("seed/benchfs.tar")).unwrap());
    }
    assert_eq!(placed[0], placed[1], "A and B start from the same bytes");
    let into = d.path().join("a/work");
    let seed_a = d.path().join("a/seed/benchfs.tar");
    let o = seed(&[
        "unpack".as_ref(),
        "--seed".as_ref(),
        seed_a.as_os_str(),
        "--sha256".as_ref(),
        sha.as_ref(),
        "--into".as_ref(),
        into.as_os_str(),
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(into.join("src/lib.c").is_file());
    assert!(!into.join(".git").exists());
    // 展開後の state は ops-r2-sandbox.sh の .git 拒否に掛からない（--state に渡せる）。
    std::fs::create_dir_all(d.path().join("rel")).unwrap();
    let o = Command::new("sh")
        .arg(repo_root().join("scripts/campaign/ops-r2-sandbox.sh"))
        .arg("--state")
        .arg(d.path().join("a"))
        .arg("--release")
        .arg(d.path().join("rel"))
        .arg("--account")
        .arg(d.path().join("b"))
        .args(["--print-args", "--", "true"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn wrong_hash_and_git_or_escaping_members_are_refused() {
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (tar, _) = make(d.path());
    let o = seed(&[
        "place".as_ref(),
        "--seed".as_ref(),
        tar.as_os_str(),
        "--sha256".as_ref(),
        "0".repeat(64).as_ref(),
        "--state".as_ref(),
        d.path().join("s").as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    // .git の member と木の外を指す symlink を持つ tar。
    let bad = d.path().join("bad");
    std::fs::create_dir_all(bad.join("t/.git")).unwrap();
    std::fs::write(bad.join("t/.git/HEAD"), "ref").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", bad.join("t/leak")).unwrap();
    for (member, why) in [(".git", ".git member"), ("leak", "link outside")] {
        let t = d.path().join(format!("{member}.tar"));
        let s = Command::new("tar")
            .arg("-cf")
            .arg(&t)
            .arg("-C")
            .arg(bad.join("t"))
            .arg(member)
            .status()
            .unwrap();
        assert!(s.success());
        let sha = String::from_utf8(Command::new("sha256sum").arg(&t).output().unwrap().stdout)
            .unwrap()[..64]
            .to_string();
        let o = seed(&[
            "place".as_ref(),
            "--seed".as_ref(),
            t.as_os_str(),
            "--sha256".as_ref(),
            sha.as_ref(),
            "--state".as_ref(),
            d.path().join("s").as_os_str(),
        ]);
        assert_eq!(o.status.code(), Some(1), "{member}");
        assert!(String::from_utf8_lossy(&o.stderr).contains(why), "{why}");
    }
}

#[test]
fn leak_check_reads_inside_the_placed_seed() {
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (tar, sha) = make(d.path());
    let state = d.path().join("a");
    let o = seed(&[
        "place".as_ref(),
        "--seed".as_ref(),
        tar.as_os_str(),
        "--sha256".as_ref(),
        sha.as_ref(),
        "--state".as_ref(),
        state.as_os_str(),
    ]);
    assert!(o.status.success());
    let private = d.path().join("private.json");
    std::fs::write(&private, r#"{"schema":"x","note":"no-secret-here-0000"}"#).unwrap();
    let out = d.path().join("leak.json");
    let o = Command::new("python3")
        .arg("-I")
        .arg(repo_root().join("scripts/campaign/check-ops-r1-leak.py"))
        .arg("--private")
        .arg(&private)
        .args(["--canary", CANARY, "--scan"])
        .arg(state.join("seed"))
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let report = std::fs::read_to_string(&out).unwrap();
    assert!(report.contains("benchfs.tar"), "{report}");
}
