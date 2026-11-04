//! TS だけの変更を検証する本物の deno_core host（ADR 2026-11-02 ts-fast-verify (3)）。
//!
//! bundle の manifest を検証して load し、採点の表（この file の [`DOMAINS`]）にある領域の Rust golden（JSONL）の全件を
//! 正順と逆順の 2 回呼んで比べ、安全シナリオ（壊した入力・壊した op の応答の拒否・宣言外の出力・不正な出力・例外・timeout・拒否）を
//! 判定して、結果を JSON 1 個で stdout に出す。検証専用の道具で、daemon・celerisctl の経路には入らない。
//!
//! op を await する領域（`cos_digest`）は、この file の op server が場面の記録から op に答える（付記 S4-16 の replay を
//! 本物の deno_core host で行う。ADR 2026-11-02 ts-fast-verify 付記 grade-cos-digest）。
//!
//! ```text
//! script-host-runner run      --bundle-dir <dir> --fixtures <dir> [--suite golden|safety|all]
//!                             [--domain <name>]... [--call-timeout-ms <n>]
//! script-host-runner activate --bundle-dir <dir> --state-dir <dir> [--call-timeout-ms <n>]
//! script-host-runner domains
//! ```
//!
//! exit code: 0 = すべて通った（`--domain` で絞ると出力の `complete` が false）。1 = 採点の失敗。
//! 2 = 使い方・入出力の誤り（引数・fixture の file が無い・fixture の行が壊れている）。
//! 3 = bundle の拒否（manifest の検証・領域の集合・capability・isolate の起動・配備の失敗）。
//!
//! 合否を決める物（領域・fixture・比べる欄・値の検査・最少行数・安全シナリオ）は Rust のこの file が持ち、
//! `control-ts/` の中の編集では変えられない。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use celeris_script::dto::{HOST_API_VERSION, HostError, HostErrorCode, HostResult};
use celeris_script::manifest::{
    VerifiedBundle, deploy_bundle, deployed_dir, host_schema_sha256, load_bundle_dir, load_deployed,
};
use celeris_script::selftest::{SELFTEST_DOMAIN, check_output, selftest_call};
use celeris_script::{
    HostOpCall, ScriptCall, ScriptError, ScriptRuntimeHandle, domain_capabilities, host_sink,
};
use serde_json::{Map, Value, json};

/// 出力の `schema` 欄。
const SCHEMA: &str = "celeris.script-host-runner/1";
/// `failures` に載せる件数の上限（件数は `domains` の欄が全部数える）。
const MAX_FAILURES: usize = 20;
/// 1 呼び出しの上限の既定値。本番の deadline（50 ms）ではなく、負荷による偽の失敗を避けるための上限。
const DEFAULT_CALL_TIMEOUT_MS: u64 = 5000;

const EXIT_FAIL: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_REJECT: u8 = 3;

const USAGE: &str = "usage:\n  script-host-runner run --bundle-dir <dir> --fixtures <dir> [--suite golden|safety|all] [--domain <name>]... [--call-timeout-ms <n>]\n  script-host-runner activate --bundle-dir <dir> --state-dir <dir> [--call-timeout-ms <n>]\n  script-host-runner domains";

// ---------------------------------------------------------------------------
// 採点の表
// ---------------------------------------------------------------------------

/// 出力の値の検査に通った値（比べる欄だけ。`route_priority` の候補は `{source, model}` の順に組み直す）と安全違反。
struct Checked {
    output: Map<String, Value>,
    violations: Vec<&'static str>,
}

/// 記録された op 1 回（`pass_traces.jsonl` の `ops` の 1 件）。
#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedOp {
    op: String,
    request: Value,
    reply: HostResult<Value>,
}

/// 1 回の呼び出しの場面: 入力と、op を await する領域なら op の答えの記録。`ops` が `None` の領域には host sink を渡さない。
#[derive(Clone)]
struct Scene {
    input: Value,
    ops: Option<Vec<RecordedOp>>,
}

impl Scene {
    fn pure(input: Value) -> Self {
        Self { input, ops: None }
    }

    /// 入力を取らない pass（入力は daemon と同じ `{}`。付記 S4-12）。
    fn pass(ops: Vec<RecordedOp>) -> Self {
        Self {
            input: json!({}),
            ops: Some(ops),
        }
    }
}

/// fixture の 1 行から作った場面と期待値（`expected` の比べる欄を出力と比べる）。
struct Parsed {
    scene: Scene,
    expected: Value,
}

/// 壊した場面 1 つ。TS は `expect` の結果で終わらなければならない（出力を返したら失敗）。
struct Broken {
    id: &'static str,
    scene: Scene,
    expect: Outcome,
}

/// 領域の fixture の file 1 つ。
struct FixtureSpec {
    /// `--fixtures` の dir からの相対 path。
    path: &'static str,
    /// この file の最少行数。空の file・欠けた file を通さない。領域の最少行数はこの和。
    min_rows: usize,
    /// fixture の 1 行から場面を作る規則（`domains` の出力に載せる説明）。
    rule: &'static str,
    parse: fn(&Value) -> Result<Parsed, String>,
}

/// 採点する領域 1 つ。
struct DomainSpec {
    name: &'static str,
    fixtures: &'static [FixtureSpec],
    /// 比べる欄。出力の欄の集合はこれと完全に同じでなければならない。
    fields: &'static [&'static str],
    /// 出力の値の検査。`scene` は TS に渡した場面。
    check: fn(&Map<String, Value>, &Scene) -> Result<Checked, String>,
    /// activate で 1 回呼ぶ場面。
    probe: fn() -> Scene,
    /// TS が拒まなければならない壊した場面。
    broken: fn() -> Vec<Broken>,
}

impl DomainSpec {
    fn min_rows(&self) -> usize {
        self.fixtures.iter().map(|f| f.min_rows).sum()
    }
}

static DOMAINS: &[DomainSpec] = &[
    DomainSpec {
        name: "lane_rules",
        fixtures: &[FixtureSpec {
            path: "lane_rules/all-features.jsonl",
            // 9 軸 × 3 値の全組み合わせ（3^9）。
            min_rows: 19_683,
            rule: "{\"features\": row.features}",
            parse: lane_rules_row,
        }],
        fields: &["rule_id", "proposed", "condition"],
        check: lane_rules_check,
        probe: lane_rules_probe,
        broken: lane_rules_broken,
    },
    DomainSpec {
        name: "route_priority",
        fixtures: &[FixtureSpec {
            path: "route_priority/cases.jsonl",
            // task-core の dump-route-priority-fixtures が今書く行数。
            min_rows: 2_026,
            rule: "row.input",
            parse: route_priority_row,
        }],
        fields: &["basis", "candidates"],
        check: route_priority_check,
        probe: route_priority_probe,
        broken: route_priority_broken,
    },
    DomainSpec {
        name: "execution_gate",
        fixtures: &[FixtureSpec {
            path: "execution_gate/cases.jsonl",
            // task-core の dump-execution-gate-fixtures が今書く行数。
            min_rows: 2_337,
            rule: "row.input",
            parse: execution_gate_row,
        }],
        fields: &EXECUTION_GATE_FIELDS,
        check: execution_gate_check,
        probe: execution_gate_probe,
        broken: execution_gate_broken,
    },
    DomainSpec {
        name: "cos_digest",
        fixtures: &[
            FixtureSpec {
                path: "cos_digest/pass_traces.jsonl",
                // task-dispatch の cos_digest_trace.rs が今書く行数。
                min_rows: 20,
                rule: "{ops: row.ops, output: row.output}",
                parse: cos_digest_trace_row,
            },
            FixtureSpec {
                path: "cos_digest/cases.jsonl",
                // task-core の dump-cos-digest-fixtures が今書く行数。
                min_rows: 2_066,
                rule: "run 1 つの pass に組み直す（list = input の thread・run_id・run_state、read = input、output = {runs: [{run_id, commands}]}。compare.mjs と同じ）",
                parse: cos_digest_case_row,
            },
        ],
        fields: &["runs"],
        check: cos_digest_check,
        probe: cos_digest_probe,
        broken: cos_digest_broken,
    },
];

