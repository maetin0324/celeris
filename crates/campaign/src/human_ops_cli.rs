//! `celeris-campaign human-op <record|span|validate|list>`: 評価者用の生記録 CLI。A/B/C 同じ command 列で同形式の行を書く。
//! 形式は `docs/ops/eval-raw-record-definitions.md`。時計は `--now` / `CELERIS_HUMAN_OPS_NOW`（試験用）で差し替えられる。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    human_ops::*,
    ledger::TrialKey,
    manifest::{CampaignManifest, ConfigId},
};

const USAGE: &str = "usage: celeris-campaign human-op <record|span|validate|list> --file <jsonl> ...\n\
record --file F --campaign ID (--manifest-sha256 H | --manifest M) --case ID --arm A0|B0|B1|C1 --rep N --op-kind K --actor NAME --mode manual|automatic --intent I \
[--actor-kind human|script] [--trigger T] [--logical-op ID] [--attempt N] [--started-at T|now] [--ended-at T|now] [--outcome ok|failed|abandoned] [--reason S] [--target kind:ref]... \
[--task-id ID] [--run-id ID] [--celeris-event-ref R] [--trial-no N] [--missing-reason R] [--time-basis B] [--time-precision P] [--note S]\n\
span --kind wait|attempt|job|cutoff (same identity flags) [--wait-kind K] [--logical-op ID] [--attempt N] [--result R] [--job-ref S:C:ID] [--job-submit|--job-start|--job-end T] [--started-at T] [--ended-at T] [--occurred-at T]\n\
validate --file F\nlist --file F [--campaign ID] [--arm A0|B0|B1|C1] [--case ID]\n\
clock: [--now T] [--clock-source system_utc|system_utc_ntp_synced|unknown] [--host H] (env CELERIS_HUMAN_OPS_NOW / CELERIS_HUMAN_OPS_HOST)";

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn opts(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].clone())
        .collect()
}

fn req(args: &[String], name: &str) -> Result<String, String> {
    opt(args, name).ok_or_else(|| format!("{name} is required\n{USAGE}"))
}

/// 文字列を enum 名（serde の名前）で解く。
fn parse_enum<T: serde::de::DeserializeOwned>(name: &str, v: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(v.to_string()))
        .map_err(|_| format!("{name}: 不正な値 {v:?}"))
}

fn parse_num(name: &str, v: &str) -> Result<u32, String> {
    v.parse().map_err(|_| format!("{name}: 数値でない {v:?}"))
}

struct Clock {
    now: String,
    source: ClockSource,
    host: String,
}

