//! TS だけの変更を検証する本物の deno_core host（ADR 2026-11-02 ts-fast-verify (3)）。
//!
//! bundle の manifest を検証して load し、採点の表（この file の [`DOMAINS`]）にある領域の Rust golden（JSONL）の全件を
//! 正順と逆順の 2 回呼んで比べ、安全シナリオ（壊した入力の拒否・宣言外の出力・不正な出力・例外・timeout・拒否）を判定して、
//! 結果を JSON 1 個で stdout に出す。検証専用の道具で、daemon・celerisctl の経路には入らない。
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

use celeris_script::dto::HOST_API_VERSION;
use celeris_script::manifest::{
    VerifiedBundle, deploy_bundle, deployed_dir, host_schema_sha256, load_bundle_dir, load_deployed,
};
use celeris_script::selftest::{SELFTEST_DOMAIN, check_output, selftest_call};
use celeris_script::{ScriptCall, ScriptError, ScriptRuntimeHandle, domain_capabilities};
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

/// 採点する領域 1 つ。
struct DomainSpec {
    name: &'static str,
    /// `--fixtures` の dir からの相対 path。
    fixture: &'static str,
    /// これより少ない行の fixture は失敗（空の fixture・欠けた fixture を通さない）。
    min_rows: usize,
    /// 比べる欄。出力の欄の集合はこれと完全に同じでなければならない。
    fields: &'static [&'static str],
    /// fixture の 1 行から入力を作る規則（`domains` の出力に載せる説明）。
    input_rule: &'static str,
    input: fn(&Value) -> Option<Value>,
    /// 出力の値の検査。`input` は TS に渡した入力。
    check: fn(&Map<String, Value>, &Value) -> Result<Checked, String>,
    /// activate で 1 回呼ぶ入力。
    probe: fn() -> Value,
    /// TS が throw しなければならない壊した入力。
    broken: fn() -> Vec<(&'static str, Value)>,
}

