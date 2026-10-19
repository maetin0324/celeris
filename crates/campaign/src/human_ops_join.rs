//! 生記録（human-ops JSONL）と Celeris 側の記録を同じ identity で結合する読み出し（集計・表は作らない）。
//! 定義は `docs/ops/eval-raw-record-definitions.md` §4。出力は 1 試行 identity（manifest・case・arm・rep）につき 1 行の JSONL。
//!
//! - 各欄は `{"value": ..., "source": "event" | "evaluator" | "missing"}`。Celeris 側から取れる区間は再記録せず導出する。
//! - どちらにも無い項目は `missing`（value は null）。0 に変換しない。`missing_fields` に欄名を並べる。
//! - 再開・重複: events は `dedup_key` / `run_id` / 台帳の鍵で、評価者記録は `record_id` で 1 件に潰す。行は identity ごとに 1 つ。
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
    started_at: Option<u64>,
    submitted_at: Option<u64>,
    task_ids: BTreeSet<String>,
    run_ids: BTreeSet<String>,
    /// run_id → wall_ms（outcome 行）。
    run_wall_ms: BTreeMap<String, Option<u64>>,
    /// intervention_id → kind。
    interventions: BTreeMap<String, String>,
}

#[derive(Default)]
struct Events {
    by_key: BTreeMap<Key, EventSide>,
    /// 試行に結べなかった介入（task_id が試行の run に無い・campaign 識別なし）。
    unjoined_interventions: BTreeMap<String, (Option<String>, String)>,
    interventions: Vec<(String, Option<String>, Option<String>, String)>,
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

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn build_events(
    inputs: Vec<(String, Vec<Value>)>,
    campaign_sha: &BTreeMap<String, String>,
) -> Events {
    let mut ev = Events {
        campaign_sha: campaign_sha.clone(),
        ..Events::default()
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut outcome_wall: BTreeMap<String, Option<u64>> = BTreeMap::new();
    let mut run_keys: Vec<(String, Key)> = Vec::new();
    for (name, rows) in inputs {
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
                        run_keys.push((run_id, key));
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
    let ivs = std::mem::take(&mut ev.interventions);
    for (id, task, campaign, kind) in ivs {
        let target = task.as_ref().and_then(|t| {
            ev.by_key
                .iter()
                .find(|(_, side)| side.task_ids.contains(t))
                .map(|(k, _)| k.clone())
        });
        match target {
            Some(k) => {
                if let Some(side) = ev.by_key.get_mut(&k) {
                    side.interventions.insert(id, kind);
                }
            }
            None => {
                ev.unjoined_interventions.insert(id, (campaign, kind));
            }
        }
    }
    ev
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
            "started_at",
            "submitted_at",
            "run_ids",
            "run_wall_ms",
            "event_interventions",
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
}

/// 結合して JSONL（末尾改行つき）を返す。
pub fn join(human_ops_file: &Path, events: &[&Path]) -> Result<String, String> {
    let recs = resolve(&read_records(human_ops_file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut campaign_sha: BTreeMap<String, String> = BTreeMap::new();
    for r in &recs {
        campaign_sha.insert(r.campaign_id.clone(), r.manifest_sha256.clone());
    }
    let ev = build_events(Events::load(events)?, &campaign_sha);

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
    let keys: BTreeSet<Key> = ops.keys().chain(ev.by_key.keys()).cloned().collect();
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
        evaluator_fields(recs, &mut row);
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
    for ((campaign_id, sha), n) in unkeyed {
        lines += &json!({"row_type": "unjoined_evaluator", "campaign_id": campaign_id,
            "manifest_sha256": sha, "records": n, "reason": "case/arm/rep の identity が無い記録"})
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
