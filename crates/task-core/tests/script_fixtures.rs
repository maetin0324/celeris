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

#[derive(serde::Serialize, serde::Deserialize)]
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

// --- cos_digest（S4 の CoS digest 経路）------------------------------------------------------
// commit 済みの control-ts/fixtures/cos_digest/cases.jsonl が `cos_digest_decide` の現在の出力と
// 一致することを確かめる。食い違ったら
// `cargo run -q -p task-core --example dump-cos-digest-fixtures > control-ts/fixtures/cos_digest/cases.jsonl`
// で作り直し、digest の文面・判定の変更として扱う。

use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, CosDigestViolation, cos_digest_decide,
    validate_cos_digest_commands,
};
use task_core::chat::{COS_DIGEST_GENERATED_CASES, cos_digest_fixture_jsonl};

fn cos_digest_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/fixtures/cos_digest/cases.jsonl")
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CosDigestLine {
    name: String,
    input: CosDigestInput,
    commands: Vec<CosDigestCommand>,
}

#[test]
fn script_fixtures_cos_digest_match_committed_fixture() {
    let generated = cos_digest_fixture_jsonl().expect("serialize fixture");
    let committed =
        std::fs::read_to_string(cos_digest_fixture_path()).expect("read committed fixture");
    assert!(generated.lines().count() >= 2000);
    assert!(
        generated == committed,
        "cos_digest fixture differs from cos_digest_decide output; regenerate with the dump-cos-digest-fixtures example"
    );
}

#[test]
fn script_fixtures_cos_digest_cases_are_valid_and_cover_boundaries() {
    let body = cos_digest_fixture_jsonl().expect("serialize fixture");
    let mut names = BTreeSet::new();
    let mut inputs = BTreeSet::new();
    let mut run_states = BTreeSet::new();
    let mut item_states = BTreeSet::new();
    let mut generated = 0usize;
    for line in body.lines() {
        let case: CosDigestLine =
            serde_json::from_str(line).expect("line parses with deny_unknown_fields");
        assert!(
            names.insert(case.name.clone()),
            "duplicate name {}",
            case.name
        );
        assert!(
            inputs.insert(serde_json::to_string(&case.input).expect("ser")),
            "duplicate input {}",
            case.name
        );
        if case.name.starts_with("gen/") {
            generated += 1;
        } else {
            assert!(case.name.starts_with("hand/"), "{}", case.name);
        }
        // The reference output passes its own validation (a failure is a Rust bug).
        assert_eq!(
            cos_digest_decide(&case.input),
            case.commands,
            "{}",
            case.name
        );
        validate_cos_digest_commands(&case.input, &case.commands)
            .unwrap_or_else(|v| panic!("{}: reference output rejected: {v}", case.name));
        // Round trip: the line is the serialization of the typed values.
        assert_eq!(serde_json::to_string(&case).expect("ser"), line);
        let requeue = matches!(case.input.run_state.as_str(), "stopped" | "interrupted");
        assert_eq!(
            case.commands.len(),
            if requeue { 2 } else { 1 },
            "{}",
            case.name
        );
        run_states.insert(case.input.run_state.clone());
        for item in &case.input.items {
            item_states.insert(item.state.clone());
        }
    }
    assert_eq!(generated, COS_DIGEST_GENERATED_CASES);
    for s in ["completed", "failed", "stopped", "interrupted", "cancelled"] {
        assert!(run_states.contains(s), "run_state {s} missing");
    }
    for s in ["escalated", "fallback", "running", "handled"] {
        assert!(item_states.contains(s), "item state {s} missing");
    }
}

fn cos_digest_stopped_input() -> CosDigestInput {
    let line = std::fs::read_to_string(cos_digest_fixture_path())
        .expect("read committed fixture")
        .lines()
        .map(|l| serde_json::from_str::<CosDigestLine>(l).expect("parse"))
        .find(|c| c.name == "hand/run-state/stopped")
        .expect("hand/run-state/stopped case");
    line.input
}

fn cos_digest_post_mut(
    commands: &mut [CosDigestCommand],
) -> (
    &mut String,
    &mut String,
    &mut String,
    &mut Vec<task_core::chat::ChatCard>,
    &mut String,
) {
    match &mut commands[0] {
        CosDigestCommand::PostDigest {
            thread,
            idempotency_key,
            text,
            cards,
            run_id,
        } => (thread, idempotency_key, text, cards, run_id),
        CosDigestCommand::RequeueRun { .. } => panic!("first command is post_digest"),
    }
}

