//! `ops-r1-trace`: 測定 daemon の DB の写しから B の 1 試行の工程 trace（6 段）を出す読み取り専用の export
//! （設計 `docs/ops/ops-r1-trace-attribution.md` §1・§2・§4、ADR 2026-10-29-ops-r1-trace-attribution-export）。
//!
//! DB は `mode=ro&immutable=1` の URI と `SQLITE_OPEN_READ_ONLY` で開き、migration を走らせない
//! （`SqliteStore::open` は使わない）。実行時刻・host 名・`--db` の path は出さず、同じ DB から 2 回出せば
//! bytes が一致する。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

/// 読む表・欄を確かめた schema 版（release 94f86928dc13）。
pub const SCHEMA_VERSION: i64 = 102;
pub const SCHEMA: &str = "ops-r1-trace/1";

const USAGE: &str = "usage: celeris-campaign ops-r1-trace --db <DB の写し> --thread <案件 CoS thread id> \
--trial-id <id> --cutoff <RFC3339 UTC> [--intake-record <ops-r1-intake の JSON>] --out <trace.json>";

/// 失敗の種類。exit code は `Input` = 2、`Read` = 1（設計 §1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    Input(String),
    Read(String),
}

impl TraceError {
    pub fn exit_code(&self) -> u8 {
        match self {
            TraceError::Input(_) => 2,
            TraceError::Read(_) => 1,
        }
    }
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceError::Input(m) => write!(f, "input: {m}"),
            TraceError::Read(m) => write!(f, "read: {m}"),
        }
    }
}

fn read_err(e: impl std::fmt::Display) -> TraceError {
    TraceError::Read(e.to_string())
}

/// 引数（設計 §1）。
#[derive(Debug, Clone)]
pub struct TraceArgs {
    pub db: PathBuf,
    pub thread: String,
    pub trial_id: String,
    pub cutoff: String,
    pub intake_record: Option<PathBuf>,
    pub out: PathBuf,
}

