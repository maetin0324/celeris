//! 入れ子 submodule（pluvio → async-ucx 型）の snapshot を asset store から読み、lock に固定して組み立てる試験。
//! fixture は合成の 3 段 repo（main → mid → leaf）で、実 asset store・ネットワークは使わない。
//! 欠落・SHA256SUMS 未登録・改変は build の前に env_failure で止まる。
use std::{fs, path::Path, process::Command};

use celeris_campaign::{
    asset_assembly::{EnvFailure, assemble_initial, check_snapshots, snapshot_name},
    lock::LockedInitial,
    lock_tree::{derive_initial, top_level_submodules, verify_archive, verify_repo},
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const MID: &str = "lib/mid";
const LEAF: &str = "lib/mid/inner/lib/leaf";

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn repo(dir: &Path, files: &[(&str, &str)], link: Option<(&str, &str)>) -> String {
    fs::create_dir_all(dir).expect("mkdir");
    git(dir, &["init", "-q"]);
    for (p, c) in files {
        let f = dir.join(p);
        fs::create_dir_all(f.parent().expect("parent")).expect("mkdir");
        fs::write(f, c).expect("write");
    }
    git(dir, &["add", "-A"]);
    if let Some((path, commit)) = link {
        git(
            dir,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{commit},{path}"),
            ],
        );
    }
    git(dir, &["commit", "-q", "-m", "c"]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Fx {
    _t: TempDir,
    assets: std::path::PathBuf,
    lock: LockedInitial,
    work: std::path::PathBuf,
}

fn sha(p: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(p).expect("read")))
}

fn write_sums(assets: &Path, skip: &[&str]) {
    let mut s = String::new();
    for rel in ["bundle.bundle", "submodules"] {
        let p = assets.join(rel);
        let files: Vec<_> = if p.is_dir() {
            let mut v: Vec<_> = fs::read_dir(&p)
                .expect("rd")
                .map(|e| e.expect("e").path())
                .collect();
            v.sort();
            v
        } else {
            vec![p]
        };
        for f in files {
            let name = f
                .strip_prefix(assets)
                .expect("rel")
                .to_str()
                .expect("u8")
                .to_string();
            if !skip.contains(&name.as_str()) {
                s.push_str(&format!("{}  {name}\n", sha(&f)));
            }
        }
    }
    fs::write(assets.join("SHA256SUMS"), s).expect("sums");
}

fn fixture() -> Fx {
    let t = tempfile::tempdir().expect("tmp");
    let r = t.path();
    let leaf = r.join("leaf");
    let leaf_c = repo(&leaf, &[("lib.rs", "pub fn leaf() {}\n")], None);
    let mid = r.join("mid");
    let mid_c = repo(
        &mid,
        &[
            ("Cargo.toml", "[package]\n"),
            ("inner/src.rs", "fn m() {}\n"),
        ],
        Some(("inner/lib/leaf", &leaf_c)),
    );
    let main = r.join("main");
    repo(
        &main,
        &[("README", "hi\n"), ("src/a.rs", "fn a() {}\n")],
        Some((MID, &mid_c)),
    );
    let (mid2, leaf2) = (mid.clone(), leaf.clone());
    let lock = derive_initial(&main, "HEAD", &move |p| match p {
        MID => Some(mid2.clone()),
        LEAF => Some(leaf2.clone()),
        _ => None,
    })
    .expect("derive");
    let assets = r.join("assets");
    fs::create_dir_all(assets.join("submodules")).expect("mkdir");
    git(
        &main,
        &[
            "bundle",
            "create",
            assets.join("bundle.bundle").to_str().expect("u8"),
            "HEAD",
        ],
    );
    for s in &lock.submodules {
        let src = if s.path == MID { &mid } else { &leaf };
        let tar = assets.join("submodules").join(snapshot_name(s));
        git(src, &["archive", "-o", tar.to_str().expect("u8"), "HEAD"]);
    }
    write_sums(&assets, &[]);
    let work = r.join("work");
    fs::create_dir_all(&work).expect("work");
    Fx {
        _t: t,
        assets,
        lock,
        work,
    }
}

