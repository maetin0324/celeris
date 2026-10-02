//! ADR-0122 D1/D2: repo に写した skill（`config/skills/<name>/`）を KB へ取り込む。
//!
//! 新しい書き込み経路は作らず、ディレクトリを読んで `skills_put` を呼ぶだけ（`celerisctl skills import`
//! が使う）。付属ファイルは UTF-8 のテキストだけを運び、読めないもの・シンボリックリンク・隠しファイルは
//! 飛ばして報告する。

use super::*;

/// 取り込まなかった付属ファイルとその理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSkillFile {
    /// skill のディレクトリからの相対パス。
    pub path: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// UTF-8 として読めない（画像など）。
    Binary,
    /// シンボリックリンク（辿らない）。
    Symlink,
    /// `.` で始まる名前（`.git`、`.DS_Store` など）。
    Hidden,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::Binary => "binary",
            SkipReason::Symlink => "symlink",
            SkipReason::Hidden => "hidden",
        }
    }
}

/// ディレクトリから読んだ skill 1 件（`skills_put` にそのまま渡せる形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSource {
    pub name: String,
    pub skill_md: String,
    /// 相対パスと本文（`SKILL.md` 自身は含まない。パス順）。
    pub files: Vec<(String, String)>,
    pub skipped: Vec<SkippedSkillFile>,
}

/// `skills_import_dir` の 1 件の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillImportReport {
    pub name: String,
    /// KB の相対パス（`skills/<name>/SKILL.md`）。
    pub path: String,
    /// 取り込んだ付属ファイルの数。
    pub files: usize,
    pub skipped: Vec<SkippedSkillFile>,
}

/// `dir/SKILL.md` と付属ファイルを読む。名前はディレクトリ名（frontmatter との一致は `skills_put` が見る）。
pub fn read_skill_dir(dir: &Path) -> Result<SkillSource, SkillError> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let skill_md = std::fs::read_to_string(dir.join(SKILL_FILE)).map_err(|e| {
        SkillError::Failed(format!(
            "{} を読めませんでした: {e}",
            dir.join(SKILL_FILE).display()
        ))
    })?;
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    walk_skill_dir(dir, dir, &mut files, &mut skipped)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(SkillSource {
        name,
        skill_md,
        files,
        skipped,
    })
}

fn walk_skill_dir(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, String)>,
    skipped: &mut Vec<SkippedSkillFile>,
) -> Result<(), SkillError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| SkillError::Failed(format!("{} を読めませんでした: {e}", dir.display())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let file_name = entry.file_name().to_string_lossy().to_string();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let skip = |reason| SkippedSkillFile {
            path: rel.clone(),
            reason,
        };
        if file_name.starts_with('.') {
            skipped.push(skip(SkipReason::Hidden));
        } else if meta.file_type().is_symlink() {
            skipped.push(skip(SkipReason::Symlink));
        } else if meta.is_dir() {
            walk_skill_dir(root, &path, files, skipped)?;
        } else if rel != SKILL_FILE {
            let bytes = std::fs::read(&path).map_err(|e| {
                SkillError::Failed(format!("{} を読めませんでした: {e}", path.display()))
            })?;
            match String::from_utf8(bytes) {
                Ok(body) => files.push((rel, body)),
                Err(_) => skipped.push(skip(SkipReason::Binary)),
            }
        }
    }
    Ok(())
}

/// 取り込む skill のディレクトリ。`dir` が `SKILL.md` を持てばその 1 件、持たなければ直下の
/// `SKILL.md` を持つディレクトリ（名前順）。`names` が空でなければその名前だけ（無い名前は誤り）。
pub fn skill_dirs_in(dir: &Path, names: &[String]) -> Result<Vec<PathBuf>, SkillError> {
    let mut dirs = Vec::new();
    if dir.join(SKILL_FILE).is_file() {
        dirs.push(dir.to_path_buf());
    } else {
        let entries = std::fs::read_dir(dir).map_err(|e| {
            SkillError::Failed(format!("{} を読めませんでした: {e}", dir.display()))
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            let is_real_dir =
                std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir());
            if is_real_dir && path.join(SKILL_FILE).is_file() {
                dirs.push(path);
            }
        }
        dirs.sort();
    }
    if names.is_empty() {
        return Ok(dirs);
    }
    let dir_name = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    };
    for name in names {
        if !dirs.iter().any(|d| &dir_name(d) == name) {
            return Err(SkillError::Failed(format!(
                "skill {name:?} は {} に見つかりません",
                dir.display()
            )));
        }
    }
    dirs.retain(|d| names.contains(&dir_name(d)));
    Ok(dirs)
}

/// ADR-0122 D1: `dir` の skill を KB に取り込む（1 件ずつ `skills_put`。冪等）。最初の失敗で止める。
pub fn skills_import_dir(
    root: &Path,
    dir: &Path,
    names: &[String],
    source: Option<&str>,
) -> Result<Vec<SkillImportReport>, SkillError> {
    let mut reports = Vec::new();
    for skill_dir in skill_dirs_in(dir, names)? {
        let skill = read_skill_dir(&skill_dir)?;
        let path = skills_put(root, &skill.name, &skill.skill_md, &skill.files, source)?;
        reports.push(SkillImportReport {
            name: skill.name,
            path,
            files: skill.files.len(),
            skipped: skill.skipped,
        });
    }
    Ok(reports)
}

#[cfg(test)]
#[path = "skill_import_tests.rs"]
mod tests;
