//! 第 4 フェーズ P4-D（ADR 2026-11-05-phase4-p4d-retention）: `project_wakes`・`project_controller_decisions`・
//! `script_compare_log` の保持と GC（migration 0117）。LLM 呼び出しは無い。
//!
//! 1 batch の流れ: 候補を読む（読み取り接続）→ archive の JSONL を書いて fsync → 書き込みの transaction で
//! `project_retention_permit` に 1 行入れ、条件を付け直した DELETE で消し、許可の行を消し、
//! `project_retention_runs` に件数と archive の sha256 を残して commit。commit の前に落ちれば行は残り、
//! archive の file だけが残る（記録の無い file は次の GC と無関係）。

use std::io::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::project_retention::{
    ProjectRetentionCounts, RETENTION_TABLES, RetentionPolicy, RetentionReport, RetentionRun,
    RetentionStats, RetentionTableReport,
};

/// 判断の DELETE の付け直しの条件（trigger と同じ。trigger が abort しないように DELETE 文でも外す）。
const DECISION_GUARD: &str = "effect_state = 'none' AND validation NOT LIKE 'pc/resume/%' \
     AND validation <> 'pc/authority-required' \
     AND NOT EXISTS (SELECT 1 FROM project_controller_state s \
                     WHERE s.last_decision_id = project_controller_decisions.id)";
/// wake の DELETE の条件（0115 が表を作り直すので trigger は置かない。ここが唯一の守り）。
const WAKE_GUARD: &str = "consumed_by IS NOT NULL AND kind = 'cron'";
const COMPARE_GUARD: &str = "COALESCE(json_extract(json, '$.applied_by'), '') <> 'ts'";

