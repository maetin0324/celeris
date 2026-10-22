//! 完了単位の記録（`completion_outcomes`）と harness 切替の guardrail 状態（`harness_balance_trips`）。
//! ADR 2026-10-22-harness-balance-and-lane-first-routing D2/D6。events・runs・決定ログからの冪等な投影。
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::completion_outcome::CompletionOutcome;

use super::{SqliteStore, StoreError};

const COLUMNS: &str = "unit_kind,unit_id,task_id,leaf,genre,passed,harness_id,harness_version,source_id,model_id,lane,jev_specificity,jev_confidence,jev_lane,classifier_status,propensity,tokens_in,tokens_out,tokens_cache_read,tokens_cache_creation,usd_list,quota_json,wall_s,attempts,check_failures,review_rejections,replans,timed_out,switched_classes_json,celeris_version,completed_at";

fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn row_to_outcome(r: &rusqlite::Row<'_>) -> rusqlite::Result<CompletionOutcome> {
    let json_err = |i: usize, e: serde_json::Error| {
        rusqlite::Error::FromSqlConversionFailure(i, rusqlite::types::Type::Text, Box::new(e))
    };
    let quota: String = r.get(21)?;
    let switched: String = r.get(28)?;
    let u = |i: usize| -> rusqlite::Result<Option<u64>> {
        Ok(r.get::<_, Option<i64>>(i)?.map(|v| v.max(0) as u64))
    };
    Ok(CompletionOutcome {
        unit_kind: r.get(0)?,
        unit_id: r.get(1)?,
        task_id: r.get(2)?,
        leaf: r.get(3)?,
        genre: r.get(4)?,
        passed: r.get(5)?,
        harness_id: r.get(6)?,
        harness_version: r.get(7)?,
        source_id: r.get(8)?,
        model_id: r.get(9)?,
        lane: r.get(10)?,
        jev_specificity: r.get(11)?,
        jev_confidence: r.get(12)?,
        jev_lane: r.get(13)?,
        classifier_status: r.get(14)?,
        propensity: r.get(15)?,
        tokens_in: u(16)?,
        tokens_out: u(17)?,
        tokens_cache_read: u(18)?,
        tokens_cache_creation: u(19)?,
        usd_list: r.get(20)?,
        quota: serde_json::from_str(&quota).map_err(|e| json_err(21, e))?,
        wall_s: r.get(22)?,
        attempts: r.get(23)?,
        check_failures: r.get(24)?,
        review_rejections: r.get(25)?,
        replans: r.get(26)?,
        timed_out: r.get(27)?,
        switched_classes: serde_json::from_str(&switched).map_err(|e| json_err(28, e))?,
        celeris_version: r.get(29)?,
        completed_at: r.get(30)?,
    })
}

