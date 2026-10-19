//! tree 内に閉じた相対 symlink だけを snapshot / isolation が許す（ADR 2026-10-19-campaign-inner-symlink）。
use std::{fs, os::unix::fs::symlink, path::Path};

use celeris_campaign::snapshot::{copy_tree, hash_tree};

fn base(root: &Path) -> std::path::PathBuf {
    let t = root.join("tree");
    fs::create_dir_all(t.join("a/b")).unwrap();
    fs::write(t.join("a/real.txt"), "real").unwrap();
    t
}

#[test]
fn inner_relative_symlinks_are_hashed_by_target_and_copied_as_symlinks() {
    let d = tempfile::tempdir().unwrap();
    let t = base(d.path());
    symlink("real.txt", t.join("a/ln")).unwrap();
    symlink("../real.txt", t.join("a/b/up")).unwrap();
    symlink("a/b", t.join("dirln")).unwrap();
    symlink("ln", t.join("a/chain")).unwrap(); // 連鎖（root 内に留まる）
    let h1 = hash_tree(&t).unwrap();
    // link 先の文字列が hash に入る。同じ中身の別 target に付け替えると hash が変わる。
    fs::write(t.join("a/other.txt"), "real").unwrap();
    fs::remove_file(t.join("a/ln")).unwrap();
    symlink("other.txt", t.join("a/ln")).unwrap();
    assert_ne!(h1, hash_tree(&t).unwrap());
    fs::remove_file(t.join("a/ln")).unwrap();
    symlink("real.txt", t.join("a/ln")).unwrap();
    fs::remove_file(t.join("a/other.txt")).unwrap();
    assert_eq!(h1, hash_tree(&t).unwrap());

    let c = d.path().join("copy");
    copy_tree(&t, &c).unwrap();
    let m = fs::symlink_metadata(c.join("a/b/up")).unwrap();
    assert!(m.file_type().is_symlink());
    assert_eq!(
        fs::read_link(c.join("a/b/up")).unwrap(),
        Path::new("../real.txt")
    );
    assert_eq!(fs::read_link(c.join("dirln")).unwrap(), Path::new("a/b"));
    assert_eq!(fs::read_to_string(c.join("a/chain")).unwrap(), "real");
}

fn rejected(setup: impl FnOnce(&Path)) {
    let d = tempfile::tempdir().unwrap();
    let t = base(d.path());
    fs::write(d.path().join("outside.txt"), "o").unwrap();
    setup(&t);
    assert!(hash_tree(&t).is_err(), "hash_tree must reject");
    assert!(
        copy_tree(&t, &d.path().join("copy")).is_err(),
        "copy_tree must reject"
    );
}

#[test]
fn absolute_symlink_is_rejected() {
    rejected(|t| symlink(t.join("a/real.txt"), t.join("abs")).unwrap());
}

#[test]
fn symlink_escaping_root_is_rejected() {
    rejected(|t| symlink("../outside.txt", t.join("esc")).unwrap());
    rejected(|t| symlink("a/../../outside.txt", t.join("esc2")).unwrap());
}

#[test]
fn chain_through_outside_is_rejected() {
    rejected(|t| {
        symlink("../outside.txt", t.join("a/hop")).unwrap();
        symlink("hop", t.join("a/b/chain")).unwrap();
    });
}

#[test]
fn dangling_symlink_is_rejected() {
    rejected(|t| symlink("nope", t.join("dang")).unwrap());
}

#[test]
fn cyclic_symlinks_are_rejected() {
    rejected(|t| {
        symlink("y", t.join("x")).unwrap();
        symlink("x", t.join("y")).unwrap();
    });
    rejected(|t| symlink("self", t.join("self")).unwrap());
}
