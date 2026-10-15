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

fn walk(root: &Path, dir: &Path, h: &mut Sha256) -> Result<(), String> {
    for p in entries(dir)? {
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let rel = p
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        if meta.file_type().is_symlink() {
            return Err(format!("symlink is not allowed in a snapshot: {rel}"));
        }
        if meta.is_dir() {
            h.update(b"D");
            h.update((rel.len() as u64).to_le_bytes());
            h.update(rel.as_bytes());
            walk(root, &p, h)?;
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

/// 相対 path・内容・空 dir を含む木の sha256。symlink は拒否する。
pub fn hash_tree(root: &Path) -> Result<String, String> {
    let mut h = Sha256::new();
    walk(root, root, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for p in entries(src)? {
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let name = p.file_name().ok_or("path without file name")?;
        let to = dst.join(name);
        if meta.file_type().is_symlink() {
            return Err(format!("symlink is not allowed: {}", p.display()));
        }
        if meta.is_dir() {
            copy_tree(&p, &to)?;
        } else {
            fs::copy(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    Ok(())
}