fn assemble(f: &Fx) -> Result<(), EnvFailure> {
    assemble_initial(
        &f.lock,
        &f.assets,
        &f.assets.join("bundle.bundle"),
        &f.work.join("tree"),
    )
}

#[test]
fn nested_submodule_is_in_the_lock_and_assembles_to_it() {
    let f = fixture();
    let paths: Vec<_> = f.lock.submodules.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, vec![MID, LEAF]);
    assert_eq!(top_level_submodules(&f.lock.submodules).len(), 1);
    assemble(&f).expect("assemble");
    let tree = f.work.join("tree");
    assert!(tree.join(LEAF).join("lib.rs").is_file());
    verify_repo(&f.lock, &tree).expect("repo");
    verify_archive(&f.lock, &tree).expect("archive");
    // 入れ子の内容を 1 byte 変えれば lock と合わない。
    fs::write(tree.join(LEAF).join("lib.rs"), "pub fn leaf() {} \n").expect("w");
    assert!(verify_archive(&f.lock, &tree).is_err());
}

#[test]
fn missing_nested_snapshot_stops_as_env_failure_before_any_build_or_clone() {
    let f = fixture();
    let leaf = f
        .lock
        .submodules
        .iter()
        .find(|s| s.path == LEAF)
        .expect("leaf");
    fs::remove_file(f.assets.join("submodules").join(snapshot_name(leaf))).expect("rm");
    let e = assemble(&f).expect_err("missing");
    assert!(e.to_string().starts_with("env_failure:"), "{e}");
    assert!(e.0.contains("missing") && e.0.contains(LEAF), "{e}");
    assert!(!f.work.join("tree").exists(), "clone must not start");
}

#[test]
fn snapshot_absent_from_sha256sums_or_altered_stops_as_env_failure() {
    let f = fixture();
    let leaf = f
        .lock
        .submodules
        .iter()
        .find(|s| s.path == LEAF)
        .expect("leaf");
    let name = snapshot_name(leaf);
    // SHA256SUMS に載っていない snapshot は固定されていない。
    write_sums(&f.assets, &[&format!("submodules/{name}")]);
    let e = check_snapshots(&f.lock, &f.assets).expect_err("unlisted");
    assert!(e.0.contains("not listed in SHA256SUMS"), "{e}");
    // 載っていても tar が改変されていれば hash が合わない。
    write_sums(&f.assets, &[]);
    check_snapshots(&f.lock, &f.assets).expect("intact");
    let tar = f.assets.join("submodules").join(&name);
    let mut b = fs::read(&tar).expect("r");
    b.extend_from_slice(&[0; 512]);
    fs::write(&tar, b).expect("w");
    let e = assemble(&f).expect_err("altered");
    assert!(e.0.contains("differs from SHA256SUMS"), "{e}");
    assert!(!f.work.join("tree").exists());
}

#[test]
fn altered_content_with_a_rewritten_sha256sums_is_still_caught_by_the_lock() {
    // SHA256SUMS まで書き換えても、lock の内容 hash（初期 tree hash）と合わなければ拒否される。
    let f = fixture();
    let leaf = f
        .lock
        .submodules
        .iter()
        .find(|s| s.path == LEAF)
        .expect("leaf");
    let tar = f.assets.join("submodules").join(snapshot_name(leaf));
    let other = f._t.path().join("other");
    repo(&other, &[("lib.rs", "pub fn evil() {}\n")], None);
    git(
        &other,
        &["archive", "-o", tar.to_str().expect("u8"), "HEAD"],
    );
    write_sums(&f.assets, &[]);
    check_snapshots(&f.lock, &f.assets).expect("sums agree");
    let e = assemble(&f).expect_err("content differs from the lock");
    assert!(e.to_string().starts_with("env_failure:"), "{e}");
    assert!(e.0.contains("does not match the lock"), "{e}");
}

#[test]
fn two_snapshots_with_the_same_name_are_ambiguous() {
    let mut f = fixture();
    let mut dup = f.lock.submodules[1].clone();
    dup.path = "other/leaf".into();
    dup.commit = f.lock.submodules[1].commit.clone();
    f.lock.submodules.push(dup);
    let e = check_snapshots(&f.lock, &f.assets).expect_err("ambiguous");
    assert!(e.0.contains("ambiguous"), "{e}");
}
