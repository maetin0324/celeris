//! S2: commit 済みの lane 規則 fixture（control-ts/fixtures/lane_rules/all-features.jsonl）が
//! `match_rule` の現在の出力と一致することを確かめる。食い違ったら
//! `cargo run -q -p task-core --example dump-lane-fixtures > control-ts/fixtures/lane_rules/all-features.jsonl`
//! で作り直し、規則表の変更として扱う。

use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/fixtures/lane_rules/all-features.jsonl")
}

#[test]
fn script_fixtures_lane_rules_match_committed_fixture() {
    let generated = task_core::model_policy::lane_fixture_jsonl().expect("serialize fixture");
    let committed = std::fs::read_to_string(fixture_path()).expect("read committed fixture");
    assert_eq!(generated.lines().count(), 19_683);
    assert_eq!(committed.lines().count(), 19_683);
    assert!(
        generated == committed,
        "lane_rules fixture differs from match_rule output; regenerate with the dump-lane-fixtures example"
    );
}

#[test]
fn script_fixtures_lane_rules_cover_all_combinations_in_order() {
    let all = task_core::model_policy::all_features();
    assert_eq!(all.len(), 19_683);
    let keys: Vec<String> = all
        .iter()
        .map(|f| serde_json::to_string(f).expect("serialize features"))
        .collect();
    let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
    assert_eq!(unique.len(), 19_683);
    let first = &all[0];
    let last = &all[all.len() - 1];
    assert!(
        serde_json::to_string(first)
            .expect("ser")
            .matches("\"low\"")
            .count()
            == 9
    );
    assert!(
        serde_json::to_string(last)
            .expect("ser")
            .matches("\"high\"")
            .count()
            == 9
    );
    // 最下位の軸（cross_cutting）が最も速く回る。
    assert_eq!(all[1].cross_cutting, task_core::Level::Medium);
    assert_eq!(all[1].judgment, task_core::Level::Low);
    assert_eq!(all[3usize.pow(8)].judgment, task_core::Level::Medium);
}

// --- route_priority（S2 の 2 つ目の領域）---------------------------------------------------
// commit 済みの control-ts/fixtures/route_priority/cases.jsonl が `route_priority_decide` の
// 現在の出力と一致することを確かめる。食い違ったら
// `cargo run -q -p task-core --example dump-route-priority-fixtures > control-ts/fixtures/route_priority/cases.jsonl`
// で作り直し、route_priority の変更として扱う。

use std::collections::BTreeSet;
use task_core::llm_route::{
    CandidateKey, ExecutionClass, HarnessId, ModelId, Registry, SourceId, route_priority,
    route_priority_decide,
};
use task_core::script_compare::{
    CompareCandidate, RoutePriorityBasis, RoutePriorityInput, RoutePriorityOutput,
    route_priority_fixture_jsonl,
};

const ROUTE_PRIORITY_GENERATED: usize = 2000;
const ROUTE_PRIORITY_HAND_MIN: usize = 16;

fn route_priority_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/fixtures/route_priority/cases.jsonl")
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutePriorityLine {
    case: String,
    input: RoutePriorityInput,
    basis: RoutePriorityBasis,
    candidates: Vec<CompareCandidate>,
}

fn route_priority_lines() -> Vec<RoutePriorityLine> {
    let body = route_priority_fixture_jsonl().expect("serialize fixture");
    body.lines()
        .map(|l| serde_json::from_str(l).expect("fixture line parses as the DTO"))
        .collect()
}

fn to_keys(list: &[CompareCandidate]) -> Vec<CandidateKey> {
    list.iter()
        .map(|c| CandidateKey {
            source: SourceId::from(c.source.as_str()),
            model: ModelId::from(c.model.as_str()),
        })
        .collect()
}

/// 入力 DTO（1 harness の行だけ）から registry を組み直す。
fn registry_from_input(input: &RoutePriorityInput) -> Registry {
    let harness = HarnessId::from(input.harness.as_str());
    let mut r = Registry {
        cos_route_candidates: to_keys(&input.table.cos_route_candidates),
        ..Registry::default()
    };
    // route_defaults が空配列のときは「宣言なし」と「空の宣言」が区別できない。どちらも basis は
    // none・候補は空になる（付記 S2-10）ので、空なら宣言しない側で組み直す。
    if !input.table.route_defaults.is_empty() {
        r.route_defaults
            .insert(harness.clone(), to_keys(&input.table.route_defaults));
    }
    for lane in &input.table.lane_route_defaults {
        r.lane_route_defaults.insert(
            (harness.clone(), lane.lane.clone()),
            to_keys(&lane.candidates),
        );
    }
    for class in &input.table.class_defaults {
        r.route_class_defaults
            .entry(harness.clone())
            .or_default()
            .insert(
                ExecutionClass::from(class.execution_class.as_str()),
                to_keys(&class.candidates),
            );
    }
    r
}