impl TraceArgs {
    /// `args[0]` は subcommand 名（`ops-r1-trace`）。
    pub fn parse(args: &[String]) -> Result<Self, TraceError> {
        let get = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let need = |name: &str| -> Result<String, TraceError> {
            get(name).ok_or_else(|| TraceError::Input(format!("{name} is required\n{USAGE}")))
        };
        Ok(Self {
            db: need("--db")?.into(),
            thread: need("--thread")?,
            trial_id: need("--trial-id")?,
            cutoff: need("--cutoff")?,
            intake_record: get("--intake-record").map(PathBuf::from),
            out: need("--out")?.into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DbIdentity {
    pub schema_version: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Stage {
    pub stage: &'static str,
    pub at: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<Value>,
    pub reason: Option<String>,
}

impl Stage {
    fn found(stage: &'static str, at: String, reference: Value) -> Self {
        Self {
            stage,
            at: Some(at),
            reference: Some(reference),
            reason: None,
        }
    }
    fn missing(stage: &'static str, reason: impl Into<String>) -> Self {
        Self {
            stage,
            at: None,
            reference: None,
            reason: Some(reason.into()),
        }
    }
    fn is_found(&self) -> bool {
        self.reason.is_none()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Trace {
    pub schema: &'static str,
    pub trial_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub db: DbIdentity,
    pub cutoff: String,
    pub stages: Vec<Stage>,
}

impl Trace {
    /// 固定の整形（struct の欄順、末尾に改行 1 つ。設計 §4）。
    pub fn render(&self) -> Result<String, TraceError> {
        let mut s = serde_json::to_string_pretty(self).map_err(read_err)?;
        s.push('\n');
        Ok(s)
    }
}

/// RFC3339 を parse する（秒止めと小数付きが混ざる。設計 §4）。
pub fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// UTC の `YYYY-MM-DDTHH:MM:SS.mmmZ`（小数 3 桁に切り捨て）。
pub fn fmt_ts(t: OffsetDateTime) -> String {
    let t = t.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.nanosecond() / 1_000_000
    )
}

/// SQLite URI の path 部分で意味を持つ文字だけを % で逃がす。
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            '#' => out.push_str("%23"),
            _ => out.push(c),
        }
    }
    out
}

fn wal_path(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

/// DB の写しを読み取り専用で開き、schema 版と sha256 を確かめる（設計 §1）。
pub fn open_copy(db: &Path) -> Result<(Connection, DbIdentity), TraceError> {
    if !db.is_file() {
        return Err(TraceError::Input(format!(
            "--db {}: not a file",
            db.display()
        )));
    }
    if wal_path(db).exists() {
        return Err(TraceError::Input(format!(
            "--db {}: a -wal file exists; make a single-file copy with sqlite3 .backup",
            db.display()
        )));
    }
    let bytes =
        std::fs::read(db).map_err(|e| TraceError::Read(format!("{}: {e}", db.display())))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let conn = Connection::open_with_flags(
        format!("file:{}?mode=ro&immutable=1", uri_path(db)),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(read_err)?;
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| TraceError::Input(format!("--db {}: {e}", db.display())))?;
    if !has_table {
        return Err(TraceError::Input(format!(
            "--db {}: no schema_migrations table",
            db.display()
        )));
    }
    let schema_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .map_err(read_err)?;
    if schema_version != SCHEMA_VERSION {
        return Err(TraceError::Input(format!(
            "--db {}: schema version {schema_version}, expected {SCHEMA_VERSION}",
            db.display()
        )));
    }
    Ok((
        conn,
        DbIdentity {
            schema_version,
            sha256,
        },
    ))
}

/// `--thread` が案件の CoS thread なら、その案件 id を返す。
pub fn project_thread(conn: &Connection, thread: &str) -> Result<String, TraceError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT kind, project_id FROM chat_threads WHERE id=?1",
            [thread],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(read_err)?;
    match row {
        None => Err(TraceError::Input(format!("--thread {thread}: not found"))),
        Some((kind, project)) if kind == "project" => {
            project.ok_or_else(|| TraceError::Input(format!("--thread {thread}: no project_id")))
        }
        Some((kind, _)) => Err(TraceError::Input(format!(
            "--thread {thread}: kind is {kind}, expected project"
        ))),
    }
}

/// `--intake-record` の JSON から受付 message の id を取る。`thread_id`・`trial_id` が書いてあれば
/// 引数と照合する（食い違いは入力の誤り）。
pub fn intake_record_message(
    path: &Path,
    thread: &str,
    trial_id: &str,
) -> Result<String, TraceError> {
    let raw = std::fs::read(path)
        .map_err(|e| TraceError::Input(format!("--intake-record {}: {e}", path.display())))?;
    let v: Value = serde_json::from_slice(&raw)
        .map_err(|e| TraceError::Input(format!("--intake-record {}: {e}", path.display())))?;
    for (key, want) in [("thread_id", thread), ("trial_id", trial_id)] {
        if let Some(got) = v.get(key).and_then(Value::as_str)
            && got != want
        {
            return Err(TraceError::Input(format!(
                "--intake-record {}: {key} {got} does not match {want}",
                path.display()
            )));
        }
    }
    v.get("message_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            TraceError::Input(format!(
                "--intake-record {}: message_id is missing",
                path.display()
            ))
        })
}

/// 受付の段の結果（後の段が使う）。
pub struct Intake {
    pub stage: Stage,
    pub at: Option<OffsetDateTime>,
}

pub fn intake_stage(
    conn: &Connection,
    thread: &str,
    trial_id: &str,
    record_message: Option<&str>,
    cutoff: OffsetDateTime,
) -> Result<Intake, TraceError> {
    const NAME: &str = "intake";
    type Row = (String, i64, Option<String>, String, Option<String>, String);
    let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Row> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    };
    const COLS: &str =
        "id, seq, client_message_id, role, json_extract(metadata_json,'$.author'), created_at";
    let row: Option<Row> = match record_message {
        Some(id) => conn
            .query_row(
                &format!("SELECT {COLS} FROM chat_messages WHERE thread_id=?1 AND id=?2"),
                params![thread, id],
                map,
            )
            .optional(),
        None => conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2"
                ),
                params![thread, format!("ops-r1-intake:{trial_id}")],
                map,
            )
            .optional(),
    }
    .map_err(read_err)?;
    let missing = |reason: &str| Intake {
        stage: Stage::missing(NAME, reason),
        at: None,
    };
    let Some((id, seq, client_message_id, role, author, created_at)) = row else {
        return Ok(missing("intake_message_not_found"));
    };
    let Some(at) = parse_ts(&created_at) else {
        return Ok(missing("bad_timestamp"));
    };
    if at > cutoff {
        return Ok(missing("intake_after_cutoff"));
    }
    if role != "user" || !author.as_deref().is_some_and(|a| a.starts_with("human:")) {
        return Ok(missing("intake_not_human"));
    }
    let cos_run_id: Option<String> = conn
        .query_row(
            "SELECT run_id FROM chat_runs WHERE input_message_id=?1 ORDER BY run_id LIMIT 1",
            [&id],
            |r| r.get(0),
        )
        .optional()
        .map_err(read_err)?;
    Ok(Intake {
        stage: Stage::found(
            NAME,
            fmt_ts(at),
            json!({
                "message_id": id,
                "seq": seq,
                "client_message_id": client_message_id,
                "author": author,
                "cos_run_id": cos_run_id,
            }),
        ),
        at: Some(at),
    })
}

