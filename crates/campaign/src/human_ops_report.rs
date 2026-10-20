//! 比較表・機械可読出力・タイムラインの再生成（`human-op report`）。入力は生記録（human-ops JSONL）と Celeris 側の記録だけで、
//! 手編集の集計値は読まない。同じ入力からは同じ出力になる（時計を読まない）。
//!
//! 取り決め（`docs/ops/eval-raw-record-definitions.md` §5・§6、`docs/ops/eval-human-ops-runbook.md` §7）:
//! - 人の時間 = `mode=manual` の op の `[start, end)` を試行ごとに**和集合**で数える（重なりの二重計上をしない。単純和は別に残す）。
//!   `protocol_step`（評価者の測定作業）と `mode=automatic`（script）は人の時間・介入に数えない。
//! - 端が無い・読めない・`end<start` の区間は長さを持たない。0 にせず `open` / `missing_start` / `inverted` に件数で残す。
//! - cutoff 以降は数えない（跨ぐ区間は cutoff で切り `clipped` に数える）。
//! - 再送・再 export・訂正の重複は `record_id` / `dedup_key` / job_ref で潰した後の値。
//! - 値は `{value, source}`。source は `event` / `evaluator` / `missing`。
//! - 失敗の原因分類は **暫定 schema**（`PROVISIONAL_CAUSE_SCHEMA`）。分類 task（研究評価接続 2）が確定したら `failure_cause` の写像だけ合わせる。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    human_ops::{
        HumanOpRecord, Intent, Mode, OpKind, RecordType, op_spans, parse_utc, read_records, resolve,
    },
    human_ops_join,
};

pub const REPORT_SCHEMA: &str = "celeris.eval-human-ops-report/1";
/// 失敗原因の分類 schema（暫定）。確定した分類に合わせるときはこの名前と `failure_cause` を更新する。
pub const PROVISIONAL_CAUSE_SCHEMA: &str = "provisional-failure-cause/1";

type Iv = (i64, i64);

fn ts(v: &str) -> Option<i64> {
    parse_utc(v).ok().map(|t| t.unix_timestamp())
}

fn rfc(t: i64) -> String {
    OffsetDateTime::from_unix_timestamp(t)
        .ok()
        .and_then(|d| d.format(&Rfc3339).ok())
        .unwrap_or_else(|| t.to_string())
}

fn field(value: Value, source: &str) -> Value {
    json!({"value": value, "source": source})
}

fn missing(reason: &str) -> Value {
    json!({"value": null, "source": "missing", "reason": reason})
}

/// 区間の和集合の長さと単純和。
fn union_and_sum(ivs: &[Iv]) -> (i64, i64) {
    let mut v = ivs.to_vec();
    v.sort_unstable();
    let (mut union, mut cur): (i64, Option<Iv>) = (0, None);
    for (s, e) in v {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                union += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        union += ce - cs;
    }
    (union, ivs.iter().map(|(s, e)| e - s).sum())
}

/// 区間の集まりの要約。端の欠けた区間は長さに入れず件数で残す。
#[derive(Default)]
struct Spans {
    closed: Vec<Iv>,
    open: u32,
    missing_start: u32,
    inverted: u32,
    after_cutoff: u32,
    clipped: u32,
    /// timeline 用（種別・区間・状態）。
    items: Vec<(String, Option<i64>, Option<i64>, &'static str)>,
}

impl Spans {
    fn add(&mut self, kind: &str, start: Option<&str>, end: Option<&str>, cutoff: Option<i64>) {
        let s = start.and_then(ts);
        let e = end.and_then(ts);
        let kind = kind.to_string();
        let (Some(s), end) = (s, e) else {
            self.missing_start += 1;
            self.items.push((kind, None, e, "missing_start"));
            return;
        };
        if cutoff.is_some_and(|c| s >= c) {
            self.after_cutoff += 1;
            return;
        }
        let Some(mut e) = end else {
            self.open += 1;
            self.items.push((kind, Some(s), None, "open"));
            return;
        };
        if e < s {
            self.inverted += 1;
            self.items.push((kind, Some(s), Some(e), "inverted"));
            return;
        }
        let mut status = "closed";
        if let Some(c) = cutoff.filter(|c| e > *c) {
            e = c;
            self.clipped += 1;
            status = "clipped";
        }
        self.closed.push((s, e));
        self.items.push((kind, Some(s), Some(e), status));
    }

