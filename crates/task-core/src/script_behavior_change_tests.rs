//! ADR 2026-11-03-ts-intended-behavior-change D1・D3 の試験。
//!
//! `UPDATE_BEHAVIOR_CASES=1 cargo nextest run -p task-core -E 'test(/behavior_change_/)'` で期待差分 fixture を
//! Rust の参照実装と仕様から作り直し、仕様の `cases_sha256` を書き換える（golden の更新規約。D3）。

use super::*;
use crate::chat::digest::{CosDigestInput, cos_digest_decide};
use serde_json::json;
use std::path::PathBuf;

const SPEC: &str = "cos_digest/cos-digest-cap-run-reason.v1.json";
const CASES: &str = "cos_digest/cos-digest-cap-run-reason.v1.cases.jsonl";

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../control-ts/behavior-changes")
}

fn load_spec() -> BehaviorChangeSpec {
    let text = std::fs::read_to_string(dir().join(SPEC)).expect("spec file");
    serde_json::from_str(&text).expect("spec parses")
}

fn rust_of(input: &Value) -> Value {
    let input: CosDigestInput = serde_json::from_value(input.clone()).expect("input DTO");
    serde_json::to_value(cos_digest_decide(&input)).expect("rust json")
}

/// 期待差分 fixture の行を作る。pilot（`ts-pilot.sh` の段 diff）と同じ 15 件: fixture の最初の failed の入力 3 件の
/// reason を "E" × 150・200・201・500・4000 に替えたもの。加えて code point の境界の 2 件と、当たらない 2 件。
fn generate_cases(spec: &BehaviorChangeSpec) -> Vec<BehaviorChangeCase> {
    let golden = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../control-ts/fixtures/cos_digest/cases.jsonl"),
    )
    .expect("golden");
    let failed: Vec<Value> = golden
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("row")["input"].clone())
        .filter(|i| i["run_state"] == "failed")
        .take(3)
        .collect();
    assert_eq!(failed.len(), 3);
    let mut inputs: Vec<(String, Value)> = Vec::new();
    for (k, base) in failed.iter().enumerate() {
        for n in [150usize, 200, 201, 500, 4000] {
            let mut i = base.clone();
            i["reason"] = json!("E".repeat(n));
            inputs.push((format!("pilot/failed-{k}/reason-{n}"), i));
        }
    }
    let mut emoji = failed[0].clone();
    emoji["reason"] = json!("😀".repeat(201));
    inputs.push(("extra/emoji-201".into(), emoji));
    let mut kana = failed[1].clone();
    kana["reason"] = json!("あ".repeat(250));
    inputs.push(("extra/kana-250".into(), kana));
    let mut completed = failed[0].clone();
    completed["run_state"] = json!("completed");
    completed["reason"] = json!("E".repeat(500));
    inputs.push(("extra/completed-long-reason".into(), completed));
    let mut null_reason = failed[2].clone();
    null_reason["reason"] = Value::Null;
    inputs.push(("extra/failed-null-reason".into(), null_reason));
    inputs
        .into_iter()
        .map(|(name, input)| {
            let rust = rust_of(&input);
            let (expected, expect) =
                match spec.applies_to(&input).then(|| spec.expected_output(&input, &rust)) {
                    Some(Some(e)) => (e, BehaviorChangeExpect::Explained),
                    _ => (rust.clone(), BehaviorChangeExpect::Equal),
                };
            BehaviorChangeCase {
                name,
                input,
                rust,
                expected,
                expect,
            }
        })
        .collect()
}

fn cases_text(cases: &[BehaviorChangeCase]) -> String {
    cases
        .iter()
        .map(|c| serde_json::to_string(c).expect("case json") + "\n")
        .collect()
}

fn file_sha(bytes: &[u8]) -> String {
    sha256_digest(bytes).trim_start_matches("sha256:").to_owned()
}

