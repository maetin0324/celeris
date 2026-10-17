//! K2 の submodule 実体は承認済み資産が無く固定できない。skip 合格にせず、`Unavailable` を実証する。
use std::{fs, path::Path, process::Command};

use celeris_campaign::lock_tree::{TreeProofError, derive_initial};

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
