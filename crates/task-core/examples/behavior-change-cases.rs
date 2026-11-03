//! P2-G（ADR 2026-11-03-llm-self-improve-trial）: 課題の期待差分 fixture を作り、旧 Rust 経路の候補を採点する。
//!
//! `cargo run -q -p task-core --example behavior-change-cases -- gen --spec <spec.json> --inputs <inputs.jsonl> --out <cases.jsonl>`
//!   入力（1 行 `{name, input}`）ごとに Rust の参照実装の出力（`rust`）と、仕様の編集を当てた期待出力（`expected`）を
//!   ADR 2026-11-03-ts-intended-behavior-change D3 の形で書き、仕様の `cases_sha256` を file の sha256 に書き換える。
//!   期待差分 fixture の作り方は task-core の試験（`script_behavior_change_tests.rs` の `generate_cases`）と同じ。
//!
//! `cargo run -q -p task-core --example behavior-change-cases -- score --spec <spec.json> --cases <cases.jsonl> --fixtures <dir>`
//!   この build の `cos_digest_decide`（旧 Rust 経路の候補）を、旧挙動の fixture（`<dir>/cases.jsonl`）と期待差分 fixture で
//!   数え、`scripts/dev/ts-behavior-eval.sh` と同じ欄の JSON を 1 行出す。合否も同じ（旧 fixture の差分 0・食い違い 0）。
//!   採点の file は候補の作業ツリーの外（課題の kit）から渡す。候補が fixture を書き換えても採点は変わらない。
//! exit: 0 = ok、1 = 採点に落ちた、2 = 使い方の誤り・読めない file。
use serde_json::Value;
use std::path::{Path, PathBuf};
use task_core::chat::digest::{CosDigestInput, cos_digest_decide};
use task_core::script_behavior_change::{
    BehaviorChangeCase, BehaviorChangeExpect, BehaviorChangeSpec,
};

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    task_core::script_compare::sha256_digest(bytes)
        .trim_start_matches("sha256:")
        .to_owned()
}

fn rust_of(input: &Value) -> Result<Value, String> {
    let dto: CosDigestInput =
        serde_json::from_value(input.clone()).map_err(|e| format!("input DTO: {e}"))?;
    serde_json::to_value(cos_digest_decide(&dto)).map_err(|e| format!("output: {e}"))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_spec(path: &Path) -> Result<BehaviorChangeSpec, String> {
    let spec: BehaviorChangeSpec =
        serde_json::from_str(&read(path)?).map_err(|e| format!("spec: {e}"))?;
    spec.validate().map_err(|e| format!("spec: {}", e.0))?;
    Ok(spec)
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim().is_empty())
}

