//! `script-host-runner` の試験（ADR 2026-11-02 ts-fast-verify (3)）。
//!
//! 業務 TS と `control-ts/dist`・`control-ts/fixtures` は使わない。試験の中で小さな手書きの bundle と、
//! 採点の表の形に合う fixture（`lane_rules` は 9 軸 × 3 値の全組み合わせ、`route_priority`・`cos_digest` は最少行数ちょうど）を
//! tempdir に書き、一致・不一致・例外・宣言外の 4 場合と bundle の拒否を固定する。`cos_digest` は op を await する pass なので、
//! 記録からの replay（一致・不一致・記録外の op・呼ばれない記録・壊した op の応答）も固定する（ADR 付記 grade-cos-digest）。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use celeris_script::manifest::{
    BUNDLE_FILE, MANIFEST_FILE, MANIFEST_SCHEMA, host_schema_sha256, sha256_hex,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

const AXES: [&str; 9] = [
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
const LEVELS: [&str; 3] = ["low", "medium", "high"];
const LANE_ROWS: usize = 19_683;
const ROUTE_ROWS: usize = 2_026;
const TRACE_ROWS: usize = 20;
const CASE_ROWS: usize = 2_066;
const DIGEST_ROWS: usize = TRACE_ROWS + CASE_ROWS;
const RUN_STATES: [&str; 2] = ["completed", "stopped"];
const GATE_ROWS: usize = 2_337;
const PC_FIXTURE: &str =
    include_str!("../../../control-ts/fixtures/project_controller/invariants.jsonl");
const ALL_DOMAINS: [&str; 6] = [
    "cos_digest",
    "execution_gate",
    "host_selftest",
    "lane_rules",
    "project_controller",
    "route_priority",
];

/// 試験用の bundle。入力の形を厳しく検査して throw し、`lane_rules` は `__LANE__` の式の値を返す。
const BUNDLE_TEMPLATE: &str = r#"
import { cosDigestListRuns, cosDigestReadRun, hostVersion, selftestEcho } from "celeris:host";
const AXES = __AXES__;
const LEVELS = ["low", "medium", "high"];
function exact(v, keys, where) {
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw new Error(where + " must be an object");
  for (const k of Object.keys(v)) if (!keys.includes(k)) throw new Error("unknown field in " + where + ": " + k);
  for (const k of keys) if (!Object.hasOwn(v, k)) throw new Error("missing field in " + where + ": " + k);
  return v;
}
function str(v, where) { if (typeof v !== "string") throw new Error(where + " must be a string"); return v; }
function nullable(v, where) { return v === null ? null : str(v, where); }
function arr(v, where) { if (!Array.isArray(v)) throw new Error(where + " must be an array"); return v; }
function cand(c) { exact(c, ["source", "model"], "candidate"); str(c.source, "source"); str(c.model, "model"); return c; }
function lane(input) {
  exact(input, ["features"], "input");
  const f = exact(input.features, AXES, "features");
  for (const a of AXES) if (!LEVELS.includes(f[a])) throw new Error("unknown level " + f[a]);
  const base = { rule_id: "standard/default", proposed: "standard", condition: "default" };
  return __LANE__;
}
function route(input) {
  exact(input, ["harness", "lane", "cos", "execution_class", "table"], "input");
  str(input.harness, "harness");
  nullable(input.lane, "lane");
  if (typeof input.cos !== "boolean") throw new Error("cos must be a boolean");
  nullable(input.execution_class, "execution_class");
  const t = exact(input.table, ["route_defaults", "lane_route_defaults", "class_defaults", "cos_route_candidates"], "table");
  arr(t.route_defaults, "route_defaults").forEach(cand);
  arr(t.cos_route_candidates, "cos_route_candidates").forEach(cand);
  for (const l of arr(t.lane_route_defaults, "lane_route_defaults")) { exact(l, ["lane", "candidates"], "lane"); arr(l.candidates, "candidates").forEach(cand); }
  for (const c of arr(t.class_defaults, "class_defaults")) { exact(c, ["execution_class", "candidates"], "class"); arr(c.candidates, "candidates").forEach(cand); }
  return { basis: "harness", candidates: t.route_defaults };
}
function gate(input) {
  exact(input, ["features", "title", "objective", "acceptance_total", "artifact_exists", "has_human", "has_command", "multi_environment", "recent_budget_exhausted_ratio", "cos_hint_compound", "max_turns", "tree_child", "threshold"], "input");
  const f = exact(input.features, AXES, "features");
  for (const a of AXES) if (!LEVELS.includes(f[a])) throw new Error("unknown level " + f[a]);
  str(input.title, "title");
  str(input.objective, "objective");
  if (!Number.isInteger(input.max_turns)) throw new Error("max_turns must be an integer");
  if (!Number.isInteger(input.threshold)) throw new Error("threshold must be an integer");
  const ratio = input.recent_budget_exhausted_ratio;
  if (ratio !== null && !(ratio >= 0 && ratio <= 1)) throw new Error("bad ratio");
  return { mode: "atomic", score: 0, rule_id: "atomic/score", signals: [] };
}
function commands(r) {
  const post = { kind: "post_digest", thread: r.thread, idempotency_key: "cos-triage-digest:" + r.run_id, text: "state " + r.run_state, cards: [], run_id: r.run_id };
  return r.run_state === "stopped" ? [post, { kind: "requeue_run", run_id: r.run_id }] : [post];
}
async function digest(ctx) {
  const list = exact(await cosDigestListRuns(ctx, {}), ["runs"], "list");
  const runs = [];
  for (const ref of arr(list.runs, "runs")) {
    exact(ref, ["thread", "run_id", "run_state"], "ref");
    let r;
    try {
      r = await cosDigestReadRun(ctx, { thread: ref.thread, run_id: ref.run_id });
    } catch (e) {
      if (e?.name === "HostError" && e.code === "exception") continue;
      throw e;
    }
    if (r.thread !== ref.thread || r.run_id !== ref.run_id) throw new Error("read_run answered another run");
    for (const it of r.items) if (it.route !== null && it.route !== "escalation" && it.route !== "fallback") throw new Error("unknown route");
    runs.push({ run_id: ref.run_id, commands: commands(r) });
  }
  return { runs };
}
const PC_KEYS = ["now","truncated","project","brief","outcomes","outcomes_invalid","roots","waits","stagnation","wakes","decisions","controller","limits","roadmap","authority"];
function pc(input) {
  if (typeof input !== "object" || input === null || Array.isArray(input)) throw new Error("not object");
  exact(input, PC_KEYS, "snapshot");
  if (typeof input.controller !== "object" || input.controller === null) throw new Error("controller");
  if (!Array.isArray(input.roots)) throw new Error("roots");
  return {
    action: { kind: "wait", waiting_on: [], next_wake_after_secs: 0, phase: "achieved" },
    gap: { reached: [], open: [], unknown: [], progress_epoch: 0 },
    explanation: { rule_id: "test/always-wait", signals: [] },
    based_on: { revision: input.controller.revision, human_epoch: input.controller.human_epoch, request_sha256: input.project.request_sha256 },
  };
}
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  switch (domain) {
    case "project_controller": return pc(input);
    case "cos_digest": return digest(ctx);
    case "execution_gate": return gate(input);
    case "host_selftest": return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
    case "lane_rules": return lane(input);
    case "route_priority": return route(input);
    default: throw new Error("unknown domain " + domain);
  }
};
"#;

