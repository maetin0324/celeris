//! ADR 2026-10-09-cos-task-repository-required: リポジトリを使う task が案件・リポジトリ無しで作られて
//! いないかの決定的な検査（LLM は使わない。字句の照合だけ）。
//!
//! CoS の起票（`/cos/operations` の `task.create` と会話の `actions.create_task`）はこれが `Some` を返すと
//! 拒否する（API は 422、旧 actions は失敗理由を記録）。人の `POST /tasks` は警告（応答の header）に留める。

use task_core::{Check, Task};

/// 拒否・警告の理由の先頭（人と CoS が読む文）。
pub const REPOSITORY_REQUIRED: &str =
    "リポジトリを使う task は project_id と repos を付けて起票する";

/// API の problem code。
pub const REPOSITORY_REQUIRED_CODE: &str = "repository_required";

/// リポジトリを前提にするコマンド（語として一致したとき）。
const REPO_COMMANDS: &[&str] = &[
    "cargo", "pnpm", "npm", "npx", "yarn", "git", "make", "rustc", "pytest", "nextest", "clippy",
];

/// リポジトリの中の path を示す字句（部分一致）。
const REPO_PATH_MARKERS: &[&str] = &[
    "crates/",
    "src/",
    "scripts/",
    "tests/",
    "web/",
    "gui/",
    "docs/",
    "Cargo.toml",
    "package.json",
    "pnpm-lock.yaml",
    "README.md",
    "Makefile",
];

/// リポジトリの中のファイルを示す拡張子（語の末尾）。
const REPO_FILE_SUFFIXES: &[&str] = &[".rs", ".ts", ".tsx", ".py", ".mjs"];

/// `task` がリポジトリを使うのに案件・リポジトリを持たないなら、その理由（[`REPOSITORY_REQUIRED`] で始まる文）。
///
/// workspace の指定によらず、案件が無いか repos が空なら検査する。
/// genre が coding、または objective・受け入れ条件が repo の path / コマンドを示すと理由を返す。
pub fn missing_repository_reason(task: &Task) -> Option<String> {
    if task.project_id.is_some() && !task.repos.is_empty() {
        return None;
    }
    let signal = if task.genre.as_deref() == Some("coding") {
        Some("harness が coding".to_string())
    } else {
        repo_signal(task)
    }?;
    let missing = if task.project_id.is_none() {
        "project_id と repos が無い"
    } else {
        "repos が空"
    };
    Some(format!("{REPOSITORY_REQUIRED}（{signal}。{missing}）"))
}

/// objective・受け入れ条件の文と command の check から、リポジトリを前提にする最初の字句。
fn repo_signal(task: &Task) -> Option<String> {
    let mut texts: Vec<&str> = vec![task.objective.as_str()];
    for criterion in &task.acceptance {
        texts.push(criterion.text.as_str());
        if let Check::Command { cmd, .. } = &criterion.check {
            texts.push(cmd.as_str());
        }
    }
    texts.into_iter().find_map(text_signal)
}

fn text_signal(text: &str) -> Option<String> {
    let words = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')))
        .filter(|w| !w.is_empty());
    for word in words {
        let trimmed = word.trim_matches(|c: char| c == '.' || c == '-');
        if REPO_COMMANDS.contains(&trimmed) {
            return Some(format!("コマンド {trimmed:?} を前提にしている"));
        }
        if REPO_PATH_MARKERS.iter().any(|m| trimmed.contains(m))
            || REPO_FILE_SUFFIXES
                .iter()
                .any(|s| trimmed.len() > s.len() && trimmed.ends_with(s))
        {
            return Some(format!("リポジトリの path {trimmed:?} を前提にしている"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_required_text_signals_are_lexical() {
        assert!(text_signal("cargo test -p task-api を通す").is_some());
        assert!(text_signal("`bash scripts/dev/test-parallel.sh` が 0").is_some());
        assert!(text_signal("crates/task-ops/src/edit.rs を直す").is_some());
        assert!(text_signal("git log を見る").is_some());
        assert!(text_signal("README.md の誤字").is_some());
        assert!(text_signal("論文の動向を調べる").is_none());
        assert!(text_signal("legit cargoship gitlab").is_none());
        assert!(text_signal(".rs").is_none());
    }
}