    fn json(&self, source: &str) -> Value {
        let (union, sum) = union_and_sum(&self.closed);
        let unobserved = self.open + self.missing_start + self.inverted;
        json!({
            "value": {
                "union_secs": union, "sum_secs": sum, "overlap_secs": sum - union,
                "observed": self.closed.len(), "open": self.open,
                "missing_start": self.missing_start, "inverted": self.inverted,
                "after_cutoff": self.after_cutoff, "clipped": self.clipped,
                // 長さを持てない区間があるとき、union_secs は下限。
                "lower_bound": unobserved > 0 || self.clipped > 0,
            },
            "source": source,
        })
    }
}

/// 失敗原因（暫定分類）。完遂は `completed`。出所は TrialRecord の outcome・reason（独立 grader/checker の判定）。
pub fn failure_cause(outcome: Option<&str>, reason: Option<&str>) -> &'static str {
    match (outcome, reason) {
        (None, _) => "unknown",
        (Some("passed"), _) => "completed",
        (Some("failed"), Some("no_artifact")) => "no_artifact",
        (Some("failed"), _) => "verification_failed",
        (Some("error"), _) => "infrastructure_error",
        (Some("timeout"), _) => "timeout",
        (Some("cancelled"), Some(r)) if r.ends_with("budget") || r == "budget_stop" => {
            "budget_stop"
        }
        (Some("cancelled"), _) => "cancelled",
        (Some("blocked"), _) => "dependency_blocked",
        _ => "unclassified",
    }
}

/// やり直しの原因（人が再投入・再開・調整した attempt の op の intent から）。
fn rework_cause(op: Option<&HumanOpRecord>) -> &'static str {
    match op.and_then(|o| o.intent) {
        Some(Intent::System) => "infrastructure",
        Some(Intent::Research | Intent::Safety) => "research",
        _ => "unclassified",
    }
}

type TrialKey = (String, String, String, u32);

struct TrialEval {
    interventions: BTreeMap<String, u64>,
    automatic_ops: u64,
    human_time: Spans,
    human_wait: Spans,
    queue_wait: Spans,
    external_run: Spans,
    jobs_deduped: u32,
    rework: BTreeMap<&'static str, u64>,
    rework_secs: Spans,
    clock_unknown: u32,
    approximate: u32,
    bases: BTreeMap<String, u64>,
    cutoff: Option<i64>,
}