/// `task-dispatch::script_policy` の `RULE_ID_MAX`・`CONDITION_MAX`・`ROUTE_PRIORITY_CANDIDATES_MAX` の写し
/// （`celeris-script` は `task-dispatch` に依存できない。ADR 2026-11-02 ts-fast-verify「境界 ADR との関係」）。
const RULE_ID_MAX: usize = 128;
const CONDITION_MAX: usize = 512;
const ROUTE_PRIORITY_CANDIDATES_MAX: usize = 256;
const LANES: &[&str] = &["frontier", "standard", "cheap"];
const BASES: &[&str] = &["cos", "execution_class", "lane", "harness", "none"];
const AXES: &[&str] = &[
    "judgment",
    "ambiguity",
    "verifiability",
    "reversibility",
    "consequence",
    "context_size",
    "tool_intensity",
    "expected_length",
    "cross_cutting",
];

/// 行そのものが期待値の領域の `Parsed`（比べる欄が行に無ければ誤り）。
fn row_expected(input: Value, row: &Value, fields: &[&str]) -> Result<Parsed, String> {
    if let Some(f) = fields.iter().find(|f| row.get(**f).is_none()) {
        return Err(format!("missing expected field {f:?}"));
    }
    Ok(Parsed {
        scene: Scene::pure(input),
        expected: row.clone(),
    })
}

fn lane_rules_row(row: &Value) -> Result<Parsed, String> {
    let features = row.get("features").ok_or("cannot build the input")?;
    row_expected(
        json!({ "features": features.clone() }),
        row,
        &["rule_id", "proposed", "condition"],
    )
}

fn lane_rules_check(out: &Map<String, Value>, _scene: &Scene) -> Result<Checked, String> {
    let rule_id = str_field(out, "rule_id")?;
    if rule_id.is_empty() || rule_id.len() > RULE_ID_MAX {
        return Err(format!(
            "rule_id must be 1..={RULE_ID_MAX} bytes (got {})",
            rule_id.len()
        ));
    }
    let proposed = str_field(out, "proposed")?;
    if !LANES.contains(&proposed) {
        return Err(format!(
            "proposed must be one of {LANES:?} (got {proposed:?})"
        ));
    }
    let condition = str_field(out, "condition")?;
    if condition.len() > CONDITION_MAX {
        return Err(format!(
            "condition must be at most {CONDITION_MAX} bytes (got {})",
            condition.len()
        ));
    }
    Ok(Checked {
        output: out.clone(),
        violations: Vec::new(),
    })
}

fn lane_rules_probe() -> Scene {
    Scene::pure(lane_rules_probe_input())
}

fn lane_rules_probe_input() -> Value {
    let features: Map<String, Value> = AXES
        .iter()
        .map(|a| ((*a).to_owned(), json!("low")))
        .collect();
    json!({ "features": features })
}

fn lane_rules_broken() -> Vec<Broken> {
    let base = lane_rules_probe_input();
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    vec![
        (
            "missing_axis",
            edit(&|v| {
                v["features"]
                    .as_object_mut()
                    .map(|m| m.remove("cross_cutting"));
            }),
        ),
        (
            "unknown_level",
            edit(&|v| v["features"]["judgment"] = json!("extreme")),
        ),
        ("extra_field", edit(&|v| v["extra"] = json!(1))),
        (
            "extra_axis",
            edit(&|v| v["features"]["bogus"] = json!("low")),
        ),
        ("not_object", json!([])),
        ("features_not_object", json!({ "features": "low" })),
        (
            "wrong_type",
            edit(&|v| v["features"]["judgment"] = json!(1)),
        ),
    ]
    .into_iter()
    .map(broken_input)
    .collect()
}