/// 起票の段の結果。`roots` は最初の起票の task と同じ窓の 2 件目以降（順に）。
pub struct Created {
    pub stage: Stage,
    pub roots: Vec<String>,
}

pub fn created_stage(
    conn: &Connection,
    thread: &str,
    intake_at: Option<OffsetDateTime>,
    cutoff: OffsetDateTime,
) -> Result<Created, TraceError> {
    const NAME: &str = "created";
    let Some(from) = intake_at else {
        return Ok(Created {
            stage: Stage::missing(NAME, "upstream_missing:intake"),
            roots: vec![],
        });
    };
    let mut stmt = conn
        .prepare(
            "SELECT id, run_id, target_kind, target_id, state, created_at FROM cos_operations \
             WHERE thread_id=?1 AND action='task.create'",
        )
        .map_err(read_err)?;
    let mut ops: Vec<(OffsetDateTime, String, String, String, String, String)> = stmt
        .query_map([thread], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?
        .into_iter()
        .filter_map(|(id, run, kind, target, state, at)| {
            let t = parse_ts(&at)?;
            (t >= from && t <= cutoff).then_some((t, id, run, kind, target, state))
        })
        .collect();
    ops.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    if ops.is_empty() {
        return Ok(Created {
            stage: Stage::missing(NAME, "no_task_create_operation"),
            roots: vec![],
        });
    }
    let applied: Vec<_> = ops
        .into_iter()
        .filter(|o| o.5 == "applied" && o.3 == "task")
        .collect();
    let Some((at, op_id, run_id, _, root, _)) = applied.first().cloned() else {
        return Ok(Created {
            stage: Stage::missing(NAME, "task_create_not_applied"),
            roots: vec![],
        });
    };
    let audit_event_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM events WHERE task_id=?1 AND json_extract(json,'$.type')='cos_operation' \
             AND json_extract(json,'$.operation_id')=?2 ORDER BY id LIMIT 1",
            params![root, op_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(read_err)?;
    let created_by: Option<String> = conn
        .query_row(
            "SELECT json_extract(json,'$.created_by') FROM events WHERE task_id=?1 \
             AND json_extract(json,'$.type')='created' ORDER BY seq LIMIT 1",
            [&root],
            |r| r.get(0),
        )
        .optional()
        .map_err(read_err)?
        .flatten();
    let roots: Vec<String> = applied.iter().map(|o| o.4.clone()).collect();
    Ok(Created {
        stage: Stage::found(
            NAME,
            fmt_ts(at),
            json!({
                "operation_id": op_id,
                "cos_run_id": run_id,
                "audit_event_id": audit_event_id,
                "root_task_id": root,
                "created_by": created_by,
                "extra_root_task_ids": roots[1..].to_vec(),
            }),
        ),
        roots,
    })
}

/// root 群とその子孫の task id（昇順、重複なし）。
pub fn trial_tasks(conn: &Connection, roots: &[String]) -> Result<Vec<String>, TraceError> {
    let mut all = std::collections::BTreeSet::new();
    let mut stmt = conn
        .prepare(
            "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION SELECT t.id FROM tasks t JOIN tree ON t.parent_id=tree.id) \
             SELECT id FROM tree",
        )
        .map_err(read_err)?;
    for root in roots {
        for id in stmt
            .query_map([root], |r| r.get::<_, String>(0))
            .map_err(read_err)?
        {
            all.insert(id.map_err(read_err)?);
        }
    }
    Ok(all.into_iter().collect())
}

pub fn executed_stage(
    conn: &Connection,
    roots: &[String],
    cutoff: OffsetDateTime,
) -> Result<Stage, TraceError> {
    const NAME: &str = "executed";
    if roots.is_empty() {
        return Ok(Stage::missing(NAME, "upstream_missing:created"));
    }
    let tasks = trial_tasks(conn, roots)?;
    let mut stmt = conn
        .prepare("SELECT run_id, task_id, role, status, started_at, finished_at FROM runs WHERE task_id=?1")
        .map_err(read_err)?;
    let mut runs = Vec::new();
    for task in &tasks {
        let rows = stmt
            .query_map([task], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(read_err)?;
        for row in rows {
            let (run_id, task_id, role, status, started, finished) = row.map_err(read_err)?;
            let started = parse_ts(&started);
            // cutoff より後に始まった run は無いものとして扱う。
            if started.is_some_and(|t| t > cutoff) {
                continue;
            }
            let finished = finished
                .as_deref()
                .and_then(parse_ts)
                .filter(|t| *t <= cutoff);
            runs.push((started, run_id, task_id, role, status, finished));
        }
    }
    if runs.is_empty() {
        return Ok(Stage::missing(NAME, "no_runs"));
    }
    // started_at, run_id の順。時刻の無い run は末尾。
    runs.sort_by(|a, b| (a.0.is_none(), a.0, &a.1).cmp(&(b.0.is_none(), b.0, &b.1)));
    let Some(first) = runs.iter().filter_map(|r| r.0).min() else {
        return Ok(Stage::missing(NAME, "bad_timestamp"));
    };
    let until = runs.iter().filter_map(|r| r.5).max().map(fmt_ts);
    let runs: Vec<Value> = runs
        .into_iter()
        .map(|(started, run_id, task_id, role, status, finished)| {
            json!({
                "run_id": run_id,
                "task_id": task_id,
                "role": role,
                "status": status,
                "started_at": started.map(fmt_ts),
                "finished_at": finished.map(fmt_ts),
            })
        })
        .collect();
    Ok(Stage::found(
        NAME,
        fmt_ts(first),
        json!({"task_ids": tasks, "runs": runs, "until": until}),
    ))
}

/// 成果確定の段の結果。`done_seq` は done で終わったときの遷移 event の seq。
pub struct Outcome {
    pub stage: Stage,
    pub done_seq: Option<i64>,
    pub to: Option<String>,
}

pub fn outcome_stage(
    conn: &Connection,
    root: Option<&str>,
    cutoff: OffsetDateTime,
) -> Result<Outcome, TraceError> {
    const NAME: &str = "outcome";
    let Some(root) = root else {
        return Ok(Outcome {
            stage: Stage::missing(NAME, "upstream_missing:created"),
            done_seq: None,
            to: None,
        });
    };
    let mut stmt = conn
        .prepare(
            "SELECT id, seq, ts, json_extract(json,'$.type'), json_extract(json,'$.to'), json_extract(json,'$.reason') \
             FROM events WHERE task_id=?1 AND json_extract(json,'$.type') IN \
             ('transitioned','reviewer_passed','required_checks_evaluated','review_verdict') ORDER BY id",
        )
        .map_err(read_err)?;
    let rows = stmt
        .query_map([root], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(read_err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(read_err)?;
    let mut terminal = None;
    let (mut passed, mut checks, mut verdicts) = (Vec::new(), Vec::new(), 0u64);
    for (id, seq, ts, kind, to, reason) in rows {
        let Some(t) = parse_ts(&ts).filter(|t| *t <= cutoff) else {
            continue;
        };
        match kind.as_str() {
            "transitioned" if matches!(to.as_deref(), Some("done" | "failed" | "cancelled")) => {
                terminal = Some((t, id, seq, to, reason));
            }
            "reviewer_passed" => passed.push(id),
            "required_checks_evaluated" => checks.push(id),
            "review_verdict" => verdicts += 1,
            _ => {}
        }
    }
    let Some((t, id, seq, to, reason)) = terminal else {
        return Ok(Outcome {
            stage: Stage::missing(NAME, "not_terminal_by_cutoff"),
            done_seq: None,
            to: None,
        });
    };
    Ok(Outcome {
        stage: Stage::found(
            NAME,
            fmt_ts(t),
            json!({
                "task_id": root,
                "event_id": id,
                "seq": seq,
                "to": to,
                "reason": reason,
                "reviewer_passed": passed,
                "required_checks": checks,
                "review_verdicts": verdicts,
            }),
        ),
        done_seq: (to.as_deref() == Some("done")).then_some(seq),
        to,
    })
}

/// 報告の段の結果。
pub struct Report {
    pub stage: Stage,
}

pub fn report_stage(
    conn: &Connection,
    root: Option<&str>,
    outcome: &Outcome,
    cutoff: OffsetDateTime,
) -> Result<Report, TraceError> {
    const NAME: &str = "report";
    let missing = |reason: &str| {
        Ok(Report {
            stage: Stage::missing(NAME, reason),
        })
    };
    let (Some(root), Some(_)) = (root, outcome.to.as_deref()) else {
        return missing("upstream_missing:outcome");
    };
    let Some(done_seq) = outcome.done_seq else {
        return missing("root_not_done");
    };
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT state, thread_id, input_message_id FROM root_done_summaries WHERE task_id=?1 AND done_seq=?2",
            params![root, done_seq],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(read_err)?;
    let Some((state, thread_id, input)) = row else {
        return missing("report_not_queued");
    };
    if state == "fallback" {
        return missing("report_fallback");
    }
    let Some(input) = input else {
        return missing("report_not_queued");
    };
    let run: Option<(String, String)> = conn
        .query_row(
            "SELECT r.run_id, r.output_message_id FROM chat_runs r JOIN chat_messages m ON m.id=r.output_message_id \
             WHERE r.input_message_id=?1 AND r.state='completed' AND m.role='assistant' ORDER BY r.run_id LIMIT 1",
            [&input],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(read_err)?;
    let Some((run_id, message_id)) = run else {
        return missing("report_run_not_completed");
    };
    let created: String = conn
        .query_row(
            "SELECT created_at FROM chat_messages WHERE id=?1",
            [&message_id],
            |r| r.get(0),
        )
        .map_err(read_err)?;
    let Some(at) = parse_ts(&created) else {
        return missing("bad_timestamp");
    };
    if at > cutoff {
        return missing("report_after_cutoff");
    }
    Ok(Report {
        stage: Stage::found(
            NAME,
            fmt_ts(at),
            json!({
                "thread_id": thread_id,
                "input_message_id": input,
                "chat_run_id": run_id,
                "message_id": message_id,
                "root_done_state": state,
            }),
        ),
    })
}

/// 6 段を組み立てる。
pub fn build(args: &TraceArgs) -> Result<Trace, TraceError> {
    let cutoff = parse_ts(&args.cutoff)
        .ok_or_else(|| TraceError::Input(format!("--cutoff {}: not RFC3339", args.cutoff)))?;
    let (conn, db) = open_copy(&args.db)?;
    let project_id = project_thread(&conn, &args.thread)?;
    let record_message = match &args.intake_record {
        Some(p) => Some(intake_record_message(p, &args.thread, &args.trial_id)?),
        None => None,
    };
    let intake = intake_stage(
        &conn,
        &args.thread,
        &args.trial_id,
        record_message.as_deref(),
        cutoff,
    )?;
    let created = created_stage(&conn, &args.thread, intake.at, cutoff)?;
    let executed = executed_stage(&conn, &created.roots, cutoff)?;
    let root = created.roots.first().map(String::as_str);
    let outcome = outcome_stage(&conn, root, cutoff)?;
    let report = report_stage(&conn, root, &outcome, cutoff)?;
    let report_found = report.stage.is_found();
    let cutoff_stage = Stage::found(
        "cutoff",
        fmt_ts(cutoff),
        json!({
            "source": "argument",
            "report_before_cutoff": report_found.then_some(true),
        }),
    );
    Ok(Trace {
        schema: SCHEMA,
        trial_id: args.trial_id.clone(),
        thread_id: args.thread.clone(),
        project_id,
        db,
        cutoff: fmt_ts(cutoff),
        stages: vec![
            intake.stage,
            created.stage,
            executed,
            outcome.stage,
            report.stage,
            cutoff_stage,
        ],
    })
}

/// trace を `--out` に上書きし、出力の sha256 を返す。
pub fn run(args: &TraceArgs) -> Result<String, TraceError> {
    let body = build(args)?.render()?;
    std::fs::write(&args.out, &body)
        .map_err(|e| TraceError::Read(format!("--out {}: {e}", args.out.display())))?;
    Ok(format!("{:x}", Sha256::digest(body.as_bytes())))
}

/// CLI の入口。`args[0]` は `ops-r1-trace`。返り値は exit code（0 = 出力した、1 = 読み取りの失敗、2 = 入力の誤り）。
pub fn cli(args: &[String]) -> u8 {
    match TraceArgs::parse(args).and_then(|a| run(&a)) {
        Ok(sha) => {
            println!("{sha}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            e.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_normalize_to_utc_millis() {
        let t = parse_ts("2026-10-28T19:00:05.123456+09:00").unwrap();
        assert_eq!(fmt_ts(t), "2026-10-28T10:00:05.123Z");
        assert_eq!(
            fmt_ts(parse_ts("2026-10-28T10:00:05Z").unwrap()),
            "2026-10-28T10:00:05.000Z"
        );
        assert!(parse_ts("yesterday").is_none());
    }

    #[test]
    fn uri_path_escapes_query_chars() {
        assert_eq!(uri_path(Path::new("/a?b#c%d")), "/a%3Fb%23c%25d");
    }

    #[test]
    fn missing_arguments_are_input_errors() {
        let e = TraceArgs::parse(&["ops-r1-trace".into()]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
    }
}
