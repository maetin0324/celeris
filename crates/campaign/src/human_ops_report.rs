//! 比較表・機械可読出力・タイムラインの再生成（`human-op report`）。入力は生記録（human-ops JSONL）と Celeris 側の記録だけで、
//! 手編集の集計値は読まない。同じ入力からは同じ出力になる（時計を読まない）。
//!
//! 取り決め（`docs/ops/eval-raw-record-definitions.md` §5・§6、`docs/ops/eval-human-ops-runbook.md` §7）:
//! - 人の時間 = `mode=manual` の op の `[start, end)` を試行ごとに**和集合**で数える（重なりの二重計上をしない。単純和は別に残す）。
//!   `protocol_step`（評価者の測定作業）と `mode=automatic`（script）は人の時間・介入・再作業に数えない。
//! - 端が無い・読めない・`end<start` の区間は長さを持たない。0 にせず `open` / `missing_start` / `inverted` に件数で残す。
//! - 観測窓（cutoff、無ければ試行終了）の後は数えない（跨ぐ区間は窓の終端で切り `clipped` に数える）。
//! - 再送・再 export・訂正の重複は `record_id` / `dedup_key` / job_ref で潰した後の値。
//! - 値は `{value, source}`。source は `event` / `evaluator` / `missing`。
//! - 完遂・失敗の分類は成功契約（`crate::success`）。checker・export と同じ関数の結論を join から受け取り、ここで再判定しない。
//! - 介入・人の時間・再作業は共通 filter（`counts_as_human_op`）と共通の観測窓（cutoff、無ければ試行終了）で数える。
//! - 取り消し（correction の `retracted`）は resolve で外れ、`corrections` に補正履歴として残る。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    human_ops::{
        HumanOpRecord, Intent, Mode, OpKind, RecordType, op_spans, parse_utc, read_records,
        resolve_with_history,
    },
    human_ops_join,
};

pub const REPORT_SCHEMA: &str = "celeris.eval-human-ops-report/2";

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

/// やり直しの原因（人が再投入・再開・調整した attempt の op の intent から）。
fn rework_cause(op: Option<&HumanOpRecord>) -> &'static str {
    match op.and_then(|o| o.intent) {
        Some(Intent::System) => "infrastructure",
        Some(Intent::Research | Intent::Safety) => "research",
        _ => "unclassified",
    }
}

/// 人の介入・時間・再作業に数える op か（共通 filter）。評価者の測定作業（`protocol_step` / intent protocol）と
/// script（`mode=automatic`）は数えない。介入件数・人の時間・attempt/再作業のすべてがこの 1 つを使う。
pub fn counts_as_human_op(r: &HumanOpRecord) -> bool {
    r.op_kind != Some(OpKind::ProtocolStep)
        && r.intent != Some(Intent::Protocol)
        && r.mode != Some(Mode::Automatic)
}

type TrialKey = (String, String, String, u32);

/// 観測窓の終端。cutoff があればそれ、無ければ試行の終了（start + wall）。どちらも無ければ None（終端で切らない）。
#[derive(Clone, Copy)]
struct Window {
    end: Option<i64>,
    basis: &'static str,
}

struct TrialEval {
    interventions: BTreeMap<String, u64>,
    after_window_ops: u64,
    automatic_ops: u64,
    protocol_ops: u64,
    human_time: Spans,
    /// 評価者が wait を記録していなければ None（欠測。0 秒にしない）。
    human_wait: Option<Spans>,
    /// job 観測が無ければ None。
    queue_wait: Option<Spans>,
    external_run: Option<Spans>,
    jobs_deduped: u32,
    rework: BTreeMap<&'static str, u64>,
    rework_after_window: u64,
    rework_secs: Spans,
    clock_unknown: u32,
    approximate: u32,
    bases: BTreeMap<String, u64>,
    cutoff: Option<i64>,
    window: Window,
}