/// 壊した入力（TS は throw する）。
fn broken_input((id, input): (&'static str, Value)) -> Broken {
    Broken {
        id,
        scene: Scene::pure(input),
        expect: Outcome::Exception,
    }
}

fn route_priority_row(row: &Value) -> Result<Parsed, String> {
    let input = row.get("input").ok_or("cannot build the input")?;
    row_expected(input.clone(), row, &["basis", "candidates"])
}

fn route_priority_check(out: &Map<String, Value>, scene: &Scene) -> Result<Checked, String> {
    let basis = str_field(out, "basis")?;
    if !BASES.contains(&basis) {
        return Err(format!("basis must be one of {BASES:?} (got {basis:?})"));
    }
    let raw = out
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or("candidates must be an array")?;
    if raw.len() > ROUTE_PRIORITY_CANDIDATES_MAX {
        return Err(format!(
            "at most {ROUTE_PRIORITY_CANDIDATES_MAX} candidates (got {})",
            raw.len()
        ));
    }
    let mut candidates = Vec::with_capacity(raw.len());
    for (i, c) in raw.iter().enumerate() {
        candidates.push(candidate(c).ok_or_else(|| {
            format!("candidates[{i}] must be an object of exactly the strings source and model")
        })?);
    }
    let declared = table_candidates(&scene.input);
    let mut violations = Vec::new();
    if !candidates.iter().all(|c| declared.contains(c)) {
        violations.push("undeclared_candidate");
    }
    let mut output = Map::new();
    output.insert("basis".into(), json!(basis));
    output.insert(
        "candidates".into(),
        Value::Array(
            candidates
                .into_iter()
                .map(|(source, model)| json!({ "source": source, "model": model }))
                .collect(),
        ),
    );
    Ok(Checked { output, violations })
}

/// `{source, model}` の文字列だけの object なら `(source, model)`。
fn candidate(v: &Value) -> Option<(String, String)> {
    let m = v.as_object()?;
    if m.len() != 2 {
        return None;
    }
    Some((
        m.get("source")?.as_str()?.to_owned(),
        m.get("model")?.as_str()?.to_owned(),
    ))
}

/// 入力 `table` のどれかの列に在る候補の集合。
fn table_candidates(input: &Value) -> BTreeSet<(String, String)> {
    let table = &input["table"];
    let mut columns: Vec<&Value> = vec![&table["route_defaults"], &table["cos_route_candidates"]];
    for key in ["lane_route_defaults", "class_defaults"] {
        if let Some(rows) = table[key].as_array() {
            columns.extend(rows.iter().map(|r| &r["candidates"]));
        }
    }
    columns
        .into_iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(candidate)
        .collect()
}

fn route_priority_probe() -> Scene {
    Scene::pure(route_priority_probe_input())
}

fn route_priority_probe_input() -> Value {
    json!({
        "harness": "claude_code",
        "lane": "standard",
        "cos": false,
        "execution_class": null,
        "table": {
            "route_defaults": [{ "source": "anthropic", "model": "sonnet" }],
            "lane_route_defaults": [],
            "class_defaults": [],
            "cos_route_candidates": [],
        },
    })
}

fn route_priority_broken() -> Vec<Broken> {
    let base = route_priority_probe_input();
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    vec![
        (
            "missing_field",
            edit(&|v| {
                v.as_object_mut().map(|m| m.remove("harness"));
            }),
        ),
        ("extra_field", edit(&|v| v["extra"] = json!(1))),
        ("not_object", json!([])),
        ("wrong_type_cos", edit(&|v| v["cos"] = json!("false"))),
        ("wrong_type_lane", edit(&|v| v["lane"] = json!(1))),
        (
            "table_missing_column",
            edit(&|v| {
                v["table"]
                    .as_object_mut()
                    .map(|m| m.remove("cos_route_candidates"));
            }),
        ),
        (
            "candidate_extra_field",
            edit(&|v| {
                v["table"]["route_defaults"] =
                    json!([{ "source": "anthropic", "model": "sonnet", "weight": 1 }]);
            }),
        ),
        (
            "candidate_wrong_type",
            edit(&|v| v["table"]["route_defaults"] = json!([{ "source": 1, "model": "sonnet" }])),
        ),
    ]
    .into_iter()
    .map(broken_input)
    .collect()
}

// ---- execution_gate（ADR 2026-11-04-ts-execution-gate-domain） ----

const EXECUTION_GATE_FIELDS: [&str; 4] = ["mode", "score", "rule_id", "signals"];
/// `task_core::execution_gate::EXECUTION_GATE_TS_RULES` と同じ（この crate は task-core に依存しない）。`(rule_id, mode)`。
const EXECUTION_GATE_RULES: [(&str, &str); 4] = [
    ("compound/long-and-broad", "compound"),
    ("atomic/small", "atomic"),
    ("compound/score", "compound"),
    ("atomic/score", "atomic"),
];
const EXECUTION_GATE_SIGNALS_MAX: usize = 16;
const EXECUTION_GATE_SIGNAL_NAME_MAX: usize = 8;
const EXECUTION_GATE_SIGNAL_DETAIL_MAX: usize = 128;

fn execution_gate_row(row: &Value) -> Result<Parsed, String> {
    let input = row.get("input").ok_or("cannot build the input")?;
    row_expected(input.clone(), row, &EXECUTION_GATE_FIELDS)
}

/// daemon の `classify_execution_gate` と同じ検証（ADR D8）。
fn execution_gate_check(out: &Map<String, Value>, scene: &Scene) -> Result<Checked, String> {
    let mode = str_field(out, "mode")?;
    if mode != "atomic" && mode != "compound" {
        return Err(format!("mode must be atomic or compound (got {mode:?})"));
    }
    let score = out
        .get("score")
        .and_then(Value::as_i64)
        .ok_or("score must be an integer")?;
    let rule_id = str_field(out, "rule_id")?;
    let signals = out
        .get("signals")
        .and_then(Value::as_array)
        .ok_or("signals must be an array")?;
    if signals.len() > EXECUTION_GATE_SIGNALS_MAX {
        return Err(format!(
            "at most {EXECUTION_GATE_SIGNALS_MAX} signals (got {})",
            signals.len()
        ));
    }
    let mut names = BTreeSet::new();
    let mut sum = 0i64;
    let mut rebuilt = Vec::with_capacity(signals.len());
    for (i, signal) in signals.iter().enumerate() {
        let m = signal
            .as_object()
            .filter(|m| m.len() == 3)
            .ok_or_else(|| format!("signals[{i}] must have exactly name, weight and detail"))?;
        let name = m.get("name").and_then(Value::as_str);
        let weight = m.get("weight").and_then(Value::as_i64);
        let detail = m.get("detail").and_then(Value::as_str);
        let (Some(name), Some(weight), Some(detail)) = (name, weight, detail) else {
            return Err(format!(
                "signals[{i}] must have exactly name, weight and detail"
            ));
        };
        if name.is_empty()
            || name.len() > EXECUTION_GATE_SIGNAL_NAME_MAX
            || detail.len() > EXECUTION_GATE_SIGNAL_DETAIL_MAX
            || !names.insert(name.to_owned())
        {
            return Err(format!("signals[{i}]: bad or duplicate name, or too long"));
        }
        sum += weight;
        rebuilt.push(json!({ "name": name, "weight": weight, "detail": detail }));
    }
    let mut violations = Vec::new();
    match EXECUTION_GATE_RULES.iter().find(|(r, _)| *r == rule_id) {
        None => violations.push("gate_rule_not_allowed"),
        Some((_, rule_mode)) => {
            let threshold = scene.input["threshold"]
                .as_i64()
                .ok_or("the input has no threshold")?;
            let holds = match rule_id {
                "compound/score" => score >= threshold,
                "atomic/score" => score < threshold,
                _ => true,
            };
            if *rule_mode != mode || score != sum || !holds {
                violations.push("gate_threshold_violated");
            }
        }
    }
    let mut output = Map::new();
    output.insert("mode".into(), json!(mode));
    output.insert("score".into(), json!(score));
    output.insert("rule_id".into(), json!(rule_id));
    output.insert("signals".into(), Value::Array(rebuilt));
    Ok(Checked { output, violations })
}

fn execution_gate_probe() -> Scene {
    Scene::pure(execution_gate_probe_input())
}

fn execution_gate_probe_input() -> Value {
    let features: Map<String, Value> = AXES
        .iter()
        .map(|a| ((*a).to_owned(), json!("low")))
        .collect();
    json!({
        "features": features,
        "title": "title",
        "objective": "objective",
        "acceptance_total": 1,
        "artifact_exists": 0,
        "has_human": false,
        "has_command": true,
        "multi_environment": false,
        "recent_budget_exhausted_ratio": null,
        "cos_hint_compound": false,
        "max_turns": 60,
        "tree_child": false,
        "threshold": 5,
    })
}

fn execution_gate_broken() -> Vec<Broken> {
    let base = execution_gate_probe_input();
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    vec![
        (
            "missing_field",
            edit(&|v| {
                v.as_object_mut().map(|m| m.remove("threshold"));
            }),
        ),
        ("extra_field", edit(&|v| v["extra"] = json!(1))),
        ("not_object", json!([])),
        (
            "unknown_level",
            edit(&|v| v["features"]["judgment"] = json!("extreme")),
        ),
        (
            "missing_axis",
            edit(&|v| {
                v["features"]
                    .as_object_mut()
                    .map(|m| m.remove("cross_cutting"));
            }),
        ),
        ("wrong_type_title", edit(&|v| v["title"] = json!(1))),
        ("wrong_type_turns", edit(&|v| v["max_turns"] = json!("60"))),
        (
            "fractional_threshold",
            edit(&|v| v["threshold"] = json!(1.5)),
        ),
        (
            "ratio_out_of_range",
            edit(&|v| v["recent_budget_exhausted_ratio"] = json!(1.5)),
        ),
    ]
    .into_iter()
    .map(broken_input)
    .collect()
}

// ---- cos_digest（pass。op を await する。付記 S4-12・S4-16） ----

const OP_LIST_RUNS: &str = "cos_digest:list_runs";
const OP_READ_RUN: &str = "cos_digest:read_run";

fn recorded(op: &str, request: Value, reply: HostResult<Value>) -> RecordedOp {
    RecordedOp {
        op: op.to_owned(),
        request,
        reply,
    }
}

fn ok_reply(value: Value) -> HostResult<Value> {
    HostResult::Ok { value }
}

fn str_of<'a>(v: &'a Value, field: &str) -> Result<&'a str, String> {
    v.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} must be a string"))
}