/// 一致する bundle の `lane_rules` の式。
const LANE_MATCH: &str = "base";

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_script-host-runner").expect("script-host-runner bin")
}

/// `lane` を差し込んだ bundle と manifest を `dir` に書く。
fn write_bundle(dir: &Path, lane: &str, domains: &[&str]) {
    let code = BUNDLE_TEMPLATE
        .replace("__AXES__", &json!(AXES).to_string())
        .replace("__LANE__", lane);
    fs::create_dir_all(dir).expect("bundle dir");
    fs::write(dir.join(BUNDLE_FILE), &code).expect("write bundle");
    let manifest = json!({
        "schema": MANIFEST_SCHEMA,
        "bundle_sha256": sha256_hex(code.as_bytes()),
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": { "typescript": "test", "esbuild": "test", "node": "test" },
        "source_commit": "test",
        "domains": domains,
        "capabilities": ["selftest:echo_task", "cos_digest:list_runs", "cos_digest:read_run"],
        "checkpoint_versions": [],
    });
    fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
}

/// run 1 つの pass の入力（`read_run` の答え）。`n` で run と状態が決まる。
fn digest_input(prefix: &str, n: usize) -> Value {
    json!({
        "thread": format!("thread-{prefix}{n}"),
        "run_id": format!("run-{prefix}{n}"),
        "run_state": RUN_STATES[n % 2],
        "reason": null,
        "items": [],
    })
}

