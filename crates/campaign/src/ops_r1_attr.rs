//! `ops-r1-attribution`: 測定 daemon の DB の写しから B の 1 試行の消費を run・CoS turn ごとに帰属させて出す
//! 読み取り専用の export（設計 `docs/ops/ops-r1-trace-attribution.md` §1・§3・§4、
//! ADR 2026-10-29-ops-r1-trace-attribution-export D4・D5）。
//!
//! 試行の解決（受付 message → 起票の root 群 → 子孫）は `ops_r1_trace` と同じ関数を使う。DB は読み取り専用で開き、
//! migration を走らせない。帰属できない行も捨てずに `trial_id: null` と `reason` で出し、帰属行と未帰属行の合計が
//! DB の候補行の全量になる。監督 CoS（測定 daemon の外）の行は DB に無いので入らない。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::ops_r1_trace::{
    DbIdentity, TraceArgs, TraceError, created_stage, fmt_ts, intake_record_message, intake_stage,
    open_copy, parse_ts, project_thread, trial_tasks,
};

pub const SCHEMA: &str = "ops-r1-attribution/2";

const USAGE: &str = "usage: celeris-campaign ops-r1-attribution --db <DB の写し> --thread <案件 CoS thread id> \
--trial-id <id> --condition-id <id> --cutoff <RFC3339 UTC> [--intake-record <ops-r1-intake の JSON>] --out <attribution.json>";

fn read_err(e: impl std::fmt::Display) -> TraceError {
    TraceError::Read(e.to_string())
}

/// 引数（設計 §1）。trace と同じ入力に `--condition-id`（必須）を足す。
#[derive(Debug, Clone)]
pub struct AttrArgs {
    pub db: PathBuf,
    pub thread: String,
    pub trial_id: String,
    pub condition_id: String,
    pub cutoff: String,
    pub intake_record: Option<PathBuf>,
    pub out: PathBuf,
}

