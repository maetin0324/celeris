//! ADR-0056 D3 / ADR-0127: mount された skill を run の作業場所に届ける。
//! Claude Code は `.claude/skills`、Codex と ACP は `.agents/skills` を使う。

use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::protocol::SkillMount;

const SKILLS_MARKER_REL: &str = ".celeris/skills.json";
const CLAUDE_ROOT: &str = ".claude/skills";
const AGENTS_ROOT: &str = ".agents/skills";
const COPY_IGNORE: &str =
    "# celeris:skill-copy (ADR-0127). このディレクトリは run ごとに celeris が置き換える\n*\n";
const MARKER_IGNORE: &str = "*\n";

#[derive(Default, Serialize, Deserialize)]
struct SkillsMarker {
    version: u8,
    roots: BTreeMap<String, BTreeMap<String, String>>,
}

fn valid_name(name: &str) -> bool {
    task_core::knowledge::is_valid_skill_name(name)
}

async fn read_skills_marker(cwd: &Path) -> SkillsMarker {
    let Ok(text) = tokio::fs::read_to_string(cwd.join(SKILLS_MARKER_REL)).await else {
        return SkillsMarker::default();
    };
    if let Ok(marker) = serde_json::from_str::<SkillsMarker>(&text)
        && marker.version == 3
    {
        return marker;
    }
    #[derive(Deserialize)]
    struct LegacyMarker {
        version: u8,
        roots: BTreeMap<String, Vec<String>>,
    }
    let roots = if let Ok(names) = serde_json::from_str::<Vec<String>>(&text) {
        BTreeMap::from([(CLAUDE_ROOT.to_string(), names)])
    } else if let Ok(marker) = serde_json::from_str::<LegacyMarker>(&text)
        && marker.version == 2
    {
        marker.roots
    } else {
        return SkillsMarker::default();
    };
    SkillsMarker {
        version: 3,
        roots: roots
            .into_iter()
            .map(|(root, names)| {
                (
                    root,
                    names
                        .into_iter()
                        .map(|name| (name, String::new()))
                        .collect(),
                )
            })
            .collect(),
    }
}

async fn write_skills_marker(
    cwd: &Path,
    root: &str,
    hashes: &BTreeMap<String, String>,
) -> std::io::Result<()> {
    let dir = cwd.join(".celeris");
    tokio::fs::create_dir_all(&dir).await?;
    append_marker_ignore(&dir).await?;
    let marker = SkillsMarker {
        version: 3,
        roots: BTreeMap::from([(root.to_string(), hashes.clone())]),
    };
    let json = serde_json::to_vec(&marker).map_err(std::io::Error::other)?;
    let staging = tempfile::tempdir_in(&dir)?;
    let path = staging.path().join("skills.json");
    tokio::fs::write(&path, json).await?;
    tokio::fs::rename(path, dir.join("skills.json")).await
}