/// `pass_traces.jsonl` の 1 行（1 pass）。
fn cos_digest_trace_row(row: &Value) -> Result<Parsed, String> {
    let ops = row.get("ops").ok_or("missing ops")?;
    let ops: Vec<RecordedOp> =
        serde_json::from_value(ops.clone()).map_err(|e| format!("bad ops: {e}"))?;
    let expected = row.get("output").ok_or("missing output")?;
    if expected.get("runs").is_none() {
        return Err("missing expected field \"runs\"".to_owned());
    }
    Ok(Parsed {
        scene: Scene::pass(ops),
        expected: expected.clone(),
    })
}

/// run 1 つの list と read の記録（compare.mjs・pass-replay.ts の `passFromCase` と同じ組み直し）。
fn one_run_ops(input: &Value) -> Result<Vec<RecordedOp>, String> {
    let thread = str_of(input, "thread")?;
    let run_id = str_of(input, "run_id")?;
    let run_state = str_of(input, "run_state")?;
    Ok(vec![
        recorded(
            OP_LIST_RUNS,
            json!({}),
            ok_reply(json!({
                "runs": [{ "thread": thread, "run_id": run_id, "run_state": run_state }],
            })),
        ),
        recorded(
            OP_READ_RUN,
            json!({ "thread": thread, "run_id": run_id }),
            ok_reply(input.clone()),
        ),
    ])
}

/// `cases.jsonl` の 1 行を「run 1 つの pass」に組み直す（付記 S4-16）。
fn cos_digest_case_row(row: &Value) -> Result<Parsed, String> {
    let input = row.get("input").ok_or("missing input")?;
    let commands = row.get("commands").ok_or("missing commands")?;
    let run_id = str_of(input, "run_id")?;
    Ok(Parsed {
        scene: Scene::pass(one_run_ops(input)?),
        expected: json!({ "runs": [{ "run_id": run_id, "commands": commands.clone() }] }),
    })
}

/// 欄の集合が `fields` と完全に同じ object か。
fn exact_object<'a>(
    v: &'a Value,
    fields: &[&str],
    where_: &str,
) -> Result<&'a Map<String, Value>, String> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("{where_} must be an object"))?;
    if m.len() != fields.len() || !fields.iter().all(|f| m.contains_key(*f)) {
        return Err(format!(
            "{where_} must have exactly the fields {fields:?} (got {:?})",
            m.keys().collect::<Vec<_>>()
        ));
    }
    Ok(m)
}

/// 出力の形（`CosDigestPassOutput`）を確かめる。値の規則は golden の byte 一致と Rust の検証（規則 1〜9）が持つ。
/// 一覧に無い run の出力は安全違反（規則 7 の `run_outside_list`）。
fn cos_digest_check(out: &Map<String, Value>, scene: &Scene) -> Result<Checked, String> {
    let runs = out
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("runs must be an array")?;
    let listed: BTreeSet<&str> = scene
        .ops
        .iter()
        .flatten()
        .filter(|o| o.op == OP_LIST_RUNS)
        .filter_map(|o| match &o.reply {
            HostResult::Ok { value } => value.get("runs").and_then(Value::as_array),
            HostResult::Error { .. } => None,
        })
        .flatten()
        .filter_map(|r| r.get("run_id").and_then(Value::as_str))
        .collect();
    let mut violations = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        let where_ = format!("runs[{i}]");
        let m = exact_object(run, &["run_id", "commands"], &where_)?;
        let run_id = m["run_id"]
            .as_str()
            .ok_or_else(|| format!("{where_}.run_id must be a string"))?;
        if !listed.contains(run_id) {
            violations.push("run_outside_list");
        }
        let commands = m["commands"]
            .as_array()
            .ok_or_else(|| format!("{where_}.commands must be an array"))?;
        for (j, c) in commands.iter().enumerate() {
            let at = format!("{where_}.commands[{j}]");
            match c.get("kind").and_then(Value::as_str) {
                Some("post_digest") => {
                    exact_object(
                        c,
                        &[
                            "kind",
                            "thread",
                            "idempotency_key",
                            "text",
                            "cards",
                            "run_id",
                        ],
                        &at,
                    )?;
                }
                Some("requeue_run") => {
                    exact_object(c, &["kind", "run_id"], &at)?;
                }
                other => return Err(format!("{at}.kind is not a command: {other:?}")),
            }
        }
    }
    Ok(Checked {
        output: out.clone(),
        violations,
    })
}

const PROBE_THREAD: &str = "probe-thread";
const PROBE_RUN: &str = "probe-run";

fn cos_digest_probe_input() -> Value {
    json!({
        "thread": PROBE_THREAD,
        "run_id": PROBE_RUN,
        "run_state": "completed",
        "reason": null,
        "items": [],
    })
}

/// run 1 つの pass。
fn cos_digest_probe() -> Scene {
    // 固定の入力なので組み直しは失敗しない。失敗しても activate の probe が落ちるだけ。
    Scene::pass(one_run_ops(&cos_digest_probe_input()).unwrap_or_default())
}

/// 壊した op の応答。TS（または Rust の op の DTO 検証）が拒み、pass は出力を返さない。
/// `read_run` の code `exception` は TS がその run を飛ばす（S4-18）ので、throw になる壊し方だけを並べる。
fn cos_digest_broken() -> Vec<Broken> {
    let list = |reply| recorded(OP_LIST_RUNS, json!({}), reply);
    let good_list = || {
        list(ok_reply(json!({
            "runs": [{ "thread": PROBE_THREAD, "run_id": PROBE_RUN, "run_state": "completed" }],
        })))
    };
    let read = |reply| {
        recorded(
            OP_READ_RUN,
            json!({ "thread": PROBE_THREAD, "run_id": PROBE_RUN }),
            reply,
        )
    };
    let item = |route: &str| {
        json!({
            "item_id": "i", "thread_id": PROBE_THREAD, "source_kind": "question", "source_key": "k",
            "source_revision": "r", "state": "escalated", "summary": "s", "detail": "", "reason": null,
            "operation_id": null, "run_id": PROBE_RUN, "created_at": "2026-11-03T00:00:00Z",
            "route": route, "decision": null,
        })
    };
    let host_error = |code, message: &str| HostResult::Error {
        error: HostError::new(code, message),
    };
    let scene = |ops| Scene::pass(ops);
    let throw = |id, ops| Broken {
        id,
        scene: scene(ops),
        expect: Outcome::Exception,
    };
    let mut other_run = cos_digest_probe_input();
    other_run["run_id"] = json!("another-run");
    let mut bad_route = cos_digest_probe_input();
    bad_route["items"] = json!([item("bogus")]);
    vec![
        throw("list_reply_missing_runs", vec![list(ok_reply(json!({})))]),
        throw(
            "list_reply_extra_field",
            vec![list(ok_reply(json!({ "runs": [], "extra": 1 })))],
        ),
        throw(
            "list_reply_wrong_type",
            vec![list(ok_reply(json!({ "runs": "none" })))],
        ),
        throw(
            "list_reply_error",
            vec![list(host_error(HostErrorCode::Exception, "store failed"))],
        ),
        throw(
            "read_reply_other_run",
            vec![good_list(), read(ok_reply(other_run))],
        ),
        throw(
            "read_reply_unknown_route",
            vec![good_list(), read(ok_reply(bad_route))],
        ),
        Broken {
            id: "read_reply_denied",
            scene: scene(vec![
                good_list(),
                read(host_error(HostErrorCode::Denied, "outside the scope")),
            ]),
            expect: Outcome::Denied,
        },
    ]
}