/// 一致する bundle の `commands(r)` の出力。
fn digest_commands(input: &Value) -> Value {
    let (thread, run_id, state) = (&input["thread"], &input["run_id"], &input["run_state"]);
    let post = json!({
        "kind": "post_digest",
        "thread": thread,
        "idempotency_key": format!("cos-triage-digest:{}", run_id.as_str().expect("run_id")),
        "text": format!("state {}", state.as_str().expect("state")),
        "cards": [],
        "run_id": run_id,
    });
    if state == "stopped" {
        json!([post, { "kind": "requeue_run", "run_id": run_id }])
    } else {
        json!([post])
    }
}

/// 採点の表の形に合う fixture。期待値は一致する bundle（[`LANE_MATCH`]）の出力。
fn write_fixtures(dir: &Path) {
    let mut lane = String::new();
    for n in 0..LANE_ROWS {
        let mut features = Map::new();
        let mut rest = n;
        for axis in AXES {
            features.insert(axis.into(), json!(LEVELS[rest % 3]));
            rest /= 3;
        }
        let row = json!({
            "features": features,
            "rule_id": "standard/default",
            "proposed": "standard",
            "condition": "default",
        });
        lane.push_str(&row.to_string());
        lane.push('\n');
    }
    let mut route = String::new();
    for n in 0..ROUTE_ROWS {
        let candidates = json!([{ "source": format!("s{n}"), "model": "m" }]);
        let row = json!({
            "case": format!("test/{n}"),
            "input": {
                "harness": "claude_code",
                "lane": if n % 2 == 0 { json!("standard") } else { Value::Null },
                "cos": false,
                "execution_class": null,
                "table": {
                    "route_defaults": candidates,
                    "lane_route_defaults": [],
                    "class_defaults": [],
                    "cos_route_candidates": [],
                },
            },
            "basis": "harness",
            "candidates": candidates,
        });
        route.push_str(&row.to_string());
        route.push('\n');
    }
    // pass の trace（1 行 1 pass。`run 2 つ` の pass で一覧の順も見る）。
    let mut traces = String::new();
    for n in 0..TRACE_ROWS {
        let inputs = [digest_input("t", 2 * n), digest_input("t", 2 * n + 1)];
        let mut ops = vec![json!({
            "op": "cos_digest:list_runs",
            "request": {},
            "reply": { "kind": "ok", "value": { "runs": inputs.iter().map(|i| json!({
                "thread": i["thread"], "run_id": i["run_id"], "run_state": i["run_state"],
            })).collect::<Vec<_>>() } },
        })];
        for i in &inputs {
            ops.push(json!({
                "op": "cos_digest:read_run",
                "request": { "thread": i["thread"], "run_id": i["run_id"] },
                "reply": { "kind": "ok", "value": i },
            }));
        }
        let row = json!({
            "name": format!("trace/{n}"),
            "ops": ops,
            "writes": { "messages": [], "requeued": [] },
            "output": { "runs": inputs.iter().map(|i| json!({
                "run_id": i["run_id"], "commands": digest_commands(i),
            })).collect::<Vec<_>>() },
        });
        traces.push_str(&row.to_string());
        traces.push('\n');
    }
    // `execution_gate`: 試験の bundle はいつも同じ答えを返す。
    let features: serde_json::Map<String, Value> = AXES
        .iter()
        .map(|a| ((*a).to_owned(), json!("low")))
        .collect();
    let mut gate = String::new();
    for n in 0..GATE_ROWS {
        let row = json!({
            "case": format!("gate/{n}"),
            "input": {
                "features": features,
                "title": "t",
                "objective": format!("o{n}"),
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
            },
            "mode": "atomic",
            "score": 0,
            "rule_id": "atomic/score",
            "signals": [],
        });
        gate.push_str(&row.to_string());
        gate.push('\n');
    }
    // `decide` の case（runner が run 1 つの pass に組み直す）。
    let mut cases = String::new();
    for n in 0..CASE_ROWS {
        let input = digest_input("c", n);
        let row = json!({
            "name": format!("case/{n}"),
            "commands": digest_commands(&input),
            "input": input,
        });
        cases.push_str(&row.to_string());
        cases.push('\n');
    }
    for (rel, text) in [
        ("lane_rules/all-features.jsonl", lane),
        ("route_priority/cases.jsonl", route),
        ("execution_gate/cases.jsonl", gate),
        ("project_controller/invariants.jsonl", PC_FIXTURE.to_owned()),
        ("cos_digest/pass_traces.jsonl", traces),
        ("cos_digest/cases.jsonl", cases),
    ] {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
        fs::write(path, text).expect("write fixture");
    }
}