async fn append_marker_ignore(dir: &Path) -> std::io::Result<()> {
    let path = dir.join(".gitignore");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if existing.lines().any(|line| line == "*") {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(MARKER_IGNORE);
    tokio::fs::write(path, updated).await
}

/// `codex` の `AGENTS.md` に足す節の区切り。
pub const SECTION_BEGIN: &str = "<!-- celeris:skills:start -->";
pub const SECTION_END: &str = "<!-- celeris:skills:end -->";

fn read_skill_md(mount: &SkillMount) -> String {
    std::fs::read_to_string(Path::new(&mount.path).join("SKILL.md")).unwrap_or_default()
}

fn skill_description(mount: &SkillMount) -> String {
    let description = if mount.description.trim().is_empty() {
        read_skill_md(mount)
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .find_map(|line| line.strip_prefix("description:").map(str::trim))
            .unwrap_or_default()
            .to_string()
    } else {
        mount.description.clone()
    };
    description
        .chars()
        .take(1024)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// AGENTS.md と ACP 前置きに共通の一覧。本文は ADR-0127 の閾値 0 バイトにより埋め込まない。
pub fn skills_block(skills: &[SkillMount]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## Skills（celeris）\n次の skill を作業場所に置いてある。仕事が skill の説明に当てはまるときだけ、その SKILL.md を最後まで読み、\nSKILL.md が参照する付属ファイル（SKILL.md のあるディレクトリからの相対パス）も必要に応じて読むこと。\n当てはまらない skill は読まなくてよい。\n",
    );
    for mount in skills {
        // 名前は KB から解決されるが、一覧にパスを載せる前にも確認する。
        if !valid_name(&mount.name) {
            continue;
        }
        let description = skill_description(mount);
        out.push_str(&format!("- `{}`", mount.name));
        if !description.is_empty() {
            out.push_str(" — ");
            out.push_str(&description);
        }
        out.push_str(&format!("（`{AGENTS_ROOT}/{}/SKILL.md`）\n", mount.name));
        // ADR-0127 D3: 本文埋め込みの閾値は 0 バイト。
    }
    Some(out)
}

/// `AGENTS.md` に足す「必須規則」節の区切り。正本は作業場所の `CLAUDE.md`（Claude Code が直接読む）。
pub const RULES_BEGIN: &str = "<!-- celeris:rules:start -->";
pub const RULES_END: &str = "<!-- celeris:rules:end -->";

/// 区切り外の内容を保ちながら、`begin`〜`end` の節だけを更新する。
fn rewrite_section(existing: &str, begin: &str, end: &str, block: Option<&str>) -> String {
    let mut base = existing.to_string();
    if let (Some(start), Some(end_tag_at)) = (base.find(begin), base.find(end)) {
        let end_at = end_tag_at + end.len();
        if start <= end_at {
            base.replace_range(start..end_at, "");
        }
    }
    let base = base.trim_end();
    let Some(block) = block else {
        return if base.is_empty() {
            String::new()
        } else {
            format!("{base}\n")
        };
    };
    let mut out = base.to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(begin);
    out.push('\n');
    out.push_str(block.trim_end());
    out.push('\n');
    out.push_str(end);
    out.push('\n');
    out
}

/// 区切り外の内容を保ちながら celeris の skills 節だけを更新する。
pub fn rewrite_agents_md(existing: &str, block: Option<&str>) -> String {
    rewrite_section(existing, SECTION_BEGIN, SECTION_END, block)
}

/// 区切り外の内容を保ちながら celeris の必須規則節だけを更新する。
pub fn rewrite_rules_section(existing: &str, rules: Option<&str>) -> String {
    rewrite_section(existing, RULES_BEGIN, RULES_END, rules)
}

/// `codex` / `pi` / ACP 用の AGENTS.md 節。空なら既存の celeris 節だけを除去する。
pub async fn deliver_agents_md(cwd: &Path, skills: &[SkillMount]) -> std::io::Result<()> {
    let path = cwd.join("AGENTS.md");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if skills.is_empty() && !existing.contains(SECTION_BEGIN) {
        return Ok(());
    }
    let updated = rewrite_agents_md(&existing, skills_block(skills).as_deref());
    tokio::fs::write(&path, updated).await
}

/// 必須規則の正本（作業場所直下の `CLAUDE.md`）を `AGENTS.md` の `celeris:rules` 節へ写す。
/// Claude Code は `CLAUDE.md` を直接読むので不要。Codex・Pi・opencode 等の ACP は `AGENTS.md` を読むが、
/// `AGENTS.md` は gitignore 対象で fresh worktree には無いため、ここで届ける。
///
/// - `CLAUDE.md` が無い作業場所（規則を持たない repo）は何もしない。
/// - `CLAUDE.md` が読めない・空でないのに書けない・書いた後に読み戻して節が無い、は `Err`（run を落とす）。
/// - 区切り外のユーザーの内容は保つ。
pub async fn deliver_required_rules(cwd: &Path) -> std::io::Result<()> {
    let source = cwd.join("CLAUDE.md");
    let rules = match tokio::fs::read_to_string(&source).await {
        Ok(rules) => rules,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 壊れた symlink（参照先なし）は規則があるはずの宣言なので、欠落として検出する。
            if tokio::fs::symlink_metadata(&source).await.is_ok() {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "required rules source is a dangling link: {}",
                        source.display()
                    ),
                ));
            }
            return Ok(());
        }
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("cannot read required rules {}: {error}", source.display()),
            ));
        }
    };
    if rules.trim().is_empty() {
        return Err(std::io::Error::other(format!(
            "required rules source is empty: {}",
            source.display()
        )));
    }
    let path = cwd.join("AGENTS.md");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let block = format!(
        "## 必須規則（celeris。正本は CLAUDE.md。作業前に必ず読み、従うこと）\n\n{}",
        rules.trim_end()
    );
    let updated = rewrite_rules_section(&existing, Some(&block));
    tokio::fs::write(&path, &updated).await?;
    let written = tokio::fs::read_to_string(&path).await?;
    if !written.contains(RULES_BEGIN) || !written.contains(rules.trim_end()) {
        return Err(std::io::Error::other(format!(
            "required rules were not delivered to {}",
            path.display()
        )));
    }
    Ok(())
}