#[cfg(test)]
thread_local! {
    /// 試験: archive を書いた後・transaction の前に落ちたことにする。
    pub(super) static FAIL_AFTER_ARCHIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct Candidate {
    key: Value,
    row: serde_json::Value,
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> StoreError {
    StoreError::Invalid(format!("retention archive {what} {}: {e}", path.display()))
}

fn cell_json(v: ValueRef<'_>) -> serde_json::Value {
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::from(i),
        ValueRef::Real(f) => serde_json::Value::from(f),
        ValueRef::Text(t) => serde_json::Value::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => {
            serde_json::Value::from(b.iter().map(|x| format!("{x:02x}")).collect::<String>())
        }
    }
}

fn key_text(v: &Value) -> String {
    match v {
        Value::Integer(i) => i.to_string(),
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

fn select_rows(
    c: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    key_col: &str,
) -> Result<Vec<Candidate>, StoreError> {
    let mut stmt = c.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let key_idx = names
        .iter()
        .position(|n| n == key_col)
        .ok_or_else(|| StoreError::Invalid(format!("retention: no column {key_col}")))?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut obj = serde_json::Map::new();
        for (i, n) in names.iter().enumerate() {
            obj.insert(n.clone(), cell_json(r.get_ref(i)?));
        }
        out.push(Candidate {
            key: r.get::<_, Value>(key_idx)?,
            row: serde_json::Value::Object(obj),
        });
    }
    Ok(out)
}

fn cutoff(now: OffsetDateTime, keep_secs: u64) -> Result<String, StoreError> {
    let secs = i64::try_from(keep_secs).unwrap_or(i64::MAX / 2);
    format_rfc3339(now - time::Duration::seconds(secs))
}

/// file 名に使える形（英数字・`-`・`_` 以外は `_`）。
fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// archive の file を新しく作る（既にある名前は使わない。同じ時刻の pass・並行の GC が上書きしない）。
fn create_archive(dir: &Path, stem: &str) -> Result<(PathBuf, std::fs::File), StoreError> {
    for i in 0u32.. {
        let name = if i == 0 {
            format!("{stem}.jsonl")
        } else {
            format!("{stem}-{i}.jsonl")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err("create", &path, e)),
        }
    }
    Err(StoreError::Invalid(
        "retention archive: no free file name".into(),
    ))
}

/// archive の file を書いて fsync し、path と sha256 を返す。
fn write_archive(
    dir: &Path,
    stem: &str,
    table: &str,
    rows: &[Candidate],
) -> Result<(PathBuf, String), StoreError> {
    let mut buf = Vec::new();
    for c in rows {
        let line = serde_json::json!({ "table": table, "row": c.row });
        serde_json::to_writer(&mut buf, &line)
            .map_err(|e| StoreError::Invalid(format!("retention archive json: {e}")))?;
        buf.push(b'\n');
    }
    let (path, mut f) = create_archive(dir, stem)?;
    f.write_all(&buf).map_err(|e| io_err("write", &path, e))?;
    f.sync_all().map_err(|e| io_err("fsync", &path, e))?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok((path, format!("{:x}", Sha256::digest(&buf))))
}

impl SqliteStore {
    /// 1 回の GC（表ごとに最大 `max_rows_per_pass` 行）。消す行は先に `archive_dir` の JSONL に書く。
    /// 未消費の wake・pending の効果・案件の最後の判断・再開と許可待ちの判断・新しい順 `decision_keep_latest`
    /// 件・案件の最初の判断・TS が適用した compare の記録は消さない。
    pub fn retention_gc(
        &self,
        policy: &RetentionPolicy,
        now: OffsetDateTime,
        archive_dir: &Path,
        actor: &str,
    ) -> Result<RetentionReport, StoreError> {
        policy.validate().map_err(StoreError::Invalid)?;
        std::fs::create_dir_all(archive_dir).map_err(|e| io_err("mkdir", archive_dir, e))?;
        let mut report = RetentionReport::default();
        for table in RETENTION_TABLES {
            let mut t = RetentionTableReport {
                table: table.into(),
                ..Default::default()
            };
            let keep = match table {
                "project_wakes" => policy.wake_keep_secs,
                "project_controller_decisions" => policy.decision_keep_secs,
                _ => policy.compare_keep_secs,
            };
            let cut = cutoff(now, keep)?;
            let mut projects: Vec<String> = if table == "project_controller_decisions" {
                self.with_read_conn(|c| {
                    let mut s = c.prepare(
                        "SELECT DISTINCT project_id FROM project_controller_decisions ORDER BY project_id",
                    )?;
                    let rows = s.query_map([], |r| r.get(0))?;
                    Ok(rows.collect::<Result<Vec<String>, _>>()?)
                })?
            } else {
                vec![String::new()]
            };
            projects.reverse();
            while (t.archived as usize) < policy.max_rows_per_pass {
                let Some(project) = projects.last().cloned() else {
                    break;
                };
                let room = policy
                    .batch_rows
                    .min(policy.max_rows_per_pass - t.archived as usize);
                let rows = self.retention_candidates(table, &project, &cut, policy, room)?;
                if rows.is_empty() {
                    projects.pop();
                    continue;
                }
                let deleted = self.retention_delete_batch(
                    table,
                    &rows,
                    &cut,
                    archive_dir,
                    actor,
                    now,
                    &mut t,
                )?;
                t.batches += 1;
                t.archived += rows.len() as u64;
                t.deleted += deleted;
                if deleted == 0 {
                    // 並行の GC が先に消した（候補を読み直しても同じ行は出ない）か、条件が変わった。次の案件へ。
                    projects.pop();
                }
            }
            report.tables.push(t);
        }
        Ok(report)
    }

    fn retention_candidates(
        &self,
        table: &str,
        project: &str,
        cut: &str,
        policy: &RetentionPolicy,
        limit: usize,
    ) -> Result<Vec<Candidate>, StoreError> {
        let lim = i64::try_from(limit).unwrap_or(i64::MAX);
        self.with_read_conn(|c| match table {
            "project_wakes" => select_rows(
                c,
                &format!(
                    "SELECT * FROM project_wakes WHERE {WAKE_GUARD} AND created_at < ?1 ORDER BY id LIMIT ?2"
                ),
                &[&cut, &lim],
                "id",
            ),
            "script_compare_log" => select_rows(
                c,
                &format!(
                    "SELECT * FROM script_compare_log WHERE {COMPARE_GUARD} AND ts < ?1 ORDER BY id LIMIT ?2"
                ),
                &[&cut, &lim],
                "id",
            ),
            _ => {
                let offset = i64::try_from(policy.decision_keep_latest - 1).unwrap_or(i64::MAX);
                let keep_from: Option<(String, String)> = c
                    .query_row(
                        "SELECT created_at, id FROM project_controller_decisions WHERE project_id = ?1 \
                         ORDER BY created_at DESC, id DESC LIMIT 1 OFFSET ?2",
                        params![project, offset],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        e => Err(e),
                    })?;
                let Some((keep_at, keep_id)) = keep_from else {
                    return Ok(Vec::new());
                };
                let first: String = c.query_row(
                    "SELECT id FROM project_controller_decisions WHERE project_id = ?1 \
                     ORDER BY created_at, id LIMIT 1",
                    [project],
                    |r| r.get(0),
                )?;
                select_rows(
                    c,
                    &format!(
                        "SELECT * FROM project_controller_decisions WHERE project_id = ?1 AND {DECISION_GUARD} \
                         AND created_at < ?2 AND (created_at, id) < (?3, ?4) AND id <> ?5 \
                         ORDER BY created_at, id LIMIT ?6"
                    ),
                    &[&project, &cut, &keep_at, &keep_id, &first, &lim],
                    "id",
                )
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn retention_delete_batch(
        &self,
        table: &str,
        rows: &[Candidate],
        cut: &str,
        archive_dir: &Path,
        actor: &str,
        now: OffsetDateTime,
        t: &mut RetentionTableReport,
    ) -> Result<u64, StoreError> {
        let first = rows.first().map(|c| key_text(&c.key));
        let last = rows.last().map(|c| key_text(&c.key));
        let stem = format!(
            "{table}-{}-{}-{}",
            now.unix_timestamp_nanos(),
            safe(first.as_deref().unwrap_or("")),
            safe(last.as_deref().unwrap_or(""))
        );
        let (path, sha) = write_archive(archive_dir, &stem, table, rows)?;
        #[cfg(test)]
        if FAIL_AFTER_ARCHIVE.with(|f| f.get()) {
            return Err(StoreError::Invalid("test: crash after archive".into()));
        }
        let guard = match table {
            "project_wakes" => WAKE_GUARD,
            "project_controller_decisions" => DECISION_GUARD,
            _ => COMPARE_GUARD,
        };
        let at = format_rfc3339(now)?;
        let path_text = path.display().to_string();
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO project_retention_permit (table_name) VALUES (?1)",
            [table],
        )?;
        let mut deleted = 0u64;
        {
            let mut stmt = tx.prepare(&format!("DELETE FROM {table} WHERE id = ?1 AND {guard}"))?;
            for c in rows {
                deleted += stmt.execute([&c.key])? as u64;
            }
        }
        tx.execute(
            "DELETE FROM project_retention_permit WHERE table_name = ?1",
            [table],
        )?;
        tx.execute(
            "INSERT INTO project_retention_runs (ts, actor, table_name, archived, deleted, first_key, \
             last_key, cutoff, archive_path, archive_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                at,
                actor,
                table,
                rows.len() as i64,
                deleted as i64,
                first,
                last,
                cut,
                path_text,
                sha
            ],
        )?;
        tx.commit()?;
        t.archive_paths.push(path_text);
        Ok(deleted)
    }

    /// 表の行数・DB の page 数・新しい順の GC の記録 10 件。
    pub fn retention_stats(&self) -> Result<RetentionStats, StoreError> {
        self.with_read_conn(|c| {
            let n = |sql: &str| -> Result<u64, StoreError> {
                Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?.max(0) as u64)
            };
            let mut s = RetentionStats {
                wakes_total: n("SELECT COUNT(*) FROM project_wakes")?,
                wakes_pending: n(
                    "SELECT COUNT(*) FROM project_wakes WHERE consumed_by IS NULL",
                )?,
                wakes_consumed_cron: n(&format!(
                    "SELECT COUNT(*) FROM project_wakes WHERE {WAKE_GUARD}"
                ))?,
                decisions_total: n("SELECT COUNT(*) FROM project_controller_decisions")?,
                compare_log_total: n("SELECT COUNT(*) FROM script_compare_log")?,
                compare_log_ts_applied: n(&format!(
                    "SELECT COUNT(*) FROM script_compare_log WHERE NOT ({COMPARE_GUARD})"
                ))?,
                page_size: n("PRAGMA page_size")?,
                page_count: n("PRAGMA page_count")?,
                freelist_count: n("PRAGMA freelist_count")?,
                ..Default::default()
            };
            let mut st = c.prepare(
                "SELECT effect_state, COUNT(*) FROM project_controller_decisions GROUP BY effect_state",
            )?;
            for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (k, v) = row?;
                s.decisions_by_effect.insert(k, v.max(0) as u64);
            }
            let mut st = c.prepare(
                "SELECT id, ts, actor, table_name, archived, deleted, first_key, last_key, cutoff, \
                 archive_path, archive_sha256 FROM project_retention_runs ORDER BY id DESC LIMIT 10",
            )?;
            for row in st.query_map([], |r| {
                Ok(RetentionRun {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    actor: r.get(2)?,
                    table_name: r.get(3)?,
                    archived: r.get::<_, i64>(4)?.max(0) as u64,
                    deleted: r.get::<_, i64>(5)?.max(0) as u64,
                    first_key: r.get(6)?,
                    last_key: r.get(7)?,
                    cutoff: r.get(8)?,
                    archive_path: r.get(9)?,
                    archive_sha256: r.get(10)?,
                })
            })? {
                s.last_runs.push(row?);
            }
            Ok(s)
        })
    }

    /// 案件 1 件の wake・判断の行数。
    pub fn retention_project_counts(
        &self,
        project_id: &str,
    ) -> Result<ProjectRetentionCounts, StoreError> {
        self.with_read_conn(|c| {
            let n = |sql: &str| -> Result<u64, StoreError> {
                Ok(c.query_row(sql, [project_id], |r| r.get::<_, i64>(0))?.max(0) as u64)
            };
            Ok(ProjectRetentionCounts {
                wakes_total: n("SELECT COUNT(*) FROM project_wakes WHERE project_id = ?1")?,
                wakes_pending: n(
                    "SELECT COUNT(*) FROM project_wakes WHERE project_id = ?1 AND consumed_by IS NULL",
                )?,
                decisions_total: n(
                    "SELECT COUNT(*) FROM project_controller_decisions WHERE project_id = ?1",
                )?,
                decisions_effect_none: n(
                    "SELECT COUNT(*) FROM project_controller_decisions \
                     WHERE project_id = ?1 AND effect_state = 'none'",
                )?,
                decisions_pending: n(
                    "SELECT COUNT(*) FROM project_controller_decisions \
                     WHERE project_id = ?1 AND effect_state = 'pending'",
                )?,
            })
        })
    }
}
