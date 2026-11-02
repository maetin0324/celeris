//! `script-host-runner` の試験（ADR 2026-11-02 ts-fast-verify (3)）。
//!
//! 業務 TS と `control-ts/dist`・`control-ts/fixtures` は使わない。試験の中で小さな手書きの bundle と、
//! 採点の表の形に合う fixture（`lane_rules` は 9 軸 × 3 値の全組み合わせ、`route_priority` は最少行数ちょうど）を
//! tempdir に書き、一致・不一致・例外・宣言外の 4 場合と bundle の拒否を固定する。

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

/// 試験用の bundle。入力の形を厳しく検査して throw し、`lane_rules` は `__LANE__` の式の値を返す。
const BUNDLE_TEMPLATE: &str = r#"
import { hostVersion, selftestEcho } from "celeris:host";
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
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  switch (domain) {
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
        "capabilities": ["selftest:echo_task"],
        "checkpoint_versions": [],
    });
    fs::write(
        dir.join(MANIFEST_FILE),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
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
    for (rel, text) in [
        ("lane_rules/all-features.jsonl", lane),
        ("route_priority/cases.jsonl", route),
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
        Self::with_domains(lane, &["host_selftest", "lane_rules", "route_priority"])
    }

    fn with_domains(lane: &str, domains: &[&str]) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        write_bundle(&root.path().join("dist"), lane, domains);
        write_fixtures(&root.path().join("fixtures"));
        Self { root }
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
    for (name, rows) in [("lane_rules", LANE_ROWS), ("route_priority", ROUTE_ROWS)] {
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
            .any(|s| s["id"] == "route_priority/extra_field")
    );
    assert!(safety.iter().all(|s| s["ok"] == true), "{report}");

    let dist = setup.path("dist");
    let state = setup.path("state");
    let (out, report) = invoke(&["activate", "--bundle-dir", &dist, "--state-dir", &state]);
    assert_eq!(out.status.code(), Some(0), "report: {report}");
    assert_eq!(report["selftest"]["ok"], true);
    let probed = report["probed"].as_array().expect("probed");
    assert_eq!(probed.len(), 2);
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

/// bundle の本文を差し替え、manifest の hash を合わせる。
fn write_raw_bundle(dir: &Path, code: &str) {
    fs::write(dir.join(BUNDLE_FILE), code).expect("write bundle");
    let path = dir.join(MANIFEST_FILE);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).expect("manifest")).expect("parse manifest");
    manifest["bundle_sha256"] = json!(sha256_hex(code.as_bytes()));
    fs::write(&path, serde_json::to_vec(&manifest).expect("json")).expect("write manifest");
}
