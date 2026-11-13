//! ADR 2026-11-12-knowledge-usage-per-run D1・D2: KB の操作の入口（`celerisctl knowledge`）で数える
//! run ごとの台帳。
//!
//! 入口は env [`ENV_USAGE_FILE`] があれば 1 操作 1 行（`{"op":"search","ok":true}`）を追記する。
//! daemon は run の前に台帳を消して path を渡し、run の後に [`read`] で合計を run の記録に付ける。
//! どの run の数かは daemon が持つ run id で決める（台帳に run id は書かない）。検索語・path・本文は
//! 書かない。harness の log は読まない。

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 台帳の path を run に渡す env。
pub const ENV_USAGE_FILE: &str = "CELERIS_KNOWLEDGE_USAGE_FILE";

/// run の artifacts の下に置く台帳の名前。
pub const USAGE_FILE_NAME: &str = "knowledge-usage.jsonl";

/// 数える KB の操作（ADR D1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeOp {
    Search,
    Get,
    Record,
}

/// 台帳の 1 行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Line {
    op: KnowledgeOp,
    ok: bool,
}

/// 1 run の KB の操作の回数（ADR D3。worker run は `RunMetrics.knowledge`、CoS run は
/// `chat_runs.knowledge_use_json`）。回数は呼び出しの数で、`failed` はそのうち失敗した数。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct KnowledgeUse {
    pub search: u32,
    pub get: u32,
    pub record: u32,
    #[serde(default)]
    pub failed: u32,
}

impl KnowledgeUse {
    /// 検索か取得を 1 回以上した（読み取り率の分子）。
    pub fn read_any(&self) -> bool {
        self.search > 0 || self.get > 0
    }

    fn add(&mut self, line: Line) {
        let slot = match line.op {
            KnowledgeOp::Search => &mut self.search,
            KnowledgeOp::Get => &mut self.get,
            KnowledgeOp::Record => &mut self.record,
        };
        *slot = slot.saturating_add(1);
        if !line.ok {
            self.failed = self.failed.saturating_add(1);
        }
    }
}

/// 台帳に 1 行を追記する（無ければ作る）。
pub fn append(path: &Path, op: KnowledgeOp, ok: bool) -> std::io::Result<()> {
    let mut line = serde_json::to_string(&Line { op, ok }).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    // 1 回の write にする（同じ run の中の並行な呼び出しで行が混ざらない。O_APPEND）。
    file.write_all(line.as_bytes())
}

/// env [`ENV_USAGE_FILE`] があれば追記する（入口の共通処理）。失敗は stderr に 1 行出して続ける
/// （計測の失敗で KB の操作を止めない。ADR D1）。
pub fn note_from_env(op: KnowledgeOp, ok: bool) {
    let Some(path) = std::env::var_os(ENV_USAGE_FILE).filter(|p| !p.is_empty()) else {
        return;
    };
    if let Err(e) = append(Path::new(&path), op, ok) {
        eprintln!(
            "warning: KB の利用の台帳に書けない（{}）: {e}",
            Path::new(&path).display()
        );
    }
}

/// 台帳の合計。file が無ければ 0 回（env を渡した run が KB を使わなかった）。読めない行は数えない。
pub fn read(path: &Path) -> KnowledgeUse {
    let mut total = KnowledgeUse::default();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return total;
    };
    for line in raw.lines() {
        if let Ok(line) = serde_json::from_str::<Line>(line) {
            total.add(line);
        }
    }
    total
}

/// ADR D3: CoS run の回数（`chat_runs.knowledge_use_json`）。CoS run は API の受け側
/// （`POST /api/v1/cos/knowledge-use`。run credential が run を決める）で数える。
pub trait KnowledgeUseStore {
    /// CoS run `run_id` の回数に 1 回を足し、足した後の回数を返す。行が無い・計測していない run（列が NULL）
    /// なら `Ok(None)`。
    fn chat_run_knowledge_use_add(
        &self,
        run_id: &str,
        op: KnowledgeOp,
        ok: bool,
    ) -> Result<Option<KnowledgeUse>, crate::StoreError>;
    /// 計測した CoS run の（thread の案件、回数）。
    fn chat_run_knowledge_uses(
        &self,
    ) -> Result<Vec<(Option<crate::ProjectId>, KnowledgeUse)>, crate::StoreError>;
}

impl KnowledgeUseStore for crate::SqliteStore {
    fn chat_run_knowledge_use_add(
        &self,
        run_id: &str,
        op: KnowledgeOp,
        ok: bool,
    ) -> Result<Option<KnowledgeUse>, crate::StoreError> {
        use rusqlite::OptionalExtension;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let raw: Option<Option<String>> = tx
            .query_row(
                "SELECT knowledge_use_json FROM chat_runs WHERE run_id = ?1",
                [run_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(Some(raw)) = raw else {
            return Ok(None);
        };
        let mut used: KnowledgeUse = serde_json::from_str(&raw)?;
        used.add(Line { op, ok });
        tx.execute(
            "UPDATE chat_runs SET knowledge_use_json = ?2 WHERE run_id = ?1",
            rusqlite::params![run_id, serde_json::to_string(&used)?],
        )?;
        tx.commit()?;
        Ok(Some(used))
    }

    fn chat_run_knowledge_uses(
        &self,
    ) -> Result<Vec<(Option<crate::ProjectId>, KnowledgeUse)>, crate::StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT t.project_id, r.knowledge_use_json FROM chat_runs r \
             JOIN chat_threads t ON t.id = r.thread_id WHERE r.knowledge_use_json IS NOT NULL \
             ORDER BY r.run_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (project, raw) = row?;
            let project = project.and_then(|p| p.parse::<crate::ProjectId>().ok());
            out.push((project, serde_json::from_str(&raw)?));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_lines_sum_per_op_and_bad_lines_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(USAGE_FILE_NAME);
        assert_eq!(read(&path), KnowledgeUse::default());
        append(&path, KnowledgeOp::Search, true).expect("append");
        append(&path, KnowledgeOp::Search, true).expect("append");
        append(&path, KnowledgeOp::Get, false).expect("append");
        append(&path, KnowledgeOp::Record, true).expect("append");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        file.write_all(b"not json\n{\"op\":\"delete\",\"ok\":true}\n")
            .expect("write");
        let total = read(&path);
        assert_eq!(
            total,
            KnowledgeUse {
                search: 2,
                get: 1,
                record: 1,
                failed: 1
            }
        );
        assert!(total.read_any());
        assert!(
            !KnowledgeUse {
                record: 3,
                ..Default::default()
            }
            .read_any()
        );
    }
}
