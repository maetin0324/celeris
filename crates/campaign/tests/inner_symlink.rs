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

#[test]
fn chain_through_dot_alias_and_parent_escapes_mid_path_is_rejected() {
    // reviewer の例: alias -> . のあとの `..` は展開後の実体（tree）に効き、root の外へ出る。
    rejected(|t| {
        symlink(".", t.join("alias")).unwrap();
        symlink("alias/../tree/a/real.txt", t.join("link")).unwrap();
    });
}

#[test]
fn mid_path_escape_via_nested_alias_is_rejected() {
    rejected(|t| {
        symlink("..", t.join("a/up")).unwrap();
        symlink("up/up/tree/a/real.txt", t.join("a/b/esc")).unwrap();
    });
}

#[test]
fn symlink_through_a_file_component_is_rejected() {
    rejected(|t| symlink("a/real.txt/x", t.join("bad")).unwrap());
}

/// 全 link を辿った先が root 内にある（canonicalize して starts_with）。
fn assert_all_links_inside(root: &Path) {
    let real_root = fs::canonicalize(root).unwrap();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.file_type().is_symlink() {
                let r = fs::canonicalize(&p).unwrap();
                assert!(r.starts_with(&real_root), "{p:?} -> {r:?} escapes");
            } else if m.is_dir() {
                stack.push(p);
            }
        }
    }
}

#[test]
fn nested_and_chained_safe_links_succeed_and_stay_inside_after_copy() {
    let d = tempfile::tempdir().unwrap();
    let t = base(d.path());
    symlink("a", t.join("alias")).unwrap(); // dir link
    symlink("alias/b/../real.txt", t.join("viadir")).unwrap(); // dir link を経た `..`
    symlink("../../viadir", t.join("a/b/chain1")).unwrap();
    symlink("b/chain1", t.join("a/chain2")).unwrap(); // 連鎖
    symlink("../../alias/./real.txt", t.join("a/b/up")).unwrap();
    let h = hash_tree(&t).unwrap();
    let c = d.path().join("copy");
    copy_tree(&t, &c).unwrap();
    assert_eq!(h, hash_tree(&c).unwrap());
    assert_eq!(fs::read_to_string(c.join("a/chain2")).unwrap(), "real");
    assert_all_links_inside(&c);
}