fn eval_trial(recs: &[&HumanOpRecord]) -> TrialEval {
    let owned: Vec<HumanOpRecord> = recs.iter().map(|r| (*r).clone()).collect();
    let cutoff = recs
        .iter()
        .filter(|r| r.record_type == RecordType::Cutoff)
        .filter_map(|r| r.occurred_at.as_deref().and_then(ts))
        .min();
    let mut t = TrialEval {
        interventions: BTreeMap::new(),
        automatic_ops: 0,
        human_time: Spans::default(),
        human_wait: Spans::default(),
        queue_wait: Spans::default(),
        external_run: Spans::default(),
        jobs_deduped: 0,
        rework: BTreeMap::new(),
        rework_secs: Spans::default(),
        clock_unknown: 0,
        approximate: 0,
        bases: BTreeMap::new(),
        cutoff,
    };
    for r in &owned {
        if r.clock_source == crate::human_ops::ClockSource::Unknown {
            t.clock_unknown += 1;
        }
        if r.time_precision == Some(crate::human_ops::TimePrecision::Approximate) {
            t.approximate += 1;
        }
        let b = r.time_basis.map_or("unknown", |b| b.as_str());
        *t.bases.entry(b.to_string()).or_default() += 1;
    }
    for (rec, span) in op_spans(&owned) {
        let kind = rec.op_kind.map_or("unclassified", |k| k.as_str());
        if rec.op_kind == Some(OpKind::ProtocolStep) || rec.intent == Some(Intent::Protocol) {
            continue;
        }
        if rec.mode == Some(Mode::Automatic) {
            t.automatic_ops += 1;
            continue;
        }
        *t.interventions.entry(kind.to_string()).or_default() += 1;
        t.human_time
            .add(kind, span.start.as_deref(), span.end.as_deref(), cutoff);
    }
    // 人待ち: (logical_op, attempt, wait_kind) で start と end を対にする。
    let mut waits: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for r in &owned {
        let key = format!("{:?}|{:?}|{:?}", r.logical_op_id, r.attempt, r.wait_kind);
        match r.record_type {
            RecordType::WaitStart => waits.entry(key).or_default().0 = r.start.clone(),
            RecordType::WaitEnd => waits.entry(key).or_default().1 = r.end.clone(),
            _ => {}
        }
    }
    for (key, (s, e)) in waits {
        let kind = key.rsplit('|').next().unwrap_or("wait").to_string();
        t.human_wait.add(
            &format!("human_wait:{}", kind.trim_matches(|c| c == '"' || c == ')')),
            s.as_deref(),
            e.as_deref(),
            cutoff,
        );
    }
    // 外部 job: 同じ job_ref の観測は 1 件（最初）にする。
    let mut seen_jobs = BTreeSet::new();
    for r in owned
        .iter()
        .filter(|r| r.record_type == RecordType::JobObservation)
    {
        let job = r.job_ref.clone().unwrap_or_default();
        if !seen_jobs.insert(job.clone()) {
            t.jobs_deduped += 1;
            continue;
        }
        t.queue_wait.add(
            &format!("external_queue_wait:{job}"),
            r.job_submit.as_deref(),
            r.job_start.as_deref(),
            cutoff,
        );
        t.external_run.add(
            &format!("external_run:{job}"),
            r.job_start.as_deref(),
            r.job_end.as_deref(),
            cutoff,
        );
    }
    // やり直し: 同じ論理作業の attempt n+1 があるとき、n は再作業。原因は n+1 の op（intent）から。
    let mut ops_by: BTreeMap<(String, u32), &HumanOpRecord> = BTreeMap::new();
    for r in owned
        .iter()
        .filter(|r| r.record_type == RecordType::OpStart)
    {
        if let (Some(l), Some(a)) = (&r.logical_op_id, r.attempt) {
            ops_by.entry((l.clone(), a)).or_insert(r);
        }
    }
    let mut attempts: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for r in &owned {
        if let (Some(l), Some(a)) = (&r.logical_op_id, r.attempt) {
            attempts.entry(l.clone()).or_default().insert(a);
        }
    }
    for (l, set) in &attempts {
        let v: Vec<u32> = set.iter().copied().collect();
        for w in v.windows(2) {
            let cause = rework_cause(ops_by.get(&(l.clone(), w[1])).copied());
            *t.rework.entry(cause).or_default() += 1;
            // 失敗した前の attempt の所要時間（attempt 行）を再作業時間として数える。
            for a in owned.iter().filter(|r| {
                r.record_type == RecordType::Attempt
                    && r.logical_op_id.as_deref() == Some(l)
                    && r.attempt == Some(w[0])
            }) {
                t.rework_secs
                    .add(cause, a.start.as_deref(), a.end.as_deref(), cutoff);
            }
        }
    }
    t
}

fn counts_json(m: &BTreeMap<String, u64>) -> Value {
    json!(m)
}

fn sum_obs(v: &Value) -> (u64, u64, u64) {
    let x = &v["value"];
    (
        x["observed_sum"].as_u64().unwrap_or(0),
        x["observed"].as_u64().unwrap_or(0),
        x["missing"].as_u64().unwrap_or(0),
    )
}

pub struct Report {
    pub json: Value,
    pub markdown: String,
    pub timeline: String,
}