/// S4-3: every rule rejects one broken command list (one mutation per rule), with its code.
#[test]
fn script_fixtures_cos_digest_validation_rejects_each_rule() {
    let input = cos_digest_stopped_input();
    let good = cos_digest_decide(&input);
    assert_eq!(validate_cos_digest_commands(&input, &good), Ok(()));
    type Mutation = (
        &'static str,
        CosDigestViolation,
        Option<&'static str>,
        Box<dyn Fn(&mut Vec<CosDigestCommand>)>,
    );
    let mutations: Vec<Mutation> = vec![
        (
            "rule1 key",
            CosDigestViolation::KeyMismatch,
            Some("key_mismatch"),
            Box::new(|c| *cos_digest_post_mut(c).1 = "cos-triage-digest:other".into()),
        ),
        (
            "rule2 thread",
            CosDigestViolation::ScopeMismatch,
            Some("scope_mismatch"),
            Box::new(|c| *cos_digest_post_mut(c).0 = "thread-2".into()),
        ),
        (
            "rule2 post run",
            CosDigestViolation::ScopeMismatch,
            Some("scope_mismatch"),
            Box::new(|c| *cos_digest_post_mut(c).4 = "run-2".into()),
        ),
        (
            "rule2 requeue run",
            CosDigestViolation::ScopeMismatch,
            Some("scope_mismatch"),
            Box::new(|c| {
                c[1] = CosDigestCommand::RequeueRun {
                    run_id: "run-2".into(),
                }
            }),
        ),
        (
            "rule3 unknown card",
            CosDigestViolation::CardOutsideItems,
            Some("card_outside_items"),
            Box::new(|c| cos_digest_post_mut(c).3[0].id = "decision:elsewhere".into()),
        ),
        (
            "rule3 too many cards",
            CosDigestViolation::CardOutsideItems,
            Some("card_outside_items"),
            Box::new(|c| {
                let cards = cos_digest_post_mut(c).3;
                let first = cards[0].clone();
                cards.push(first);
            }),
        ),
        (
            "rule5 two posts",
            CosDigestViolation::DuplicateCommand,
            Some("duplicate_command"),
            Box::new(|c| {
                let p = c[0].clone();
                c.push(p);
            }),
        ),
        (
            "rule5 two requeues",
            CosDigestViolation::DuplicateCommand,
            Some("duplicate_command"),
            Box::new(|c| {
                let r = c[1].clone();
                c.push(r);
            }),
        ),
        (
            "rule6 no post",
            CosDigestViolation::MissingPostDigest,
            None,
            Box::new(|c| {
                c.remove(0);
            }),
        ),
        (
            "rule6 text size",
            CosDigestViolation::TextTooLarge,
            None,
            Box::new(|c| *cos_digest_post_mut(c).2 = "x".repeat(64 * 1024 + 1)),
        ),
        (
            "rule6 key size",
            CosDigestViolation::KeyTooLarge,
            None,
            Box::new(|c| {
                *cos_digest_post_mut(c).1 = format!("cos-triage-digest:{}", "r".repeat(128))
            }),
        ),
    ];
    for (name, expected, code, mutate) in mutations {
        let mut commands = good.clone();
        mutate(&mut commands);
        let got = validate_cos_digest_commands(&input, &commands);
        assert_eq!(got, Err(expected.clone()), "{name}");
        assert_eq!(expected.code(), code, "{name}");
    }
    // Rule 4: a requeue for a run that was not stopped or interrupted.
    for run_state in ["completed", "failed", "cancelled"] {
        let mut other = input.clone();
        other.run_state = run_state.into();
        let mut commands = cos_digest_decide(&other);
        assert_eq!(
            validate_cos_digest_commands(&other, &commands),
            Ok(()),
            "{run_state}"
        );
        commands.push(CosDigestCommand::RequeueRun {
            run_id: other.run_id.clone(),
        });
        let got = validate_cos_digest_commands(&other, &commands);
        assert_eq!(
            got,
            Err(CosDigestViolation::RequeueStateInvalid),
            "{run_state}"
        );
        assert_eq!(
            CosDigestViolation::RequeueStateInvalid.code(),
            Some("requeue_state_invalid")
        );
    }
}

/// S4-2: the DTOs reject unknown fields (deny_unknown_fields on the input, items, decision,
/// options and commands).
#[test]
fn script_fixtures_cos_digest_dtos_deny_unknown_fields() {
    let input = cos_digest_stopped_input();
    let mut value = serde_json::to_value(&input).expect("ser");
    value["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CosDigestInput>(value).is_err());
    let mut value = serde_json::to_value(&input).expect("ser");
    value["items"][0]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CosDigestInput>(value).is_err());
    let mut decided = input.clone();
    decided.items[0].decision = Some(task_core::chat::triage::CosTriageDecision {
        options: vec![task_core::chat::triage::CosTriageOption {
            key: "a".into(),
            label: "A".into(),
            detail: None,
        }],
        ..Default::default()
    });
    let mut value = serde_json::to_value(&decided).expect("ser");
    value["items"][0]["decision"]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CosDigestInput>(value).is_err());
    let mut value = serde_json::to_value(&decided).expect("ser");
    value["items"][0]["decision"]["options"][0]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CosDigestInput>(value).is_err());
    let cmd = serde_json::json!({"kind": "requeue_run", "run_id": "run-1", "extra": 1});
    assert!(serde_json::from_value::<CosDigestCommand>(cmd).is_err());
    let cmd = serde_json::json!({"kind": "requeue_run", "run_id": "run-1"});
    assert!(serde_json::from_value::<CosDigestCommand>(cmd).is_ok());
}
