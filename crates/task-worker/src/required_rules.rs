//! ADR 2026-10-23-required-rules-prompt-delivery: 作業場所の必須規則（正本は作業場所直下の `CLAUDE.md`）を
//! Claude Code 以外の harness（Codex・Pi・ACP）に届ける。
//!
//! Claude Code は `CLAUDE.md` を直接読む。Codex は `AGENTS.override.md`/`AGENTS.md` だけを読み
//! （既定の `project_doc_fallback_filenames` は空）、Pi・opencode は `AGENTS.md` があると `CLAUDE.md` を読まない。
//! そこで規則は prompt の先頭に写して渡し、作業ツリーには 1 バイトも書かない（追跡された `AGENTS.md` を
//! 書き換えない・未追跡 file を作らない・`AGENTS.md`→`CLAUDE.md` の symlink でも入れ子に増えない）。

use std::path::Path;

/// 規則の正本の file 名（作業場所直下）。
pub const RULES_SOURCE: &str = "CLAUDE.md";

/// 前置きの見出し。試験と reviewer がこの印で配布を確かめる。
pub const RULES_HEADING: &str =
    "## 必須規則（celeris: 作業場所の CLAUDE.md の写し。作業前に読み、従うこと）";

/// `cwd` の `CLAUDE.md` を prompt の前置きにする。
///
/// - `CLAUDE.md` が無く、git が作業場所直下の `CLAUDE.md` を追跡していない作業場所（git でない dir・
///   規則を持たない repo）は空文字列。
/// - git の HEAD か index が `CLAUDE.md` を追跡しているのに作業ツリーに無い・参照先の無い symlink・
///   読めない・空は `Err`（呼び出し側は run を落とす）。
pub async fn preamble(cwd: &Path) -> std::io::Result<String> {
    let source = cwd.join(RULES_SOURCE);
    let rules = match tokio::fs::read_to_string(&source).await {
        Ok(rules) => rules,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 壊れた symlink は規則があるはずの宣言なので、欠落として検出する。
            if tokio::fs::symlink_metadata(&source).await.is_ok() {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "required rules source is a dangling link: {}",
                        source.display()
                    ),
                ));
            }
            // 正本を持つ repo で消えた規則と、規則を持たない作業場所を git の追跡で区別する。
            if tracked_by_git(cwd).await {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("required rules source is missing: {}", source.display()),
                ));
            }
            return Ok(String::new());
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
    Ok(format!(
        "{RULES_HEADING}\n\n{}\n\n---\n\n",
        rules.trim_end()
    ))
}

/// `cwd` が git の作業ツリーで、HEAD か index が `cwd` 直下の `CLAUDE.md` を追跡しているか。
/// git でない dir・git を起動できないときは追跡なし（規則を要求しない）とする。
async fn tracked_by_git(cwd: &Path) -> bool {
    let succeeds = |args: &'static [&'static str]| async move {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success())
    };
    succeeds(&["ls-files", "--error-unmatch", "--", RULES_SOURCE]).await
        || succeeds(&["cat-file", "-e", "HEAD:./CLAUDE.md"]).await
}

/// adapter 用: 前置きを読み、失敗は run を落とす `AdapterError` にする。
pub async fn preamble_for_run(cwd: &Path) -> Result<String, crate::adapter::AdapterError> {
    preamble(cwd).await.map_err(|e| {
        crate::adapter::AdapterError::Other(format!("required rules delivery failed: {e}"))
    })
}

#[cfg(test)]
mod tests;