#[test]
fn behavior_change_spec_file_is_valid_and_supported() {
    let spec = load_spec();
    spec.validate().expect("valid");
    assert!(domain_supported(spec.domain));
    assert_eq!(spec.version, 1);
    assert!(spec.supersedes.is_none());
    assert_eq!(spec.sha256().len(), 64);
}

#[test]
fn behavior_change_cases_match_rust_reference_and_spec() {
    let spec = load_spec();
    let generated = cases_text(&generate_cases(&spec));
    let path = dir().join(CASES);
    if std::env::var_os("UPDATE_BEHAVIOR_CASES").is_some() {
        std::fs::write(&path, &generated).expect("write cases");
        let sha = file_sha(generated.as_bytes());
        let spec_path = dir().join(SPEC);
        let text = std::fs::read_to_string(&spec_path).expect("spec");
        let old = format!("\"cases_sha256\": \"{}\"", spec.cases_sha256);
        let new = format!("\"cases_sha256\": \"{sha}\"");
        std::fs::write(&spec_path, text.replace(&old, &new)).expect("write spec");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("cases file");
    assert_eq!(
        committed, generated,
        "the expected-diff fixture is not generated from Rust's reference and the spec (UPDATE_BEHAVIOR_CASES=1)"
    );
    assert_eq!(
        file_sha(committed.as_bytes()),
        spec.cases_sha256,
        "cases_sha256"
    );
    let cases: Vec<BehaviorChangeCase> = committed
        .lines()
        .map(|l| serde_json::from_str(l).expect("case"))
        .collect();
    let mut pilot_explained = 0;
    for c in &cases {
        assert_eq!(c.rust, rust_of(&c.input), "{}: rust", c.name);
        match c.expect {
            BehaviorChangeExpect::Explained => {
                assert!(spec.explains(&c.input, &c.rust, &c.expected), "{}", c.name);
                assert_ne!(c.rust, c.expected, "{}", c.name);
                if c.name.starts_with("pilot/") {
                    pilot_explained += 1;
                }
            }
            BehaviorChangeExpect::Equal => {
                assert_eq!(c.rust, c.expected, "{}", c.name);
                assert!(!spec.explains(&c.input, &c.rust, &c.expected), "{}", c.name);
            }
        }
    }
    // S6 pilot の「差分 9 件」（201・500・4000 文字 × 3 入力）。
    assert_eq!(pilot_explained, 9);
    assert_eq!(cases.iter().filter(|c| c.name.starts_with("pilot/")).count(), 15);
}

/// 仕様と独立に、期待出力の見出しが 200 code point と「…」であることを手で確かめる。
#[test]
fn behavior_change_expected_text_is_capped_at_200_code_points() {
    let spec = load_spec();
    let case = generate_cases(&spec)
        .into_iter()
        .find(|c| c.name == "extra/emoji-201")
        .expect("case");
    let text = case.expected[0]["text"].as_str().expect("text");
    let want = format!("。run は失敗: {}…)", "😀".repeat(200));
    assert!(text.contains(&want), "{text}");
    assert!(!text.contains(&"😀".repeat(201)));
}

fn long_failed() -> (Value, Value) {
    let spec = load_spec();
    let case = generate_cases(&spec)
        .into_iter()
        .find(|c| c.name == "pilot/failed-0/reason-500")
        .expect("case");
    (case.input, case.rust)
}

#[test]
fn behavior_change_unapproved_or_wrong_deltas_are_not_explained() {
    let spec = load_spec();
    let (input, rust) = long_failed();
    let reason = "E".repeat(500);
    let text = rust[0]["text"].as_str().expect("text").to_owned();
    let with = |replacement: String| {
        let mut ts = rust.clone();
        ts[0]["text"] = json!(text.replacen(&reason, &replacement, 1));
        ts
    };
    // 199 文字で切る TS は仕様と違う。
    assert!(!spec.explains(&input, &rust, &with(format!("{}…", "E".repeat(199)))));
    // 「…」の無い 200 文字も違う。
    assert!(!spec.explains(&input, &rust, &with("E".repeat(200))));
    // 正しい切り方に別の差分が足されたら説明しない。
    let mut extra = with(format!("{}…", "E".repeat(200)));
    extra[0]["thread"] = json!("other-thread");
    assert!(!spec.explains(&input, &rust, &extra));
    // 正しい切り方だけなら説明する。
    let good = with(format!("{}…", "E".repeat(200)));
    assert!(spec.explains(&input, &rust, &good));
    // 仕様が承認されていなければ説明しない（承認済みの集合が空）。
    assert_eq!(
        explain(&[], ScriptDomain::CosDigest, &input, &rust, &good),
        None
    );
    let approved = [ApprovedBehaviorChange {
        spec_sha256: spec.sha256(),
        spec: spec.clone(),
    }];
    assert_eq!(
        explain(&approved, ScriptDomain::CosDigest, &input, &rust, &good),
        Some(ExplainedBy {
            id: spec.id.clone(),
            version: 1,
            spec_sha256: spec.sha256()
        })
    );
    // 別の領域では使わない。
    assert_eq!(
        explain(&approved, ScriptDomain::LaneRules, &input, &rust, &good),
        None
    );
    // 入力条件が成り立たない（run は completed）なら説明しない。
    let mut completed = input.clone();
    completed["run_state"] = json!("completed");
    assert!(!spec.explains(&completed, &rust, &good));
}

#[test]
fn behavior_change_ambiguous_find_is_not_explained() {
    let spec = load_spec();
    let (input, mut rust) = long_failed();
    let text = rust[0]["text"].as_str().expect("text").to_owned();
    let find = format!("。run は失敗: {})", "E".repeat(500));
    rust[0]["text"] = json!(format!("{text}{find}"));
    assert_eq!(spec.expected_output(&input, &rust), None);
}

#[test]
fn behavior_change_validate_rejects_wide_or_malformed_specs() {
    let base = load_spec();
    let reject = |f: &dyn Fn(&mut BehaviorChangeSpec)| {
        let mut s = base.clone();
        f(&mut s);
        assert!(s.validate().is_err(), "{s:?}");
    };
    reject(&|s| s.schema = "x".into());
    reject(&|s| s.id = "Has Space".into());
    reject(&|s| s.version = 0);
    reject(&|s| s.summary = String::new());
    reject(&|s| s.cases_sha256 = "abc".into());
    reject(&|s| s.supersedes = Some("zz".into()));
    reject(&|s| s.when.clear());
    reject(&|s| s.when[0].path = "/*".into());
    reject(&|s| s.when[1].value = json!("x"));
    reject(&|s| s.edits.clear());
    reject(&|s| s.edits = vec![s.edits[0].clone(); 5]);
    reject(&|s| s.edits[0].replace = s.edits[0].find.clone());
    reject(&|s| s.edits[0].path = "text".into());
    reject(&|s| {
        s.edits[0].replace[1] = TextPart::TruncateChars {
            input: "/reason".into(),
            max: 0,
            ellipsis: String::new(),
        }
    });
    reject(&|s| s.edits[0].find[0] = TextPart::Lit("x".repeat(BEHAVIOR_CHANGE_LIT_MAX + 1)));
    // 未知の欄は deserialize で落ちる。
    let mut v = serde_json::to_value(&base).expect("json");
    v["normalize"] = json!(true);
    assert!(serde_json::from_value::<BehaviorChangeSpec>(v).is_err());
    assert!(!domain_supported(ScriptDomain::LaneRules));
    assert!(!domain_supported(ScriptDomain::RoutePriority));
}

#[test]
fn behavior_change_sha256_ignores_file_layout_but_binds_content() {
    let spec = load_spec();
    let compact = serde_json::to_string(&spec).expect("json");
    let reparsed: BehaviorChangeSpec = serde_json::from_str(&compact).expect("parse");
    assert_eq!(reparsed.sha256(), spec.sha256());
    let mut other = spec.clone();
    other.summary.push('x');
    assert_ne!(other.sha256(), spec.sha256());
}
