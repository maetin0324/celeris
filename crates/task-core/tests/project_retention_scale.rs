//! P4-D（第 4 フェーズ）: wake・判断・compare の記録が 30 日（仮想）たまった DB で、query・DB の大きさ・
//! GC の時間を GC の前後で測る（接頭辞 `project_retention_scale_`）。
//!
//! 一時 DB に SQL で直接書く（LLM なし、外部ネットワークなし、本番に触れない、CPU を焼かない）。案件ごとに毎時の
//! cron の wake と効果 `none` の判断を 720 時間分、6 時間ごとに root_done の wake と効果 `applied` の判断を置き、
//! compare の記録を wake・判断の 1/10 置く。行の数は `RETENTION_SCALE_ROWS`（wake + 判断。既定 20,000）で決め、
//! 案件の数は `ROWS / (2 × 720)`。`RETENTION_SCALE_OUT` があれば結果の JSON
//! （`celeris.project-retention-scale/1`）をその file に書く。100 万行は数分かかるので test binary を `--exact` で
//! 直接呼ぶ（`agent-docs/progress/2026-11-05-phase4/p4d.md` の手順）。

use std::time::Instant;

use rusqlite::{Connection, params};
use task_core::SqliteStore;
use task_core::project_retention::RetentionPolicy;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const HOURS: i64 = 720;

fn ts(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap()
}

fn t0() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
}

