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

/// symlink を展開する回数の上限（連鎖・入れ子の合計）。超えたら循環とみなして拒否する。
const MAX_SYMLINK_HOPS: usize = 40;

/// tree 内に閉じた相対 symlink だけを許す（ADR 2026-10-19-campaign-inner-symlink）。link 先の文字列を返す。
/// link 先を component ごとに辿り、出会った symlink はその場で（連鎖・入れ子を含め）展開する。`..` は展開後の実体に
/// 適用し、全ての中間段階で root の内側に留まることを確かめる。絶対 path、途中でも root 外に出るもの、
/// dangling、循環（展開回数が `MAX_SYMLINK_HOPS` 超）は拒否する。
pub(crate) fn check_inner_symlink(root: &Path, link: &Path) -> Result<String, String> {
    use std::{collections::VecDeque, ffi::OsString, path::Component};
    let shown = link
        .strip_prefix(root)
        .unwrap_or(link)
        .display()
        .to_string();
    let raw = fs::read_link(link).map_err(|e| format!("{}: {e}", link.display()))?;
    // root からの実体 path（symlink を含まない component の列）。link の親 dir は walk が実 dir だけを辿るので実体。
    let mut stack: Vec<OsString> = link
        .parent()
        .unwrap_or(root)
        .strip_prefix(root)
        .map_err(|e| e.to_string())?
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    let mut queue: VecDeque<OsString> = VecDeque::new();
    let push_target = |queue: &mut VecDeque<OsString>, t: &Path| -> Result<(), String> {
        let mut comps = Vec::new();
        for c in t.components() {
            match c {
                Component::Normal(n) => comps.push(n.to_owned()),
                Component::ParentDir => comps.push(OsString::from("..")),
                Component::CurDir => {}
                _ => return Err(format!("absolute symlink is not allowed: {shown}")),
            }
        }
        for c in comps.into_iter().rev() {
            queue.push_front(c);
        }
        Ok(())
    };
    push_target(&mut queue, &raw)?;
    let mut hops = 0usize;
    while let Some(c) = queue.pop_front() {
        if c == ".." {
            if stack.pop().is_none() {
                return Err(format!("symlink escapes the tree root: {shown}"));
            }
            continue;
        }
        stack.push(c);
        let cur: PathBuf = stack.iter().fold(root.to_path_buf(), |p, n| p.join(n));
        let meta = fs::symlink_metadata(&cur)
            .map_err(|e| format!("symlink is dangling or cyclic: {shown}: {e}"))?;
        if meta.file_type().is_symlink() {
            hops += 1;
            if hops > MAX_SYMLINK_HOPS {
                return Err(format!(
                    "symlink is dangling or cyclic: {shown}: too many links"
                ));
            }
            let t = fs::read_link(&cur).map_err(|e| format!("{}: {e}", cur.display()))?;
            stack.pop();
            push_target(&mut queue, &t)?;
        } else if !queue.is_empty() && !meta.is_dir() {
            return Err(format!(
                "symlink is dangling or cyclic: {shown}: not a directory"
            ));
        }
    }
    Ok(raw.to_string_lossy().into_owned())
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