fn str_field<'a>(out: &'a Map<String, Value>, field: &str) -> Result<&'a str, String> {
    out.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{field} must be a string"))
}

// ---------------------------------------------------------------------------
// 判定
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Match,
    Mismatch,
    UndeclaredField,
    InvalidOutput,
    Exception,
    Timeout,
    Denied,
    /// 上の分類に当たらない host の誤り（`Busy`・`GenerationGone`・`InputTooLarge`）。
    HostError,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::UndeclaredField => "undeclared_field",
            Self::InvalidOutput => "invalid_output",
            Self::Exception => "exception",
            Self::Timeout => "timeout",
            Self::Denied => "denied",
            Self::HostError => "host_error",
        }
    }
}

struct Judged {
    outcome: Outcome,
    violations: Vec<&'static str>,
    error: Option<String>,
}

fn error_outcome(err: &ScriptError) -> Outcome {
    match err {
        ScriptError::Exception { .. } => Outcome::Exception,
        ScriptError::Timeout | ScriptError::Terminated | ScriptError::HeapLimit => Outcome::Timeout,
        ScriptError::Denied { .. } => Outcome::Denied,
        ScriptError::InvalidOutput { .. } => Outcome::InvalidOutput,
        ScriptError::Busy | ScriptError::GenerationGone | ScriptError::InputTooLarge { .. } => {
            Outcome::HostError
        }
    }
}

/// 出力の形と値を検査する（期待値とは比べない）。
fn check_shape(
    spec: &DomainSpec,
    scene: &Scene,
    result: &Result<Value, ScriptError>,
) -> Result<Checked, Judged> {
    let fail = |outcome, error: String| Judged {
        outcome,
        violations: Vec::new(),
        error: Some(error),
    };
    let output = match result {
        Ok(v) => v,
        Err(e) => return Err(fail(error_outcome(e), e.to_string())),
    };
    let Some(map) = output.as_object() else {
        return Err(fail(
            Outcome::InvalidOutput,
            "output must be an object".into(),
        ));
    };
    if let Some(extra) = map.keys().find(|k| !spec.fields.contains(&k.as_str())) {
        return Err(fail(
            Outcome::UndeclaredField,
            format!("undeclared output field {extra:?}"),
        ));
    }
    if let Some(missing) = spec.fields.iter().find(|f| !map.contains_key(**f)) {
        return Err(fail(
            Outcome::InvalidOutput,
            format!("missing output field {missing:?}"),
        ));
    }
    (spec.check)(map, scene).map_err(|e| fail(Outcome::InvalidOutput, e))
}

/// golden の 1 行と比べる。欄ごとの serde_json の直列化が byte 一致なら match（境界 ADR 付記 S2-2）。
/// workspace の serde_json は `preserve_order` を持たない（ルートの Cargo.toml）ので、object の key は整列して直列化される。
/// TS の出力の key の順はこの比較には見えない（Rust の検証→apply は型で受けるので順に依存しない）。順は compare.mjs が見る。
fn judge(spec: &DomainSpec, scene: &Scene, expected: &Value, ran: &Ran) -> Judged {
    let checked = match check_shape_replayed(spec, scene, ran) {
        Ok(c) => c,
        Err(j) => return j,
    };
    let same = spec.fields.iter().all(|f| {
        let a = serde_json::to_string(&checked.output[*f]).ok();
        let b = serde_json::to_string(&expected[*f]).ok();
        a.is_some() && a == b
    });
    Judged {
        outcome: if same {
            Outcome::Match
        } else {
            Outcome::Mismatch
        },
        violations: checked.violations,
        error: None,
    }
}

/// [`check_shape`] に op の replay の判定を足す（付記 S4-16）。記録に無い要求を呼んだ pass は、TS がその `denied` を
/// 握りつぶして正しい出力を返しても `denied`。記録の op を呼ばなかった pass は `mismatch`。
fn check_shape_replayed(spec: &DomainSpec, scene: &Scene, ran: &Ran) -> Result<Checked, Judged> {
    let fail = |outcome, error: String| Judged {
        outcome,
        violations: Vec::new(),
        error: Some(error),
    };
    if let Some(first) = ran.unrecorded.first() {
        return Err(fail(
            Outcome::Denied,
            format!(
                "{} op request(s) not in the record; first: {first}",
                ran.unrecorded.len()
            ),
        ));
    }
    let checked = check_shape(spec, scene, &ran.result)?;
    if let Some(first) = ran.unused.first() {
        return Err(fail(
            Outcome::Mismatch,
            format!(
                "{} recorded op(s) not called; first: {first}",
                ran.unused.len()
            ),
        ));
    }
    Ok(checked)
}

/// 正順と逆順の結果を比べるための指紋（呼んだ op の違いも入れる）。
fn fingerprint(ran: &Ran) -> String {
    let body = match &ran.result {
        Ok(v) => format!("ok:{v}"),
        Err(e) => format!("err:{e}"),
    };
    format!("{body}|{:?}|{:?}", ran.unrecorded, ran.unused)
}

// ---------------------------------------------------------------------------
// host
// ---------------------------------------------------------------------------

/// 本物の deno_core host（`ScriptRuntimeHandle`）と、同期で呼ぶための tokio runtime。
struct Host {
    rt: tokio::runtime::Runtime,
    handle: ScriptRuntimeHandle,
    timeout: Duration,
}

impl Host {
    fn start(bundle: &VerifiedBundle, timeout: Duration) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let handle = ScriptRuntimeHandle::start(bundle.source())
            .map_err(|e| format!("cannot start the isolate: {e}"))?;
        Ok(Self {
            rt,
            handle,
            timeout,
        })
    }

    /// 場面を空の scope で呼ぶ。純粋な領域は op を使わない（使えば `denied`）。op を await する領域の op には
    /// 場面の記録から答える。
    fn run_scene(&self, domain: &str, scene: &Scene) -> Ran {
        let call = ScriptCall {
            domain: domain.to_owned(),
            input: scene.input.clone(),
            scope_task_ids: BTreeSet::new(),
            timeout: self.timeout,
        };
        let Some(ops) = &scene.ops else {
            let (result, took) = self.call_with(call);
            return Ran {
                result,
                took,
                unrecorded: Vec::new(),
                unused: Vec::new(),
            };
        };
        let start = Instant::now();
        let mut server = OpServer::new(ops);
        let result = self.rt.block_on(async {
            let (sink, mut rx) = host_sink();
            let reply = self.handle.call_with_host_sink(call, Some(sink));
            tokio::pin!(reply);
            loop {
                tokio::select! {
                    result = &mut reply => break result,
                    Some(op) = rx.recv() => server.answer(op),
                }
            }
        });
        Ran {
            result: result.map(|r| r.output),
            took: start.elapsed(),
            unused: server.unused(),
            unrecorded: server.unrecorded,
        }
    }

    fn call_with(&self, call: ScriptCall) -> (Result<Value, ScriptError>, Duration) {
        let start = Instant::now();
        let result = self.rt.block_on(self.handle.call(call)).map(|r| r.output);
        (result, start.elapsed())
    }
}