impl AttrArgs {
    /// `args[0]` は subcommand 名（`ops-r1-attribution`）。
    pub fn parse(args: &[String]) -> Result<Self, TraceError> {
        let base =
            TraceArgs::parse(args).map_err(|e| TraceError::Input(format!("{e}\n{USAGE}")))?;
        let condition_id = args
            .iter()
            .position(|a| a == "--condition-id")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or_else(|| TraceError::Input(format!("--condition-id is required\n{USAGE}")))?;
        Ok(Self {
            db: base.db,
            thread: base.thread,
            trial_id: base.trial_id,
            condition_id,
            cutoff: base.cutoff,
            intake_record: base.intake_record,
            out: base.out,
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Window {
    pub from: Option<String>,
    pub to: String,
}

/// 1 run（`runs`）か 1 CoS turn（`chat_runs`）の行（設計 §3）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub source: &'static str,
    pub run_id: String,
    pub task_id: Option<String>,
    pub thread_id: Option<String>,
    pub trial_id: Option<String>,
    pub condition_id: Option<String>,
    pub role: Option<String>,
    pub role_raw: Option<String>,
    pub model: Option<String>,
    pub harness: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub wall_ms: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_basis: Option<&'static str>,
    pub usage_reason: Option<&'static str>,
    pub time_reason: Option<&'static str>,
    pub reason: Option<&'static str>,
}

/// null の数（合計に入れなかった行の数）。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct NullCount {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub wall_ms: u64,
    pub cost_usd: u64,
}

/// 行の合計。null は合計に入れず `null_count` に数える。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Sum {
    pub rows: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub wall_ms: i64,
    pub cost_usd: f64,
    pub null_count: NullCount,
}

impl Sum {
    fn add(&mut self, r: &Row) {
        fn put(sum: &mut u64, nulls: &mut u64, v: Option<u64>) {
            match v {
                Some(v) => *sum += v,
                None => *nulls += 1,
            }
        }
        self.rows += 1;
        let n = &mut self.null_count;
        put(&mut self.input_tokens, &mut n.input_tokens, r.input_tokens);
        put(
            &mut self.output_tokens,
            &mut n.output_tokens,
            r.output_tokens,
        );
        put(
            &mut self.cache_read_tokens,
            &mut n.cache_read_tokens,
            r.cache_read_tokens,
        );
        put(
            &mut self.cache_write_tokens,
            &mut n.cache_write_tokens,
            r.cache_write_tokens,
        );
        match r.wall_ms {
            Some(v) => self.wall_ms += v,
            None => n.wall_ms += 1,
        }
        match r.cost_usd {
            Some(v) => self.cost_usd += v,
            None => n.cost_usd += 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Attribution {
    pub schema: &'static str,
    pub trial_id: String,
    pub condition_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub cutoff: String,
    pub window: Window,
    pub root_task_ids: Vec<String>,
    pub db: DbIdentity,
    pub rows: Vec<Row>,
    /// 帰属行の role ごとの合計（role が決まらない行は `other:<role_raw>`）。
    pub totals: BTreeMap<String, Sum>,
    /// 未帰属行の reason ごとの合計。
    pub unattributed: BTreeMap<String, Sum>,
}

impl Attribution {
    /// 固定の整形（struct の欄順、末尾に改行 1 つ。設計 §4）。
    pub fn render(&self) -> Result<String, TraceError> {
        let mut s = serde_json::to_string_pretty(self).map_err(read_err)?;
        s.push('\n');
        Ok(s)
    }
}

/// `usage_json` の 4 欄と cost（設計 §3）。
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    cost: Option<f64>,
    reason: Option<&'static str>,
}

fn usage(raw: Option<&str>) -> Usage {
    let empty = |reason| Usage {
        input: None,
        output: None,
        cache_read: None,
        cache_write: None,
        cost: None,
        reason: Some(reason),
    };
    let Some(raw) = raw else {
        return empty("usage_missing");
    };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return empty("usage_invalid");
    };
    if !v.is_object() {
        return empty("usage_invalid");
    }
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    Usage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_tokens"),
        cache_write: n("cache_creation_tokens"),
        cost: v.get("cost_usd").and_then(Value::as_f64),
        reason: None,
    }
}

/// 候補の行の素（帰属を決める前）。
struct Raw {
    source: &'static str,
    run_id: String,
    task_id: Option<String>,
    thread_id: Option<String>,
    role_raw: Option<String>,
    model: Option<String>,
    harness: Option<String>,
    started: Option<String>,
    finished: Option<String>,
    usage_json: Option<String>,
    /// chat run の thread の kind（無い thread は None）。
    thread_kind: Option<String>,
    input_message_id: Option<String>,
}

fn runs_rows(conn: &Connection) -> Result<Vec<Raw>, TraceError> {
    let mut stmt = conn
        .prepare("SELECT run_id, task_id, role, started_at, finished_at, usage_json, model, adapter FROM runs")
        .map_err(read_err)?;
    stmt.query_map([], |r| {
        Ok(Raw {
            model: r.get(6)?,
            harness: r.get(7)?,
            source: "runs",
            run_id: r.get(0)?,
            task_id: Some(r.get(1)?),
            thread_id: None,
            role_raw: Some(r.get(2)?),
            started: Some(r.get(3)?),
            finished: r.get(4)?,
            usage_json: r.get(5)?,
            thread_kind: None,
            input_message_id: None,
        })
    })
    .map_err(read_err)?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(read_err)
}

fn chat_rows(conn: &Connection) -> Result<Vec<Raw>, TraceError> {
    let mut stmt = conn
        .prepare(
            "SELECT r.run_id, r.thread_id, t.kind, r.started_at, r.finished_at, r.usage_json, r.input_message_id, \
             json_extract(CASE WHEN json_valid(r.resolved_config_json) THEN r.resolved_config_json END,'$.model'), \
             json_extract(CASE WHEN json_valid(r.resolved_config_json) THEN r.resolved_config_json END,'$.harness') \
             FROM chat_runs r LEFT JOIN chat_threads t ON t.id=r.thread_id",
        )
        .map_err(read_err)?;
    stmt.query_map([], |r| {
        let kind: Option<String> = r.get(2)?;
        Ok(Raw {
            // CoS の model・harness は run が解決した設定（`resolved_config_json`）にある。
            model: r.get(7)?,
            harness: r.get(8)?,
            source: "chat_runs",
            run_id: r.get(0)?,
            task_id: None,
            thread_id: Some(r.get(1)?),
            role_raw: kind.clone(),
            started: r.get(3)?,
            finished: r.get(4)?,
            usage_json: r.get(5)?,
            thread_kind: kind,
            input_message_id: r.get(6)?,
        })
    })
    .map_err(read_err)?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(read_err)
}

fn role_of(raw: &Raw) -> Option<String> {
    let r = raw.role_raw.as_deref()?;
    let mapped = match (raw.source, r) {
        ("runs", "planner" | "worker" | "reviewer") => r,
        ("chat_runs", "project") => "cos_project",
        ("chat_runs", "general" | "review") => "cos_general",
        _ => return None,
    };
    Some(mapped.to_owned())
}

/// 試行の参照（帰属の規則 3〜6 が使う）。
struct Trial {
    thread: String,
    intake_at: OffsetDateTime,
    tasks: BTreeSet<String>,
    /// 試行の root の `root_done_summaries.input_message_id`。
    done_inputs: BTreeSet<String>,
    /// 試行の task か案件を対象にした `cos_operations` の run_id。
    op_runs: BTreeSet<String>,
}

impl Trial {
    fn load(
        conn: &Connection,
        thread: &str,
        project: &str,
        intake_at: OffsetDateTime,
        roots: &[String],
    ) -> Result<Self, TraceError> {
        let tasks: BTreeSet<String> = trial_tasks(conn, roots)?.into_iter().collect();
        let mut done_inputs = BTreeSet::new();
        let mut stmt = conn
            .prepare(
                "SELECT input_message_id FROM root_done_summaries WHERE task_id=?1 AND input_message_id IS NOT NULL",
            )
            .map_err(read_err)?;
        for root in roots {
            for id in stmt
                .query_map([root], |r| r.get::<_, String>(0))
                .map_err(read_err)?
            {
                done_inputs.insert(id.map_err(read_err)?);
            }
        }
        let mut op_runs = BTreeSet::new();
        let mut stmt = conn
            .prepare(
                "SELECT run_id, target_kind, target_id, CASE WHEN json_valid(result_json) \
                 AND json_type(result_json,'$.task_id')='text' THEN json_extract(result_json,'$.task_id') END \
                 FROM cos_operations",
            )
            .map_err(read_err)?;
        let ops = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(read_err)?;
        for op in ops {
            let (run, kind, target, result_task) = op.map_err(read_err)?;
            let hit = (kind == "task" && tasks.contains(&target))
                || (kind == "project" && target == project)
                || result_task.is_some_and(|t| tasks.contains(&t));
            if hit {
                op_runs.insert(run);
            }
        }
        Ok(Self {
            thread: thread.to_owned(),
            intake_at,
            tasks,
            done_inputs,
            op_runs,
        })
    }