fn fill(path: &std::path::Path, projects: usize) -> (i64, i64, i64) {
    let mut conn = Connection::open(path).unwrap();
    let tx = conn.transaction().unwrap();
    let (mut wakes, mut decisions, mut compares) = (0i64, 0i64, 0i64);
    {
        let mut w = tx
            .prepare(
                "INSERT INTO project_wakes (project_id, wake_key, kind, created_at, consumed_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap();
        let mut d = tx
            .prepare(
                "INSERT INTO project_controller_decisions (id, project_id, based_on_revision, human_epoch, \
                 snapshot_sha256, request_sha256, mode, proposal_json, validation, effect_json, effect_state, \
                 created_at) VALUES (?1, ?2, ?3, 0, 'snapshot', 'request', 'enabled', \
                 '{\"kind\":\"none\",\"reason\":\"routine\"}', ?4, ?5, ?6, ?7)",
            )
            .unwrap();
        let mut c = tx
            .prepare(
                "INSERT INTO script_compare_log (ts, domain, thread_id, run_id, json) \
                 VALUES (?1, 'cos_digest', ?2, ?3, ?4)",
            )
            .unwrap();
        for p in 0..projects {
            let project = format!("P{p:05}");
            let mut last = String::new();
            for h in 0..HOURS {
                let at = ts(t0() + time::Duration::hours(h));
                let id = format!("D{p:05}-{h:04}");
                w.execute(params![project, format!("cron:j:{h}"), "cron", at, id])
                    .unwrap();
                d.execute(params![
                    id,
                    project,
                    h,
                    "pc/no-op",
                    "{\"kind\":\"none\"}",
                    "none",
                    at
                ])
                .unwrap();
                wakes += 1;
                decisions += 1;
                if h % 6 == 5 {
                    let rid = format!("R{p:05}-{h:04}");
                    w.execute(params![
                        project,
                        format!("root_done:{h}"),
                        "root_done",
                        at,
                        rid
                    ])
                    .unwrap();
                    d.execute(params![
                        rid,
                        project,
                        h,
                        "pc/create-root",
                        "{\"kind\":\"create_root\"}",
                        "applied",
                        at
                    ])
                    .unwrap();
                    wakes += 1;
                    decisions += 1;
                    last = rid;
                } else {
                    last = id;
                }
                if (h as usize + p) % 10 == 0 {
                    let by = if h % 100 == 0 { "ts" } else { "rust" };
                    c.execute(params![
                        at,
                        format!("T{p:05}"),
                        format!("run-{p}-{h}"),
                        format!("{{\"applied_by\":\"{by}\",\"outcome\":\"match\"}}")
                    ])
                    .unwrap();
                    compares += 1;
                }
            }
            tx.execute(
                "INSERT INTO project_controller_state (project_id, mode, phase, last_decision_id, \
                 updated_at, updated_by) VALUES (?1, 'enabled', 'active', ?2, ?3, 'test')",
                params![project, last, ts(t0())],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
    (wakes, decisions, compares)
}

fn micros<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let s = Instant::now();
    let v = f();
    (v, s.elapsed().as_micros() as u64)
}

/// controller の 1 評価が store から読む主な query（`project_guard`・`project_resume`・summary）の時間。
fn probe(store: &SqliteStore, project: &str, now: OffsetDateTime) -> serde_json::Value {
    let (due, due_us) = micros(|| store.project_controller_due(now, 10).unwrap().len());
    let (_, pending_us) = micros(|| store.project_wakes_pending(project).unwrap().len());
    let (all, all_us) = micros(|| store.project_wakes_all(project).unwrap().len());
    let (_, d256_us) = micros(|| {
        store
            .project_controller_decisions(project, None, 256)
            .unwrap()
            .len()
    });
    let (scan, scan_us) = micros(|| {
        store
            .project_controller_decisions(project, None, 100_000)
            .unwrap()
            .len()
    });
    let (_, input_us) = micros(|| store.project_controller_last_input_at(project).unwrap());
    let (stats, stats_us) = micros(|| store.retention_stats().unwrap());
    serde_json::json!({
        "due_projects": due, "due_micros": due_us,
        "wakes_pending_micros": pending_us,
        "wakes_all_rows": all, "wakes_all_micros": all_us,
        "decisions_256_micros": d256_us,
        "decisions_scan_rows": scan, "decisions_scan_micros": scan_us,
        "last_input_at_micros": input_us,
        "stats_micros": stats_us,
        "evaluation_reads_micros": pending_us + all_us + d256_us + scan_us + input_us,
        "wakes_total": stats.wakes_total, "decisions_total": stats.decisions_total,
        "compare_log_total": stats.compare_log_total,
        "db_bytes": stats.page_count * stats.page_size,
        "freelist_bytes": stats.freelist_count * stats.page_size,
    })
}

#[test]
fn project_retention_scale_30d_before_after_gc() {
    let rows: usize = std::env::var("RETENTION_SCALE_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let projects = (rows / (2 * HOURS as usize)).max(1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    drop(SqliteStore::open(&path).unwrap());
    let ((wakes, decisions, compares), fill_us) = micros(|| fill(&path, projects));
    let store = SqliteStore::open(&path).unwrap();
    let now = t0() + time::Duration::hours(HOURS);
    let probe_project = format!("P{:05}", projects / 2);
    let before = probe(&store, &probe_project, now);
    let applied_before: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM project_controller_decisions WHERE effect_state <> 'none'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // 30 日分の行を 30 日目に GC するので、判断と compare の保持は既定の 30 日ではなく 7 日で測る（既定では候補が 0）。
    let policy = RetentionPolicy {
        decision_keep_secs: 7 * 86_400,
        compare_keep_secs: 7 * 86_400,
        ..RetentionPolicy::default()
    };
    let archive = dir.path().join("archive");
    let mut passes = Vec::new();
    let gc_started = Instant::now();
    loop {
        let (r, us) = micros(|| {
            store
                .retention_gc(&policy, now, &archive, "system:scale-test")
                .unwrap()
        });
        let deleted = r.deleted();
        passes.push(serde_json::json!({ "deleted": deleted, "micros": us,
            "tables": r.tables.iter().map(|t| (t.table.clone(), t.deleted)).collect::<std::collections::BTreeMap<_, _>>() }));
        if deleted == 0 {
            break;
        }
    }
    let gc_us = gc_started.elapsed().as_micros() as u64;
    let after = probe(&store, &probe_project, now);
    let conn = Connection::open(&path).unwrap();
    let applied_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_controller_decisions WHERE effect_state <> 'none'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let last_kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_controller_state s JOIN project_controller_decisions d \
             ON d.id = s.last_decision_id",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let archive_bytes: u64 = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    let archive_lines: usize = std::fs::read_dir(&archive)
        .unwrap()
        .map(|e| {
            std::fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum();
    let deleted_total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(deleted), 0) FROM project_retention_runs",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(store);
    let (_, vacuum_us) = micros(|| conn.execute_batch("VACUUM").unwrap());
    let vacuum_bytes: i64 = conn
        .query_row(
            "SELECT page_count * page_size FROM pragma_page_count, pragma_page_size",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let out = serde_json::json!({
        "schema": "celeris.project-retention-scale/1",
        "rows_requested": rows, "projects": projects, "virtual_hours": HOURS,
        "rows": { "wakes": wakes, "decisions": decisions, "compare_log": compares },
        "fill_micros": fill_us,
        "policy": policy,
        "before": before, "after": after,
        "gc_micros": gc_us, "gc_passes": passes,
        "archive_bytes": archive_bytes, "archive_lines": archive_lines, "deleted_total": deleted_total,
        "effect_rows_before": applied_before, "effect_rows_after": applied_after,
        "last_decisions_kept": last_kept,
        "vacuum_micros": vacuum_us, "db_bytes_after_vacuum": vacuum_bytes,
    });
    if let Some(p) = std::env::var_os("RETENTION_SCALE_OUT") {
        std::fs::write(p, serde_json::to_vec_pretty(&out).unwrap()).unwrap();
    }
    println!("{out}");
    // 不変条件: 効果のある判断と案件の最後の判断は全部残る、未消費の wake は無いので 0 のまま、行は減る。
    assert_eq!(applied_after, applied_before);
    assert_eq!(
        archive_lines as i64, deleted_total,
        "every deleted row is in the archive"
    );
    assert_eq!(last_kept, projects as i64);
    assert!(after["wakes_total"].as_u64() < before["wakes_total"].as_u64());
    assert!(after["decisions_total"].as_u64() < before["decisions_total"].as_u64());
    // wake は 7 日分の cron と全部の root_done、判断は新しい 256 件・効果のある判断・最初の 1 件までに収まる。
    let kept_decisions = after["decisions_total"].as_u64().unwrap() as i64;
    let decision_ceiling = projects as i64 * (256 + HOURS / 6 + 1);
    assert!(
        kept_decisions <= decision_ceiling,
        "{kept_decisions} > {decision_ceiling}"
    );
    let kept_wakes = after["wakes_total"].as_u64().unwrap() as i64;
    let ceiling = projects as i64 * (7 * 24 + 1 + HOURS / 6);
    assert!(kept_wakes <= ceiling, "{kept_wakes} > {ceiling}");
}