struct Setup {
    root: TempDir,
}

impl Setup {
    fn new(lane: &str) -> Self {
        Self::with_domains(lane, &ALL_DOMAINS)
    }

    fn with_domains(lane: &str, domains: &[&str]) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        write_bundle(&root.path().join("dist"), lane, domains);
        write_fixtures(&root.path().join("fixtures"));
        Self { root }
    }

    /// 一致する bundle の本文を `from` → `to` に書き換えた場面。
    fn patched(from: &str, to: &str) -> Self {
        let setup = Self::new(LANE_MATCH);
        let dist = setup.root.path().join("dist");
        let code = fs::read_to_string(dist.join(BUNDLE_FILE)).expect("bundle");
        assert!(code.contains(from), "{from} is not in the bundle");
        write_raw_bundle(&dist, &code.replace(from, to));
        setup
    }

    /// `cos_digest` だけを golden で回す。
    fn digest_golden(&self) -> (Output, Value) {
        self.run(&["--suite", "golden", "--domain", "cos_digest"])
    }

    fn path(&self, rel: &str) -> String {
        self.root.path().join(rel).display().to_string()
    }

    fn run(&self, extra: &[&str]) -> (Output, Value) {
        let dist = self.path("dist");
        let fixtures = self.path("fixtures");
        let mut args = vec!["run", "--bundle-dir", &dist, "--fixtures", &fixtures];
        args.extend_from_slice(extra);
        invoke(&args)
    }
}

fn invoke(args: &[&str]) -> (Output, Value) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("spawn runner");
    let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON value ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, report)
}

fn domain<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["domains"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["domain"] == name))
        .unwrap_or_else(|| panic!("no domain {name} in {report}"))
}

/// 不一致・例外・宣言外の試験の共通部分: `lane_rules` だけを golden で回し、exit 1 と数を確かめる。
fn assert_lane_failure(lane: &str, field: &str, count: u64, kind: &str) {
    let setup = Setup::new(lane);
    let (out, report) = setup.run(&["--suite", "golden", "--domain", "lane_rules"]);
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    assert_eq!(report["ok"], false);
    assert_eq!(report["complete"], false);
    let d = domain(&report, "lane_rules");
    assert_eq!(d[field], count, "{d}");
    assert_eq!(d["matched"], LANE_ROWS as u64 - count, "{d}");
    assert_eq!(d["order_dependent"], 0, "{d}");
    assert_eq!(d["ok"], false);
    let first = &report["failures"][0];
    assert_eq!(first["kind"], kind, "{first}");
    assert_eq!(first["domain"], "lane_rules");
    assert_eq!(first["pass"], 1);
    assert!(report["failures"].as_array().expect("failures").len() <= 20);
}