/// 結合（`human_ops_join`）と区間評価から報告を作る。
pub fn build(human_ops_file: &Path, events: &[&Path]) -> Result<Report, String> {
    let joined = human_ops_join::join(human_ops_file, events)?;
    let join_rows: Vec<Value> = joined
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let recs = resolve(&read_records(human_ops_file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let mut by_trial: BTreeMap<TrialKey, Vec<&HumanOpRecord>> = BTreeMap::new();
    let mut shared: BTreeMap<String, Vec<&HumanOpRecord>> = BTreeMap::new();
    for r in &recs {
        match (&r.case_id, r.arm, r.repetition) {
            (Some(c), Some(a), Some(rep)) => by_trial
                .entry((
                    r.manifest_sha256.clone(),
                    c.clone(),
                    format!("{a:?}").chars().next().unwrap_or('?').to_string(),
                    rep,
                ))
                .or_default()
                .push(r),
            _ => shared.entry(r.campaign_id.clone()).or_default().push(r),
        }
    }

    let mut trials = Vec::new();
    let mut timeline: Vec<(i64, String, Value)> = Vec::new();
    let mut arms: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut other_rows = Vec::new();
    for jr in &join_rows {
        if jr["row_type"] != "trial" {
            other_rows.push(jr.clone());
            continue;
        }
        let key: TrialKey = (
            jr["manifest_sha256"].as_str().unwrap_or("").to_string(),
            jr["case_id"].as_str().unwrap_or("").to_string(),
            jr["arm"].as_str().unwrap_or("").to_string(),
            jr["rep"].as_u64().unwrap_or(0) as u32,
        );
        let ev = by_trial.get(&key).map(|r| eval_trial(r));
        let arm_label = jr["arm_labels"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map_or_else(|| key.2.clone(), String::from);
        let mut t = Map::new();
        for k in [
            "manifest_sha256",
            "campaign_id",
            "case_id",
            "arm",
            "rep",
            "arm_labels",
        ] {
            t.insert(k.into(), jr[k].clone());
        }
        let outcome = jr["outcome"]["value"].as_str();
        let reason = jr["failure_reason"]["value"].as_str();
        t.insert(
            "completion".into(),
            if outcome.is_some() {
                field(json!(outcome == Some("passed")), "event")
            } else {
                missing("TrialRecord が無い（outcome 欠測）")
            },
        );
        t.insert(
            "failure_cause".into(),
            if outcome.is_some() {
                json!({"value": failure_cause(outcome, reason), "source": "event",
                       "schema": PROVISIONAL_CAUSE_SCHEMA})
            } else {
                missing("TrialRecord が無い")
            },
        );
        for k in ["outcome", "wall_secs", "tokens", "cost_micro_usd"] {
            t.insert(k.into(), jr[k].clone());
        }
        let nomiss = |what: &str| missing(&format!("この試行に評価者記録が無い（{what}）"));
        match &ev {
            Some(e) => {
                t.insert(
                    "interventions".into(),
                    field(counts_json(&e.interventions), "evaluator"),
                );
                t.insert("automatic_ops".into(), json!(e.automatic_ops));
                t.insert("human_time".into(), e.human_time.json("evaluator"));
                t.insert("human_wait".into(), e.human_wait.json("evaluator"));
                t.insert("external_queue_wait".into(), e.queue_wait.json("evaluator"));
                t.insert("external_run".into(), e.external_run.json("evaluator"));
                t.insert(
                    "internal_wait".into(),
                    missing(
                        "ready_not_running は event 由来で、現在の trials/ledger/export には出ない",
                    ),
                );
                t.insert(
                    "rework".into(),
                    json!({"value": {"count": e.rework, "failed_attempt_secs": e.rework_secs.json("evaluator")["value"]},
                           "source": "evaluator"}),
                );
                t.insert(
                    "time_quality".into(),
                    json!({"clock_unknown_records": e.clock_unknown, "approximate_records": e.approximate,
                           "time_basis": e.bases, "jobs_deduped": e.jobs_deduped,
                           "observation_cutoff": e.cutoff.map(rfc)}),
                );
                for (kind, s, en, status) in e
                    .human_time
                    .items
                    .iter()
                    .map(|i| ("human_op", i))
                    .chain(e.human_wait.items.iter().map(|i| ("human_wait", i)))
                    .chain(
                        e.queue_wait
                            .items
                            .iter()
                            .map(|i| ("external_queue_wait", i)),
                    )
                    .chain(e.external_run.items.iter().map(|i| ("external_run", i)))
                    .map(|(l, i)| (l, &i.0, &i.1, &i.2, i.3))
                    .map(|(l, k, s, en, st)| (format!("{l}:{k}"), s, en, st))
                {
                    let sort = s.or(*en).unwrap_or(i64::MAX);
                    timeline.push((
                        sort,
                        kind.clone(),
                        json!({"manifest_sha256": key.0, "case_id": key.1, "arm": arm_label,
                               "rep": key.3, "lane": kind, "start": s.map(rfc), "end": en.map(rfc),
                               "status": status, "source": "evaluator"}),
                    ));
                }
            }
            None => {
                for k in [
                    "interventions",
                    "human_time",
                    "human_wait",
                    "external_queue_wait",
                    "external_run",
                    "rework",
                ] {
                    t.insert(k.into(), nomiss(k));
                }
                t.insert("internal_wait".into(), nomiss("internal_wait"));
            }
        }
        if let Some(st) = jr["started_at"]["value"].as_i64() {
            let wall = &jr["wall_secs"]["value"];
            let end = (wall["missing"] == 0)
                .then(|| wall["observed_sum"].as_i64())
                .flatten()
                .map(|w| st + w);
            timeline.push((
                st,
                "trial".into(),
                json!({"manifest_sha256": key.0, "case_id": key.1, "arm": arm_label,
                       "rep": key.3, "lane": "trial", "start": rfc(st), "end": end.map(rfc),
                       "status": if end.is_some() { "closed" } else { "open_or_wall_missing" },
                       "source": "event"}),
            ));
        }
        t.insert("join".into(), jr.clone());

        // arm 集計
        let a = arms.entry(arm_label.clone()).or_default();
        let bump = |a: &mut Map<String, Value>, k: &str, n: u64| {
            let v = a.get(k).and_then(Value::as_u64).unwrap_or(0) + n;
            a.insert(k.into(), json!(v));
        };
        bump(a, "trials", 1);
        if outcome == Some("passed") {
            bump(a, "completed", 1);
        }
        let fc = failure_cause(outcome, reason);
        let causes = a
            .entry("failure_causes".to_string())
            .or_insert_with(|| json!({}));
        let c = causes[fc].as_u64().unwrap_or(0) + 1;
        causes[fc] = json!(c);
        for (k, jk) in [
            ("tokens_observed", "tokens"),
            ("cost_micro_usd_observed", "cost_micro_usd"),
        ] {
            let (sum, _, miss) = sum_obs(&jr[jk]);
            bump(a, k, sum);
            if jr[jk]["source"] == "missing" || miss > 0 {
                bump(a, &format!("{jk}_missing_trials"), 1);
            }
        }
        match &ev {
            Some(e) => {
                let hv = t["human_time"]["value"].clone();
                bump(
                    a,
                    "human_union_secs",
                    hv["union_secs"].as_u64().unwrap_or(0),
                );
                bump(a, "human_sum_secs", hv["sum_secs"].as_u64().unwrap_or(0));
                if hv["lower_bound"] == true {
                    bump(a, "human_time_lower_bound_trials", 1);
                }
                for (src, name) in [
                    (&e.human_wait, "human_wait_union_secs"),
                    (&e.queue_wait, "external_queue_wait_union_secs"),
                    (&e.external_run, "external_run_union_secs"),
                ] {
                    bump(a, name, union_and_sum(&src.closed).0 as u64);
                }
                for (k, n) in &e.interventions {
                    let m = a
                        .entry("interventions".to_string())
                        .or_insert_with(|| json!({}));
                    m[k] = json!(m[k].as_u64().unwrap_or(0) + n);
                }
                for (k, n) in &e.rework {
                    let m = a.entry("rework".to_string()).or_insert_with(|| json!({}));
                    m[*k] = json!(m[*k].as_u64().unwrap_or(0) + n);
                }
                let m = a
                    .entry("time_basis".to_string())
                    .or_insert_with(|| json!({}));
                for (k, n) in &e.bases {
                    m[k] = json!(m[k].as_u64().unwrap_or(0) + n);
                }
            }
            None => bump(a, "trials_without_evaluator_records", 1),
        }
        trials.push(Value::Object(t));
    }

    let shared_json: Vec<Value> = shared
        .iter()
        .map(|(campaign, rs)| {
            let e = eval_trial(rs);
            json!({"campaign_id": campaign, "records": rs.len(),
                   "interventions": e.interventions, "human_time": e.human_time.json("evaluator")["value"],
                   "attribution": "arm へ配分しない（case/arm/rep の identity が無い記録）"})
        })
        .collect();

    timeline.sort_by(|a, b| (a.0, &a.1, a.2.to_string()).cmp(&(b.0, &b.1, b.2.to_string())));
    let timeline_text: String = timeline
        .iter()
        .map(|(_, _, v)| format!("{v}\n"))
        .collect::<Vec<_>>()
        .concat();

    let notices = notices();
    let json = json!({
        "schema": REPORT_SCHEMA,
        "failure_cause_schema": PROVISIONAL_CAUSE_SCHEMA,
        "trials": trials,
        "arms": arms,
        "shared_overhead": shared_json,
        "unjoined": other_rows,
        "notices": notices,
    });
    let markdown = markdown(&json, &timeline);
    Ok(Report {
        json,
        markdown,
        timeline: timeline_text,
    })
}

fn notices() -> Vec<&'static str> {
    vec![
        "人の時間は『記録された操作の範囲』の下限で、総労働時間・認知負荷・判断の質ではない（定義書 §6）。端の欠けた区間は長さに入れず件数で併記する。",
        "A と B で観測方法が異なる（human_wait は B が event・A が評価者記録/transcript、time_basis を併記）。単純な差を性能差として読まない。",
        "case/arm/rep の identity が無い共有 overhead（CoS 等）は arm へ配分せず、shared_overhead に別掲する。記録が無いものは欠測で 0 ではない。",
        "internal_wait（ready_not_running）は現在の入力に無く欠測。external_queue_wait はクラスタ混雑に依存しシステム性能として帰属しない。",
        "failure_cause は暫定 schema。試行数は小さく統計的有意性は主張しない。",
    ]
}

fn cell(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_u64)
        .map_or("0".into(), |n| n.to_string())
}

fn markdown(j: &Value, timeline: &[(i64, String, Value)]) -> String {
    let mut m = String::from("# 人の介入・時間・再作業・消費の比較表（再生成物）\n\n");
    m += "この表は `celeris-campaign human-op report` が生記録から再生成した。手で編集しない。\n\n## 注意\n\n";
    for n in j["notices"].as_array().into_iter().flatten() {
        m += &format!("- {}\n", n.as_str().unwrap_or(""));
    }
    m += "\n## arm 別\n\n| arm | 試行 | 完遂 | 失敗原因 | 介入(区分別) | 人の時間 union 秒 (単純和) | 下限の試行 | 人待ち秒 | 外部 queue 秒 | 再作業(原因別) | tokens 観測合計 (欠測試行) | 費用 µUSD 観測合計 (欠測試行) | 評価者記録なし | time_basis |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n";
    for (arm, a) in j["arms"].as_object().into_iter().flatten() {
        let o = |k: &str| a.get(k).map_or("-".into(), |v| v.to_string());
        m += &format!(
            "| {arm} | {} | {} | {} | {} | {} ({}) | {} | {} | {} | {} | {} ({}) | {} ({}) | {} | {} |\n",
            cell(a, "trials"),
            cell(a, "completed"),
            o("failure_causes"),
            o("interventions"),
            cell(a, "human_union_secs"),
            cell(a, "human_sum_secs"),
            cell(a, "human_time_lower_bound_trials"),
            cell(a, "human_wait_union_secs"),
            cell(a, "external_queue_wait_union_secs"),
            o("rework"),
            cell(a, "tokens_observed"),
            cell(a, "tokens_missing_trials"),
            cell(a, "cost_micro_usd_observed"),
            cell(a, "cost_micro_usd_missing_trials"),
            cell(a, "trials_without_evaluator_records"),
            o("time_basis"),
        );
    }
    m += "\n## 共有 overhead（arm へ配分しない）\n\n";
    let sh = j["shared_overhead"].as_array().cloned().unwrap_or_default();
    if sh.is_empty() {
        m += "記録なし（欠測。0 ではない）。\n";
    }
    for s in &sh {
        m += &format!(
            "- campaign {}: 記録 {} 件、介入 {}、人の時間 {}\n",
            s["campaign_id"], s["records"], s["interventions"], s["human_time"]
        );
    }
    m += "\n## 試行別\n\n| arm | case | rep | 完遂 | 原因 | 人の時間 union 秒 | open/欠測始点 | 人待ち秒 | 外部 queue 秒 | 再作業 | tokens |\n|---|---|---|---|---|---|---|---|---|---|---|\n";
    for t in j["trials"].as_array().into_iter().flatten() {
        let hv = &t["human_time"]["value"];
        let src = |v: &Value| {
            if v["source"] == "missing" {
                "欠測".to_string()
            } else {
                v["value"].to_string()
            }
        };
        let secs = |k: &str| {
            let v = &t[k];
            if v["source"] == "missing" {
                "欠測".to_string()
            } else {
                v["value"]["union_secs"].to_string()
            }
        };
        m += &format!(
            "| {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} |\n",
            t["arm_labels"]
                .as_array()
                .and_then(|a| a.first())
                .map_or(t["arm"].to_string(), |x| x.to_string()),
            t["case_id"],
            t["rep"],
            src(&t["completion"]),
            src(&t["failure_cause"]),
            secs("human_time"),
            hv["open"],
            hv["missing_start"],
            secs("human_wait"),
            secs("external_queue_wait"),
            src(&t["rework"]),
            src(&t["tokens"]),
        );
    }
    m += "\n## タイムライン\n\n| 開始 | 終了 | arm | case | rep | lane | 状態 |\n|---|---|---|---|---|---|---|\n";
    for (_, _, v) in timeline {
        let f = |k: &str| v[k].as_str().unwrap_or("(欠測)").to_string();
        m += &format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            f("start"),
            f("end"),
            f("arm"),
            v["case_id"].as_str().unwrap_or(""),
            v["rep"],
            f("lane"),
            f("status")
        );
    }
    m
}

/// `human-op report --file F --events X... --out DIR`。成果 3 file を書き、書いた先を返す。
pub fn report_cli(args: &[String]) -> Result<String, String> {
    let opt = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let file = opt("--file").ok_or("--file is required")?;
    let out = opt("--out").ok_or("--out is required")?;
    let events: Vec<&Path> = args
        .windows(2)
        .filter(|w| w[0] == "--events")
        .map(|w| Path::new(w[1].as_str()))
        .collect();
    if events.is_empty() {
        return Err("--events is required".into());
    }
    let r = build(Path::new(&file), &events)?;
    let dir = Path::new(&out);
    fs::create_dir_all(dir).map_err(|e| format!("{out}: {e}"))?;
    let body = serde_json::to_string_pretty(&r.json).map_err(|e| e.to_string())? + "\n";
    for (name, text) in [
        ("report.json", body),
        ("report.md", r.markdown),
        ("timeline.jsonl", r.timeline),
    ] {
        fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(format!(
        "wrote {out}/report.json report.md timeline.jsonl\n"
    ))
}