fn clock(args: &[String]) -> Result<Clock, String> {
    let now = match opt(args, "--now").or_else(|| std::env::var("CELERIS_HUMAN_OPS_NOW").ok()) {
        Some(n) => n,
        None => OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .map_err(|e| e.to_string())?
            .format(&Rfc3339)
            .map_err(|e| e.to_string())?,
    };
    let source = match opt(args, "--clock-source") {
        Some(s) => parse_enum("--clock-source", &s)?,
        None => ClockSource::SystemUtc,
    };
    let host = opt(args, "--host")
        .or_else(|| std::env::var("CELERIS_HUMAN_OPS_HOST").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown".into());
    Ok(Clock { now, source, host })
}

/// 事実時刻の指定。`now` は時計の値（evaluator_live）。
struct Fact {
    value: Option<String>,
    live: bool,
}

fn fact(args: &[String], name: &str, clock: &Clock) -> Fact {
    match opt(args, name) {
        Some(v) if v == "now" => Fact {
            value: Some(clock.now.clone()),
            live: true,
        },
        Some(v) => Fact {
            value: Some(v),
            live: false,
        },
        None => Fact {
            value: None,
            live: false,
        },
    }
}

struct Ctx {
    base: HumanOpRecord,
    clock: Clock,
    missing_reason: MissingReason,
    basis: Option<(TimeBasis, TimePrecision)>,
}

fn arm_of(label: &str) -> Result<(ExperimentSet, ConfigId), String> {
    Ok(match label {
        "A0" => (ExperimentSet::MainAb, ConfigId::A),
        "B0" => (ExperimentSet::MainAb, ConfigId::B),
        "B1" => (ExperimentSet::JevBc, ConfigId::B),
        "C1" => (ExperimentSet::JevBc, ConfigId::C),
        o => return Err(format!("--arm: A0|B0|B1|C1 のどれか（{o:?}）")),
    })
}

fn ctx(args: &[String]) -> Result<Ctx, String> {
    let clock = clock(args)?;
    let (set, arm) = arm_of(&req(args, "--arm")?)?;
    let campaign_id = req(args, "--campaign")?;
    let manifest_sha256 = match (opt(args, "--manifest-sha256"), opt(args, "--manifest")) {
        (Some(h), _) => h,
        (None, Some(p)) => CampaignManifest::load(Path::new(&p))?.sha256,
        _ => return Err(format!("--manifest-sha256 か --manifest が必要\n{USAGE}")),
    };
    let case = req(args, "--case")?;
    let rep = parse_num("--rep", &req(args, "--rep")?)?;
    let trial_no = match opt(args, "--trial-no") {
        Some(n) => parse_num("--trial-no", &n)?,
        None => 1,
    };
    let key = TrialKey {
        manifest_sha256: manifest_sha256.clone(),
        problem_id: case.clone(),
        arm,
        trial_no,
        rep,
    }
    .idempotency_key();
    let mut missing = BTreeMap::new();
    // A に Celeris の task / run は無い（定義書 §2.1）。
    let a_arm = arm == ConfigId::A;
    let task_id = opt(args, "--task-id");
    let run_id = opt(args, "--run-id");
    if a_arm {
        for (f, v) in [("task_id", &task_id), ("run_id", &run_id)] {
            if v.is_none() {
                missing.insert(f.to_string(), MissingReason::NotApplicable);
            }
        }
    }
    let basis = match (opt(args, "--time-basis"), opt(args, "--time-precision")) {
        (None, None) => None,
        (b, p) => Some((
            b.map(|b| parse_enum("--time-basis", &b))
                .transpose()?
                .unwrap_or(TimeBasis::EvaluatorReported),
            p.map(|p| parse_enum("--time-precision", &p))
                .transpose()?
                .unwrap_or(TimePrecision::Second),
        )),
    };
    let missing_reason = match opt(args, "--missing-reason") {
        Some(r) => parse_enum("--missing-reason", &r)?,
        None => MissingReason::Unknown,
    };
    let base = HumanOpRecord {
        schema: HUMAN_OP_SCHEMA.into(),
        record_id: String::new(),
        record_type: RecordType::OpStart,
        experiment_set: set,
        campaign_id,
        manifest_sha256,
        case_id: Some(case),
        arm: Some(arm),
        repetition: Some(rep),
        trial_no: Some(trial_no),
        trial_key: Some(key),
        task_id,
        run_id,
        job_ref: None,
        logical_op_id: None,
        attempt: None,
        recorded_at: clock.now.clone(),
        clock_source: clock.source,
        host: clock.host.clone(),
        start: None,
        end: None,
        occurred_at: None,
        job_submit: None,
        job_start: None,
        job_end: None,
        time_basis: None,
        time_precision: None,
        missing,
        op_kind: None,
        mode: None,
        intent: None,
        actor: None,
        reason: None,
        trigger: None,
        targets: vec![],
        celeris_event_ref: opt(args, "--celeris-event-ref"),
        outcome: None,
        wait_kind: None,
        result: None,
        corrects: None,
        set: None,
        note: opt(args, "--note"),
    };
    Ok(Ctx {
        base,
        clock,
        missing_reason,
        basis,
    })
}

impl Ctx {
    /// 事実時刻を欄に入れ、null なら missing に理由を書き、time_basis/precision を決める。
    fn times(&self, r: &mut HumanOpRecord, facts: &[(&str, Fact)]) {
        let mut any = false;
        let mut live = true;
        for (name, f) in facts {
            let slot = match *name {
                "start" => &mut r.start,
                "end" => &mut r.end,
                "occurred_at" => &mut r.occurred_at,
                "job_submit" => &mut r.job_submit,
                "job_start" => &mut r.job_start,
                _ => &mut r.job_end,
            };
            match &f.value {
                Some(v) => {
                    *slot = Some(v.clone());
                    any = true;
                    live &= f.live;
                }
                None => {
                    r.missing.insert((*name).to_string(), self.missing_reason);
                }
            }
        }
        if any {
            let (b, p) = self.basis.unwrap_or(if live {
                (TimeBasis::EvaluatorLive, TimePrecision::Second)
            } else {
                (TimeBasis::EvaluatorReported, TimePrecision::Second)
            });
            r.time_basis = Some(b);
            r.time_precision = Some(p);
        }
    }
}

fn append_all(file: &Path, recs: Vec<HumanOpRecord>) -> Result<String, String> {
    let log = HumanOpLog::new(file);
    let mut out = Vec::new();
    for r in recs {
        let r = r.with_record_id();
        let st = log.append(&r).map_err(|e| e.to_string())?;
        out.push(json!({
            "record_id": r.record_id,
            "record_type": r.record_type.as_str(),
            "status": match st { AppendOutcome::Appended => "appended", AppendOutcome::Duplicate => "duplicate" },
        }));
    }
    Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
}

fn logical_and_attempt(args: &[String], default_op: &str) -> Result<(String, u32), String> {
    let l = opt(args, "--logical-op").unwrap_or_else(|| default_op.to_string());
    let a = match opt(args, "--attempt") {
        Some(a) => parse_num("--attempt", &a)?,
        None => 1,
    };
    Ok((l, a))
}

fn record(args: &[String]) -> Result<String, String> {
    let file = req(args, "--file")?;
    let c = ctx(args)?;
    let kind: OpKind = parse_enum("--op-kind", &req(args, "--op-kind")?)?;
    let (logical, attempt) = logical_and_attempt(args, kind.as_str())?;
    let mut targets = Vec::new();
    for t in opts(args, "--target") {
        let (k, r) = t.split_once(':').ok_or("--target は <kind>:<ref>")?;
        targets.push(Target {
            kind: parse_enum("--target kind", k)?,
            target_ref: r.to_string(),
            sha256: None,
        });
    }
    let mut r = c.base.clone();
    r.logical_op_id = Some(logical);
    r.attempt = Some(attempt);
    r.op_kind = Some(kind);
    r.mode = Some(parse_enum("--mode", &req(args, "--mode")?)?);
    r.intent = Some(parse_enum("--intent", &req(args, "--intent")?)?);
    r.actor = Some(Actor {
        kind: match opt(args, "--actor-kind") {
            Some(k) => parse_enum("--actor-kind", &k)?,
            None => ActorKind::Human,
        },
        id: req(args, "--actor")?,
    });
    r.trigger = Some(match opt(args, "--trigger") {
        Some(t) => parse_enum("--trigger", &t)?,
        None => Trigger::SelfInitiated,
    });
    r.reason = opt(args, "--reason");
    r.targets = targets;
    let start = fact(args, "--started-at", &c.clock);
    let end = fact(args, "--ended-at", &c.clock);
    let outcome = opt(args, "--outcome")
        .map(|o| parse_enum::<Outcome>("--outcome", &o))
        .transpose()?;
    // 終了が分かる（時刻か outcome がある）ときだけ op_end を書く。分からない終了は書かず missing のまま。
    let close = end.value.is_some() || outcome.is_some();
    let mut s = r.clone();
    s.record_type = RecordType::OpStart;
    c.times(&mut s, &[("start", start)]);
    let mut recs = vec![s];
    if close {
        let mut e = r;
        e.record_type = RecordType::OpEnd;
        e.outcome = Some(outcome.unwrap_or(Outcome::Ok));
        c.times(&mut e, &[("end", end)]);
        recs.push(e);
    }
    append_all(Path::new(&file), recs)
}

fn span(args: &[String]) -> Result<String, String> {
    let file = req(args, "--file")?;
    let c = ctx(args)?;
    let kind = req(args, "--kind")?;
    let start = fact(args, "--started-at", &c.clock);
    let end = fact(args, "--ended-at", &c.clock);
    let mut r = c.base.clone();
    let recs = match kind.as_str() {
        "wait" => {
            let (l, a) = logical_and_attempt(args, "wait")?;
            r.logical_op_id = Some(l);
            r.attempt = Some(a);
            r.wait_kind = Some(req(args, "--wait-kind")?);
            let close = end.value.is_some();
            let mut s = r.clone();
            s.record_type = RecordType::WaitStart;
            c.times(&mut s, &[("start", start)]);
            let mut v = vec![s];
            if close {
                r.record_type = RecordType::WaitEnd;
                c.times(&mut r, &[("end", end)]);
                v.push(r);
            }
            v
        }
        "attempt" => {
            let l = req(args, "--logical-op")?;
            let (l, a) = logical_and_attempt(args, &l)?;
            r.record_type = RecordType::Attempt;
            r.logical_op_id = Some(l);
            r.attempt = Some(a);
            r.result = Some(parse_enum("--result", &req(args, "--result")?)?);
            c.times(&mut r, &[("start", start), ("end", end)]);
            vec![r]
        }
        "job" => {
            r.record_type = RecordType::JobObservation;
            r.job_ref = Some(req(args, "--job-ref")?);
            let f = |n| fact(args, n, &c.clock);
            c.times(
                &mut r,
                &[
                    ("job_submit", f("--job-submit")),
                    ("job_start", f("--job-start")),
                    ("job_end", f("--job-end")),
                ],
            );
            vec![r]
        }
        "cutoff" => {
            r.record_type = RecordType::Cutoff;
            c.times(
                &mut r,
                &[("occurred_at", fact(args, "--occurred-at", &c.clock))],
            );
            vec![r]
        }
        o => return Err(format!("--kind: wait|attempt|job|cutoff（{o:?}）")),
    };
    append_all(Path::new(&file), recs)
}

fn read(args: &[String]) -> Result<Vec<HumanOpRecord>, String> {
    let file = req(args, "--file")?;
    if !Path::new(&file).exists() {
        return Err(format!("{file}: file が無い"));
    }
    read_records(Path::new(&file)).map_err(|e| e.to_string())
}

fn validate(args: &[String]) -> Result<String, String> {
    let recs = read(args)?;
    let n = recs.len();
    resolve(&recs).map_err(|e| e.to_string())?;
    Ok(format!("ok: {n} records"))
}

fn list(args: &[String]) -> Result<String, String> {
    let recs = resolve(&read(args)?).map_err(|e| e.to_string())?;
    let arm = opt(args, "--arm");
    let (campaign, case) = (opt(args, "--campaign"), opt(args, "--case"));
    let keep: Vec<&HumanOpRecord> = recs
        .iter()
        .filter(|r| arm.as_deref().is_none_or(|a| r.arm_label() == Some(a)))
        .filter(|r| campaign.as_deref().is_none_or(|c| r.campaign_id == c))
        .filter(|r| {
            case.as_deref()
                .is_none_or(|c| r.case_id.as_deref() == Some(c))
        })
        .collect();
    let owned: Vec<HumanOpRecord> = keep.iter().map(|r| (*r).clone()).collect();
    let records: Vec<Value> = keep
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            if let Some(m) = v.as_object_mut() {
                m.insert("arm_label".into(), json!(r.arm_label()));
            }
            v
        })
        .collect();
    // 所要時間: 端が欠ければ null（0 にしない）。
    let ops: Vec<Value> = op_spans(&owned)
        .into_iter()
        .map(|(r, s)| {
            json!({
                "arm_label": r.arm_label(), "case_id": r.case_id, "logical_op_id": r.logical_op_id,
                "attempt": r.attempt, "op_kind": r.op_kind, "start": s.start, "end": s.end,
                "duration_secs": s.duration_secs(),
            })
        })
        .collect();
    let redo: Vec<Value> = redo_counts(&owned)
        .into_iter()
        .map(|((t, l), n)| json!({"trial_key": t, "logical_op_id": l, "redo": n}))
        .collect();
    serde_json::to_string_pretty(&json!({"records": records, "ops": ops, "redo_counts": redo}))
        .map_err(|e| e.to_string())
}

/// `args[0] == "human-op"`。
pub fn cli(args: &[String]) -> Result<String, String> {
    let rest = args.get(2..).unwrap_or(&[]);
    match args.get(1).map(String::as_str) {
        Some("record") => record(rest),
        Some("span") => span(rest),
        Some("validate") => validate(rest),
        Some("list") => list(rest),
        _ => Err(USAGE.into()),
    }
}
