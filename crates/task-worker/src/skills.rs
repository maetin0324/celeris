//! ADR-0056 D3（Phase 79）: mount された skills（`RunContext.skills`）を run 開始時にアダプタへ届ける。
//!
//! `RunContext.skills` は名前・KB のディレクトリの絶対パス・frontmatter の説明だけを運ぶ（本文は
//! 含まない）。ここは本文（`SKILL.md`）を実際に読み、アダプタごとの届け先に書く/組み立てる
//! （`claude_code` / `codex` / `acp` から呼ばれる。研究系アダプタは呼ばない）。
//!
//! 届け方（ADR-0056 D3）:
//! - `claude-code`: 作業場所の `.claude/skills/<name>/` に KB のディレクトリを丸ごと写す
//!   （そのサブディレクトリだけを置き換える。`.claude` の他の内容には触れない）。
//! - `codex`: 作業場所の `AGENTS.md` の末尾の `<!-- celeris:skills:start -->` 〜
//!   `<!-- celeris:skills:end -->` の間を run ごとに書き直す（既存の `AGENTS.md` の他の内容は保つ。
//!   無ければ作る）。
//! - `acp`: 前置き（プロンプト文面）に `## Skills（celeris）` 節として直接埋め込む。
//!
//! ADR-0056 Phase 81 追記: `claude-code` は unmount した skill のディレクトリが `.claude/skills/`
//! に残り続ける問題があった（Phase 79 は「対象の skill だけ置き換える」までしかやらず、
//! 「前回あったが今回は無い」skill の削除は範囲外だった）。`<cwd>/.celeris/skills.json` に
//! 前回 celeris が書いた skill 名の一覧を残し、次回の届け先でそこにあって今回無い名前だけを
//! 消す（マーカーに無いディレクトリ＝人・他の仕組みが置いたものには触れない）。`codex` の
//! `AGENTS.md` は区切りの節を run ごとに丸ごと書き直す（`rewrite_agents_md`）ので、同じ問題は
//! 元から無い（マーカーは要らない）。

use std::path::Path;
use std::pin::Pin;

use crate::protocol::SkillMount;

/// Phase 81: `claude-code` が前回書いた skill 名の一覧（`.celeris/skills.json`）。
const SKILLS_MARKER_REL: &str = ".celeris/skills.json";

fn skills_marker_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(SKILLS_MARKER_REL)
}

/// マーカーを読む。無い・壊れている・読めないときは空（＝前回の記録が無いものとして扱う。
/// 削除を試みない方が安全 — ユーザーが手で作ったディレクトリを誤って消さないため）。
async fn read_skills_marker(cwd: &Path) -> Vec<String> {
    match tokio::fs::read_to_string(skills_marker_path(cwd)).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// マーカーを書く（`.celeris/` が無ければ作る）。書けなくても run は落とさない（呼び出し側が warn する）。
async fn write_skills_marker(cwd: &Path, names: &[String]) -> std::io::Result<()> {
    let path = skills_marker_path(cwd);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let json = serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string());
    tokio::fs::write(&path, json).await
}

/// `codex` の `AGENTS.md` に足す節の区切り。この 2 行の間だけを run ごとに書き直す。
pub const SECTION_BEGIN: &str = "<!-- celeris:skills:start -->";
pub const SECTION_END: &str = "<!-- celeris:skills:end -->";

/// KB の `<mount.path>/SKILL.md` を読む（読めなければ空文字列。run は落とさない）。
fn read_skill_md(mount: &SkillMount) -> String {
    std::fs::read_to_string(Path::new(&mount.path).join("SKILL.md")).unwrap_or_default()
}

/// `## Skills（celeris）` 節の中身（区切り行は含まない）。`skills` が空なら `None`。
/// 各 skill を `### <name>` の見出し + description（あれば）+ `SKILL.md` の本文で連結する。
pub fn skills_block(skills: &[SkillMount]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut out = String::from("## Skills（celeris）\n");
    for mount in skills {
        out.push_str(&format!("\n### {}\n", mount.name));
        if !mount.description.is_empty() {
            out.push_str(&mount.description);
            out.push('\n');
        }
        let body = read_skill_md(mount);
        let body = body.trim_end();
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
    }
    Some(out)
}

