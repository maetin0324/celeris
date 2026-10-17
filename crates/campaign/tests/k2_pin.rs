//! K2 の submodule 実体は承認済み資産が無く固定できない。skip 合格にせず、`Unavailable` を実証する。
use std::{fs, path::Path, process::Command};

use celeris_campaign::{
    lock::LockedInitial,
    lock_tree::{TreeProofError, derive_initial, verify_archive, verify_repo},
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn k2_submodules() -> Vec<(String, String)> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("cases/final/k2-bf-xattr-restart-campaign/case.json");
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p).expect("read")).expect("json");
    v["initial"]["submodules"]
        .as_array()
        .expect("array")
        .iter()
        .map(|s| {
            let (p, c) = s.as_str().expect("str").split_once('@').expect("@");
            (p.to_string(), c.to_string())
        })
        .collect()
}

#[test]
fn k2_pinned_gitlinks_without_snapshot_are_unavailable_not_pinned() {
    let subs = k2_submodules();
    assert_eq!(subs.len(), 3);
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path();
    git(repo, &["init", "-q"]);
    fs::write(repo.join("a.txt"), "x").expect("write");
    git(repo, &["add", "a.txt"]);
    for (p, c) in &subs {
        git(
            repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{c},{p}"),
            ],
        );
    }
    git(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "k2 gitlinks",
        ],
    );
    let r = derive_initial(repo, "HEAD", &|_| None);
    assert!(matches!(r, Err(TreeProofError::Unavailable(_))), "{r:?}");
}

const ASSETS: &str = "/local/celeris/data/campaign-assets/k2";

fn locked() -> LockedInitial {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("cases/final/k2-bf-xattr-restart-campaign/initial.lock.json");
    serde_json::from_str(&fs::read_to_string(p).expect("read")).expect("lock json")
}

#[test]
fn k2_lock_matches_case_json_gitlinks() {
    let l = locked();
    let mut want = k2_submodules();
    want.sort();
    let got: Vec<(String, String)> = l
        .submodules
        .iter()
        .map(|s| (s.path.clone(), s.commit.clone()))
        .collect();
    assert_eq!(got, want);
}

/// 運用者が配置した承認済み資産（読み取り専用）から初期 tree を組み、固定値と照合する。
/// 資産が無ければ失敗する（skip 合格にしない）。
#[test]
fn k2_pinned_content_matches_approved_assets() {
    let a = Path::new(ASSETS);
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    let bundle = a.join("benchfs-086c4618.bundle");
    assert!(bundle.is_file(), "approved asset missing: {bundle:?}");
    let out = Command::new("git")
        .args(["clone", "-q"])
        .arg(&bundle)
        .arg(&repo)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let l = locked();
    git(&repo, &["checkout", "-q", &l.commit]);
    verify_repo(&l, &repo).expect("body matches lock");
    for (path, tar) in [
        ("lib/pluvio", "pluvio-9ecd51ee.tar"),
        ("lib/locusta", "locusta-68f8b210.tar"),
        ("ior_integration/ior", "ior-7054224d.tar"),
    ] {
        let dest = repo.join(path);
        fs::create_dir_all(&dest).expect("mkdir");
        let st = Command::new("tar")
            .arg("-xf")
            .arg(a.join("submodules").join(tar))
            .arg("-C")
            .arg(&dest)
            .status()
            .expect("tar");
        assert!(st.success());
    }
    verify_archive(&l, &repo).expect("assembled tree matches lock");
    // 1 byte 修正した submodule は拒否される。
    let f = repo.join("lib/pluvio/Cargo.toml");
    let mut b = fs::read(&f).expect("read");
    b.push(b'\n');
    fs::write(&f, b).expect("write");
    assert!(verify_archive(&l, &repo).is_err());
}
