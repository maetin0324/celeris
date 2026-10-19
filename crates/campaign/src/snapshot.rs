//! ディレクトリ木の内容 hash と複製。試行ごとの workspace/KB を snapshot から作る。
use std::{
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

fn entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    v.sort();
    Ok(v)
}

fn walk(root: &Path, dir: &Path, skip: &[&str], h: &mut Sha256) -> Result<(), String> {
    for p in entries(dir)? {
        if dir == root
            && p.file_name()
                .is_some_and(|n| skip.iter().any(|s| n == std::ffi::OsStr::new(s)))
        {
            continue;
        }
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let rel = p
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        if meta.file_type().is_symlink() {
            let target = check_inner_symlink(root, &p)?;
            h.update(b"L");
            h.update((rel.len() as u64).to_le_bytes());
            h.update(rel.as_bytes());
            h.update((target.len() as u64).to_le_bytes());
            h.update(target.as_bytes());
            continue;
        }
        if meta.is_dir() {
            h.update(b"D");
            h.update((rel.len() as u64).to_le_bytes());
            h.update(rel.as_bytes());
            walk(root, &p, skip, h)?;
        } else {
            let bytes = fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            h.update(b"F");
            h.update((rel.len() as u64).to_le_bytes());
            h.update(rel.as_bytes());
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(&bytes);
        }
    }
    Ok(())
}

/// tree 内に閉じた相対 symlink だけを許す（ADR 2026-10-19-campaign-inner-symlink）。link 先の文字列を返す。
/// 絶対 path、`..` で root の外へ出るもの、解決先（連鎖の最後まで）が root 外のもの、dangling、循環は拒否する。
pub(crate) fn check_inner_symlink(root: &Path, link: &Path) -> Result<String, String> {
    use std::path::Component;
    let shown = link
        .strip_prefix(root)
        .unwrap_or(link)
        .display()
        .to_string();
    let target = fs::read_link(link).map_err(|e| format!("{}: {e}", link.display()))?;
    if target.is_absolute() {
        return Err(format!("absolute symlink is not allowed: {shown}"));
    }
    // 字面上、link の置かれた dir から root の外へ出ないこと。
    let parent = link.parent().unwrap_or(root);
    let mut depth = parent
        .strip_prefix(root)
        .map_err(|e| e.to_string())?
        .components()
        .count() as i64;
    for c in target.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return Err(format!("symlink is not allowed: {shown}")),
        }
        if depth < 0 {
            return Err(format!("symlink escapes the tree root: {shown}"));
        }
    }
    // 連鎖を最後まで辿った実体が root 内にあること（dangling・循環は canonicalize が失敗する）。
    let real = fs::canonicalize(link)
        .map_err(|e| format!("symlink is dangling or cyclic: {shown}: {e}"))?;
    let root_real = fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    if !real.starts_with(&root_real) {
        return Err(format!("symlink resolves outside the tree root: {shown}"));
    }
    Ok(target.to_string_lossy().into_owned())
}

/// 相対 path・内容・空 dir を含む木の sha256。symlink は tree 内に閉じた相対のものだけ許し、link 先の文字列ごと数える。
pub fn hash_tree(root: &Path) -> Result<String, String> {
    hash_tree_excluding(root, &[])
}

/// `hash_tree` から、直下の `skip` の名前の file / dir を除いたもの（配った入力を成果物に数えないため）。
pub fn hash_tree_excluding(root: &Path, skip: &[&str]) -> Result<String, String> {
    let mut h = Sha256::new();
    walk(root, root, skip, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    copy_tree_in(src, src, dst)
}

fn copy_tree_in(root: &Path, src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for p in entries(src)? {
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = p.file_name().ok_or("path without file name")?;
        let to = dst.join(name);
        if meta.file_type().is_symlink() {
            copy_inner_symlink(root, &p, &to)?;
        } else if meta.is_dir() {
            copy_tree_in(root, &p, &to)?;
        } else {
            fs::copy(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// tree 内に閉じた相対 symlink を、symlink のまま（link 先の文字列を変えず）置く。
pub(crate) fn copy_inner_symlink(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    check_inner_symlink(root, from)?;
    let target = fs::read_link(from).map_err(|e| format!("{}: {e}", from.display()))?;
    std::os::unix::fs::symlink(&target, to).map_err(|e| format!("{}: {e}", to.display()))
}