/// 1 回の呼び出しの結果。
struct Ran {
    result: Result<Value, ScriptError>,
    took: Duration,
    /// 記録に無かった要求（op と request の直列化）。
    unrecorded: Vec<String>,
    /// 呼ばれなかった記録の op。
    unused: Vec<String>,
}

/// runner の中の op server: TS の op に場面の記録から答える。照合は op と request の直列化の byte 一致
/// （`control-ts/src/testing/pass-replay.ts` と同じ規則）。記録に無い要求には `denied` を返し、その場面を不一致にする。
struct OpServer<'a> {
    ops: &'a [RecordedOp],
    used: Vec<bool>,
    unrecorded: Vec<String>,
}

impl<'a> OpServer<'a> {
    fn new(ops: &'a [RecordedOp]) -> Self {
        Self {
            ops,
            used: vec![false; ops.len()],
            unrecorded: Vec::new(),
        }
    }

    fn answer(&mut self, call: HostOpCall) {
        let name = call.op.as_str();
        let bytes = call.request.to_string();
        let at = self.ops.iter().position(|o| {
            let recorded = o.request.to_string();
            o.op == name && recorded == bytes
        });
        let reply = match at {
            Some(i) => {
                self.used[i] = true;
                self.ops[i].reply.clone()
            }
            None => {
                let what = format!("{name} {bytes}");
                let error = HostError::new(
                    HostErrorCode::Denied,
                    format!("request not in the record: {what}"),
                );
                self.unrecorded.push(what);
                HostResult::Error { error }
            }
        };
        // 受け手が居ない = 要求は終わっている。結果は呼び出しの戻り値が持つ。
        let _ = call.reply.send(reply);
    }

    fn unused(&self) -> Vec<String> {
        self.ops
            .iter()
            .zip(&self.used)
            .filter(|(_, used)| !**used)
            .map(|(o, _)| format!("{} {}", o.op, o.request))
            .collect()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 引数
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Suite {
    Golden,
    Safety,
    All,
}

struct RunArgs {
    bundle_dir: PathBuf,
    fixtures: PathBuf,
    suite: Suite,
    domains: Vec<String>,
    timeout: Duration,
}

struct ActivateArgs {
    bundle_dir: PathBuf,
    state_dir: PathBuf,
    timeout: Duration,
}

enum Command {
    Run(RunArgs),
    Activate(ActivateArgs),
    Domains,
}

/// 実行の誤り。exit code と理由。
struct Fatal {
    code: u8,
    message: String,
}

impl Fatal {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            message: message.into(),
        }
    }
    fn reject(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_REJECT,
            message: message.into(),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<Command, Fatal> {
    let mut it = args.into_iter();
    let sub = it.next().ok_or_else(|| Fatal::usage(USAGE))?;
    let mut bundle_dir = None;
    let mut fixtures = None;
    let mut state_dir = None;
    let mut suite = None;
    let mut domains = Vec::new();
    let mut timeout_ms = None;
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| Fatal::usage(format!("{flag} needs a value\n{USAGE}")))
        };
        let dup = |name: &str| Fatal::usage(format!("{name} given twice\n{USAGE}"));
        match flag.as_str() {
            "--bundle-dir" => {
                if bundle_dir.replace(PathBuf::from(value()?)).is_some() {
                    return Err(dup("--bundle-dir"));
                }
            }
            "--fixtures" => {
                if fixtures.replace(PathBuf::from(value()?)).is_some() {
                    return Err(dup("--fixtures"));
                }
            }
            "--state-dir" => {
                if state_dir.replace(PathBuf::from(value()?)).is_some() {
                    return Err(dup("--state-dir"));
                }
            }
            "--suite" => {
                let s = match value()?.as_str() {
                    "golden" => Suite::Golden,
                    "safety" => Suite::Safety,
                    "all" => Suite::All,
                    other => return Err(Fatal::usage(format!("unknown suite {other:?}\n{USAGE}"))),
                };
                if suite.replace(s).is_some() {
                    return Err(dup("--suite"));
                }
            }
            "--domain" => {
                let d = value()?;
                if !DOMAINS.iter().any(|s| s.name == d) {
                    return Err(Fatal::usage(format!("unknown domain {d:?}")));
                }
                if !domains.contains(&d) {
                    domains.push(d);
                }
            }
            "--call-timeout-ms" => {
                let v = value()?;
                let ms: u64 = v.parse().ok().filter(|ms| *ms > 0).ok_or_else(|| {
                    Fatal::usage(format!(
                        "--call-timeout-ms must be a positive integer: {v:?}"
                    ))
                })?;
                if timeout_ms.replace(ms).is_some() {
                    return Err(dup("--call-timeout-ms"));
                }
            }
            other => return Err(Fatal::usage(format!("unknown argument {other:?}\n{USAGE}"))),
        }
    }
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_CALL_TIMEOUT_MS));
    let need = |v: Option<PathBuf>, name: &str| {
        v.ok_or_else(|| Fatal::usage(format!("{name} is required\n{USAGE}")))
    };
    let unexpected = |present: bool, name: &str| {
        if present {
            Err(Fatal::usage(format!(
                "{name} is not accepted by {sub}\n{USAGE}"
            )))
        } else {
            Ok(())
        }
    };
    match sub.as_str() {
        "run" => {
            unexpected(state_dir.is_some(), "--state-dir")?;
            Ok(Command::Run(RunArgs {
                bundle_dir: need(bundle_dir, "--bundle-dir")?,
                fixtures: need(fixtures, "--fixtures")?,
                suite: suite.unwrap_or(Suite::All),
                domains,
                timeout,
            }))
        }
        "activate" => {
            unexpected(fixtures.is_some(), "--fixtures")?;
            unexpected(suite.is_some(), "--suite")?;
            unexpected(!domains.is_empty(), "--domain")?;
            Ok(Command::Activate(ActivateArgs {
                bundle_dir: need(bundle_dir, "--bundle-dir")?,
                state_dir: need(state_dir, "--state-dir")?,
                timeout,
            }))
        }
        "domains" => {
            unexpected(
                bundle_dir.is_some()
                    || fixtures.is_some()
                    || state_dir.is_some()
                    || suite.is_some()
                    || !domains.is_empty()
                    || timeout_ms.is_some(),
                "an option",
            )?;
            Ok(Command::Domains)
        }
        other => Err(Fatal::usage(format!("unknown command {other:?}\n{USAGE}"))),
    }
}

// ---------------------------------------------------------------------------
// manifest の検査（安全シナリオ 1）
// ---------------------------------------------------------------------------

