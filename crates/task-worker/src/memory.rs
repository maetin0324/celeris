//! ノードごとの長期記憶（ADR-0033 D6 / Phase 24）。**案件をまたいで覚える**。
//!
//! ```text
//! <memory_dir>/<node_id>/notes.md                 案件をまたぐ記憶
//! <memory_dir>/<node_id>/projects/<project_id>.md 案件の引き出し
//! ```
//!
//! run の前に読んでプロンプトに前置きし（`crate::preamble`）、run の後に結果ファイルの `memory` を
//! 日付付きの箇条書きで追記する。ADR-0001 D2 原則 2「状態はエージェントの外に置く」は守られる:
//! ハーネスのプロセスはステートレスのままで、記憶はファイルにあり、注入されるだけ。
//!
//! ここは**ファイル I/O だけ**で、LLM 呼び出しも判断も無い（どう覚えるかを決めるのはワーカー側）。

use std::io;
use std::path::{Path, PathBuf};

use crate::protocol::MemoryContext;

/// 前置きに載せる 1 ファイルあたりの上限（ADR-0033 D6「既定 8,000 字ずつ」）。
pub const MEMORY_MAX_CHARS: usize = 8_000;

/// 上限で切ったときに先頭へ置く印。
const TRUNCATED_MARK: &str = "…（古い記憶は省略）\n";

/// 記憶の置き場所（`[memory] dir`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDir {
    root: PathBuf,
}

/// 結果ファイル（`artifacts/result.json`）の `memory`（ADR-0033 D6）。
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
pub struct MemoryUpdate {
    /// 案件をまたぐ記憶に足す箇条書き。
    #[serde(default)]
    pub notes: Vec<String>,
    /// この案件の引き出しに足す箇条書き。
    #[serde(default)]
    pub project: Vec<String>,
}

impl MemoryUpdate {
    pub fn is_empty(&self) -> bool {
        self.notes.iter().all(|n| n.trim().is_empty())
            && self.project.iter().all(|n| n.trim().is_empty())
    }
}

impl MemoryDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn node_dir(&self, node_id: &str) -> PathBuf {
        self.root.join(node_id)
    }

    /// `<dir>/<node_id>/notes.md`。
    pub fn notes_path(&self, node_id: &str) -> PathBuf {
        self.node_dir(node_id).join("notes.md")
    }

    /// `<dir>/<node_id>/projects/<project_id>.md`。
    pub fn project_path(&self, node_id: &str, project_id: &str) -> PathBuf {
        self.node_dir(node_id)
            .join("projects")
            .join(format!("{project_id}.md"))
    }

    /// run の前に読む（無ければ空。読めなくても run は止めない）。それぞれ `MEMORY_MAX_CHARS` で切り、
    /// **新しい方（末尾）を残す**（記憶は追記なので、末尾が直近の内容）。
    pub fn load(&self, node_id: &str, project_id: Option<&str>) -> MemoryContext {
        MemoryContext {
            notes: read_tail(&self.notes_path(node_id)),
            project: project_id
                .map(|p| read_tail(&self.project_path(node_id, p)))
                .unwrap_or_default(),
        }
    }

    /// run の後に追記する（ADR-0033 D6）。`- <date>: <text>` の 1 行を 1 項目ぶん足す。
    /// 空白だけの項目は捨てる。`update` が空なら**何もしない**（ファイルも作らない）。
    pub fn append(
        &self,
        node_id: &str,
        project_id: Option<&str>,
        update: &MemoryUpdate,
        date: &str,
    ) -> io::Result<()> {
        if !update.notes.is_empty() {
            append_bullets(&self.notes_path(node_id), &update.notes, date)?;
        }
        match project_id {
            Some(project_id) if !update.project.is_empty() => {
                append_bullets(
                    &self.project_path(node_id, project_id),
                    &update.project,
                    date,
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// 結果ファイル（`<artifacts_dir>/result.json`）の `memory` を読む（ADR-0033 D6 の結果ファイル規約の拡張。
/// 置き場は ADR-0036 D2 で `RunRequest.artifacts_dir` になった）。
/// ファイルが無い・JSON でない・`memory` が無い・形が違うときは `None`（run は失敗させない）。
pub fn read_result_memory(artifacts_dir: &Path) -> Option<MemoryUpdate> {
    let text = std::fs::read_to_string(artifacts_dir.join("result.json")).ok()?;
    memory_from_result_json(&text)
}

/// 結果ファイルの本文から `memory` を取り出す（純粋関数）。
pub fn memory_from_result_json(text: &str) -> Option<MemoryUpdate> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let memory = value.get("memory")?;
    let update: MemoryUpdate = serde_json::from_value(memory.clone()).ok()?;
    if update.is_empty() {
        None
    } else {
        Some(update)
    }
}

fn read_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let count = text.chars().count();
    if count <= MEMORY_MAX_CHARS {
        return text;
    }
    let tail: String = text.chars().skip(count - MEMORY_MAX_CHARS).collect();
    // 行の途中で切れた先頭を捨てる（箇条書きの半端な断片を読ませない）。
    let body = match tail.find('\n') {
        Some(pos) => &tail[pos + 1..],
        None => tail.as_str(),
    };
    format!("{TRUNCATED_MARK}{body}")
}

fn append_bullets(path: &Path, items: &[String], date: &str) -> io::Result<()> {
    let lines: Vec<String> = items
        .iter()
        .filter(|item| !item.trim().is_empty())
        .map(|item| {
            format!(
                "- {date}: {}\n",
                item.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    if lines.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_dir_all_0700(parent)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(lines.concat().as_bytes())
}

/// 0700 で作る（記憶には人の好みや相談の中身が入るので、他のユーザには見せない）。
pub fn create_dir_all_0700(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    // 途中で作るディレクトリも 1 つずつ 0700 にする（`create_dir_all` は親を既定の umask で作るため）。
    if let Some(parent) = dir.parent() {
        create_dir_all_0700(parent)?;
    }
    std::fs::create_dir(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
