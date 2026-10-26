//! 生記録（human-ops JSONL）と Celeris 側の記録を同じ identity で結合する読み出し（集計・表は作らない）。
//! 定義は `docs/ops/eval-raw-record-definitions.md` §4。出力は 1 試行 identity（manifest・case・arm・rep）につき 1 行の JSONL。
//!
//! - 各欄は `{"value": ..., "source": "event" | "evaluator" | "missing"}`。Celeris 側から取れる区間は再記録せず導出する。
//! - どちらにも無い項目は `missing`（value は null）。0 に変換しない。`missing_fields` に欄名を並べる。
//! - 再開・重複: events は `dedup_key` / `run_id` / 台帳の鍵で、評価者記録は `record_id` で 1 件に潰す。行は identity ごとに 1 つ。
//! - 成否は成功契約（`crate::success`）: runner の out dir（`campaign.json` がある dir）を渡すと、manifest の計画 cell を
//!   すべて行にし、checker と同じ規則（trial_no 最小・結合検査を通った採点）で `executor_status` と `validated_success` を別に出す。
//!   manifest が無い入力では `validated_success` は欠測（executor passed を成功に読み替えない）。
//! - B の通常 event: 人の応答待ち（export の `wait` 行）・自動 retry（`run` 行の `is_retry`）・`celeris_event_ref` の照合を試行に結ぶ。
//! - `--events` は繰り返せる。runner の out dir（`trials.jsonl`・`ledger.jsonl`）と、`celerisctl campaign export` の出力 dir（decision-log の `records.jsonl`）を両方渡す。file 単独も可。
//!   `celeris-campaign export` は table/final-accounting だけを書き、この 3 file は出さない。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Map, Value, json};

use crate::{
    human_ops::{
        HumanOpRecord, RecordType, TimingSpan, op_spans, read_records, redo_counts, resolve,
    },
    manifest::ConfigId,
};

type Key = (String, String, ConfigId, u32);

const EVENT: &str = "event";
const EVALUATOR: &str = "evaluator";
const MISSING: &str = "missing";
/// 成功契約（`crate::success`。結合検査を通った独立採点）由来。
const GRADE: &str = "grade";

fn field(value: Value, source: &str) -> Value {
    json!({"value": value, "source": source})
}

fn missing() -> Value {
    field(Value::Null, MISSING)
}

fn config_of(s: &str) -> Option<ConfigId> {
    match s.chars().next()? {
        'A' => Some(ConfigId::A),
        'B' => Some(ConfigId::B),
        'C' => Some(ConfigId::C),
        _ => None,
    }
}

fn config_str(c: ConfigId) -> &'static str {
    match c {
        ConfigId::A => "A",
        ConfigId::B => "B",
        ConfigId::C => "C",
    }
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))
        })
        .collect()
}

/// Celeris 側から読んだ、1 試行 identity ぶんの事実。
#[derive(Default)]
struct EventSide {
    /// export の run 行を見たか（Celeris が走らせた試行）。無ければ event 由来の介入件数は欠測。
    has_records: bool,
    campaign_id: Option<String>,
    /// trial_no → wall_secs（TrialRecord。0 が欠測の試行は None）。
    wall: BTreeMap<u64, Option<u64>>,
    /// trial_no → TrialRecord の結果（outcome・reason・tokens・cost_micro_usd）。未実行の試行の tokens/cost は欠測。
    results: BTreeMap<u64, TrialResultFacts>,
    started_at: Option<u64>,
    submitted_at: Option<u64>,
    task_ids: BTreeSet<String>,
    run_ids: BTreeSet<String>,
    /// run_id → wall_ms（outcome 行）。
    run_wall_ms: BTreeMap<String, Option<u64>>,
    /// intervention_id → kind。
    interventions: BTreeMap<String, String>,
    /// 人の応答待ち（export の `wait` 行）。
    waits: Vec<WaitFact>,
    /// Celeris が自動で作った再試行 run（`is_retry`）: run_id → (parent_run_id, attempt_no)。人の再作業ではない。
    auto_retries: BTreeMap<String, (Option<String>, Option<u64>)>,
    /// この試行の event 参照（介入の source_event・待機の開始/終了 event・`run:<run_id>`）。
    refs: BTreeSet<String>,
}