/// `AGENTS.md` の中身（`existing`）の末尾にある区切りの節を書き直す。`block`（`skills_block` の結果）が
/// `Some` ならその節を末尾に置く（無ければ足す）。`None` なら既存の節を取り除くだけ（新しい節は足さない）。
/// 区切りの**外側**の内容は常にそのまま保つ。冪等（同じ入力を 2 回かけても結果は変わらない）。
pub fn rewrite_agents_md(existing: &str, block: Option<&str>) -> String {
    let mut base = existing.to_string();
    if let (Some(start), Some(end_tag_at)) = (base.find(SECTION_BEGIN), base.find(SECTION_END)) {
        let end = end_tag_at + SECTION_END.len();
        if start <= end {
            base.replace_range(start..end, "");
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
    out.push_str(SECTION_BEGIN);
    out.push('\n');
    out.push_str(block.trim_end());
    out.push('\n');
    out.push_str(SECTION_END);
    out.push('\n');
    out
}

/// `codex` 用: `<cwd>/AGENTS.md` を読み直し（無ければ空から）、skills の節を書き直す。`skills` が空なら
/// 何もしない（ファイルが無いのに作らない。既存の `AGENTS.md` にも触れない）。
pub async fn deliver_agents_md(cwd: &Path, skills: &[SkillMount]) -> std::io::Result<()> {
    if skills.is_empty() {
        return Ok(());
    }
    let path = cwd.join("AGENTS.md");
    let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
    let updated = rewrite_agents_md(&existing, skills_block(skills).as_deref());
    tokio::fs::write(&path, updated).await
}

/// `acp` 用: 前置き（プロンプト文面）の末尾に足す文字列（`skills` が空なら空文字列）。
pub fn preamble_section(skills: &[SkillMount]) -> String {
    match skills_block(skills) {
        Some(block) => format!("\n{block}"),
        None => String::new(),
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// `src` の中身（サブディレクトリ含む）を `dest` に丸ごと写す。
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
            // シンボリックリンク等は写さない（KB のディレクトリは celeris 自身が書くので想定しない）。
        }
        Ok(())
    })
}

/// `claude-code` 用: KB の `<mount.path>/` を丸ごと `<cwd>/.claude/skills/<name>/` へ写す
/// （そのディレクトリ**だけ**を置き換える。`.claude` の他の内容には触れない）。
///
/// Phase 81: 前回このワーカーが書いた skill 名（`.celeris/skills.json`）を読み、今回の `skills` に
/// 無い名前だけ `.claude/skills/<name>/` を削除する（マーカーに無い名前＝人・他の仕組みが置いた
/// ディレクトリには触れない）。`skills` が空でも、前回の記録があれば掃除だけは行う（マーカーも
/// 空にする）。前回の記録が無く今回も空なら、何も無いので `.claude` すら作らない（従来どおり）。
pub async fn deliver_claude_code(cwd: &Path, skills: &[SkillMount]) -> std::io::Result<()> {
    let previous = read_skills_marker(cwd).await;
    let current_names: Vec<String> = skills.iter().map(|m| m.name.clone()).collect();

    if !previous.is_empty() {
        let root = cwd.join(".claude").join("skills");
        for name in &previous {
            if current_names.contains(name) {
                continue;
            }
            let dest = root.join(name);
            if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
                tokio::fs::remove_dir_all(&dest).await?;
            }
        }
    }

    if skills.is_empty() {
        if !previous.is_empty() {
            write_skills_marker(cwd, &current_names).await?;
        }
        return Ok(());
    }

    let root = cwd.join(".claude").join("skills");
    tokio::fs::create_dir_all(&root).await?;
    for mount in skills {
        let dest = root.join(&mount.name);
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            tokio::fs::remove_dir_all(&dest).await?;
        }
        copy_dir(Path::new(&mount.path), &dest).await?;
    }
    write_skills_marker(cwd, &current_names).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