#[test]
fn script_fixtures_route_priority_match_committed_fixture() {
    let generated = route_priority_fixture_jsonl().expect("serialize fixture");
    let committed =
        std::fs::read_to_string(route_priority_fixture_path()).expect("read committed fixture");
    let min = ROUTE_PRIORITY_GENERATED + ROUTE_PRIORITY_HAND_MIN;
    assert!(generated.lines().count() >= min);
    assert!(committed.lines().count() >= min);
    assert!(
        generated == committed,
        "route_priority fixture differs from route_priority_decide output; regenerate with the dump-route-priority-fixtures example"
    );
}

#[test]
fn script_fixtures_route_priority_counts_and_all_bases() {
    let lines = route_priority_lines();
    let hand: Vec<_> = lines
        .iter()
        .filter(|l| l.case.starts_with("hand/"))
        .collect();
    let generated: Vec<_> = lines
        .iter()
        .filter(|l| l.case.starts_with("gen/"))
        .collect();
    assert!(
        hand.len() >= ROUTE_PRIORITY_HAND_MIN,
        "hand cases: {}",
        hand.len()
    );
    assert_eq!(generated.len(), ROUTE_PRIORITY_GENERATED);
    assert_eq!(hand.len() + generated.len(), lines.len());
    let all = [
        RoutePriorityBasis::Cos,
        RoutePriorityBasis::ExecutionClass,
        RoutePriorityBasis::Lane,
        RoutePriorityBasis::Harness,
        RoutePriorityBasis::None,
    ];
    for basis in all {
        assert!(
            hand.iter().any(|l| l.basis == basis),
            "hand cases lack basis {basis:?}"
        );
        assert!(
            generated.iter().any(|l| l.basis == basis),
            "generated cases lack basis {basis:?}"
        );
    }
}

#[test]
fn script_fixtures_route_priority_has_no_duplicates() {
    let body = route_priority_fixture_jsonl().expect("serialize fixture");
    let rows: BTreeSet<&str> = body.lines().collect();
    assert_eq!(rows.len(), body.lines().count(), "duplicate lines");
    let lines = route_priority_lines();
    let cases: BTreeSet<&str> = lines.iter().map(|l| l.case.as_str()).collect();
    assert_eq!(cases.len(), lines.len(), "duplicate case names");
    let inputs: BTreeSet<String> = lines
        .iter()
        .map(|l| serde_json::to_string(&l.input).expect("serialize input"))
        .collect();
    assert_eq!(inputs.len(), lines.len(), "duplicate inputs");
}

/// 期待値は入力だけで決まる: 入力から組み直した registry で `route_priority_decide` と包みの
/// `route_priority` を呼んでも、各行と同じ basis・候補列になる。
#[test]
fn script_fixtures_route_priority_input_alone_determines_output() {
    for line in route_priority_lines() {
        let registry = registry_from_input(&line.input);
        let harness = HarnessId::from(line.input.harness.as_str());
        let class = line
            .input
            .execution_class
            .as_deref()
            .map(ExecutionClass::from);
        let decided = route_priority_decide(
            &registry,
            &harness,
            line.input.lane.as_deref(),
            line.input.cos,
            class.as_ref(),
        );
        let output = RoutePriorityOutput::from_decision(decided);
        assert_eq!(output.basis, line.basis, "case {}", line.case);
        assert_eq!(output.candidates, line.candidates, "case {}", line.case);
        let wrapped = route_priority(
            &registry,
            &harness,
            line.input.lane.as_deref(),
            line.input.cos,
            class.as_ref(),
        );
        assert_eq!(wrapped, decided.1, "case {}", line.case);
        if line.candidates.is_empty() {
            assert_eq!(line.basis, RoutePriorityBasis::None, "case {}", line.case);
        } else {
            assert_ne!(line.basis, RoutePriorityBasis::None, "case {}", line.case);
        }
    }
}