/// manifest の `domains` = 採点の表 ∪ {host_selftest}、`capabilities` ⊆ 各領域の許可表の和。
/// 失敗は bundle の拒否（exit 3）。成功すれば安全シナリオの結果の行を返す。
fn check_manifest(bundle: &VerifiedBundle) -> Result<Vec<Value>, Fatal> {
    let m = &bundle.manifest;
    let declared: BTreeSet<&str> = m.domains.iter().map(String::as_str).collect();
    if declared.len() != m.domains.len() {
        return Err(Fatal::reject("manifest declares a domain twice"));
    }
    let expected: BTreeSet<&str> = DOMAINS
        .iter()
        .map(|d| d.name)
        .chain([SELFTEST_DOMAIN])
        .collect();
    if declared != expected {
        let missing: Vec<_> = expected.difference(&declared).collect();
        let extra: Vec<_> = declared.difference(&expected).collect();
        return Err(Fatal::reject(format!(
            "manifest domains do not match the graded domains: missing {missing:?}, not graded {extra:?}"
        )));
    }
    let allowed: BTreeSet<_> = m
        .domains
        .iter()
        .flat_map(|d| domain_capabilities(d).iter().copied())
        .collect();
    if let Some(cap) = m.capabilities.iter().find(|c| !allowed.contains(c)) {
        return Err(Fatal::reject(format!(
            "manifest capability {cap:?} is not allowed for any declared domain"
        )));
    }
    Ok(vec![
        json!({ "id": "manifest/domains", "expect": "graded_domains", "outcome": "graded_domains", "ok": true }),
        json!({ "id": "manifest/capabilities", "expect": "subset_of_allowed", "outcome": "subset_of_allowed", "ok": true }),
    ])
}

fn load_checked(dir: &Path) -> Result<VerifiedBundle, Fatal> {
    load_bundle_dir(dir).map_err(|e| Fatal::reject(format!("bundle rejected: {e}")))
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

struct Row {
    /// 行の file（`--fixtures` の dir からの相対 path）。
    file: &'static str,
    line: usize,
    scene: Scene,
    expected: Value,
}

/// 領域の fixture の file を全部読む。入出力の誤りは exit 2。
fn read_fixture(dir: &Path, spec: &DomainSpec) -> Result<Vec<Row>, Fatal> {
    let mut rows = Vec::new();
    for fixture in spec.fixtures {
        let path = dir.join(fixture.path);
        let text = fs::read_to_string(&path)
            .map_err(|e| Fatal::usage(format!("cannot read fixture {}: {e}", path.display())))?;
        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let bad = |why: &str| Fatal::usage(format!("{}:{line}: {why}", path.display()));
            let row: Value = serde_json::from_str(raw).map_err(|e| bad(&e.to_string()))?;
            let parsed = (fixture.parse)(&row).map_err(|e| bad(&e))?;
            rows.push(Row {
                file: fixture.path,
                line,
                scene: parsed.scene,
                expected: parsed.expected,
            });
        }
    }
    Ok(rows)
}

#[derive(Default)]
struct Counts {
    matched: u64,
    mismatch: u64,
    undeclared_field: u64,
    invalid_output: u64,
    exception: u64,
    timeout: u64,
    denied: u64,
    host_error: u64,
    order_dependent: u64,
    violations: std::collections::BTreeMap<&'static str, u64>,
}

impl Counts {
    fn add(&mut self, o: Outcome) {
        let slot = match o {
            Outcome::Match => &mut self.matched,
            Outcome::Mismatch => &mut self.mismatch,
            Outcome::UndeclaredField => &mut self.undeclared_field,
            Outcome::InvalidOutput => &mut self.invalid_output,
            Outcome::Exception => &mut self.exception,
            Outcome::Timeout => &mut self.timeout,
            Outcome::Denied => &mut self.denied,
            Outcome::HostError => &mut self.host_error,
        };
        *slot += 1;
    }
}

struct Failures(Vec<Value>);

impl Failures {
    fn push(&mut self, v: Value) {
        if self.0.len() < MAX_FAILURES {
            self.0.push(v);
        }
    }
}

/// 最近傍順位の百分位（昇順に並べた `sorted` の ceil(p·n) 番目）。
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

/// 1 領域の golden の全件を正順と逆順で呼ぶ。失敗が 1 件でもあれば false。
fn run_golden(
    host: &Host,
    spec: &DomainSpec,
    rows: &[Row],
    failures: &mut Failures,
) -> (Value, bool) {
    let start = Instant::now();
    let mut counts = Counts::default();
    let mut call_us = Vec::with_capacity(rows.len() * 2);
    let mut first = Vec::with_capacity(rows.len());
    for row in rows {
        let ran = host.run_scene(spec.name, &row.scene);
        call_us.push(micros(ran.took));
        let j = judge(spec, &row.scene, &row.expected, &ran);
        counts.add(j.outcome);
        for v in &j.violations {
            *counts.violations.entry(v).or_default() += 1;
        }
        if j.outcome != Outcome::Match || !j.violations.is_empty() {
            failures.push(json!({
                "kind": if j.outcome == Outcome::Match { j.violations[0] } else { j.outcome.name() },
                "domain": spec.name,
                "file": row.file,
                "line": row.line,
                "pass": 1,
                "expected": Value::Object(spec.fields.iter().map(|f| ((*f).to_owned(), row.expected[*f].clone())).collect()),
                "actual": ran.result.as_ref().ok(),
                "error": j.error,
                "safety_violations": j.violations,
            }));
        }
        first.push(fingerprint(&ran));
    }
    // 逆順の 2 回目。同じ行の結果が 1 回目と違えば隠れた入力がある（境界 ADR 付記 S2-7）。
    for (i, row) in rows.iter().enumerate().rev() {
        let ran = host.run_scene(spec.name, &row.scene);
        call_us.push(micros(ran.took));
        if fingerprint(&ran) != first[i] {
            counts.order_dependent += 1;
            failures.push(json!({
                "kind": "order_dependent",
                "domain": spec.name,
                "file": row.file,
                "line": row.line,
                "pass": 2,
                "expected": first[i],
                "actual": fingerprint(&ran),
                "error": null,
            }));
        }
    }
    let mut too_few = false;
    for fixture in spec.fixtures {
        let n = rows.iter().filter(|r| r.file == fixture.path).count();
        if n < fixture.min_rows {
            too_few = true;
            failures.push(json!({
                "kind": "too_few_rows",
                "domain": spec.name,
                "file": fixture.path,
                "line": null,
                "pass": null,
                "expected": fixture.min_rows,
                "actual": n,
                "error": format!("{} has {n} rows, at least {} required", fixture.path, fixture.min_rows),
            }));
        }
    }
    let ok = !too_few
        && counts.matched == rows.len() as u64
        && counts.order_dependent == 0
        && counts.violations.is_empty();
    call_us.sort_unstable();
    let report = json!({
        "domain": spec.name,
        "fixtures": spec.fixtures.iter().map(|f| f.path).collect::<Vec<_>>(),
        "rows": rows.len(),
        "min_rows": spec.min_rows(),
        "passes": 2,
        "matched": counts.matched,
        "mismatch": counts.mismatch,
        "undeclared_field": counts.undeclared_field,
        "invalid_output": counts.invalid_output,
        "exception": counts.exception,
        "timeout": counts.timeout,
        "denied": counts.denied,
        "host_error": counts.host_error,
        "order_dependent": counts.order_dependent,
        "safety_violations": counts.violations,
        "ok": ok,
        "elapsed_ms": start.elapsed().as_millis() as u64,
        "call_us": {
            "p50": percentile(&call_us, 0.50),
            "p99": percentile(&call_us, 0.99),
            "max": call_us.last().copied().unwrap_or(0),
        },
    });
    (report, ok)
}