/// 1 単位の集計。`run_ids` はその単位に属する run、`replans` は task の実行計画の作り直し回数。
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn aggregate_unit(
    c: &Connection,
    unit_kind: &str,
    unit_id: &str,
    task_id: &str,
    leaf: bool,
    genre: &str,
    passed: bool,
    run_ids: &[String],
    replans: u32,
    version: &str,
    now: &str,
) -> Result<CompletionOutcome, StoreError> {
    let in_list = |n: usize| -> String { (0..n).map(|_| "?").collect::<Vec<_>>().join(",") };
    let list = in_list(run_ids.len().max(1));
    let args: Vec<&dyn rusqlite::ToSql> = if run_ids.is_empty() {
        vec![&"" as &dyn rusqlite::ToSql]
    } else {
        run_ids.iter().map(|r| r as &dyn rusqlite::ToSql).collect()
    };
    // 最後の worker run: 最終の route（harness・source・model）と、その決定の記録の読み元。
    let last: Option<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)> =
        c.query_row(
            &format!(
                "SELECT run_id, selected_harness_id, selected_source_id, selected_model_id, adapter, model \
                 FROM runs WHERE run_id IN ({list}) AND role = 'worker' \
                 ORDER BY started_at DESC, run_id DESC LIMIT 1"
            ),
            args.as_slice(),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let attempts: u32 = c.query_row(
        &format!("SELECT COUNT(*) FROM runs WHERE run_id IN ({list}) AND role = 'worker'"),
        args.as_slice(),
        |r| r.get(0),
    )?;
    let (tin, tout, tcr, tcc, usd): (Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<f64>) =
        c.query_row(
            &format!(
                "SELECT SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), SUM(cache_creation_tokens), SUM(list_cost_usd) \
                 FROM decision_log_usage WHERE run_id IN ({list})"
            ),
            args.as_slice(),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    // julianday の浮動小数の誤差を落とすため、ミリ秒に丸める。
    let wall_s: Option<f64> = c
        .query_row(
            &format!(
                "SELECT (julianday(MAX(finished_at)) - julianday(MIN(started_at))) * 86400.0 \
             FROM runs WHERE run_id IN ({list}) AND finished_at IS NOT NULL"
            ),
            args.as_slice(),
            |r| r.get::<_, Option<f64>>(0),
        )?
        .map(|s| (s * 1000.0).round() / 1000.0);
    let (check_failures, timed_out): (u32, bool) = c.query_row(
        &format!(
            "SELECT COALESCE(SUM(CASE WHEN wu_verdict = 'failed' THEN 1 ELSE 0 END), 0), \
                    COALESCE(MAX(CASE WHEN run_end IN ('timeout', 'budget_exhausted') THEN 1 ELSE 0 END), 0) \
             FROM decision_log_runs WHERE run_id IN ({list})"
        ),
        args.as_slice(),
        |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
    )?;
    let review_rejections: u32 = c.query_row(
        &format!(
            "SELECT COUNT(DISTINCT json_extract(json, '$.run_id')) FROM events \
             WHERE task_id = ? AND json_extract(json, '$.type') = 'review_verdict' \
               AND json_extract(json, '$.pass') = 0 AND json_extract(json, '$.run_id') IN ({list})"
        ),
        rusqlite::params_from_iter(
            std::iter::once(&task_id as &dyn rusqlite::ToSql).chain(args.iter().copied()),
        ),
        |r| r.get(0),
    )?;
    // quota: run ごとの最後の quota_estimated の使用率を足す（%→割合）。
    let mut quota: BTreeMap<String, f64> = BTreeMap::new();
    {
        let mut stmt = c.prepare(&format!(
            "SELECT json_extract(json, '$.run_id'), json FROM events \
             WHERE task_id = ? AND json_extract(json, '$.type') = 'quota_estimated' \
               AND json_extract(json, '$.run_id') IN ({list}) ORDER BY seq"
        ))?;
        let mut per_run: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let rows = stmt.query_map(
            rusqlite::params_from_iter(
                std::iter::once(&task_id as &dyn rusqlite::ToSql).chain(args.iter().copied()),
            ),
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        for row in rows {
            let (run, json) = row?;
            if let Ok(v) = serde_json::from_str(&json) {
                per_run.insert(run, v);
            }
        }
        for event in per_run.values() {
            for w in event["windows"].as_array().into_iter().flatten() {
                if let (Some(name), Some(pct)) = (w["window"].as_str(), w["used_pct"].as_f64()) {
                    *quota.entry(name.to_owned()).or_default() += pct / 100.0;
                }
            }
        }
    }
    // 最後の worker run の決定記録（lane・Jev の判定・propensity・harness の版）と、切替したクラス。
    let mut out = CompletionOutcome {
        unit_kind: unit_kind.into(),
        unit_id: unit_id.into(),
        task_id: task_id.into(),
        leaf,
        genre: genre.into(),
        passed,
        harness_id: None,
        harness_version: None,
        source_id: None,
        model_id: None,
        lane: None,
        jev_specificity: None,
        jev_confidence: None,
        jev_lane: None,
        classifier_status: None,
        propensity: None,
        tokens_in: tin.map(|v| v.max(0) as u64),
        tokens_out: tout.map(|v| v.max(0) as u64),
        tokens_cache_read: tcr.map(|v| v.max(0) as u64),
        tokens_cache_creation: tcc.map(|v| v.max(0) as u64),
        usd_list: usd,
        quota,
        wall_s,
        attempts,
        check_failures,
        review_rejections,
        replans,
        timed_out,
        switched_classes: Vec::new(),
        celeris_version: version.into(),
        completed_at: now.into(),
    };
    if let Some((run_id, harness, source, model, adapter, adapter_model)) = last {
        out.harness_id = harness.or(adapter);
        out.source_id = source;
        out.model_id = model.or(adapter_model);
        let decision: Option<String> = c
            .query_row(
                "SELECT json_extract(json, '$.record.features.decision') FROM events \
                 WHERE task_id = ?1 AND json_extract(json, '$.type') = 'routing_decided' \
                   AND json_extract(json, '$.run_id') = ?2 ORDER BY seq DESC LIMIT 1",
                params![task_id, run_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(d) = decision.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            let text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
            out.harness_version = text(&d["harness_version"]);
            out.lane = text(&d["lane"]);
            out.propensity = d["propensity"].as_f64();
            let a = &d["lane_assist"];
            out.jev_specificity = text(&a["jev_specificity"]);
            out.jev_confidence = a["jev_confidence"].as_f64();
            out.jev_lane = text(&a["jev_lane"]);
            out.classifier_status = text(&a["classifier_status"]["status"]);
        }
    }
    let mut stmt = c.prepare(&format!(
        "SELECT DISTINCT json_extract(json, '$.record.features.decision.harness_order.execution_class') FROM events \
         WHERE task_id = ? AND json_extract(json, '$.type') = 'routing_decided' \
           AND json_extract(json, '$.run_id') IN ({list}) \
           AND json_extract(json, '$.record.features.decision.harness_order.switched') = 1"
    ))?;
    let classes = stmt.query_map(
        rusqlite::params_from_iter(
            std::iter::once(&task_id as &dyn rusqlite::ToSql).chain(args.iter().copied()),
        ),
        |r| r.get::<_, Option<String>>(0),
    )?;
    let mut set = BTreeSet::new();
    for class in classes.flatten().flatten() {
        set.insert(class);
    }
    out.switched_classes = set.into_iter().collect();
    Ok(out)
}

impl SqliteStore {
    /// task とその終端の WorkUnit の完了単位を冪等に書く（同じ単位は上書き）。終端でない単位は書かない。
    /// 書いた件数を返す。
    pub(super) fn completion_outcomes_project_impl(
        &self,
        task_id: &str,
        celeris_version: &str,
        now: &str,
    ) -> Result<usize, StoreError> {
        let conn = self.lock()?;
        let task: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT status, genre FROM tasks WHERE id = ?1",
                params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((task_status, genre)) = task else {
            return Ok(0);
        };
        let genre = genre.unwrap_or_default();
        let replans: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 1) - 1 FROM execution_plans WHERE task_id = ?1",
                params![task_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let units: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, status FROM work_units WHERE task_id = ?1 AND status IN ('done', 'failed') ORDER BY seq",
            )?;
            let rows = stmt.query_map(params![task_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let wu_total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM work_units WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )?;
        let runs_of = |sql: &str, id: &str| -> Result<Vec<String>, StoreError> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        };
        let mut outcomes = Vec::new();
        for (wu_id, status) in &units {
            let runs = runs_of("SELECT run_id FROM runs WHERE work_unit_id = ?1", wu_id)?;
            outcomes.push(aggregate_unit(
                &conn,
                "work_unit",
                wu_id,
                task_id,
                true,
                &genre,
                status == "done",
                &runs,
                replans,
                celeris_version,
                now,
            )?);
        }
        if matches!(task_status.as_str(), "done" | "failed") {
            let runs = runs_of("SELECT run_id FROM runs WHERE task_id = ?1", task_id)?;
            outcomes.push(aggregate_unit(
                &conn,
                "task",
                task_id,
                task_id,
                wu_total == 0,
                &genre,
                task_status == "done",
                &runs,
                replans,
                celeris_version,
                now,
            )?);
        }
        let mut stmt = conn.prepare(&format!(
            "INSERT OR REPLACE INTO completion_outcomes ({COLUMNS}) VALUES ({})",
            placeholders(31)
        ))?;
        for o in &outcomes {
            // 再投影で完了時刻を動かさない（最初に書いた時刻を残す）。
            let completed_at: String = conn
                .query_row(
                    "SELECT completed_at FROM completion_outcomes WHERE unit_kind = ?1 AND unit_id = ?2",
                    params![o.unit_kind, o.unit_id],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_else(|| o.completed_at.clone());
            stmt.execute(params![
                o.unit_kind,
                o.unit_id,
                o.task_id,
                o.leaf,
                o.genre,
                o.passed,
                o.harness_id,
                o.harness_version,
                o.source_id,
                o.model_id,
                o.lane,
                o.jev_specificity,
                o.jev_confidence,
                o.jev_lane,
                o.classifier_status,
                o.propensity,
                o.tokens_in.map(|v| v as i64),
                o.tokens_out.map(|v| v as i64),
                o.tokens_cache_read.map(|v| v as i64),
                o.tokens_cache_creation.map(|v| v as i64),
                o.usd_list,
                serde_json::to_string(&o.quota)?,
                o.wall_s,
                o.attempts,
                o.check_failures,
                o.review_rejections,
                o.replans,
                o.timed_out,
                serde_json::to_string(&o.switched_classes)?,
                o.celeris_version,
                completed_at,
            ])?;
        }
        Ok(outcomes.len())
    }

    pub(super) fn completion_outcomes_list_impl(
        &self,
        leaf_only: bool,
        limit: u32,
    ) -> Result<Vec<CompletionOutcome>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM completion_outcomes WHERE (?1 = 0 OR leaf = 1) \
                 ORDER BY completed_at DESC, unit_id DESC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![leaf_only, limit], row_to_outcome)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub(super) fn harness_balance_trip_impl(
        &self,
        class: &str,
        reason: &str,
        at: &str,
    ) -> Result<(), StoreError> {
        self.lock()?.execute(
            "INSERT OR REPLACE INTO harness_balance_trips (execution_class, reason, tripped_at) VALUES (?1, ?2, ?3)",
            params![class, reason, at],
        )?;
        Ok(())
    }

    pub(super) fn harness_balance_trips_impl(
        &self,
    ) -> Result<BTreeMap<String, String>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt =
                c.prepare("SELECT execution_class, reason FROM harness_balance_trips")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub(super) fn harness_balance_reset_impl(&self, class: &str) -> Result<bool, StoreError> {
        let n = self.lock()?.execute(
            "DELETE FROM harness_balance_trips WHERE execution_class = ?1",
            params![class],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion_outcome::{
        evaluate_harness_balance, evidence_rows, refresh_routing_evidence,
    };
    use crate::model::TaskId;
    use crate::route_select::{PolicyInput, RoutingStage, select};
    use crate::store::TaskStore;

    fn task(store: &SqliteStore) -> String {
        let id = TaskId::new();
        let task: crate::Task = serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "parent_id": null, "kind": "execute", "title": "t",
            "objective": "o", "acceptance": [], "inputs": [], "depends_on": [],
            "status": "ready", "priority": 0, "genre": "coding",
            "worker_hint": {"tier": "standard"},
            "workspace": {"kind": "local", "path": "/tmp"},
            "budget": {"max_turns": 1, "max_wall_secs": 1, "max_retries": 0},
            "attempts": 0, "lease": null,
            "created_at": "2026-10-22T00:00:00Z", "updated_at": "2026-10-22T00:00:00Z",
        }))
        .unwrap();
        store.insert(&task).unwrap();
        id.to_string()
    }

    struct Unit<'a> {
        key: &'a str,
        status: &'a str,
        model: &'a str,
        lane: &'a str,
        quota_pct: f64,
        usd: f64,
        /// 最初の run を検査失敗にする（試行 2 回）。
        retried: bool,
        review_rejected: bool,
        switched_class: Option<&'a str>,
        propensity: f64,
    }

    fn seed(store: &SqliteStore, task_id: &str, seq: i64, u: &Unit<'_>) {
        let conn = store.lock().unwrap();
        let wu = format!("wu-{}", u.key);
        conn.execute(
            "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, json, created_at, updated_at) \
             VALUES (?1, ?2, 'p1', ?3, ?4, 'implement', ?5, '{}', '2026-10-22T00:00:00Z', '2026-10-22T00:00:00Z')",
            params![wu, task_id, u.key, seq, u.status],
        )
        .unwrap();
        let attempts = if u.retried { 2 } else { 1 };
        let mut ev_seq = seq * 100;
        for n in 0..attempts {
            let run = format!("{}-r{n}", u.key);
            let start = format!("2026-10-22T00:{:02}:00Z", n * 5);
            let end = format!("2026-10-22T00:{:02}:00Z", n * 5 + 4);
            conn.execute(
                "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, started_at, finished_at, \
                 selected_harness_id, selected_source_id, selected_model_id) \
                 VALUES (?1, ?2, ?3, 'worker', ?4, 'done', ?5, ?6, 'claude_code', 'claude_oauth', ?7)",
                params![run, task_id, wu, n, start, end, u.model],
            )
            .unwrap();
            let failed = u.retried && n == 0;
            conn.execute(
                "INSERT INTO decision_log_runs (run_id, wu_verdict, run_end, updated_at) VALUES (?1, ?2, 'completed', 'now')",
                params![run, if failed { "failed" } else { "passed" }],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO decision_log_usage (dedup_key, run_id, role, input_tokens, output_tokens, cache_read_tokens, \
                 cache_creation_tokens, list_cost_usd, updated_at) VALUES (?1, ?2, 'worker', 100, 10, 1000, 50, ?3, 'now')",
                params![format!("run:{run}:x"), run, u.usd / attempts as f64],
            )
            .unwrap();
            let mut decision = serde_json::json!({
                "harness_version": "2.1.0", "lane": u.lane, "propensity": u.propensity,
                "lane_assist": {"jev_specificity": "specified", "jev_confidence": 0.9, "jev_lane": u.lane,
                                 "classifier_status": {"status": "jev_ok"}},
            });
            if let Some(class) = u.switched_class {
                decision["harness_order"] =
                    serde_json::json!({"execution_class": class, "switched": true});
            }
            for event in [
                serde_json::json!({"type": "routing_decided", "run_id": run,
                    "record": {"features": {"decision": decision}}}),
                serde_json::json!({"type": "quota_estimated", "run_id": run,
                    "windows": [{"window": "seven_day", "used_pct": u.quota_pct / attempts as f64}]}),
            ] {
                ev_seq += 1;
                conn.execute(
                    "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, '2026-10-22T00:00:00Z', ?3)",
                    params![task_id, ev_seq, event.to_string()],
                )
                .unwrap();
            }
        }
        if u.review_rejected {
            let run = format!("{}-rv", u.key);
            conn.execute(
                "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, started_at, finished_at) \
                 VALUES (?1, ?2, ?3, 'reviewer', 9, 'done', '2026-10-22T00:20:00Z', '2026-10-22T00:21:00Z')",
                params![run, task_id, wu],
            )
            .unwrap();
            ev_seq += 1;
            conn.execute(
                "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, '2026-10-22T00:00:00Z', ?3)",
                params![task_id, ev_seq, serde_json::json!({"type": "review_verdict", "run_id": run,
                    "criterion_idx": 0, "pass": false, "reason": "x"}).to_string()],
            )
            .unwrap();
        }
    }

    fn unit<'a>(key: &'a str, model: &'a str, lane: &'a str, quota_pct: f64) -> Unit<'a> {
        Unit {
            key,
            status: "done",
            model,
            lane,
            quota_pct,
            usd: 1.0,
            retried: false,
            review_rejected: false,
            switched_class: None,
            propensity: 1.0,
        }
    }

    #[test]
    fn a_finished_unit_is_recorded_with_its_totals_and_the_jev_judgement() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task_id = task(&store);
        let mut done = unit("a", "claude-opus-5-5", "frontier", 4.0);
        done.retried = true;
        done.review_rejected = true;
        done.switched_class = Some("planner");
        done.propensity = 0.5;
        seed(&store, &task_id, 1, &done);
        let mut running = unit("b", "claude-sonnet-5-5", "standard", 1.0);
        running.status = "running";
        seed(&store, &task_id, 2, &running);

        let n = store
            .completion_outcomes_project(&task_id, "abc123def456", "2026-10-22T01:00:00Z")
            .unwrap();
        assert_eq!(n, 1, "only the finished unit is recorded");
        // 冪等: 何度投影しても行は増えず、完了時刻は動かない。
        store
            .completion_outcomes_project(&task_id, "abc123def456", "2026-10-22T02:00:00Z")
            .unwrap();
        let rows = store.completion_outcomes_list(false, 10).unwrap();
        assert_eq!(rows.len(), 1);
        let o = &rows[0];
        assert_eq!(
            (o.unit_kind.as_str(), o.unit_id.as_str(), o.leaf, o.passed),
            ("work_unit", "wu-a", true, true)
        );
        assert_eq!(o.completed_at, "2026-10-22T01:00:00Z");
        assert_eq!(
            (
                o.harness_id.as_deref(),
                o.source_id.as_deref(),
                o.model_id.as_deref()
            ),
            (
                Some("claude_code"),
                Some("claude_oauth"),
                Some("claude-opus-5-5")
            )
        );
        assert_eq!(o.lane.as_deref(), Some("frontier"));
        assert_eq!(o.jev_specificity.as_deref(), Some("specified"));
        assert_eq!(o.jev_confidence, Some(0.9));
        assert_eq!(o.classifier_status.as_deref(), Some("jev_ok"));
        assert_eq!(o.propensity, Some(0.5));
        assert_eq!(
            (
                o.tokens_in,
                o.tokens_out,
                o.tokens_cache_read,
                o.tokens_cache_creation
            ),
            (Some(200), Some(20), Some(2000), Some(100))
        );
        assert_eq!(o.usd_list, Some(1.0));
        assert!((o.quota["seven_day"] - 0.04).abs() < 1e-9, "{:?}", o.quota);
        // 最初の worker run の開始 00:00 から reviewer run の終了 00:21 まで。
        assert_eq!(o.wall_s, Some(21.0 * 60.0));
        assert_eq!(
            (o.attempts, o.check_failures, o.review_rejections),
            (2, 1, 1)
        );
        assert_eq!(o.switched_classes, ["planner"]);
        assert_eq!(o.celeris_version, "abc123def456");
        assert!(o.bad());
    }

    #[test]
    fn completions_update_the_per_lane_table_and_the_selector_reads_it() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task_id = task(&store);
        // standard lane: opus は同じ完了率で quota コストが 3 倍。frontier lane は opus だけ。
        for i in 0..12 {
            let (s, o, f) = (format!("s{i}"), format!("o{i}"), format!("f{i}"));
            seed(
                &store,
                &task_id,
                i,
                &unit(&s, "claude-sonnet-5-5", "standard", 1.0),
            );
            seed(
                &store,
                &task_id,
                100 + i,
                &unit(&o, "claude-opus-5-5", "standard", 3.0),
            );
            seed(
                &store,
                &task_id,
                200 + i,
                &unit(&f, "claude-opus-5-5", "frontier", 3.0),
            );
        }
        store
            .completion_outcomes_project(&task_id, "v1", "2026-10-22T01:00:00Z")
            .unwrap();
        assert_eq!(
            refresh_routing_evidence(&store, "2026-10-22T03:00:00Z").unwrap(),
            3
        );

        let rows = store
            .latest_routing_evidence("coding", "claude_code")
            .unwrap();
        assert_eq!(rows.len(), 3);
        let find = |lane: &str, model: &str| {
            rows.iter()
                .find(|r| r.key.lane == lane && r.key.model_id == model)
                .unwrap_or_else(|| panic!("{lane}/{model}"))
        };
        let sonnet = find("standard", "claude-sonnet-5-5");
        assert_eq!((sonnet.n, sonnet.pass), (12, 12));
        assert_eq!(sonnet.key.harness_version, "2");
        assert!((sonnet.quota_cost_per_pass.unwrap() - 0.01).abs() < 1e-9);
        assert!((sonnet.quota_per_run["seven_day"] - 0.01).abs() < 1e-9);
        assert!(
            (find("standard", "claude-opus-5-5")
                .quota_cost_per_pass
                .unwrap()
                - 0.03)
                .abs()
                < 1e-9
        );
        assert_eq!(sonnet.evidence_version, "completion-2026-10-22T03:00:00Z");

        // 選択器は lane が一致する行だけを読み、quota コストの安い方を先にする。
        let candidates: Vec<_> = ["claude-sonnet-5-5", "claude-opus-5-5"]
            .iter()
            .map(|m| crate::llm_route::Candidate {
                source: "claude_oauth".into(),
                model: (*m).into(),
                api_format: "claude_cli".into(),
            })
            .collect();
        let remaining = Default::default();
        let pick = |lane: &str| {
            select(&PolicyInput {
                eligible: &candidates,
                evidence: &rows,
                genre: "coding",
                harness_id: "claude_code",
                harness_version: "2.1.0",
                lane,
                pinned: &[],
                route_defaults: &[],
                quota_remaining: &remaining,
                stage: RoutingStage::Enforce,
                epsilon: 0.0,
                run_id: "r",
            })
        };
        let standard = pick("standard").unwrap();
        assert_eq!(standard.selected.model.as_ref(), "claude-sonnet-5-5");
        assert_eq!(
            standard.source,
            crate::route_select::SelectionSource::Evidence
        );
        // lane のない旧い根拠はどの lane の選択にも効かない: 行のない lane は route_defaults に落ち、
        // 宣言も無ければ決められない。
        assert!(pick("cheap").is_err());
    }

    #[test]
    fn the_guardrail_stops_a_class_when_switched_units_get_worse_and_a_human_resumes_it() {
        let store = SqliteStore::open_in_memory().unwrap();
        let task_id = task(&store);
        for i in 0..12 {
            let key = format!("w{i}");
            let mut u = unit(&key, "claude-sonnet-5-5", "standard", 1.0);
            u.switched_class = Some("planner");
            u.review_rejected = i < 6;
            seed(&store, &task_id, i, &u);
        }
        for i in 0..20 {
            let key = format!("n{i}");
            seed(
                &store,
                &task_id,
                100 + i,
                &unit(&key, "claude-sonnet-5-5", "standard", 1.0),
            );
        }
        store
            .completion_outcomes_project(&task_id, "v1", "2026-10-22T01:00:00Z")
            .unwrap();
        let classes: std::collections::BTreeSet<String> =
            ["planner".to_owned(), "review".to_owned()].into();
        let tripped =
            evaluate_harness_balance(&store, &classes, 50, 10, 0.15, "2026-10-22T02:00:00Z")
                .unwrap();
        assert_eq!(tripped.len(), 1);
        assert_eq!(tripped[0].0, "planner");
        assert!(
            store
                .harness_balance_trips()
                .unwrap()
                .contains_key("planner")
        );
        // 止まったクラスは再判定しない。人の reset だけが戻す。
        assert!(
            evaluate_harness_balance(&store, &classes, 50, 10, 0.15, "2026-10-22T03:00:00Z")
                .unwrap()
                .is_empty()
        );
        assert!(store.harness_balance_reset("planner").unwrap());
        assert!(store.harness_balance_trips().unwrap().is_empty());
    }

    #[test]
    fn units_without_a_lane_or_a_route_do_not_feed_the_table() {
        let mut o = crate::completion_outcome::CompletionOutcome {
            unit_kind: "work_unit".into(),
            unit_id: "x".into(),
            task_id: "t".into(),
            leaf: true,
            genre: "coding".into(),
            passed: true,
            harness_id: None,
            harness_version: None,
            source_id: None,
            model_id: None,
            lane: None,
            jev_specificity: None,
            jev_confidence: None,
            jev_lane: None,
            classifier_status: None,
            propensity: None,
            tokens_in: None,
            tokens_out: None,
            tokens_cache_read: None,
            tokens_cache_creation: None,
            usd_list: None,
            quota: Default::default(),
            wall_s: None,
            attempts: 1,
            check_failures: 0,
            review_rejections: 0,
            replans: 0,
            timed_out: false,
            switched_classes: vec![],
            celeris_version: "v".into(),
            completed_at: "t".into(),
        };
        assert!(evidence_rows(&[o.clone()], "v").is_empty());
        o.harness_id = Some("h".into());
        o.source_id = Some("s".into());
        o.model_id = Some("m".into());
        assert!(evidence_rows(&[o.clone()], "v").is_empty(), "no lane");
        o.lane = Some("standard".into());
        assert_eq!(evidence_rows(&[o], "v").len(), 1);
    }
}