/// 手書き例の basis を、関数の出力とは別に固定する（付記 S2-10 の規則）。
#[test]
fn script_fixtures_route_priority_hand_cases_follow_s2_10() {
    use RoutePriorityBasis as B;
    let expected: &[(&str, RoutePriorityBasis, usize)] = &[
        ("hand/class-declared", B::ExecutionClass, 2),
        ("hand/class-unknown-falls-to-lane", B::Lane, 2),
        ("hand/class-unknown-falls-to-harness", B::Harness, 2),
        ("hand/class-empty-row-is-none", B::None, 0),
        ("hand/class-declared-only-for-other-harness", B::Lane, 1),
        ("hand/lane-declared", B::Lane, 1),
        ("hand/lane-unknown-falls-to-harness", B::Harness, 2),
        ("hand/lane-case-differs-falls-to-harness", B::Harness, 2),
        ("hand/lane-empty-row-is-none", B::None, 0),
        ("hand/lane-empty-string-row", B::Lane, 1),
        ("hand/lane-null-takes-empty-string-row", B::Lane, 1),
        ("hand/lane-empty-string-without-row", B::Harness, 2),
        ("hand/lane-null-without-empty-string-row", B::Harness, 2),
        ("hand/harness-without-rows", B::None, 0),
        ("hand/harness-defaults-empty-is-none", B::None, 0),
        ("hand/lane-row-without-harness-defaults", B::None, 0),
        ("hand/cos-candidates", B::Cos, 1),
        ("hand/cos-empty-is-none", B::None, 0),
        ("hand/cos-with-class", B::Cos, 1),
        ("hand/duplicate-candidates", B::Harness, 4),
        ("hand/non-ascii-ids", B::Lane, 2),
        ("hand/json-escapes", B::Harness, 2),
        ("hand/object-prototype-names-lane", B::Lane, 1),
        ("hand/object-prototype-names-class", B::ExecutionClass, 1),
        ("hand/all-empty", B::None, 0),
        ("hand/all-empty-cos", B::None, 0),
    ];
    let lines = route_priority_lines();
    let hand: Vec<_> = lines
        .iter()
        .filter(|l| l.case.starts_with("hand/"))
        .collect();
    assert_eq!(hand.len(), expected.len());
    for ((name, basis, len), line) in expected.iter().zip(hand) {
        assert_eq!(line.case, *name);
        assert_eq!(line.basis, *basis, "case {name}");
        assert_eq!(line.candidates.len(), *len, "case {name}");
    }
    let by_name = |n: &str| {
        lines
            .iter()
            .find(|l| l.case == n)
            .expect("hand case present")
    };
    // lane が null と空文字の組: null は空文字の行を引く。
    assert_eq!(
        by_name("hand/lane-null-takes-empty-string-row").input.lane,
        None
    );
    assert_eq!(
        by_name("hand/lane-empty-string-row").input.lane.as_deref(),
        Some("")
    );
    // 重複は除かない。
    let dup = &by_name("hand/duplicate-candidates").candidates;
    assert_eq!(dup[0], dup[1]);
    assert_eq!(dup[0], dup[3]);
    // 別の harness の行は入力に入らない（redacted）。
    let line = serde_json::to_string(&by_name("hand/lane-declared").input).expect("ser");
    assert!(!line.contains("gpt-codex") && !line.contains("codex"));
}

#[test]
fn script_fixtures_route_priority_output_dto_shape() {
    let out = RoutePriorityOutput {
        basis: RoutePriorityBasis::ExecutionClass,
        candidates: vec![CompareCandidate {
            source: "s".into(),
            model: "m".into(),
        }],
    };
    assert_eq!(
        serde_json::to_string(&out).expect("ser"),
        r#"{"basis":"execution_class","candidates":[{"source":"s","model":"m"}]}"#
    );
    for (basis, name) in [
        (RoutePriorityBasis::Cos, "cos"),
        (RoutePriorityBasis::ExecutionClass, "execution_class"),
        (RoutePriorityBasis::Lane, "lane"),
        (RoutePriorityBasis::Harness, "harness"),
        (RoutePriorityBasis::None, "none"),
    ] {
        assert_eq!(
            serde_json::to_string(&basis).expect("ser"),
            format!("\"{name}\"")
        );
    }
    assert!(
        serde_json::from_str::<RoutePriorityOutput>(r#"{"basis":"none","candidates":[],"x":1}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<RoutePriorityOutput>(r#"{"basis":"None","candidates":[]}"#).is_err()
    );
}
