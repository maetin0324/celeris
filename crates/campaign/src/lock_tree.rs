//! commit / tree / gitlink から信頼できる期待内容を導き、展開済み archive（git metadata の有無を問わない）の
//! 内容を照合する（ADR 2026-10-17-campaign-lock）。渡された repo / ディレクトリを期待値として採用しない:
//! 期待値は lock（`LockedInitial`）か、固定した commit から `derive_initial` で導いたものだけ。
//!
//! 内容 hash は「file 一覧（path 昇順）× mode（100644 / 100755 / 120000）× blob 内容の sha256」の正規化列の sha256。
//! git が持たない空 dir・owner/mtime・root の `.git`（metadata）は数えない。submodule は gitlink の path と commit を
//! 期待値とし、本体の hash からは path ごと除いて別に hash する。
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use sha2::{Digest, Sha256};

use crate::lock::{LockedInitial, LockedSubmodule};

const GITLINK: &str = "160000";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeProofError {
    Git(String),
    Io(String),
    /// submodule の実体 repo が渡されていない（内容を導けない）。
    Unavailable(String),
    Unsupported(String),
    CommitMismatch(String),
    TreeMismatch(String),
    ContentMismatch(String),
    SubmoduleMismatch(String),
}

impl fmt::Display for TreeProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (k, m) = match self {
            Self::Git(m) => ("git", m),
            Self::Io(m) => ("io", m),
            Self::Unavailable(m) => ("unavailable", m),
            Self::Unsupported(m) => ("unsupported", m),
            Self::CommitMismatch(m) => ("commit mismatch", m),
            Self::TreeMismatch(m) => ("tree mismatch", m),
            Self::ContentMismatch(m) => ("content mismatch", m),
            Self::SubmoduleMismatch(m) => ("submodule mismatch", m),
        };
        write!(f, "{k}: {m}")
    }
}

impl std::error::Error for TreeProofError {}

/// path → (mode, blob sha256)。
type Files = BTreeMap<String, (String, String)>;

