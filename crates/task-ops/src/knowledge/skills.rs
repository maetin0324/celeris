//! KB に保管する skill の検証、読み書き、mount 操作。

use super::*;

/// `skills_list` の 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

/// `skills_get` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDetail {
    pub name: String,
    /// `SKILL.md` の中身（frontmatter 込み）。
    pub skill_md: String,
    /// `SKILL.md` と同じディレクトリの付属ファイル（相対パス。`SKILL.md` 自身は含まない）。
    pub files: Vec<String>,
}

/// `skills_put` の失敗。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillError {
    #[error("skill name {name:?} must match [a-z0-9-] (lowercase, 1..=64 characters)")]
    InvalidName { name: String },
    #[error("skill_md must have a frontmatter block (--- … ---) with `name` and `description`")]
    MissingFrontmatter,
    #[error("frontmatter `name` ({found:?}) must equal the skill name ({name:?})")]
    NameMismatch { name: String, found: String },
    #[error("frontmatter `description` must not be blank")]
    NoDescription,
    #[error("attached file path {path:?} is not allowed (no `..`, no absolute paths)")]
    BadFilePath { path: String },
    #[error("{0}")]
    Failed(String),
}

fn skill_dir(root: &Path, name: &str) -> Result<PathBuf, SkillError> {
    if !kb::is_valid_skill_name(name) {
        return Err(SkillError::InvalidName {
            name: name.to_string(),
        });
    }
    Ok(root.join(SKILLS_ROOT_DIR).join(name))
}

const SKILLS_ROOT_DIR: &str = "skills";
const SKILL_FILE: &str = "SKILL.md";

/// SKILL.md の frontmatter（`---\n...\n---\n`）から `key: value` の行を素直に読む（クォート無し、
/// 1 行 1 鍵の最小 YAML もどき。KB の front matter とは別の形式なので `kb::front_matter` は使わない）。
type SkillFrontmatter = (Vec<(String, String)>, usize, usize);

fn skill_frontmatter(raw: &str) -> Option<SkillFrontmatter> {
    let raw_trimmed_start = raw.trim_start_matches('\u{feff}');
    if !raw_trimmed_start.starts_with("---") {
        return None;
    }
    let after_open = &raw_trimmed_start[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close_rel = after_open.find("\n---")?;
    let body = &after_open[..close_rel];
    let mut fields = Vec::new();
    for line in body.lines() {
        if let Some((k, v)) = line.split_once(':') {
            fields.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let open_len = raw_trimmed_start.len() - after_open.len();
    Some((fields, open_len, open_len + close_rel))
}

fn frontmatter_field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `name` / `description` を検証し、`source` 鍵が無ければ frontmatter に足す（ADR-0056 D3: 「出典
/// `mcp:<client_id>` を frontmatter に残す」）。冪等（既に `source:` があれば触らない）。
fn prepare_skill_md(
    name: &str,
    skill_md: &str,
    source: Option<&str>,
) -> Result<String, SkillError> {
    let Some((fields, open, close)) = skill_frontmatter(skill_md) else {
        return Err(SkillError::MissingFrontmatter);
    };
    let found_name = frontmatter_field(&fields, "name").unwrap_or_default();
    if found_name.is_empty() || found_name != name {
        return Err(SkillError::NameMismatch {
            name: name.to_string(),
            found: found_name.to_string(),
        });
    }
    let description = frontmatter_field(&fields, "description").unwrap_or_default();
    if description.trim().is_empty() {
        return Err(SkillError::NoDescription);
    }
    if let Some(source) = source
        && frontmatter_field(&fields, "source").is_none()
    {
        let mut out = String::with_capacity(skill_md.len() + source.len() + 16);
        out.push_str(&skill_md[..close]);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("source: {source}\n"));
        out.push_str(&skill_md[close..]);
        return Ok(out);
    }
    let _ = open;
    Ok(skill_md.to_string())
}

/// 相対パスとして安全か（`..` を含まない、絶対パスでない、空でない）。
fn safe_relative_path(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// ADR-0056 D3: `skills/<name>/SKILL.md`（＋付属ファイル）を書く（**直接コミット**。mount されるまで
/// 何にも効かない）。`source`（`mcp:<client_id>`）は frontmatter に無ければ足す。
pub fn skills_put(
    root: &Path,
    name: &str,
    skill_md: &str,
    files: &[(String, String)],
    source: Option<&str>,
) -> Result<String, SkillError> {
    let dir = skill_dir(root, name)?;
    for (path, _) in files {
        if !safe_relative_path(path) || path == SKILL_FILE {
            return Err(SkillError::BadFilePath { path: path.clone() });
        }
    }
    let content = prepare_skill_md(name, skill_md, source)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| SkillError::Failed(format!("{} を作れませんでした: {e}", dir.display())))?;
    std::fs::write(dir.join(SKILL_FILE), content.as_bytes())
        .map_err(|e| SkillError::Failed(format!("{SKILL_FILE} を書けませんでした: {e}")))?;
    let mut rel_paths = vec![format!("{SKILLS_ROOT_DIR}/{name}/{SKILL_FILE}")];
    for (path, body) in files {
        let file_path = dir.join(path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SkillError::Failed(format!("{} を作れませんでした: {e}", parent.display()))
            })?;
        }
        std::fs::write(&file_path, body.as_bytes())
            .map_err(|e| SkillError::Failed(format!("{path} を書けませんでした: {e}")))?;
        rel_paths.push(format!("{SKILLS_ROOT_DIR}/{name}/{path}"));
    }
    let refs: Vec<&str> = rel_paths.iter().map(String::as_str).collect();
    commit_paths(
        root,
        &format!("knowledge: skills/{name} を更新"),
        (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
        &refs,
    )
    .map_err(SkillError::Failed)?;
    Ok(format!("{SKILLS_ROOT_DIR}/{name}/{SKILL_FILE}"))
}

/// ADR-0056 D3（Phase 79）: `SKILL.md`（`skills_get` の `skill_md`）の frontmatter の `description`
/// （無ければ空文字列）。ディスパッチャが `RunContext.skills[].description` を組むのに使う。
pub fn skill_description(skill_md: &str) -> String {
    skill_frontmatter(skill_md)
        .and_then(|(fields, ..)| frontmatter_field(&fields, "description").map(str::to_string))
        .unwrap_or_default()
}

/// ADR-0122 D4: skill が届く run の種類。指定が無い場合と未知の値だけの場合は
/// 従来どおり work に届ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillUse {
    Work,
    Review,
}