/// `acp` の前置きに足す一覧。
pub fn preamble_section(skills: &[SkillMount]) -> String {
    skills_block(skills)
        .map(|block| format!("\n{block}"))
        .unwrap_or_default()
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

fn copy_dir<'a>(src: &'a Path, dest: &'a Path) -> BoxFuture<'a, std::io::Result<()>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(dest).await?;
        let mut entries = tokio::fs::read_dir(src).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let from = entry.path();
            let to = dest.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir(&from, &to).await?;
            } else if file_type.is_file() {
                tokio::fs::copy(&from, &to).await?;
            }
            // KB 内の symlink は辿らない。
        }
        Ok(())
    })
}

// Source content includes its own root .gitignore. Delivery overwrites that file,
// so also compute the expected delivered hash with COPY_IGNORE substituted.
struct ContentHashes {
    content: String,
    delivered: String,
}

async fn content_hash(root: &Path, source: bool) -> std::io::Result<ContentHashes> {
    if !tokio::fs::symlink_metadata(root).await?.is_dir() {
        return Err(std::io::Error::other("skill root is not a directory"));
    }
    let mut pending = vec![std::path::PathBuf::new()];
    let mut records = BTreeMap::new();
    while let Some(relative) = pending.pop() {
        let mut entries = tokio::fs::read_dir(root.join(&relative)).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = relative.join(entry.file_name());
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                records.insert(path.clone(), b'd');
                pending.push(path);
            } else if kind.is_file() {
                records.insert(path, b'f');
            } else if !source {
                return Err(std::io::Error::other("unexpected entry in skill copy"));
            }
            // Source symlinks and special files are ignored, just as in copy_dir.
        }
    }
    if source {
        records.entry(".gitignore".into()).or_insert(b'x');
    }
    let mut content = Sha256::new();
    let mut delivered = Sha256::new();
    for (path, kind) in records {
        let bytes = if kind == b'f' {
            tokio::fs::read(root.join(&path)).await?
        } else {
            Vec::new()
        };
        if kind != b'x' {
            hash_record(&mut content, &path, kind, &bytes);
        }
        if source && path == Path::new(".gitignore") {
            hash_record(&mut delivered, &path, b'f', COPY_IGNORE.as_bytes());
        } else {
            hash_record(&mut delivered, &path, kind, &bytes);
        }
    }
    Ok(ContentHashes {
        content: format!("{:x}", content.finalize()),
        delivered: format!("{:x}", delivered.finalize()),
    })
}