/// 壊した場面（入力か op の応答）を渡し、TS が拒むことを確かめる（安全シナリオ 2）。
fn run_broken(host: &Host, spec: &DomainSpec, failures: &mut Failures) -> (Vec<Value>, bool) {
    let mut rows = Vec::new();
    let mut all_ok = true;
    for broken in (spec.broken)() {
        let ran = host.run_scene(spec.name, &broken.scene);
        let result = &ran.result;
        let (outcome, error) = match result {
            Ok(_) => ("returned_output", None),
            Err(e) => (error_outcome(e).name(), Some(e.to_string())),
        };
        let expect = broken.expect.name();
        // 壊した場面の記録に無い要求を呼んだら、結果が期待どおりでも失敗（別の理由で拒まれている）。
        let ok = outcome == expect && ran.unrecorded.is_empty();
        all_ok &= ok;
        let id = format!("{}/{}", spec.name, broken.id);
        if !ok {
            failures.push(json!({
                "kind": "safety",
                "domain": spec.name,
                "id": id,
                "expected": expect,
                "actual": result.as_ref().ok(),
                "error": error,
                "unrecorded": ran.unrecorded,
            }));
        }
        rows.push(json!({ "id": id, "expect": expect, "outcome": outcome, "ok": ok }));
    }
    (rows, all_ok)
}

fn run(args: RunArgs) -> Result<(Value, bool), Fatal> {
    let start = Instant::now();
    let specs: Vec<&DomainSpec> = DOMAINS
        .iter()
        .filter(|s| args.domains.is_empty() || args.domains.iter().any(|d| d == s.name))
        .collect();
    // 呼ぶ前に fixture を全部読む（入出力の誤りは exit 2）。
    let fixtures = if args.suite == Suite::Safety {
        Vec::new()
    } else {
        specs
            .iter()
            .map(|s| read_fixture(&args.fixtures, s))
            .collect::<Result<Vec<_>, _>>()?
    };
    if args.suite == Suite::Safety && !args.fixtures.is_dir() {
        return Err(Fatal::usage(format!(
            "--fixtures {} is not a directory",
            args.fixtures.display()
        )));
    }
    let bundle = load_checked(&args.bundle_dir)?;
    let manifest_rows = check_manifest(&bundle)?;
    let host = Host::start(&bundle, args.timeout).map_err(Fatal::reject)?;

    let mut failures = Failures(Vec::new());
    let mut ok = true;
    let mut safety = Vec::new();
    if args.suite != Suite::Golden {
        safety.extend(manifest_rows);
        for spec in &specs {
            let (rows, passed) = run_broken(&host, spec, &mut failures);
            safety.extend(rows);
            ok &= passed;
        }
    }
    let mut domains = Vec::new();
    for (spec, rows) in specs.iter().zip(&fixtures) {
        let (report, passed) = run_golden(&host, spec, rows, &mut failures);
        domains.push(report);
        ok &= passed;
    }
    let report = json!({
        "schema": SCHEMA,
        "command": "run",
        "suite": match args.suite { Suite::Golden => "golden", Suite::Safety => "safety", Suite::All => "all" },
        "ok": ok,
        "complete": args.domains.is_empty(),
        "bundle_sha256": bundle.manifest.bundle_sha256,
        "host_api": HOST_API_VERSION,
        "schema_sha256": host_schema_sha256(),
        "call_timeout_ms": args.timeout.as_millis() as u64,
        "domains": domains,
        "safety": safety,
        "failures": failures.0,
        "elapsed_ms": start.elapsed().as_millis() as u64,
    });
    Ok((report, ok))
}

// ---------------------------------------------------------------------------
// activate
// ---------------------------------------------------------------------------

/// daemon と同じ `deploy_bundle`・`load_deployed` で一時 state_dir に配備して load し、selftest と各領域 1 件を呼ぶ。
fn activate(args: ActivateArgs) -> Result<(Value, bool), Fatal> {
    let start = Instant::now();
    let deployed = deploy_bundle(&args.bundle_dir, &args.state_dir)
        .map_err(|e| Fatal::reject(format!("deploy failed: {e}")))?;
    let sha = deployed.manifest.bundle_sha256.clone();
    let bundle = load_deployed(&args.state_dir, &sha)
        .map_err(|e| Fatal::reject(format!("deployed bundle rejected: {e}")))?;
    check_manifest(&bundle)?;
    let host = Host::start(&bundle, args.timeout).map_err(Fatal::reject)?;

    let (result, _) = host.call_with(selftest_call(args.timeout));
    let selftest = match result
        .map_err(|e| e.to_string())
        .and_then(|v| check_output(&v))
    {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    };
    let mut ok = selftest["ok"] == json!(true);
    let mut probed = Vec::new();
    for spec in DOMAINS {
        let scene = (spec.probe)();
        let ran = host.run_scene(spec.name, &scene);
        let (outcome, error) = match check_shape_replayed(spec, &scene, &ran) {
            Ok(c) if c.violations.is_empty() => ("valid", None),
            Ok(c) => (c.violations[0], None),
            Err(j) => (j.outcome.name(), j.error),
        };
        ok &= outcome == "valid";
        probed.push(json!({
            "domain": spec.name,
            "outcome": outcome,
            "ok": outcome == "valid",
            "error": error,
            "call_us": micros(ran.took),
        }));
    }
    let report = json!({
        "schema": SCHEMA,
        "command": "activate",
        "ok": ok,
        "bundle_sha256": sha,
        "host_api": HOST_API_VERSION,
        "schema_sha256": host_schema_sha256(),
        "deployed_dir": deployed_dir(&args.state_dir, &sha).display().to_string(),
        "selftest": selftest,
        "probed": probed,
        "elapsed_ms": start.elapsed().as_millis() as u64,
    });
    Ok((report, ok))
}

// ---------------------------------------------------------------------------
// domains
// ---------------------------------------------------------------------------

fn domains_report() -> Value {
    json!({
        "schema": SCHEMA,
        "command": "domains",
        "selftest_domain": SELFTEST_DOMAIN,
        "domains": DOMAINS.iter().map(|s| json!({
            "domain": s.name,
            "fixtures": s.fixtures.iter().map(|f| json!({
                "path": f.path,
                "min_rows": f.min_rows,
                "rule": f.rule,
            })).collect::<Vec<_>>(),
            "min_rows": s.min_rows(),
            "fields": s.fields,
            "broken_inputs": (s.broken)().into_iter().map(|b| b.id).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command_name = args.first().cloned().unwrap_or_default();
    let result = parse_args(args).and_then(|cmd| match cmd {
        Command::Run(a) => run(a),
        Command::Activate(a) => activate(a),
        Command::Domains => Ok((domains_report(), true)),
    });
    match result {
        Ok((report, ok)) => {
            println!("{report}");
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_FAIL)
            }
        }
        Err(fatal) => {
            eprintln!("script-host-runner: {}", fatal.message);
            if fatal.code != EXIT_USAGE {
                let report = json!({
                    "schema": SCHEMA,
                    "command": command_name,
                    "ok": false,
                    "exit": fatal.code,
                    "error": fatal.message,
                });
                println!("{report}");
            }
            ExitCode::from(fatal.code)
        }
    }
}