    /// 帰属の規則 2〜7（設計 §3）。帰属なら None、未帰属なら reason。
    fn reason(&self, raw: &Raw, started: Option<OffsetDateTime>) -> Option<&'static str> {
        if started.is_some_and(|t| t < self.intake_at) {
            return Some("before_intake");
        }
        if raw.source == "runs" {
            return match &raw.task_id {
                Some(t) if self.tasks.contains(t) => None,
                _ => Some("task_not_in_trial_tree"),
            };
        }
        if raw.thread_id.as_deref() == Some(self.thread.as_str()) {
            return None;
        }
        match raw.thread_kind.as_deref() {
            Some("project") => Some("other_project_thread"),
            Some("general" | "review") => {
                let referenced = self.op_runs.contains(&raw.run_id)
                    || raw
                        .input_message_id
                        .as_ref()
                        .is_some_and(|m| self.done_inputs.contains(m));
                (!referenced).then_some("no_trial_reference")
            }
            _ => Some("other_thread"),
        }
    }
}

/// 時刻を読む。NULL は `not_started`（始まっていない chat run）、parse できない値は `bad_timestamp`。
fn read_time(raw: Option<&str>) -> (Option<OffsetDateTime>, Option<&'static str>) {
    match raw {
        None => (None, Some("not_started")),
        Some(s) => match parse_ts(s) {
            Some(t) => (Some(t), None),
            None => (None, Some("bad_timestamp")),
        },
    }
}