static DOMAINS: &[DomainSpec] = &[
    DomainSpec {
        name: "lane_rules",
        fixture: "lane_rules/all-features.jsonl",
        // 9 軸 × 3 値の全組み合わせ（3^9）。
        min_rows: 19_683,
        fields: &["rule_id", "proposed", "condition"],
        input_rule: "{\"features\": row.features}",
        input: lane_rules_input,
        check: lane_rules_check,
        probe: lane_rules_probe,
        broken: lane_rules_broken,
    },
    DomainSpec {
        name: "route_priority",
        fixture: "route_priority/cases.jsonl",
        // task-core の dump-route-priority-fixtures が今書く行数。
        min_rows: 2_026,
        fields: &["basis", "candidates"],
        input_rule: "row.input",
        input: route_priority_input,
        check: route_priority_check,
        probe: route_priority_probe,
        broken: route_priority_broken,
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

fn lane_rules_input(row: &Value) -> Option<Value> {
    Some(json!({ "features": row.get("features")?.clone() }))
}

fn lane_rules_check(out: &Map<String, Value>, _input: &Value) -> Result<Checked, String> {
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

fn lane_rules_probe() -> Value {
    let features: Map<String, Value> = AXES
        .iter()
        .map(|a| ((*a).to_owned(), json!("low")))
        .collect();
    json!({ "features": features })
}

fn lane_rules_broken() -> Vec<(&'static str, Value)> {
    let base = lane_rules_probe();
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
}

fn route_priority_input(row: &Value) -> Option<Value> {
    row.get("input").cloned()
}

fn route_priority_check(out: &Map<String, Value>, input: &Value) -> Result<Checked, String> {
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
    let declared = table_candidates(input);
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

fn route_priority_probe() -> Value {
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

fn route_priority_broken() -> Vec<(&'static str, Value)> {
    let base = route_priority_probe();
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
    input: &Value,
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
    (spec.check)(map, input).map_err(|e| fail(Outcome::InvalidOutput, e))
}

/// golden の 1 行と比べる。欄ごとの serde_json の直列化が byte 一致なら match（境界 ADR 付記 S2-2）。
fn judge(
    spec: &DomainSpec,
    input: &Value,
    expected: &Value,
    result: &Result<Value, ScriptError>,
) -> Judged {
    let checked = match check_shape(spec, input, result) {
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

/// 正順と逆順の結果を比べるための指紋。
fn fingerprint(result: &Result<Value, ScriptError>) -> String {
    match result {
        Ok(v) => format!("ok:{v}"),
        Err(e) => format!("err:{e}"),
    }
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

    /// 空の scope で呼ぶ（純粋な領域は op を使わない。使えば `denied`）。
    fn call(&self, domain: &str, input: Value) -> (Result<Value, ScriptError>, Duration) {
        self.call_with(ScriptCall {
            domain: domain.to_owned(),
            input,
            scope_task_ids: BTreeSet::new(),
            timeout: self.timeout,
        })
    }

    fn call_with(&self, call: ScriptCall) -> (Result<Value, ScriptError>, Duration) {
        let start = Instant::now();
        let result = self.rt.block_on(self.handle.call(call)).map(|r| r.output);
        (result, start.elapsed())
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
    line: usize,
    input: Value,
    expected: Value,
}

fn read_fixture(dir: &Path, spec: &DomainSpec) -> Result<Vec<Row>, Fatal> {
    let path = dir.join(spec.fixture);
    let text = fs::read_to_string(&path)
        .map_err(|e| Fatal::usage(format!("cannot read fixture {}: {e}", path.display())))?;
    let mut rows = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let bad = |why: &str| Fatal::usage(format!("{}:{line}: {why}", path.display()));
        let row: Value = serde_json::from_str(raw).map_err(|e| bad(&e.to_string()))?;
        let input = (spec.input)(&row).ok_or_else(|| bad("cannot build the input"))?;
        if let Some(f) = spec.fields.iter().find(|f| row.get(**f).is_none()) {
            return Err(bad(&format!("missing expected field {f:?}")));
        }
        rows.push(Row {
            line,
            input,
            expected: row,
        });
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
        let (result, took) = host.call(spec.name, row.input.clone());
        call_us.push(micros(took));
        let j = judge(spec, &row.input, &row.expected, &result);
        counts.add(j.outcome);
        for v in &j.violations {
            *counts.violations.entry(v).or_default() += 1;
        }
        if j.outcome != Outcome::Match || !j.violations.is_empty() {
            failures.push(json!({
                "kind": if j.outcome == Outcome::Match { j.violations[0] } else { j.outcome.name() },
                "domain": spec.name,
                "line": row.line,
                "pass": 1,
                "expected": Value::Object(spec.fields.iter().map(|f| ((*f).to_owned(), row.expected[*f].clone())).collect()),
                "actual": result.as_ref().ok(),
                "error": j.error,
                "safety_violations": j.violations,
            }));
        }
        first.push(fingerprint(&result));
    }
    // 逆順の 2 回目。同じ行の結果が 1 回目と違えば隠れた入力がある（境界 ADR 付記 S2-7）。
    for (i, row) in rows.iter().enumerate().rev() {
        let (result, took) = host.call(spec.name, row.input.clone());
        call_us.push(micros(took));
        if fingerprint(&result) != first[i] {
            counts.order_dependent += 1;
            failures.push(json!({
                "kind": "order_dependent",
                "domain": spec.name,
                "line": row.line,
                "pass": 2,
                "expected": first[i],
                "actual": fingerprint(&result),
                "error": null,
            }));
        }
    }
    let too_few = rows.len() < spec.min_rows;
    if too_few {
        failures.push(json!({
            "kind": "too_few_rows",
            "domain": spec.name,
            "line": null,
            "pass": null,
            "expected": spec.min_rows,
            "actual": rows.len(),
            "error": format!("{} has {} rows, at least {} required", spec.fixture, rows.len(), spec.min_rows),
        }));
    }
    let ok = !too_few
        && counts.matched == rows.len() as u64
        && counts.order_dependent == 0
        && counts.violations.is_empty();
    call_us.sort_unstable();
    let report = json!({
        "domain": spec.name,
        "fixture": spec.fixture,
        "rows": rows.len(),
        "min_rows": spec.min_rows,
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

/// 壊した入力を渡し、TS が throw する（`exception`）ことを確かめる（安全シナリオ 2）。
fn run_broken(host: &Host, spec: &DomainSpec, failures: &mut Failures) -> (Vec<Value>, bool) {
    let mut rows = Vec::new();
    let mut all_ok = true;
    for (id, input) in (spec.broken)() {
        let (result, _) = host.call(spec.name, input);
        let (outcome, error) = match &result {
            Ok(_) => ("returned_output", None),
            Err(e) => (error_outcome(e).name(), Some(e.to_string())),
        };
        let ok = outcome == "exception";
        all_ok &= ok;
        let id = format!("{}/{id}", spec.name);
        if !ok {
            failures.push(json!({
                "kind": "safety",
                "domain": spec.name,
                "id": id,
                "expected": "exception",
                "actual": result.as_ref().ok(),
                "error": error,
            }));
        }
        rows.push(json!({ "id": id, "expect": "exception", "outcome": outcome, "ok": ok }));
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
        let input = (spec.probe)();
        let (result, took) = host.call(spec.name, input.clone());
        let (outcome, error) = match check_shape(spec, &input, &result) {
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
            "call_us": micros(took),
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
            "fixture": s.fixture,
            "min_rows": s.min_rows,
            "fields": s.fields,
            "input_rule": s.input_rule,
            "broken_inputs": (s.broken)().into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
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