pub fn skill_applies_to(skill_md: &str, run: SkillUse) -> bool {
    let uses = skill_frontmatter(skill_md)
        .and_then(|(fields, ..)| frontmatter_field(&fields, "celeris-use").map(str::to_string));
    let Some(uses) = uses else {
        return run == SkillUse::Work;
    };
    let mut work = false;
    let mut review = false;
    for value in uses.split(',').map(str::trim) {
        match value {
            "work" => work = true,
            "review" => review = true,
            _ => {}
        }
    }
    if !work && !review {
        work = true;
    }
    match run {
        SkillUse::Work => work,
        SkillUse::Review => review,
    }
}

/// ADR-0056 D2: `skills/` にある skill の一覧（`name` / frontmatter の `description`）。
pub fn skills_list(root: &Path) -> Vec<SkillSummary> {
    let dir = root.join(SKILLS_ROOT_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !kb::is_valid_skill_name(name) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path.join(SKILL_FILE)) else {
            continue;
        };
        let description = skill_frontmatter(&raw)
            .and_then(|(fields, ..)| frontmatter_field(&fields, "description").map(str::to_string))
            .unwrap_or_default();
        out.push(SkillSummary {
            name: name.to_string(),
            description,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Phase 82（ADR-0056 D3 続き）: skill 全体を消す（`skills/<name>/` をまるごと）。**mount されているかの
/// 判断は呼び出し側**（`task-api`/`celeris-mcp` は組織の profile を読めるが、task-ops は KB のことしか
/// 知らないため。呼び出し側が 409 相当を先に判断してから呼ぶ）。無い skill は `Failed`。
pub fn skills_delete(root: &Path, name: &str) -> Result<String, SkillError> {
    let dir = skill_dir(root, name)?;
    if !dir.join(SKILL_FILE).exists() {
        return Err(SkillError::Failed(format!("skill not found: {name}")));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| SkillError::Failed(format!("{} を消せませんでした: {e}", dir.display())))?;
    let scope = format!("{SKILLS_ROOT_DIR}/{name}");
    commit_paths(
        root,
        &format!("knowledge: skills/{name} を削除"),
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &[scope.as_str()],
    )
    .map_err(SkillError::Failed)
}

/// Phase 82（ADR-0056 D3 続き）: ノードの `profile.skills_mounts` に skill 名を足す・外す
/// （`celeris-mcp` の `org_mount_skill`/`org_unmount_skill` と、task-api の `POST/DELETE
/// /org/{id}/skills…` が**同じこの関数**を呼ぶ。挙動が食い違わないようにするため）。
/// 名前の形が不正なら触らずに `Err`（mount 済みの一覧はそのまま）。
pub fn set_skill_mount(
    mounts: &mut Vec<String>,
    skill: &str,
    mount: bool,
) -> Result<(), SkillError> {
    if !kb::is_valid_skill_name(skill) {
        return Err(SkillError::InvalidName {
            name: skill.to_string(),
        });
    }
    if mount {
        if !mounts.iter().any(|s| s == skill) {
            mounts.push(skill.to_string());
        }
    } else {
        mounts.retain(|s| s != skill);
    }
    Ok(())
}

/// ADR-0056 D2: skill 1 件（`SKILL.md` 本文と付属ファイルの一覧）。無ければ `None`。
pub fn skills_get(root: &Path, name: &str) -> Option<SkillDetail> {
    if !kb::is_valid_skill_name(name) {
        return None;
    }
    let dir = root.join(SKILLS_ROOT_DIR).join(name);
    let skill_md = std::fs::read_to_string(dir.join(SKILL_FILE)).ok()?;
    let mut files = Vec::new();
    collect_skill_files(&dir, &dir, &mut files);
    files.sort();
    Some(SkillDetail {
        name: name.to_string(),
        skill_md,
        files,
    })
}

fn collect_skill_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_skill_files(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel != SKILL_FILE {
                out.push(rel);
            }
        }
    }
}