#[test]
fn script_host_runner_match_passes_all_suites_and_activates() {
    let setup = Setup::new(LANE_MATCH);
    let (out, report) = setup.run(&[]);
    assert_eq!(out.status.code(), Some(0), "report: {report}");
    assert_eq!(report["schema"], "celeris.script-host-runner/1");
    assert_eq!(report["ok"], true);
    assert_eq!(report["complete"], true);
    assert_eq!(report["failures"], json!([]));
    for (name, rows) in [
        ("lane_rules", LANE_ROWS),
        ("route_priority", ROUTE_ROWS),
        ("execution_gate", GATE_ROWS),
        ("project_controller", PC_FIXTURE.lines().count()),
        ("cos_digest", DIGEST_ROWS),
    ] {
        let d = domain(&report, name);
        assert_eq!(d["rows"], rows as u64, "{d}");
        assert_eq!(d["matched"], rows as u64, "{d}");
        assert_eq!(d["passes"], 2);
        assert_eq!(d["ok"], true);
    }
    let safety = report["safety"].as_array().expect("safety");
    assert!(safety.iter().any(|s| s["id"] == "manifest/domains"));
    assert!(safety.iter().any(|s| s["id"] == "lane_rules/missing_axis"));
    assert!(
        safety
            .iter()
            .any(|s| s["id"] == "execution_gate/ratio_out_of_range")
    );
    assert!(
        safety
            .iter()
            .any(|s| s["id"] == "route_priority/extra_field")
    );
    assert!(
        safety
            .iter()
            .any(|s| s["id"] == "cos_digest/list_reply_missing_runs" && s["expect"] == "exception")
    );
    assert!(
        safety
            .iter()
            .any(|s| s["id"] == "cos_digest/read_reply_denied" && s["outcome"] == "denied")
    );
    assert!(safety.iter().all(|s| s["ok"] == true), "{report}");

    let dist = setup.path("dist");
    let state = setup.path("state");
    let (out, report) = invoke(&["activate", "--bundle-dir", &dist, "--state-dir", &state]);
    assert_eq!(out.status.code(), Some(0), "report: {report}");
    assert_eq!(report["selftest"]["ok"], true);
    let probed = report["probed"].as_array().expect("probed");
    assert_eq!(probed.len(), 5);
    assert!(
        probed.iter().any(|p| p["domain"] == "cos_digest"),
        "{report}"
    );
    assert!(probed.iter().all(|p| p["outcome"] == "valid"), "{report}");
    assert!(Path::new(report["deployed_dir"].as_str().expect("dir")).is_dir());
}

#[test]
fn script_host_runner_mismatch_fails() {
    // judgment=high かつ ambiguity=high の 3^7 行で proposed が違う。
    assert_lane_failure(
        r#"(f.judgment === "high" && f.ambiguity === "high") ? { ...base, proposed: "frontier" } : base"#,
        "mismatch",
        2_187,
        "mismatch",
    );
}

#[test]
fn script_host_runner_exception_fails() {
    // 全軸 high の 1 行だけで throw する。
    assert_lane_failure(
        r#"AXES.every((a) => f[a] === "high") ? (() => { throw new Error("boom"); })() : base"#,
        "exception",
        1,
        "exception",
    );
}

#[test]
fn script_host_runner_undeclared_field_fails() {
    // judgment=medium の 3^8 行で比べる欄に無い欄を足す。
    assert_lane_failure(
        r#"f.judgment === "medium" ? { ...base, extra: 1 } : base"#,
        "undeclared_field",
        6_561,
        "undeclared_field",
    );
}

#[test]
fn script_host_runner_rejects_ungraded_domain_set_and_permissive_input() {
    // 採点の表の領域が manifest に無い bundle は呼ぶ前に拒否する（exit 3）。
    let setup = Setup::with_domains(LANE_MATCH, &["host_selftest", "lane_rules"]);
    let (out, report) = setup.run(&["--suite", "safety"]);
    assert_eq!(out.status.code(), Some(3), "report: {report}");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|e| e.contains("route_priority")),
        "{report}"
    );

    // 壊した入力を黙って受ける TS（入力を検査しない）は安全シナリオで落ちる（exit 1）。
    let setup = Setup::new(LANE_MATCH);
    let dist = setup.root.path().join("dist");
    let code = fs::read_to_string(dist.join(BUNDLE_FILE))
        .expect("bundle")
        .replace(
            "case \"lane_rules\": return lane(input);",
            "case \"lane_rules\": return { rule_id: \"standard/default\", proposed: \"standard\", condition: \"default\" };",
        );
    write_raw_bundle(&dist, &code);
    let (out, report) = setup.run(&["--suite", "safety"]);
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let missing_axis = report["safety"]
        .as_array()
        .and_then(|s| s.iter().find(|r| r["id"] == "lane_rules/missing_axis"))
        .expect("missing_axis row");
    assert_eq!(missing_axis["outcome"], "returned_output");
    assert_eq!(missing_axis["ok"], false);
}