#[derive(Clone)]
struct WaitFact {
    kind: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    start_event: Option<String>,
    end_event: Option<String>,
}

struct TrialResultFacts {
    outcome: String,
    reason: Option<String>,
    tokens: Option<u64>,
    cost_micro_usd: Option<u64>,
}

/// export の介入行: (intervention_id, task_id, campaign_id, kind, source_event)。
type EventIntervention = (
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

#[derive(Default)]
struct Events {
    by_key: BTreeMap<Key, EventSide>,
    /// 試行に結べなかった介入（task_id が試行の run に無い・campaign 識別なし）。
    unjoined_interventions: BTreeMap<String, (Option<String>, String)>,
    interventions: Vec<EventIntervention>,
    waits: Vec<(Option<String>, WaitFact)>,
    /// runner の out dir から読んだ成功契約の判定。
    canonical: Vec<crate::success::Canonical>,
    /// campaign_id → manifest sha（評価者記録から）。
    campaign_sha: BTreeMap<String, String>,
}

impl Events {
    /// `--events` は繰り返せる。各入力は dir（既知の 3 file のうちあるもの）か file。
    /// 全入力を通して 1 つも読めなければエラー。
    fn load(paths: &[&Path]) -> Result<Vec<(String, Vec<Value>)>, String> {
        let mut out = Vec::new();
        for path in paths {
            let files: Vec<(String, std::path::PathBuf)> = if path.is_dir() {
                if path.join(crate::record::CAMPAIGN_FILE).is_file() {
                    out.push((
                        CANONICAL_DIR.into(),
                        vec![json!(path.display().to_string())],
                    ));
                }
                ["records.jsonl", "trials.jsonl", "ledger.jsonl"]
                    .iter()
                    .map(|n| (n.to_string(), path.join(n)))
                    .filter(|(_, p)| p.is_file())
                    .collect()
            } else if path.is_file() {
                let n = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                vec![(n, path.to_path_buf())]
            } else {
                return Err(format!("{}: events の入力が無い", path.display()));
            };
            for (n, p) in files {
                out.push((n, read_jsonl(&p)?));
            }
        }
        if out.is_empty() {
            return Err(format!(
                "{}: records.jsonl / trials.jsonl / ledger.jsonl のどれも無い",
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(out)
    }
}

/// `Events::load` の出力で、成功契約を読む runner out dir を表す名前。
const CANONICAL_DIR: &str = "<campaign out dir>";

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn build_events(
    inputs: Vec<(String, Vec<Value>)>,
    campaign_sha: &BTreeMap<String, String>,
) -> Result<Events, String> {
    let mut ev = Events {
        campaign_sha: campaign_sha.clone(),
        ..Events::default()
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut outcome_wall: BTreeMap<String, Option<u64>> = BTreeMap::new();
    let mut run_keys: Vec<(String, Key)> = Vec::new();
    for (name, rows) in inputs {
        if name == CANONICAL_DIR {
            for r in &rows {
                let dir = r.as_str().unwrap_or_default();
                if let Some(c) = crate::success::load(Path::new(dir))?
                    && !ev
                        .canonical
                        .iter()
                        .any(|x| x.manifest_sha256 == c.manifest_sha256)
                {
                    ev.canonical.push(c);
                }
            }
            continue;
        }
        for r in rows {
            if let Some(rt) = r.get("record_type").and_then(Value::as_str) {
                // export の records.jsonl: dedup_key で潰す（再 export・重複行で増やさない）。
                let dk = s(&r, "dedup_key").unwrap_or_else(|| r.to_string());
                if !seen.insert(dk) {
                    continue;
                }
                match rt {
                    "run" => {
                        let (Some(run_id), Some(case), Some(arm)) =
                            (s(&r, "run_id"), s(&r, "case_id"), s(&r, "arm"))
                        else {
                            continue;
                        };
                        let Some(cfg) = config_of(&arm) else { continue };
                        let rep = u(&r, "repetition").unwrap_or(1) as u32;
                        let campaign = s(&r, "campaign_id");
                        let sha = s(&r, "manifest_hash")
                            .or_else(|| {
                                campaign
                                    .as_ref()
                                    .and_then(|c| ev.campaign_sha.get(c).cloned())
                            })
                            .or_else(|| campaign.as_ref().map(|c| format!("campaign:{c}")));
                        let Some(sha) = sha else { continue };
                        let key = (sha, case, cfg, rep);
                        let side = ev.by_key.entry(key.clone()).or_default();
                        side.campaign_id = campaign.or(side.campaign_id.take());
                        side.has_records = true;
                        side.run_ids.insert(run_id.clone());
                        if let Some(t) = s(&r, "task_id") {
                            side.task_ids.insert(t);
                        }
                        side.refs.insert(format!("run:{run_id}"));
                        if r.get("is_retry").and_then(Value::as_bool) == Some(true) {
                            side.auto_retries.insert(
                                run_id.clone(),
                                (s(&r, "parent_run_id"), u(&r, "attempt_no")),
                            );
                        }
                        run_keys.push((run_id, key));
                    }
                    "wait" => {
                        ev.waits.push((
                            s(&r, "task_id"),
                            WaitFact {
                                kind: s(&r, "wait_kind").unwrap_or_else(|| "unknown".into()),
                                started_at: s(&r, "started_at"),
                                ended_at: s(&r, "ended_at"),
                                start_event: s(&r, "start_event"),
                                end_event: s(&r, "end_event"),
                            },
                        ));
                    }
                    "outcome" => {
                        if let Some(run_id) = s(&r, "run_id") {
                            outcome_wall.insert(run_id, u(&r, "wall_ms"));
                        }
                    }
                    "intervention" => {
                        if let Some(id) = s(&r, "intervention_id") {
                            ev.interventions.push((
                                id,
                                s(&r, "task_id"),
                                s(&r, "campaign_id"),
                                s(&r, "kind").unwrap_or_else(|| "unclassified".into()),
                                s(&r, "source_event"),
                            ));
                        }
                    }
                    _ => {}
                }
            } else if name.starts_with("trials") {
                let (Some(sha), Some(case), Some(cfg)) = (
                    s(&r, "campaign_sha256"),
                    s(&r, "problem_id"),
                    s(&r, "config").and_then(|c| config_of(&c)),
                ) else {
                    continue;
                };
                let rep = u(&r, "rep").unwrap_or(1) as u32;
                let trial_no = u(&r, "trial_no").unwrap_or(0);
                let outcome = s(&r, "outcome").unwrap_or_default();
                let wall = u(&r, "wall_secs");
                // 実行しなかった・executor の失敗の 0 は欠測（定義 §6）。
                let unexecuted = s(&r, "workspace").is_none_or(|w| w.is_empty())
                    || outcome == "blocked"
                    || (outcome == "error" && wall == Some(0));
                let side = ev.by_key.entry((sha, case, cfg, rep)).or_default();
                side.wall
                    .insert(trial_no, if unexecuted { None } else { wall });
                side.results.insert(
                    trial_no,
                    TrialResultFacts {
                        outcome: outcome.clone(),
                        reason: s(&r, "reason"),
                        tokens: if unexecuted { None } else { u(&r, "tokens") },
                        cost_micro_usd: if unexecuted {
                            None
                        } else {
                            u(&r, "cost_micro_usd")
                        },
                    },
                );
            } else if name.starts_with("ledger") {
                let (Some(sha), Some(case), Some(cfg)) = (
                    s(&r, "manifest_sha256"),
                    s(&r, "problem_id"),
                    s(&r, "arm").and_then(|c| config_of(&c)),
                ) else {
                    continue;
                };
                let rep = u(&r, "rep").unwrap_or(1) as u32;
                let key = (sha, case, cfg, rep);
                let side = ev.by_key.entry(key.clone()).or_default();
                if let Some(t) = s(&r, "task_id") {
                    side.task_ids.insert(t);
                }
                if let Some(run) = s(&r, "run_id") {
                    side.run_ids.insert(run.clone());
                    run_keys.push((run, key));
                }
                // 台帳は状態ごとに行が増える: 最初の観測を採る。
                side.started_at = side.started_at.or(u(&r, "started_at"));
                side.submitted_at = side.submitted_at.or(u(&r, "submitted_at"));
            }
        }
    }
    for (run, key) in run_keys {
        if let (Some(w), Some(side)) = (outcome_wall.get(&run), ev.by_key.get_mut(&key)) {
            side.run_wall_ms.insert(run, *w);
        }
    }
    // 介入を試行へ: task_id が試行の task にある場合だけ結ぶ。
    let waits = std::mem::take(&mut ev.waits);
    for (task, w) in waits {
        let target = task.as_ref().and_then(|t| {
            ev.by_key
                .iter_mut()
                .find(|(_, side)| side.task_ids.contains(t))
        });
        if let Some((_, side)) = target {
            side.refs
                .extend(w.start_event.iter().chain(&w.end_event).cloned());
            side.waits.push(w);
        }
    }
    let ivs = std::mem::take(&mut ev.interventions);
    for (id, task, campaign, kind, source) in ivs {
        let target = task.as_ref().and_then(|t| {
            ev.by_key
                .iter()
                .find(|(_, side)| side.task_ids.contains(t))
                .map(|(k, _)| k.clone())
        });
        match target {
            Some(k) => {
                if let Some(side) = ev.by_key.get_mut(&k) {
                    side.refs.extend(source);
                    side.interventions.insert(id, kind);
                }
            }
            None => {
                ev.unjoined_interventions.insert(id, (campaign, kind));
            }
        }
    }
    Ok(ev)
}

fn counts(items: impl Iterator<Item = String>) -> Value {
    let mut m: BTreeMap<String, u64> = BTreeMap::new();
    for i in items {
        *m.entry(i).or_default() += 1;
    }
    json!(m)
}

/// 評価者記録の要約（同じ identity の解決済み行）。
fn evaluator_fields(recs: &[HumanOpRecord], row: &mut Map<String, Value>) {
    let spans = op_spans(recs);
    if recs.is_empty() {
        for f in [
            "human_ops",
            "human_op_secs",
            "redo",
            "human_wait",
            "job_observations",
        ] {
            row.insert(f.into(), missing());
        }
        return;
    }
    row.insert(
        "human_ops".into(),
        field(
            counts(
                spans
                    .iter()
                    .map(|(r, _)| r.op_kind.map_or("unclassified", |k| k.as_str()).to_string()),
            ),
            EVALUATOR,
        ),
    );
    // 所要時間: 端が欠ける op は合計に入れず欠測数を並べる。
    let ts: Vec<TimingSpan> = spans.iter().map(|(_, t)| t.clone()).collect();
    let d = crate::human_ops::DurationStat::from_spans(&ts);
    row.insert(
        "human_op_secs".into(),
        if ts.is_empty() {
            missing()
        } else {
            field(
                json!({"observed_sum": d.observed_sum, "observed": d.observed, "missing": d.missing}),
                EVALUATOR,
            )
        },
    );
    let redo: u32 = redo_counts(recs).values().sum();
    let has_attempts = recs.iter().any(|r| r.attempt.is_some());
    row.insert(
        "redo".into(),
        if has_attempts {
            field(json!(redo), EVALUATOR)
        } else {
            missing()
        },
    );
    let n = |t: RecordType| recs.iter().filter(|r| r.record_type == t).count();
    for (name, t) in [
        ("human_wait", RecordType::WaitStart),
        ("job_observations", RecordType::JobObservation),
    ] {
        row.insert(
            name.into(),
            if n(t) == 0 {
                missing()
            } else {
                field(json!(n(t)), EVALUATOR)
            },
        );
    }
}

fn event_fields(side: Option<&EventSide>, row: &mut Map<String, Value>) {
    let Some(side) = side else {
        for f in [
            "wall_secs",
            "executor_status",
            "executor_reason",
            "tokens",
            "cost_micro_usd",
            "started_at",
            "submitted_at",
            "run_ids",
            "run_wall_ms",
            "event_interventions",
            "event_waits",
            "auto_retries",
        ] {
            row.insert(f.into(), missing());
        }
        return;
    };
    let observed: Vec<u64> = side.wall.values().filter_map(|w| *w).collect();
    row.insert(
        "wall_secs".into(),
        if observed.is_empty() {
            missing()
        } else {
            field(
                json!({"observed_sum": observed.iter().sum::<u64>(), "observed": observed.len(),
                       "missing": side.wall.len() - observed.len()}),
                EVENT,
            )
        },
    );
    // 実行器の状態は成功契約と同じ最初の trial_no（後の trial_no は判定に使わない）。tokens・費用は観測できた試行の合計と欠測数。
    let first = side.results.values().next();
    row.insert(
        "executor_status".into(),
        first.map_or_else(missing, |r| field(json!(r.outcome), EVENT)),
    );
    row.insert(
        "executor_reason".into(),
        first
            .and_then(|r| r.reason.as_ref())
            .map_or_else(missing, |x| field(json!(x), EVENT)),
    );
    for (name, pick) in [
        (
            "tokens",
            (|r: &TrialResultFacts| r.tokens) as fn(&TrialResultFacts) -> Option<u64>,
        ),
        ("cost_micro_usd", |r: &TrialResultFacts| r.cost_micro_usd),
    ] {
        let obs: Vec<u64> = side.results.values().filter_map(pick).collect();
        row.insert(
            name.into(),
            if obs.is_empty() {
                missing()
            } else {
                field(
                    json!({"observed_sum": obs.iter().sum::<u64>(), "observed": obs.len(),
                           "missing": side.results.len() - obs.len()}),
                    EVENT,
                )
            },
        );
    }
    for (name, v) in [
        ("started_at", side.started_at),
        ("submitted_at", side.submitted_at),
    ] {
        row.insert(
            name.into(),
            v.map_or_else(missing, |x| field(json!(x), EVENT)),
        );
    }
    row.insert(
        "run_ids".into(),
        if side.run_ids.is_empty() {
            missing()
        } else {
            field(json!(side.run_ids), EVENT)
        },
    );
    let walls: Vec<u64> = side.run_wall_ms.values().filter_map(|w| *w).collect();
    row.insert(
        "run_wall_ms".into(),
        if walls.is_empty() {
            missing()
        } else {
            field(
                json!({"observed_sum": walls.iter().sum::<u64>(), "observed": walls.len(),
                       "missing": side.run_wall_ms.len() - walls.len()}),
                EVENT,
            )
        },
    );
    // projection の介入は件数と区分だけ（wall_ms は持たない: 所要時間は評価者記録）。
    row.insert(
        "event_interventions".into(),
        if side.has_records {
            field(counts(side.interventions.values().cloned()), EVENT)
        } else {
            missing()
        },
    );
    // 人の応答待ち（Question / Approval）。export の run 行が無い試行（A・未 export）は欠測で、0 件と区別する。
    row.insert(
        "event_waits".into(),
        if side.has_records {
            let items: Vec<Value> = side
                .waits
                .iter()
                .map(|w| {
                    json!({"wait_kind": w.kind, "started_at": w.started_at, "ended_at": w.ended_at,
                           "start_event": w.start_event, "end_event": w.end_event})
                })
                .collect();
            field(
                json!({"count": counts(side.waits.iter().map(|w| w.kind.clone())),
                       "open": side.waits.iter().filter(|w| w.ended_at.is_none()).count(),
                       "items": items}),
                EVENT,
            )
        } else {
            missing()
        },
    );
    // Celeris が自動で作った再試行 run（人の再作業ではない）。
    row.insert(
        "auto_retries".into(),
        if side.has_records {
            let items: Vec<Value> = side
                .auto_retries
                .iter()
                .map(|(run, (parent, attempt))| {
                    json!({"run_id": run, "parent_run_id": parent, "attempt_no": attempt,
                           "wall_ms": side.run_wall_ms.get(run).copied().flatten()})
                })
                .collect();
            field(
                json!({"count": side.auto_retries.len(), "runs": items}),
                EVENT,
            )
        } else {
            missing()
        },
    );
}

/// 成功契約の判定（runner の out dir がある入力だけ）。無ければ欠測（executor passed を成功に読み替えない）。
fn canonical_fields(cell: Option<&crate::success::Cell>, row: &mut Map<String, Value>) {
    let Some(c) = cell else {
        for f in [
            "validated_success",
            "success_class",
            "canonical_trial_no",
            "executor_trials",
        ] {
            row.insert(f.into(), missing());
        }
        return;
    };
    let class = c.class;
    row.insert(
        "validated_success".into(),
        if class == crate::success::SuccessClass::Missing {
            missing()
        } else {
            field(json!(class.is_success()), GRADE)
        },
    );
    row.insert(
        "success_class".into(),
        json!({"value": class.as_str(), "source": GRADE,
               "needs_regrade": class.needs_regrade(), "unexecuted": c.unexecuted, "ran": c.ran,
               "contract": crate::success::SUCCESS_CONTRACT}),
    );
    row.insert(
        "canonical_trial_no".into(),
        c.canonical_trial_no
            .map_or_else(missing, |n| field(json!(n), GRADE)),
    );
    row.insert(
        "executor_trials".into(),
        field(json!(c.recorded_trials), EVENT),
    );
    // 実行器の状態は canonical の試行の値で上書きする（trials.jsonl の同じ行）。
    if let Some(st) = c.executor_status {
        row.insert("executor_status".into(), field(json!(st), EVENT));
    }
}

/// 結合して JSONL（末尾改行つき）を返す。
pub fn join(human_ops_file: &Path, events: &[&Path]) -> Result<String, String> {
    let recs = resolve(&read_records(human_ops_file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut campaign_sha: BTreeMap<String, String> = BTreeMap::new();
    for r in &recs {
        campaign_sha.insert(r.campaign_id.clone(), r.manifest_sha256.clone());
    }
    let ev = build_events(Events::load(events)?, &campaign_sha)?;
    let mut cells: BTreeMap<Key, &crate::success::Cell> = BTreeMap::new();
    for c in &ev.canonical {
        for ((cfg, case, rep), cell) in &c.cells {
            cells.insert((c.manifest_sha256.clone(), case.clone(), *cfg, *rep), cell);
        }
    }

    let mut ops: BTreeMap<Key, Vec<HumanOpRecord>> = BTreeMap::new();
    let mut unkeyed: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in recs {
        match (&r.case_id, r.arm, r.repetition) {
            (Some(c), Some(a), Some(rep)) => ops
                .entry((r.manifest_sha256.clone(), c.clone(), a, rep))
                .or_default()
                .push(r),
            _ => {
                *unkeyed
                    .entry((r.campaign_id.clone(), r.manifest_sha256.clone()))
                    .or_default() += 1
            }
        }
    }
    // 分母は manifest の計画 cell（渡されていれば）。観測行だけの key も残す（計画外は checker が違反にする）。
    let keys: BTreeSet<Key> = ops
        .keys()
        .chain(ev.by_key.keys())
        .chain(cells.keys())
        .cloned()
        .collect();
    let mut lines = String::new();
    for key in keys {
        let (sha, case, cfg, rep) = &key;
        let recs = ops.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let side = ev.by_key.get(&key);
        let mut row = Map::new();
        row.insert("row_type".into(), json!("trial"));
        row.insert("manifest_sha256".into(), json!(sha));
        row.insert(
            "campaign_id".into(),
            json!(
                recs.first()
                    .map(|r| r.campaign_id.clone())
                    .or_else(|| side.and_then(|s| s.campaign_id.clone()))
            ),
        );
        row.insert("case_id".into(), json!(case));
        row.insert("arm".into(), json!(config_str(*cfg)));
        row.insert(
            "arm_labels".into(),
            json!(
                recs.iter()
                    .filter_map(|r| r.arm_label())
                    .collect::<BTreeSet<_>>()
            ),
        );
        row.insert("rep".into(), json!(rep));
        event_fields(side, &mut row);
        canonical_fields(cells.get(&key).copied(), &mut row);
        evaluator_fields(recs, &mut row);
        // 評価者記録の celeris_event_ref を Celeris 側の参照（介入・待機の event、run）と照合する。
        let refs: BTreeSet<&String> = recs
            .iter()
            .filter_map(|r| r.celeris_event_ref.as_ref())
            .collect();
        row.insert(
            "event_refs".into(),
            if refs.is_empty() {
                missing()
            } else {
                let known = side.map(|s| &s.refs);
                let (joined, unjoined): (Vec<&String>, Vec<&String>) = refs
                    .into_iter()
                    .partition(|r| known.is_some_and(|k| k.contains(*r)));
                field(json!({"joined": joined, "unjoined": unjoined}), EVALUATOR)
            },
        );
        // 評価者記録が task/run を持つが event が無い（A 腕）ときは評価者由来で補う。
        if row["run_ids"]["source"] == MISSING {
            let ids: BTreeSet<&String> = recs.iter().filter_map(|r| r.run_id.as_ref()).collect();
            if !ids.is_empty() {
                row.insert("run_ids".into(), field(json!(ids), EVALUATOR));
            }
        }
        let missing_fields: Vec<String> = row
            .iter()
            .filter(|(_, v)| v["source"] == MISSING)
            .map(|(k, _)| k.clone())
            .collect();
        row.insert("missing_count".into(), json!(missing_fields.len()));
        row.insert("missing_fields".into(), json!(missing_fields));
        lines += &Value::Object(row).to_string();
        lines.push('\n');
    }
    for c in &ev.canonical {
        lines += &json!({"row_type": "canonical", "manifest_sha256": c.manifest_sha256,
            "contract": crate::success::SUCCESS_CONTRACT, "grades_file": c.grades_file,
            "grade_violations": c.grade_violations, "cells": c.cells.len()})
        .to_string();
        lines.push('\n');
    }
    for ((campaign_id, sha), n) in unkeyed {
        lines += &json!({"row_type": "unjoined_evaluator", "campaign_id": campaign_id,
            "manifest_sha256": sha, "records": n, "reason": "case/arm/rep の identity が無い記録（shared scope。arm へ配分しない）"})
        .to_string();
        lines.push('\n');
    }
    for (id, (campaign, kind)) in &ev.unjoined_interventions {
        lines += &json!({"row_type": "unjoined_event", "intervention_id": id,
            "campaign_id": campaign, "kind": kind, "source": EVENT,
            "reason": "task_id が試行の run に無い"})
        .to_string();
        lines.push('\n');
    }
    Ok(lines)
}

pub fn join_cli(args: &[String]) -> Result<String, String> {
    let opt = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let file = opt("--file").ok_or("--file is required")?;
    let events: Vec<&Path> = args
        .windows(2)
        .filter(|w| w[0] == "--events")
        .map(|w| Path::new(w[1].as_str()))
        .collect();
    if events.is_empty() {
        return Err("--events is required".into());
    }
    join(Path::new(&file), &events)
}