fn gen_cases(spec_path: &Path, inputs: &Path, out: &Path) -> Result<(), String> {
    let spec = load_spec(spec_path)?;
    let mut text = String::new();
    let (mut explained, mut equal) = (0usize, 0usize);
    for l in lines(&read(inputs)?) {
        let row: Value = serde_json::from_str(l).map_err(|e| format!("inputs: {e}"))?;
        let name = row["name"].as_str().ok_or("inputs: name")?.to_owned();
        let input = row["input"].clone();
        let rust = rust_of(&input)?;
        let (expected, expect) = match spec
            .applies_to(&input)
            .then(|| spec.expected_output(&input, &rust))
        {
            Some(Some(e)) => {
                explained += 1;
                (e, BehaviorChangeExpect::Explained)
            }
            _ => {
                equal += 1;
                (rust.clone(), BehaviorChangeExpect::Equal)
            }
        };
        let case = BehaviorChangeCase {
            name,
            input,
            rust,
            expected,
            expect,
        };
        let v = sorted(&serde_json::to_value(&case).map_err(|e| e.to_string())?);
        text.push_str(&serde_json::to_string(&v).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    std::fs::write(out, &text).map_err(|e| format!("{}: {e}", out.display()))?;
    let sha = sha256_hex(text.as_bytes());
    let spec_text = read(spec_path)?;
    let old = format!("\"cases_sha256\": \"{}\"", spec.cases_sha256);
    if spec_text.matches(&old).count() != 1 {
        return Err("spec: the line `\"cases_sha256\": \"…\"` must appear once".into());
    }
    let new = format!("\"cases_sha256\": \"{sha}\"");
    std::fs::write(spec_path, spec_text.replace(&old, &new))
        .map_err(|e| format!("{}: {e}", spec_path.display()))?;
    let spec = load_spec(spec_path)?;
    println!(
        "{}",
        serde_json::json!({"cases": explained + equal, "explained": explained, "equal": equal,
            "cases_sha256": sha, "spec_sha256": spec.sha256()})
    );
    Ok(())
}

fn score(spec_path: &Path, cases: &Path, fixtures: &Path) -> Result<bool, String> {
    let spec = load_spec(spec_path)?;
    let (mut old_cases, mut old_diffs) = (0usize, 0usize);
    let mut old_examples: Vec<String> = Vec::new();
    for l in lines(&read(&fixtures.join("cases.jsonl"))?) {
        let row: Value = serde_json::from_str(l).map_err(|e| format!("fixture: {e}"))?;
        old_cases += 1;
        let got = rust_of(&row["input"]).unwrap_or_else(|e| serde_json::json!({ "error": e }));
        if serde_json::to_vec(&got).ok() != serde_json::to_vec(&row["commands"]).ok() {
            old_diffs += 1;
            if old_examples.len() < 3 {
                old_examples.push(row["name"].as_str().unwrap_or("?").to_owned());
            }
        }
    }
    let cases_bytes = std::fs::read(cases).map_err(|e| format!("{}: {e}", cases.display()))?;
    let cases_sha256_ok = sha256_hex(&cases_bytes) == spec.cases_sha256;
    let (mut n, mut explained_ok, mut equal_ok, mut wrong) = (0usize, 0usize, 0usize, 0usize);
    let mut wrong_examples: Vec<Value> = Vec::new();
    for l in lines(std::str::from_utf8(&cases_bytes).map_err(|e| e.to_string())?) {
        let c: BehaviorChangeCase = serde_json::from_str(l).map_err(|e| format!("case: {e}"))?;
        n += 1;
        let got = rust_of(&c.input).map(|v| sorted(&v));
        let ok = got
            .as_ref()
            .is_ok_and(|g| serde_json::to_vec(g).ok() == serde_json::to_vec(&c.expected).ok());
        match (ok, c.expect) {
            (true, BehaviorChangeExpect::Explained) => explained_ok += 1,
            (true, BehaviorChangeExpect::Equal) => equal_ok += 1,
            _ => {
                wrong += 1;
                if wrong_examples.len() < 3 {
                    wrong_examples.push(serde_json::json!({"name": c.name, "expect": c.expect}));
                }
            }
        }
    }
    let ok = old_diffs == 0 && cases_sha256_ok && wrong == 0 && n > 0;
    println!(
        "{}",
        serde_json::json!({
            "old_fixture_cases": old_cases, "old_fixture_diffs": old_diffs,
            "old_fixture_diff_examples": old_examples,
            "specs": [{"id": spec.id, "version": spec.version, "cases_sha256_ok": cases_sha256_ok,
                "cases": n, "explained_ok": explained_ok, "equal_ok": equal_ok, "wrong": wrong,
                "wrong_examples": wrong_examples}],
            "ok": ok,
        })
    );
    Ok(ok)
}

fn arg(args: &[String], name: &str) -> Result<PathBuf, String> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| PathBuf::from(&w[1]))
        .ok_or_else(|| format!("missing {name}"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gen") => (|| {
            gen_cases(
                &arg(&args, "--spec")?,
                &arg(&args, "--inputs")?,
                &arg(&args, "--out")?,
            )
            .map(|()| true)
        })(),
        Some("score") => (|| {
            score(
                &arg(&args, "--spec")?,
                &arg(&args, "--cases")?,
                &arg(&args, "--fixtures")?,
            )
        })(),
        _ => Err("usage: behavior-change-cases gen|score …".into()),
    };
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("behavior-change-cases: {e}");
            std::process::exit(2);
        }
    }
}