#[test]
fn script_host_runner_rejects_bundle_without_cos_digest() {
    let setup = Setup::with_domains(
        LANE_MATCH,
        &["host_selftest", "lane_rules", "route_priority"],
    );
    let (out, report) = setup.run(&["--suite", "safety"]);
    assert_eq!(out.status.code(), Some(3), "report: {report}");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|e| e.contains("cos_digest")),
        "{report}"
    );
}

#[test]
fn script_host_runner_cos_digest_rule_change_is_mismatch() {
    // stopped の run の requeue を落とす。trace は全部の pass（各 pass に stopped が 1 つ）、case は半分の行が違う。
    let setup = Setup::patched(
        r#"r.run_state === "stopped" ? [post, { kind: "requeue_run", run_id: r.run_id }] : [post]"#,
        "[post]",
    );
    let (out, report) = setup.digest_golden();
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let d = domain(&report, "cos_digest");
    let expected = (TRACE_ROWS + CASE_ROWS / 2) as u64;
    assert_eq!(d["mismatch"], expected, "{d}");
    assert_eq!(d["matched"], DIGEST_ROWS as u64 - expected, "{d}");
    assert_eq!(d["ok"], false);
    let first = &report["failures"][0];
    assert_eq!(first["kind"], "mismatch", "{first}");
    assert_eq!(first["file"], "cos_digest/pass_traces.jsonl", "{first}");
}

#[test]
fn script_host_runner_cos_digest_one_byte_change_is_mismatch() {
    // 本文の末尾の 1 byte の違いも不一致。全行が違う。
    let setup = Setup::patched(
        r#"text: "state " + r.run_state,"#,
        r#"text: "state " + r.run_state + " ","#,
    );
    let (out, report) = setup.digest_golden();
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let d = domain(&report, "cos_digest");
    assert_eq!(d["mismatch"], DIGEST_ROWS as u64, "{d}");
    assert_eq!(d["matched"], 0, "{d}");
}

#[test]
fn script_host_runner_cos_digest_unrecorded_op_is_denied() {
    // 記録に無い read_run を呼んで denied を握りつぶす。出力は正しいままでも不一致。
    let setup = Setup::patched(
        r#"const runs = [];"#,
        r#"const runs = []; await cosDigestReadRun(ctx, { thread: "x", run_id: "y" }).catch(() => null);"#,
    );
    let (out, report) = setup.digest_golden();
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let d = domain(&report, "cos_digest");
    assert_eq!(d["denied"], DIGEST_ROWS as u64, "{d}");
    assert_eq!(d["matched"], 0, "{d}");
    let first = &report["failures"][0];
    assert_eq!(first["kind"], "denied", "{first}");
    assert!(
        first["error"]
            .as_str()
            .is_some_and(|e| e.contains("cos_digest:read_run")),
        "{first}"
    );
}

#[test]
fn script_host_runner_cos_digest_unused_recorded_op_is_mismatch() {
    // 記録の 1 行に、TS が呼ばない read_run を足す。出力が一致しても不一致。
    let setup = Setup::new(LANE_MATCH);
    let path = setup
        .root
        .path()
        .join("fixtures/cos_digest/pass_traces.jsonl");
    let text = fs::read_to_string(&path).expect("traces");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut row: Value = serde_json::from_str(&lines[0]).expect("row");
    row["ops"].as_array_mut().expect("ops").push(json!({
        "op": "cos_digest:read_run",
        "request": { "thread": "never", "run_id": "called" },
        "reply": { "kind": "error", "error": { "code": "exception", "message": "unused" } },
    }));
    lines[0] = row.to_string();
    fs::write(&path, lines.join("\n") + "\n").expect("write traces");
    let (out, report) = setup.digest_golden();
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let d = domain(&report, "cos_digest");
    assert_eq!(d["mismatch"], 1, "{d}");
    assert_eq!(d["matched"], DIGEST_ROWS as u64 - 1, "{d}");
}