// Sorted relative paths, entry kinds and length framing avoid ambiguous trees.
fn hash_record(hash: &mut Sha256, path: &Path, kind: u8, bytes: &[u8]) {
    let path = path.as_os_str().as_encoded_bytes();
    hash.update([kind]);
    hash.update((path.len() as u64).to_le_bytes());
    hash.update(path);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

async fn replace_copy(source: &Path, dest: &Path) -> std::io::Result<()> {
    let staging = tempfile::tempdir_in(dest.parent().expect("skill has a parent"))?;
    let new = staging.path().join("new");
    copy_dir(source, &new).await?;
    tokio::fs::write(new.join(".gitignore"), COPY_IGNORE).await?;
    let backup = staging.path().join("old");
    let had_old = match tokio::fs::rename(dest, &backup).await {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if let Err(error) = tokio::fs::rename(&new, dest).await {
        if had_old && let Err(restore_error) = tokio::fs::rename(&backup, dest).await {
            // Do not let TempDir cleanup delete the last complete copy if the
            // filesystem also refuses rollback. Retain it for recovery.
            let retained = staging.keep();
            return Err(std::io::Error::new(
                restore_error.kind(),
                format!(
                    "skill replacement failed ({error}); restore failed ({restore_error}); backup retained at {}",
                    retained.join("old").display()
                ),
            ));
        }
        return Err(error);
    }
    Ok(())
}

async fn owned_copy(path: &Path) -> bool {
    tokio::fs::read_to_string(path.join(".gitignore"))
        .await
        .is_ok_and(|text| text == COPY_IGNORE)
}

async fn known_copies(cwd: &Path, marker: &SkillsMarker) -> std::io::Result<Vec<(String, String)>> {
    let mut copies = Vec::new();
    for root in [CLAUDE_ROOT, AGENTS_ROOT] {
        if let Some(names) = marker.roots.get(root) {
            for name in names.keys() {
                if valid_name(name) {
                    copies.push((root.to_string(), name.clone()));
                }
            }
        }
        let path = cwd.join(root);
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if valid_name(&name) && owned_copy(&entry.path()).await {
                copies.push((root.to_string(), name));
            }
        }
    }
    copies.sort();
    copies.dedup();
    Ok(copies)
}

/// KB のディレクトリを届け先に丸写しし、前回の celeris の写しだけを掃除する。
/// `root` は固定の作業場所相対パス。アダプタ切替時にも旧届け先の写しを掃除する。
async fn deliver_to(cwd: &Path, root: &str, skills: &[SkillMount]) -> std::io::Result<()> {
    for mount in skills {
        if !valid_name(&mount.name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid skill name",
            ));
        }
    }
    let marker = read_skills_marker(cwd).await;
    let copies = known_copies(cwd, &marker).await?;
    let names: Vec<String> = skills.iter().map(|mount| mount.name.clone()).collect();
    if copies.is_empty() && skills.is_empty() && marker.roots.is_empty() {
        return Ok(());
    }
    // 先に衝突を確認する。アダプタ切替時に人の同名ディレクトリがある場合、
    // 古い届け先を消してから失敗すると利用可能だった skill まで失われる。
    if root == AGENTS_ROOT {
        for mount in skills {
            let dest = cwd.join(root).join(&mount.name);
            if tokio::fs::symlink_metadata(&dest).await.is_ok()
                && !owned_copy(&dest).await
                && !marker
                    .roots
                    .get(root)
                    .is_some_and(|names| names.contains_key(&mount.name))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "unowned skill directory already exists",
                ));
            }
        }
    }
    let mut hashes = BTreeMap::new();
    if !skills.is_empty() {
        let target_root = cwd.join(root);
        tokio::fs::create_dir_all(&target_root).await?;
        for mount in skills {
            let source = Path::new(&mount.path);
            let hash = content_hash(source, true).await?;
            let dest = target_root.join(&mount.name);
            let unchanged = owned_copy(&dest).await
                && marker
                    .roots
                    .get(root)
                    .and_then(|names| names.get(&mount.name))
                    == Some(&hash.content)
                && content_hash(&dest, false)
                    .await
                    .is_ok_and(|actual| actual.content == hash.delivered);
            if !unchanged {
                replace_copy(source, &dest).await?;
            }
            hashes.insert(mount.name.clone(), hash.content);
        }
    }
    for (old_root, name) in copies {
        if old_root == root && names.contains(&name) {
            continue;
        }
        let dest = cwd.join(&old_root).join(&name);
        if tokio::fs::symlink_metadata(&dest)
            .await
            .is_ok_and(|meta| meta.is_dir())
        {
            tokio::fs::remove_dir_all(dest).await?;
        }
    }
    write_skills_marker(cwd, root, &hashes).await
}

/// Claude Code の従来の届け先。marker は v1/v2 も読み、v3 を書く。
pub async fn deliver_claude_code(cwd: &Path, skills: &[SkillMount]) -> std::io::Result<()> {
    deliver_to(cwd, CLAUDE_ROOT, skills).await
}

/// Codex と ACP の作業場所内の届け先。
pub async fn deliver_agent_skills(cwd: &Path, skills: &[SkillMount]) -> std::io::Result<()> {
    deliver_to(cwd, AGENTS_ROOT, skills).await
}

#[cfg(test)]
mod tests;