fn eval_trial(recs: &[&HumanOpRecord], trial_end: Option<i64>) -> TrialEval {
    let owned: Vec<HumanOpRecord> = recs.iter().map(|r| (*r).clone()).collect();
    let cutoff = recs
        .iter()
        .filter(|r| r.record_type == RecordType::Cutoff)
        .filter_map(|r| r.occurred_at.as_deref().and_then(ts))
        .min();
    let window = match (cutoff, trial_end) {
        (Some(c), _) => Window {
            end: Some(c),
            basis: "cutoff",
        },
        (None, Some(e)) => Window {
            end: Some(e),
            basis: "trial_end",
        },
        (None, None) => Window {
            end: None,
            basis: "none",
        },
    };
    let limit = window.end;
    let in_window = |t: Option<i64>| match (t, limit) {
        (Some(t), Some(l)) => t < l,
        _ => true,
    };
    let mut t = TrialEval {
        interventions: BTreeMap::new(),
        after_window_ops: 0,
        automatic_ops: 0,
        protocol_ops: 0,
        human_time: Spans::default(),
        human_wait: None,
        queue_wait: None,
        external_run: None,
        jobs_deduped: 0,
        rework: BTreeMap::new(),
        rework_after_window: 0,
        rework_secs: Spans::default(),
        clock_unknown: 0,
        approximate: 0,
        bases: BTreeMap::new(),
        cutoff,
        window,
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
        if !counts_as_human_op(&rec) {
            if rec.mode == Some(Mode::Automatic) {
                t.automatic_ops += 1;
            } else {
                t.protocol_ops += 1;
            }
            continue;
        }
        // 窓の外で始まった op は介入件数にも時間にも入れない（Spans も同じ窓で after_cutoff に数える）。
        if !in_window(span.start.as_deref().and_then(ts)) {
            t.after_window_ops += 1;
            t.human_time
                .add(kind, span.start.as_deref(), span.end.as_deref(), limit);
            continue;
        }
        *t.interventions.entry(kind.to_string()).or_default() += 1;
        t.human_time
            .add(kind, span.start.as_deref(), span.end.as_deref(), limit);
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
    if !waits.is_empty() {
        let mut sp = Spans::default();
        for (key, (s, e)) in waits {
            let kind = key.rsplit('|').next().unwrap_or("wait").to_string();
            sp.add(
                &format!("human_wait:{}", kind.trim_matches(|c| c == '"' || c == ')')),
                s.as_deref(),
                e.as_deref(),
                limit,
            );
        }
        t.human_wait = Some(sp);
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
        t.queue_wait.get_or_insert_with(Spans::default).add(
            &format!("external_queue_wait:{job}"),
            r.job_submit.as_deref(),
            r.job_start.as_deref(),
            limit,
        );
        t.external_run.get_or_insert_with(Spans::default).add(
            &format!("external_run:{job}"),
            r.job_start.as_deref(),
            r.job_end.as_deref(),
            limit,
        );
    }
    // やり直し: 同じ論理作業の attempt n+1 があるとき、n は再作業。原因は n+1 の op（intent）から。
    // 共通 filter で数えない op（protocol_step・automatic）を含む論理作業は再作業に数えない
    // （protocol 上の trial-start attempt=2 は科学的な redo ではない）。attempt n+1 が窓の外で始まれば数えない。
    let excluded: BTreeSet<&String> = owned
        .iter()
        .filter(|r| matches!(r.record_type, RecordType::OpStart | RecordType::OpEnd))
        .filter(|r| !counts_as_human_op(r))
        .filter_map(|r| r.logical_op_id.as_ref())
        .collect();
    let mut ops_by: BTreeMap<(String, u32), &HumanOpRecord> = BTreeMap::new();
    for r in owned
        .iter()
        .filter(|r| r.record_type == RecordType::OpStart)
    {
        if let (Some(l), Some(a)) = (&r.logical_op_id, r.attempt) {
            ops_by.entry((l.clone(), a)).or_insert(r);
        }
    }
    let mut attempts: BTreeMap<String, BTreeMap<u32, Option<i64>>> = BTreeMap::new();
    for r in owned
        .iter()
        .filter(|r| matches!(r.record_type, RecordType::OpStart | RecordType::Attempt))
    {
        if let (Some(l), Some(a)) = (&r.logical_op_id, r.attempt) {
            if excluded.contains(l) {
                continue;
            }
            let st = r.start.as_deref().and_then(ts);
            let e = attempts
                .entry(l.clone())
                .or_default()
                .entry(a)
                .or_insert(st);
            if e.is_none() {
                *e = st;
            }
        }
    }
    for (l, set) in &attempts {
        let v: Vec<(u32, Option<i64>)> = set.iter().map(|(a, s)| (*a, *s)).collect();
        for w in v.windows(2) {
            if !in_window(w[1].1) {
                t.rework_after_window += 1;
                continue;
            }
            let cause = rework_cause(ops_by.get(&(l.clone(), w[1].0)).copied());
            *t.rework.entry(cause).or_default() += 1;
            // 失敗した前の attempt の所要時間（attempt 行）を再作業時間として数える。
            for a in owned.iter().filter(|r| {
                r.record_type == RecordType::Attempt
                    && r.logical_op_id.as_deref() == Some(l)
                    && r.attempt == Some(w[0].0)
            }) {
                t.rework_secs
                    .add(cause, a.start.as_deref(), a.end.as_deref(), limit);
            }
        }
    }
    t
}

/// Celeris の応答待ち（join の `event_waits`）を同じ観測窓で区間にする。
fn event_wait_spans(j: &Value, limit: Option<i64>) -> Option<Spans> {
    if j["source"] == "missing" {
        return None;
    }
    let mut sp = Spans::default();
    for w in j["value"]["items"].as_array().into_iter().flatten() {
        sp.add(
            &format!(
                "celeris_wait:{}",
                w["wait_kind"].as_str().unwrap_or("unknown")
            ),
            w["started_at"].as_str(),
            w["ended_at"].as_str(),
            limit,
        );
    }
    Some(sp)
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
    let (recs, corrections) =
        resolve_with_history(&read_records(human_ops_file).map_err(|e| e.to_string())?)
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
        // 観測窓の終端（cutoff が無いとき）: 試行の開始 + wall（どちらも観測できたときだけ）。
        let started = jr["started_at"]["value"].as_i64();
        let wall = &jr["wall_secs"]["value"];
        let trial_end = started.and_then(|st| {
            (wall["missing"] == 0)
                .then(|| wall["observed_sum"].as_i64())
                .flatten()
                .map(|w| st + w)
        });
        let ev = by_trial.get(&key).map(|r| eval_trial(r, trial_end));
        let window = ev.as_ref().map(|e| e.window).unwrap_or(Window {
            end: trial_end,
            basis: if trial_end.is_some() {
                "trial_end"
            } else {
                "none"
            },
        });
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
        // 完遂 = 成功契約の validated success（executor passed AND 独立採点 final_pass）。executor の状態は別欄。
        let validated = jr["validated_success"]["value"].as_bool();
        t.insert(
            "completion".into(),
            match validated {
                Some(v) => field(json!(v), "grade"),
                None => missing(
                    "成功契約の判定が無い（runner の out dir（campaign.json・grades.jsonl）を --events に渡していない、または cell の記録が無い）",
                ),
            },
        );
        for k in [
            "success_class",
            "executor_status",
            "executor_reason",
            "canonical_trial_no",
            "executor_trials",
            "wall_secs",
            "tokens",
            "cost_micro_usd",
            "auto_retries",
        ] {
            t.insert(k.into(), jr[k].clone());
        }
        t.insert(
            "observation_window".into(),
            json!({"end": window.end.map(rfc), "basis": window.basis,
                   "rule": crate::success::OBSERVATION_WINDOW}),
        );
        let celeris_wait = event_wait_spans(&jr["event_waits"], window.end);
        t.insert(
            "celeris_wait".into(),
            celeris_wait.as_ref().map_or_else(
                || missing("Celeris の応答待ち event が無い（A・export 未入力）"),
                |s| s.json("event"),
            ),
        );
        let nomiss = |what: &str| missing(&format!("この試行に評価者記録が無い（{what}）"));
        let spans_or = |s: &Option<Spans>, why: &str| {
            s.as_ref()
                .map_or_else(|| missing(why), |s| s.json("evaluator"))
        };
        match &ev {
            Some(e) => {
                t.insert(
                    "interventions".into(),
                    field(counts_json(&e.interventions), "evaluator"),
                );
                t.insert(
                    "excluded_ops".into(),
                    json!({"automatic": e.automatic_ops, "protocol": e.protocol_ops,
                           "after_window": e.after_window_ops}),
                );
                t.insert("human_time".into(), e.human_time.json("evaluator"));
                t.insert(
                    "human_wait".into(),
                    spans_or(
                        &e.human_wait,
                        "評価者の wait 記録が無い（観測なし。0 秒ではない）",
                    ),
                );
                t.insert(
                    "external_queue_wait".into(),
                    spans_or(&e.queue_wait, "外部 job の観測が無い"),
                );
                t.insert(
                    "external_run".into(),
                    spans_or(&e.external_run, "外部 job の観測が無い"),
                );
                t.insert(
                    "internal_wait".into(),
                    missing(
                        "ready_not_running は event 由来で、現在の trials/ledger/export には出ない",
                    ),
                );
                t.insert(
                    "rework".into(),
                    json!({"value": {"count": e.rework, "after_window": e.rework_after_window,
                                     "failed_attempt_secs": e.rework_secs.json("evaluator")["value"]},
                           "source": "evaluator"}),
                );
                t.insert(
                    "time_quality".into(),
                    json!({"clock_unknown_records": e.clock_unknown, "approximate_records": e.approximate,
                           "time_basis": e.bases, "jobs_deduped": e.jobs_deduped,
                           "observation_cutoff": e.cutoff.map(rfc)}),
                );
                let empty = Spans::default();
                for (kind, s, en, status) in e
                    .human_time
                    .items
                    .iter()
                    .map(|i| ("human_op", i))
                    .chain(
                        e.human_wait
                            .as_ref()
                            .unwrap_or(&empty)
                            .items
                            .iter()
                            .map(|i| ("human_wait", i)),
                    )
                    .chain(
                        e.queue_wait
                            .as_ref()
                            .unwrap_or(&empty)
                            .items
                            .iter()
                            .map(|i| ("external_queue_wait", i)),
                    )
                    .chain(
                        e.external_run
                            .as_ref()
                            .unwrap_or(&empty)
                            .items
                            .iter()
                            .map(|i| ("external_run", i)),
                    )
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
        for (_, s, en, status) in celeris_wait.iter().flat_map(|w| w.items.iter()) {
            let sort = s.or(*en).unwrap_or(i64::MAX);
            timeline.push((
                sort,
                "celeris_wait".into(),
                json!({"manifest_sha256": key.0, "case_id": key.1, "arm": arm_label,
                       "rep": key.3, "lane": "celeris_wait", "start": s.map(rfc), "end": en.map(rfc),
                       "status": status, "source": "event"}),
            ));
        }
        if let Some(st) = started {
            timeline.push((
                st,
                "trial".into(),
                json!({"manifest_sha256": key.0, "case_id": key.1, "arm": arm_label,
                       "rep": key.3, "lane": "trial", "start": rfc(st), "end": trial_end.map(rfc),
                       "status": if trial_end.is_some() { "closed" } else { "open_or_wall_missing" },
                       "source": "event"}),
            ));
        }
        t.insert("join".into(), jr.clone());

        // arm 集計（分母 = 行の数。manifest を渡せば計画 cell すべて）。
        let a = arms.entry(arm_label.clone()).or_default();
        let bump = |a: &mut Map<String, Value>, k: &str, n: u64| {
            let v = a.get(k).and_then(Value::as_u64).unwrap_or(0) + n;
            a.insert(k.into(), json!(v));
        };
        let tally = |a: &mut Map<String, Value>, k: &str, item: &str, n: u64| {
            let m = a.entry(k.to_string()).or_insert_with(|| json!({}));
            m[item] = json!(m[item].as_u64().unwrap_or(0) + n);
        };
        bump(a, "trials", 1);
        match validated {
            Some(true) => bump(a, "validated_success", 1),
            Some(false) => {}
            None => bump(a, "validated_success_missing_trials", 1),
        }
        if jr["executor_status"]["value"] == "passed" {
            bump(a, "executor_passed", 1);
        }
        let class = jr["success_class"]["value"].as_str().unwrap_or("unknown");
        tally(a, "success_classes", class, 1);
        if jr["success_class"]["needs_regrade"] == true {
            bump(a, "needs_regrade", 1);
        }
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
        match &jr["auto_retries"]["value"]["count"] {
            Value::Number(n) => bump(a, "auto_retries", n.as_u64().unwrap_or(0)),
            _ => bump(a, "auto_retries_missing_trials", 1),
        }
        match &celeris_wait {
            Some(w) => bump(
                a,
                "celeris_wait_union_secs",
                union_and_sum(&w.closed).0 as u64,
            ),
            None => bump(a, "celeris_wait_missing_trials", 1),
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
                    (&e.human_wait, "human_wait"),
                    (&e.queue_wait, "external_queue_wait"),
                    (&e.external_run, "external_run"),
                ] {
                    match src {
                        Some(sp) => bump(
                            a,
                            &format!("{name}_union_secs"),
                            union_and_sum(&sp.closed).0 as u64,
                        ),
                        None => bump(a, &format!("{name}_missing_trials"), 1),
                    }
                }
                for (k, n) in &e.interventions {
                    tally(a, "interventions", k, *n);
                }
                for (k, n) in &e.rework {
                    tally(a, "rework", k, *n);
                }
                for (k, n) in &e.bases {
                    tally(a, "time_basis", k, *n);
                }
            }
            None => bump(a, "trials_without_evaluator_records", 1),
        }
        trials.push(Value::Object(t));
    }

    let shared_json: Vec<Value> = shared
        .iter()
        .map(|(campaign, rs)| {
            let e = eval_trial(rs, None);
            json!({"campaign_id": campaign, "records": rs.len(),
                   "interventions": e.interventions, "human_time": e.human_time.json("evaluator")["value"],
                   "attribution": "arm へ配分しない（shared scope: case/arm/rep の identity が無い記録。1 度だけ計上）"})
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
        "success_contract": {
            "schema": crate::success::SUCCESS_CONTRACT,
            "success": "executor passed AND bound independent grade final_pass (same function as checker/export)",
            "retry_selection": crate::success::RETRY_SELECTION,
            "denominator": crate::success::DENOMINATOR,
            "observation_window": crate::success::OBSERVATION_WINDOW,
            "human_op_filter": "protocol_step / intent=protocol / mode=automatic are excluded from interventions, human time and rework alike",
        },
        "cost_scope": crate::export::COST_SCOPE,
        "trials": trials,
        "arms": arms,
        "shared_overhead": shared_json,
        "corrections": corrections,
        "other_rows": other_rows,
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
        "shared scope の作業（共有 env 修正・CoS 等）は arm へ配分せず、shared_overhead に 1 度だけ別掲する。記録が無いものは欠測で 0 ではない。",
        "internal_wait（ready_not_running）は現在の入力に無く欠測。external_queue_wait はクラスタ混雑に依存しシステム性能として帰属しない。",
        "完遂は成功契約の validated success（executor passed AND 独立採点 final_pass）で、executor の状態とは別欄。試行数は小さく統計的有意性は主張しない。",
        "auto_retries は Celeris が自動で作った再試行 run で、人の再作業（rework）には数えない。",
        "消費（tokens・費用）は試行の run だけで、CoS の call と共有 overhead は含まない（cost_scope）。",
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
    m += "\n## arm 別\n\n| arm | 試行 | validated success | executor passed | 結論(区分別) | 介入(区分別) | 人の時間 union 秒 (単純和) | 下限の試行 | 人待ち秒 (欠測試行) | Celeris 待ち秒 (欠測試行) | 外部 queue 秒 (欠測試行) | 再作業(原因別) | 自動 retry | tokens 観測合計 (欠測試行) | 費用 µUSD 観測合計 (欠測試行) | 評価者記録なし | time_basis |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n";
    for (arm, a) in j["arms"].as_object().into_iter().flatten() {
        let o = |k: &str| a.get(k).map_or("-".into(), |v| v.to_string());
        m += &format!(
            "| {arm} | {} | {} | {} | {} | {} | {} ({}) | {} | {} ({}) | {} ({}) | {} ({}) | {} | {} | {} ({}) | {} ({}) | {} | {} |\n",
            cell(a, "trials"),
            cell(a, "validated_success"),
            cell(a, "executor_passed"),
            o("success_classes"),
            o("interventions"),
            cell(a, "human_union_secs"),
            cell(a, "human_sum_secs"),
            cell(a, "human_time_lower_bound_trials"),
            cell(a, "human_wait_union_secs"),
            cell(a, "human_wait_missing_trials"),
            cell(a, "celeris_wait_union_secs"),
            cell(a, "celeris_wait_missing_trials"),
            cell(a, "external_queue_wait_union_secs"),
            cell(a, "external_queue_wait_missing_trials"),
            o("rework"),
            cell(a, "auto_retries"),
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
    m += "\n## 補正履歴（append-only correction）\n\n";
    let cs = j["corrections"].as_array().cloned().unwrap_or_default();
    if cs.is_empty() {
        m += "なし。\n";
    }
    for c in &cs {
        m += &format!(
            "- {} → {}: {}（{}）\n",
            c["correction_id"], c["corrects"], c["set"], c["note"]
        );
    }
    m += "\n## 試行別\n\n| arm | case | rep | 完遂 | 結論 | 人の時間 union 秒 | open/欠測始点 | 人待ち秒 | 外部 queue 秒 | 再作業 | tokens |\n|---|---|---|---|---|---|---|---|---|---|---|\n";
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
            src(&t["success_class"]),
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