#[test]
fn script_host_runner_cos_digest_checks_each_fixture_file() {
    // 合計の行数が足りていても、file ごとの最少行数を割れば落ちる。
    let setup = Setup::new(LANE_MATCH);
    let path = setup
        .root
        .path()
        .join("fixtures/cos_digest/pass_traces.jsonl");
    let text = fs::read_to_string(&path).expect("traces");
    let kept: Vec<&str> = text.lines().skip(1).collect();
    fs::write(&path, kept.join("\n") + "\n").expect("write traces");
    let cases = setup.root.path().join("fixtures/cos_digest/cases.jsonl");
    let mut more = fs::read_to_string(&cases).expect("cases");
    let first = more.lines().next().expect("line").to_owned();
    more.push_str(&first);
    more.push('\n');
    fs::write(&cases, more).expect("write cases");
    let (out, report) = setup.digest_golden();
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let failures = report["failures"].as_array().expect("failures");
    assert!(
        failures
            .iter()
            .any(|f| f["kind"] == "too_few_rows" && f["file"] == "cos_digest/pass_traces.jsonl"),
        "{report}"
    );
}

#[test]
fn script_host_runner_cos_digest_broken_op_reply_must_be_rejected() {
    // 一覧の答えを検査しない TS は、Rust の DTO が通す壊れた応答（他の run の答え）を受ける。
    let setup = Setup::patched(
        r#"if (r.thread !== ref.thread || r.run_id !== ref.run_id) throw new Error("read_run answered another run");"#,
        "",
    );
    let (out, report) = setup.run(&["--suite", "safety", "--domain", "cos_digest"]);
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let row = report["safety"]
        .as_array()
        .and_then(|s| {
            s.iter()
                .find(|r| r["id"] == "cos_digest/read_reply_other_run")
        })
        .expect("read_reply_other_run row");
    assert_eq!(row["outcome"], "returned_output");
    assert_eq!(row["ok"], false);
    // denied を飲み込む TS も落ちる。
    let setup = Setup::patched(
        r#"if (e?.name === "HostError" && e.code === "exception") continue;"#,
        r#"if (e?.name === "HostError") continue;"#,
    );
    let (out, report) = setup.run(&["--suite", "safety", "--domain", "cos_digest"]);
    assert_eq!(out.status.code(), Some(1), "report: {report}");
    let row = report["safety"]
        .as_array()
        .and_then(|s| s.iter().find(|r| r["id"] == "cos_digest/read_reply_denied"))
        .expect("read_reply_denied row");
    assert_eq!(row["outcome"], "returned_output");
}

#[test]
fn domains_lists_cos_digest_with_rows_and_broken_scenes() {
    let (out, report) = invoke(&["domains"]);
    assert_eq!(out.status.code(), Some(0), "report: {report}");
    let d = domain(&report, "cos_digest");
    assert!(d["min_rows"].as_u64().expect("min_rows") >= 2086, "{d}");
    let broken = d["broken_inputs"].as_array().expect("broken_inputs");
    assert!(
        broken.iter().any(|id| id == "read_reply_denied"),
        "{broken:?}"
    );
}

/// bundle の本文を差し替え、manifest の hash を合わせる。
fn write_raw_bundle(dir: &Path, code: &str) {
    fs::write(dir.join(BUNDLE_FILE), code).expect("write bundle");
    let path = dir.join(MANIFEST_FILE);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("parse manifest");
    manifest["bundle_sha256"] = json!(sha256_hex(code.as_bytes()));
    fs::write(&path, serde_json::to_vec(&manifest).expect("json")).expect("write manifest");
}