/// 帰属表を組み立てる。
pub fn build(args: &AttrArgs) -> Result<Attribution, TraceError> {
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
    let trial = match intake.at {
        Some(at) if !created.roots.is_empty() => Some(Trial::load(
            &conn,
            &args.thread,
            &project_id,
            at,
            &created.roots,
        )?),
        _ => None,
    };

    let mut keyed = Vec::new();
    for raw in runs_rows(&conn)?.into_iter().chain(chat_rows(&conn)?) {
        let (started, start_reason) = read_time(raw.started.as_deref());
        // cutoff より後に始まった行は候補に入れない。
        if started.is_some_and(|t| t > cutoff) {
            continue;
        }
        let (finished, finish_reason) = match raw.finished.as_deref() {
            None => (None, None),
            Some(s) => match parse_ts(s) {
                Some(t) => (Some(t), None),
                None => (None, Some("bad_timestamp")),
            },
        };
        let wall_ms = match (started, finished) {
            (Some(s), Some(f)) => {
                Some(i64::try_from((f - s).whole_milliseconds()).unwrap_or(i64::MAX))
            }
            _ => None,
        };
        let reason = match &trial {
            None => Some("trial_roots_unknown"),
            Some(t) => t.reason(&raw, started),
        };
        let u = usage(raw.usage_json.as_deref());
        let attributed = reason.is_none();
        let row = Row {
            source: raw.source,
            role: role_of(&raw),
            role_raw: raw.role_raw.clone(),
            model: raw.model.clone(),
            harness: raw.harness.clone(),
            run_id: raw.run_id,
            task_id: raw.task_id,
            thread_id: raw.thread_id,
            trial_id: attributed.then(|| args.trial_id.clone()),
            condition_id: attributed.then(|| args.condition_id.clone()),
            started_at: started.map(fmt_ts),
            finished_at: finished.map(fmt_ts),
            input_tokens: u.input,
            output_tokens: u.output,
            cache_read_tokens: u.cache_read,
            cache_write_tokens: u.cache_write,
            wall_ms,
            cost_basis: u.cost.map(|_| "pricing_estimate"),
            cost_usd: u.cost,
            usage_reason: u.reason,
            time_reason: start_reason.or(finish_reason),
            reason,
        };
        keyed.push((started, row));
    }
    // (started_at, source, run_id)。時刻の無い行は末尾で (source, run_id)。
    keyed.sort_by(|a, b| {
        (a.0.is_none(), a.0, a.1.source, &a.1.run_id).cmp(&(
            b.0.is_none(),
            b.0,
            b.1.source,
            &b.1.run_id,
        ))
    });
    let rows: Vec<Row> = keyed.into_iter().map(|(_, r)| r).collect();

    let mut totals: BTreeMap<String, Sum> = BTreeMap::new();
    let mut unattributed: BTreeMap<String, Sum> = BTreeMap::new();
    for r in &rows {
        match r.reason {
            None => {
                let key = r.role.clone().unwrap_or_else(|| {
                    format!("other:{}", r.role_raw.as_deref().unwrap_or("unknown"))
                });
                totals.entry(key).or_default().add(r);
            }
            Some(reason) => unattributed.entry(reason.to_owned()).or_default().add(r),
        }
    }

    Ok(Attribution {
        schema: SCHEMA,
        trial_id: args.trial_id.clone(),
        condition_id: args.condition_id.clone(),
        thread_id: args.thread.clone(),
        project_id,
        cutoff: fmt_ts(cutoff),
        window: Window {
            from: intake.at.map(fmt_ts),
            to: fmt_ts(cutoff),
        },
        root_task_ids: created.roots,
        db,
        rows,
        totals,
        unattributed,
    })
}

/// 帰属表を `--out` に上書きし、出力の sha256 を返す。
pub fn run(args: &AttrArgs) -> Result<String, TraceError> {
    let body = build(args)?.render()?;
    std::fs::write(&args.out, &body)
        .map_err(|e| TraceError::Read(format!("--out {}: {e}", args.out.display())))?;
    Ok(format!("{:x}", Sha256::digest(body.as_bytes())))
}

/// CLI の入口。`args[0]` は `ops-r1-attribution`。返り値は exit code（0 = 出力した、1 = 読み取りの失敗、
/// 2 = 入力の誤り）。
pub fn cli(args: &[String]) -> u8 {
    match AttrArgs::parse(args).and_then(|a| run(&a)) {
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
    fn usage_maps_four_fields_and_cost() {
        let u = usage(Some(
            r#"{"input_tokens":5,"output_tokens":6,"cache_read_tokens":7,"cache_creation_tokens":8,"cost_usd":0.5}"#,
        ));
        assert_eq!(
            (
                u.input,
                u.output,
                u.cache_read,
                u.cache_write,
                u.cost,
                u.reason
            ),
            (Some(5), Some(6), Some(7), Some(8), Some(0.5), None)
        );
        assert_eq!(usage(None).reason, Some("usage_missing"));
        assert_eq!(usage(Some("nope")).reason, Some("usage_invalid"));
    }

    #[test]
    fn condition_id_is_required() {
        let args: Vec<String> = [
            "ops-r1-attribution",
            "--db",
            "x",
            "--thread",
            "t",
            "--trial-id",
            "a",
            "--cutoff",
            "2026-10-29T00:00:00Z",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(AttrArgs::parse(&args).unwrap_err().exit_code(), 2);
    }
}