fn hex(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, TreeProofError> {
    let mut c = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            c.env_remove(k);
        }
    }
    let out = c
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| TreeProofError::Git(format!("git {}: {e}", args.join(" "))))?;
    if !out.status.success() {
        return Err(TreeProofError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

fn git_line(repo: &Path, args: &[&str]) -> Result<String, TreeProofError> {
    Ok(String::from_utf8_lossy(&git(repo, args)?)
        .trim()
        .to_string())
}

fn resolve(repo: &Path, rev: &str) -> Result<(String, String), TreeProofError> {
    let commit = git_line(
        repo,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?;
    let tree = git_line(
        repo,
        &["rev-parse", "--verify", &format!("{commit}^{{tree}}")],
    )?;
    Ok((commit, tree))
}

/// 内容の正規化列から hash を出す。
fn digest(files: &Files) -> String {
    let mut h = Sha256::new();
    for (p, (mode, sha)) in files {
        h.update(format!("{p}\0{mode}\0{sha}\n").as_bytes());
    }
    format!("{:x}", h.finalize())
}

/// `commit` の木の file（blob / symlink）と gitlink を分けて返す。
fn read_git_tree(
    repo: &Path,
    commit: &str,
) -> Result<(Files, Vec<(String, String)>), TreeProofError> {
    let raw = git(repo, &["ls-tree", "-r", "-z", "--full-tree", commit])?;
    let mut blobs: Vec<(String, String, String)> = Vec::new();
    let mut links = Vec::new();
    for rec in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let s = std::str::from_utf8(rec)
            .map_err(|_| TreeProofError::Unsupported("non-utf8 path in tree".into()))?;
        let (meta, path) = s
            .split_once('\t')
            .ok_or_else(|| TreeProofError::Git(format!("bad ls-tree record: {s}")))?;
        let mut it = meta.split(' ');
        let (mode, _ty, oid) = (it.next(), it.next(), it.next());
        let (Some(mode), Some(oid)) = (mode, oid) else {
            return Err(TreeProofError::Git(format!("bad ls-tree record: {s}")));
        };
        if mode == GITLINK {
            links.push((path.to_string(), oid.to_string()));
        } else {
            blobs.push((path.to_string(), mode.to_string(), oid.to_string()));
        }
    }
    let shas = blob_hashes(repo, &blobs.iter().map(|b| b.2.clone()).collect::<Vec<_>>())?;
    let mut files = Files::new();
    for ((path, mode, _), sha) in blobs.into_iter().zip(shas) {
        files.insert(path, (mode, sha));
    }
    links.sort();
    Ok((files, links))
}

/// `git cat-file --batch` で blob の内容 sha256 をまとめて得る。
fn blob_hashes(repo: &Path, oids: &[String]) -> Result<Vec<String>, TreeProofError> {
    if oids.is_empty() {
        return Ok(Vec::new());
    }
    let mut c = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            c.env_remove(k);
        }
    }
    let mut child = c
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| TreeProofError::Git(format!("git cat-file: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| TreeProofError::Git("no stdin".into()))?;
    let input: String = oids.iter().map(|o| format!("{o}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let mut out = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| TreeProofError::Git("no stdout".into()))?,
    );
    let mut res = Vec::with_capacity(oids.len());
    for oid in oids {
        let mut head = String::new();
        out.read_line(&mut head)
            .map_err(|e| TreeProofError::Git(e.to_string()))?;
        let parts: Vec<&str> = head.split_whitespace().collect();
        let size: usize = match parts.as_slice() {
            [_, "blob", n] => n.parse().map_err(|_| TreeProofError::Git(head.clone()))?,
            _ => return Err(TreeProofError::Git(format!("blob {oid}: {}", head.trim()))),
        };
        let mut buf = vec![0u8; size + 1];
        out.read_exact(&mut buf)
            .map_err(|e| TreeProofError::Git(e.to_string()))?;
        res.push(hex(&buf[..size]));
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(res)
}

/// 展開済みディレクトリの file 一覧。root の `.git`（metadata）と `exclude`（root 相対 path）以下は数えない。
/// symlink は mode 120000・内容は link 先として数える（git の blob と同じ）。
fn read_dir_files(root: &Path, exclude: &[String]) -> Result<Files, TreeProofError> {
    fn walk(
        root: &Path,
        dir: &Path,
        exclude: &[String],
        out: &mut Files,
    ) -> Result<(), TreeProofError> {
        let io = |p: &Path, e: std::io::Error| TreeProofError::Io(format!("{}: {e}", p.display()));
        let mut ents: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| io(dir, e))?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()
            .map_err(|e| io(dir, e))?;
        ents.sort();
        for p in ents {
            let rel = p
                .strip_prefix(root)
                .map_err(|e| TreeProofError::Io(e.to_string()))?
                .to_str()
                .ok_or_else(|| TreeProofError::Unsupported("non-utf8 path in archive".into()))?
                .to_string();
            if dir == root && rel == ".git" {
                continue;
            }
            if exclude.contains(&rel) {
                continue;
            }
            let meta = fs::symlink_metadata(&p).map_err(|e| io(&p, e))?;
            if meta.is_dir() {
                walk(root, &p, exclude, out)?;
            } else if meta.file_type().is_symlink() {
                let t = fs::read_link(&p).map_err(|e| io(&p, e))?;
                out.insert(rel, ("120000".into(), hex(t.to_string_lossy().as_bytes())));
            } else if meta.is_file() {
                let mode = if meta.permissions().mode() & 0o100 != 0 {
                    "100755"
                } else {
                    "100644"
                };
                let bytes = fs::read(&p).map_err(|e| io(&p, e))?;
                out.insert(rel, (mode.into(), hex(&bytes)));
            } else {
                return Err(TreeProofError::Unsupported(format!(
                    "special file in archive: {rel}"
                )));
            }
        }
        Ok(())
    }
    let mut out = Files::new();
    walk(root, root, exclude, &mut out)?;
    Ok(out)
}

/// 展開済みディレクトリ（git metadata の有無を問わない）の内容 hash。`exclude` は root 相対の submodule path。
pub fn archive_content_sha256(root: &Path, exclude: &[String]) -> Result<String, TreeProofError> {
    Ok(digest(&read_dir_files(root, exclude)?))
}

/// repo の commit が指す木（gitlink を除く）の内容 hash。
pub fn git_content_sha256(repo: &Path, commit: &str) -> Result<String, TreeProofError> {
    Ok(digest(&read_git_tree(repo, commit)?.0))
}

/// 固定した `rev`（commit）から期待値を導く。submodule は gitlink の path・commit を採り、内容は
/// `submodule_repo(path)` が返す repo の同 commit から導く（返さなければ `Unavailable`）。
pub fn derive_initial(
    repo: &Path,
    rev: &str,
    submodule_repo: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<LockedInitial, TreeProofError> {
    let (commit, tree) = resolve(repo, rev)?;
    let (files, links) = read_git_tree(repo, &commit)?;
    let mut submodules = Vec::new();
    for (path, sub_commit) in links {
        derive_submodule(&path, &sub_commit, submodule_repo, &mut submodules)?;
    }
    Ok(LockedInitial {
        commit,
        tree,
        content_sha256: digest(&files),
        submodules,
    })
}

/// submodule `path`（root 相対）の内容を導き、その中の入れ子 submodule も同じ lock に平らに足す。
/// 入れ子の path は root 相対の全 path（例 `lib/pluvio/pluvio_ucx/lib/async-ucx`）。親の内容 hash は入れ子を含まない。
fn derive_submodule(
    path: &str,
    sub_commit: &str,
    submodule_repo: &dyn Fn(&str) -> Option<PathBuf>,
    out: &mut Vec<LockedSubmodule>,
) -> Result<(), TreeProofError> {
    let sub = submodule_repo(path).ok_or_else(|| {
        TreeProofError::Unavailable(format!("submodule {path}: no repo for commit {sub_commit}"))
    })?;
    let (resolved, _) = resolve(&sub, sub_commit).map_err(|e| {
        TreeProofError::Unavailable(format!("submodule {path}: commit {sub_commit}: {e}"))
    })?;
    let (sub_files, nested) = read_git_tree(&sub, &resolved)?;
    out.push(LockedSubmodule {
        path: path.to_string(),
        commit: resolved,
        content_sha256: digest(&sub_files),
    });
    for (rel, c) in nested {
        derive_submodule(&format!("{path}/{rel}"), &c, submodule_repo, out)?;
    }
    Ok(())
}

fn is_under(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|r| r.starts_with('/'))
}

/// 他の submodule の中に入っていない（本体の gitlink になる）submodule の path。
pub fn top_level_submodules(subs: &[LockedSubmodule]) -> Vec<&LockedSubmodule> {
    subs.iter()
        .filter(|s| !subs.iter().any(|o| is_under(&s.path, &o.path)))
        .collect()
}

/// `parent` の中にある入れ子 submodule の、`parent` 相対 path。
pub fn nested_paths(subs: &[LockedSubmodule], parent: &str) -> Vec<String> {
    subs.iter()
        .filter_map(|s| s.path.strip_prefix(parent)?.strip_prefix('/'))
        .map(str::to_string)
        .collect()
}

/// 渡された repo が lock の期待値と同じ commit / tree / 内容を持つか（repo 側の値は採用しない）。
pub fn verify_repo(expected: &LockedInitial, repo: &Path) -> Result<(), TreeProofError> {
    let (commit, tree) = resolve(repo, &expected.commit)
        .map_err(|e| TreeProofError::CommitMismatch(format!("{}: {e}", expected.commit)))?;
    if commit != expected.commit {
        return Err(TreeProofError::CommitMismatch(format!(
            "expected {} got {commit}",
            expected.commit
        )));
    }
    if tree != expected.tree {
        return Err(TreeProofError::TreeMismatch(format!(
            "expected {} got {tree}",
            expected.tree
        )));
    }
    let (files, links) = read_git_tree(repo, &commit)?;
    if digest(&files) != expected.content_sha256 {
        return Err(TreeProofError::ContentMismatch("repo tree content".into()));
    }
    let got: Vec<(&str, &str)> = links
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let want: Vec<(&str, &str)> = top_level_submodules(&expected.submodules)
        .into_iter()
        .map(|s| (s.path.as_str(), s.commit.as_str()))
        .collect();
    let mut want_sorted = want.clone();
    want_sorted.sort();
    if got != want_sorted {
        return Err(TreeProofError::SubmoduleMismatch(format!(
            "gitlinks differ: expected {want_sorted:?} got {got:?}"
        )));
    }
    Ok(())
}

/// 展開済みディレクトリ（`.git` の有無は問わない）が lock の期待内容と一致するか。
/// 本体は submodule path を除いて、各 submodule は path 配下を別に照合する。欠落・差替え・追加・修正は拒否。
pub fn verify_archive(expected: &LockedInitial, root: &Path) -> Result<(), TreeProofError> {
    let paths: Vec<String> = top_level_submodules(&expected.submodules)
        .into_iter()
        .map(|s| s.path.clone())
        .collect();
    for s in &expected.submodules {
        let dir = root.join(&s.path);
        if !dir.is_dir() {
            return Err(TreeProofError::SubmoduleMismatch(format!(
                "{}: missing",
                s.path
            )));
        }
        let got = archive_content_sha256(&dir, &nested_paths(&expected.submodules, &s.path))?;
        if got != s.content_sha256 {
            return Err(TreeProofError::SubmoduleMismatch(format!(
                "{}: content differs (commit {})",
                s.path, s.commit
            )));
        }
    }
    let main = archive_content_sha256(root, &paths)?;
    if main != expected.content_sha256 {
        return Err(TreeProofError::ContentMismatch(
            "archive tree content (missing, extra or modified file)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "lock_tree_tests.rs"]
mod tests;
