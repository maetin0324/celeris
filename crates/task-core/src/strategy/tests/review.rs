//! `strategy_review_contract_*`: ADR 2026-11-22-strategy-review-critic D1・D2.4・D3.7・D4 の純関数の規則の試験（D7.1・D7.2）。
//!
//! 守る不変条件: 候補は識別実験・期待情報価値・停止条件を持ち、実験段階の版（selected が無い）は open の候補ごとに 1 件の
//! atomic の実験だけを通し、選んだ版は前の版の実験がすべて終端のときだけ通る。却下した候補は次の版で消えず・選べず・起票できない。
//! 前の版の終端の実験を証拠が参照しない版は通らない。文書を作った task が done で Critic の対象にある版だけが通る。
//! `strategy_contract_*` は子 2 の規則だけを見るので、これらを検出できない（子 2 の型は新しい欄を知らず、入口の規則も無い）。

use super::*;
use crate::ExecutionMode;

const ID: &str = "arch-v4-cycle";
/// v1 の実験 task（v2-keeps-refute の証拠がすでに参照する task）。
const TASK_D: &str = "01M7TASKD00000000000000000";
const TASK_H: &str = "01M7TASKH00000000000000000";
/// どの版の証拠も参照しない task。
const TASK_Z: &str = "01M7TASKZ00000000000000000";

fn doc(bytes: &[u8]) -> StrategyDocument {
    serde_json::from_slice(bytes).expect("strategy document")
}

fn edit(text: &str, f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    serde_json::to_vec_pretty(&v).unwrap()
}

fn link(text: &str, version: u32) -> PreviousVersion {
    PreviousVersion {
        pointer: ArtifactPointer {
            task_id: TASK.into(),
            name: artifact_name(ID, version),
            sha256: sha(text.as_bytes()),
        },
        bytes: Some(text.as_bytes().to_vec()),
        task_project_id: Some(P.into()),
    }
}

/// v2-keeps-refute の次の版 v3（候補・証拠をそのまま残す）を `f` で変える。
fn v3(f: impl FnOnce(&mut Value)) -> Vec<u8> {
    edit(V2_KEEPS, |v| {
        v["version"] = 3.into();
        v["idempotency_key"] = idempotency_key(ID, 3).into();
        v["supersedes"] = serde_json::to_value(link(V2_KEEPS, 2).pointer).unwrap();
        v["change_reason"] = "本実装の後の再評価".into();
        f(v);
    })
}

/// v3 の文書の検査（鎖は v2-keeps-refute → baseline-v1）。
fn check_v3(bytes: &[u8]) -> Vec<Violation> {
    let mut ctx = ctx_for(bytes, Intent::Execute);
    ctx.chain = vec![link(V2_KEEPS, 2), link(V1, 1)];
    validate_document(bytes, &ctx)
}

fn exp(version: u32, task_id: &str, candidate: &str, terminal: bool) -> ExperimentTask {
    ExperimentTask {
        version,
        task_id: task_id.into(),
        candidate: Some(candidate.into()),
        terminal,
    }
}

/// 基準 B1: 実験段階の v1（open の候補 2 件）に、open の候補 cut-min-edges の atomic の実験を初めて起票する。
fn b1() -> ExecutionFacts {
    ExecutionFacts {
        source_done: true,
        critiqued: true,
        experiments: vec![],
        execution: Some(ExecutionMode::Atomic),
        candidate: Some("cut-min-edges".into()),
        max_wall_secs: None,
    }
}

/// 基準 B2: v1 の実験 2 件（D・H）が終わり、それを証拠で読んで cut-min-edges を選んだ v2 で本実装（compound）を起票する。
fn b2() -> ExecutionFacts {
    ExecutionFacts {
        experiments: vec![
            exp(1, TASK_D, "cut-min-edges", true),
            exp(1, TASK_H, "continue-pilots", true),
        ],
        execution: Some(ExecutionMode::Compound),
        candidate: None,
        ..b1()
    }
}

fn exec_codes(text: &[u8], facts: &ExecutionFacts) -> BTreeSet<&'static str> {
    codes(&check_execution(&doc(text), facts))
}

fn only(code: &'static str) -> BTreeSet<&'static str> {
    BTreeSet::from([code])
}

/// `candidates[1]`（continue-pilots）を `n` 件写して足す（id は c0…）。`f` で写しを変える。
fn copies(v: &mut Value, n: usize, f: impl Fn(&mut Value)) {
    let mut c = v["candidates"][1].clone();
    f(&mut c);
    let list: Vec<Value> = (0..n)
        .map(|i| {
            c["id"] = format!("c{i}").into();
            c.clone()
        })
        .collect();
    v["candidates"].as_array_mut().unwrap().extend(list);
}

fn rejected(c: &mut Value) {
    c["state"] = "rejected".into();
    c["rejected_by"] = serde_json::json!(["e-pilot-refute"]);
}

#[test]
fn strategy_review_contract_baselines_accepted() {
    for (name, bytes) in [("v1", V1), ("v2-keeps-refute", V2_KEEPS)] {
        for intent in [Intent::Record, Intent::Execute] {
            let got = check(bytes.as_bytes(), intent);
            assert!(got.is_empty(), "{name} {intent:?}: {got:?}");
        }
    }
    let same = v3(|_| {});
    assert!(check_v3(&same).is_empty(), "{:?}", check_v3(&same));
    // v1 は実験段階（selected が無く open が 2 件）、v2 は v1 の実験を読んで選んだ実行段階。
    let v1 = doc(V1.as_bytes());
    assert!(v1.candidates.iter().all(|c| !c.selected));
    assert!(
        v1.candidates
            .iter()
            .all(|c| c.state == CandidateState::Open)
    );
    assert_eq!(v1.held_assumptions.len(), 1);
    assert!(exec_codes(V1.as_bytes(), &b1()).is_empty());
    assert!(exec_codes(V2_KEEPS.as_bytes(), &b2()).is_empty());
    // 却下した候補を残すので、候補は 8 件（open 2・rejected 6）まで書ける。
    let eight = edit(V1, |v| copies(v, 6, rejected));
    let got = check(&eight, Intent::Record);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn strategy_review_contract_missing_fields_rejected() {
    let no_held = edit(V1, |v| {
        v.as_object_mut().unwrap().remove("held_assumptions");
    });
    let got = check(&no_held, Intent::Record);
    assert_eq!(rules(&got), BTreeSet::from([Rule::Required]), "{got:?}");
    assert_eq!(got[0].path, "held_assumptions");
    for key in [
        "state",
        "experiment",
        "expected_information_value",
        "stop_when",
    ] {
        let bytes = edit(V1, |v| {
            v["candidates"][0].as_object_mut().unwrap().remove(key);
        });
        let got = check(&bytes, Intent::Record);
        assert_eq!(rules(&got), BTreeSet::from([Rule::Shape]), "{key}: {got:?}");
        assert_eq!(codes(&got), only("strategy_invalid"), "{key}");
    }
}

#[test]
fn strategy_review_contract_field_limits_rejected() {
    type Case = (&'static str, &'static str, Box<dyn Fn(&mut Value)>);
    let over = |n: usize| Value::from("x".repeat(n + 1));
    let cases: Vec<Case> = vec![
        (
            "experiment.content",
            V1,
            Box::new(move |v| v["candidates"][0]["experiment"]["content"] = over(TEXT_MAX_CHARS)),
        ),
        (
            "experiment.content empty",
            V1,
            Box::new(|v| v["candidates"][0]["experiment"]["content"] = " ".into()),
        ),
        (
            "expected_information_value",
            V1,
            Box::new(move |v| {
                v["candidates"][1]["expected_information_value"] = over(TEXT_MAX_CHARS)
            }),
        ),
        (
            "stop_when item",
            V1,
            Box::new(move |v| v["candidates"][0]["stop_when"][0] = over(LIST_ITEM_MAX_CHARS)),
        ),
        (
            "stop_when empty",
            V1,
            Box::new(|v| v["candidates"][0]["stop_when"] = serde_json::json!([])),
        ),
        (
            "stop_when 9",
            V1,
            Box::new(|v| v["candidates"][0]["stop_when"] = Value::from(vec!["止める"; 9])),
        ),
        (
            "discriminates empty",
            V1,
            Box::new(|v| v["candidates"][0]["experiment"]["discriminates"] = serde_json::json!([])),
        ),
        (
            "discriminates 9",
            V1,
            Box::new(|v| {
                v["candidates"][0]["experiment"]["discriminates"] =
                    Value::from(vec!["h-cycle-edges"; 9])
            }),
        ),
        (
            "held_assumptions 17",
            V1,
            Box::new(|v| {
                let held: Vec<Value> = (0..17)
                    .map(|i| {
                        let mut a = v["held_assumptions"][0].clone();
                        a["id"] = format!("a{i}").into();
                        a
                    })
                    .collect();
                v["held_assumptions"] = Value::Array(held);
            }),
        ),
        (
            "held statement",
            V1,
            Box::new(move |v| v["held_assumptions"][0]["statement"] = over(TEXT_MAX_CHARS)),
        ),
        (
            "held revisit_when",
            V1,
            Box::new(move |v| v["held_assumptions"][0]["revisit_when"] = over(LIST_ITEM_MAX_CHARS)),
        ),
        (
            "candidates 1",
            V1,
            Box::new(|v| {
                v["candidates"].as_array_mut().unwrap().truncate(1);
            }),
        ),
        ("candidates 9", V1, Box::new(|v| copies(v, 7, rejected))),
        ("open 4", V1, Box::new(|v| copies(v, 2, |_| {}))),
        (
            "open 1 without selected",
            V1,
            Box::new(|v| rejected(&mut v["candidates"][1])),
        ),
        (
            "rejected_by empty",
            V2_KEEPS,
            Box::new(|v| v["candidates"][1]["rejected_by"] = serde_json::json!([])),
        ),
        (
            "rejected_by 17",
            V2_KEEPS,
            Box::new(|v| {
                v["candidates"][1]["rejected_by"] = Value::from(vec!["e-pilot-refute"; 17])
            }),
        ),
    ];
    for (name, base, f) in cases {
        let bytes = edit(base, |v| f(v));
        let got = check(&bytes, Intent::Record);
        assert_eq!(
            rules(&got),
            BTreeSet::from([Rule::Limit]),
            "{name}: {got:?}"
        );
    }
}

#[test]
fn strategy_review_contract_shape_rules_rejected() {
    type Case = (&'static str, &'static str, Box<dyn Fn(&mut Value)>);
    let cases: Vec<Case> = vec![
        (
            "open with rejected_by",
            V1,
            Box::new(|v| v["candidates"][1]["rejected_by"] = serde_json::json!(["e-pilot-refute"])),
        ),
        (
            "rejected_by support evidence",
            V2_KEEPS,
            Box::new(|v| {
                v["candidates"][1]["rejected_by"] = serde_json::json!(["e-scc-unchanged"])
            }),
        ),
        (
            "rejected_by missing evidence",
            V2_KEEPS,
            Box::new(|v| v["candidates"][1]["rejected_by"] = serde_json::json!(["e-none"])),
        ),
        (
            "budget usd over cost",
            V1,
            Box::new(|v| v["candidates"][0]["experiment"]["budget"]["usd_max"] = 41.into()),
        ),
        (
            "budget wall zero",
            V1,
            Box::new(|v| v["candidates"][0]["experiment"]["budget"]["wall_hours_max"] = 0.into()),
        ),
        (
            "budget wall over cost",
            V1,
            Box::new(|v| v["candidates"][0]["experiment"]["budget"]["wall_hours_max"] = 7.into()),
        ),
        (
            "discriminates unknown hypothesis",
            V1,
            Box::new(|v| {
                v["candidates"][0]["experiment"]["discriminates"] = serde_json::json!(["h-none"])
            }),
        ),
        (
            "discriminates none of the candidate's hypotheses",
            V1,
            Box::new(|v| {
                v["candidates"][0]["experiment"]["discriminates"] =
                    serde_json::json!(["h-pilots-enough"])
            }),
        ),
        (
            "held id duplicate",
            V1,
            Box::new(|v| {
                let a = v["held_assumptions"][0].clone();
                v["held_assumptions"].as_array_mut().unwrap().push(a);
            }),
        ),
        (
            "held id shape",
            V1,
            Box::new(|v| v["held_assumptions"][0]["id"] = "Held Assumption".into()),
        ),
    ];
    for (name, base, f) in cases {
        let bytes = edit(base, |v| f(v));
        let got = check(&bytes, Intent::Record);
        assert_eq!(
            rules(&got),
            BTreeSet::from([Rule::Shape]),
            "{name}: {got:?}"
        );
    }
}

#[test]
fn strategy_review_contract_rejected_removed_next_version_is_revisit() {
    let cases: [(&str, Box<dyn Fn(&mut Value)>); 3] = [
        (
            "removed",
            Box::new(|v| {
                let mut other = v["candidates"][0].clone();
                other["id"] = "split-layers".into();
                other["selected"] = false.into();
                v["candidates"][1] = other;
            }),
        ),
        (
            "reopened",
            Box::new(|v| {
                v["candidates"][1]["state"] = "open".into();
                v["candidates"][1]["rejected_by"] = serde_json::json!([]);
            }),
        ),
        (
            "rewritten",
            Box::new(|v| v["candidates"][1]["approach"] = "pilot を F・G と続ける".into()),
        ),
    ];
    for (name, f) in cases {
        let got = check_v3(&v3(|v| f(v)));
        assert_eq!(
            rules(&got),
            BTreeSet::from([Rule::RevisitRejected]),
            "{name}: {got:?}"
        );
        assert_eq!(codes(&got), only("strategy_revisit_rejected"), "{name}");
    }
}

#[test]
fn strategy_review_contract_selected_rejected_is_revisit() {
    let bytes = edit(V2_KEEPS, |v| {
        v["candidates"][0]["selected"] = false.into();
        v["candidates"][1]["selected"] = true.into();
    });
    let got = check(&bytes, Intent::Execute);
    assert_eq!(codes(&got), only("strategy_revisit_rejected"), "{got:?}");
    // 次の版で却下した候補を選び直すのも同じ code（鎖の規則と selected の規則）。
    let got = check_v3(&v3(|v| {
        v["candidates"][0]["selected"] = false.into();
        v["candidates"][1]["selected"] = true.into();
    }));
    assert_eq!(codes(&got), only("strategy_revisit_rejected"), "{got:?}");
}

#[test]
fn strategy_review_contract_rejected_candidate_task_is_revisit() {
    for execution in [ExecutionMode::Atomic, ExecutionMode::Compound] {
        let facts = ExecutionFacts {
            candidate: Some("continue-pilots".into()),
            execution: Some(execution),
            ..b2()
        };
        assert_eq!(
            exec_codes(V2_KEEPS.as_bytes(), &facts),
            only("strategy_revisit_rejected"),
            "{execution:?}"
        );
    }
}

#[test]
fn strategy_review_contract_execution_compound_in_experiment_phase() {
    for execution in [Some(ExecutionMode::Compound), None] {
        let facts = ExecutionFacts { execution, ..b1() };
        assert_eq!(
            exec_codes(V1.as_bytes(), &facts),
            only("strategy_experiment_phase"),
            "{execution:?}"
        );
    }
}

#[test]
fn strategy_review_contract_experiments_up_to_open_candidates() {
    let open: Vec<String> = doc(V1.as_bytes())
        .candidates
        .iter()
        .filter(|c| c.state == CandidateState::Open)
        .map(|c| c.id.clone())
        .collect();
    assert_eq!(open.len(), 2);
    let mut facts = b1();
    for (i, id) in open.iter().enumerate() {
        facts.candidate = Some(id.clone());
        assert!(exec_codes(V1.as_bytes(), &facts).is_empty(), "{id}");
        facts
            .experiments
            .push(exp(1, &format!("01M7EXPERIMENT{i:012}"), id, false));
    }
    // 版の実験 task は open の候補の数まで。同じ候補の 2 件目は拒否する。
    for id in &open {
        facts.candidate = Some(id.clone());
        assert_eq!(
            exec_codes(V1.as_bytes(), &facts),
            only("strategy_experiment_phase"),
            "{id}"
        );
    }
    // candidate の無い起票・文書に無い候補も拒否する。
    facts.experiments.clear();
    for candidate in [None, Some("no-such".to_string())] {
        facts.candidate = candidate.clone();
        assert_eq!(
            exec_codes(V1.as_bytes(), &facts),
            only("strategy_experiment_phase"),
            "{candidate:?}"
        );
    }
    // max_wall_secs は候補の実験の予算（cut-min-edges は 6 時間）の内。
    facts.candidate = Some("cut-min-edges".into());
    facts.max_wall_secs = Some(6 * 3600);
    assert!(exec_codes(V1.as_bytes(), &facts).is_empty());
    facts.max_wall_secs = Some(6 * 3600 + 1);
    assert_eq!(
        exec_codes(V1.as_bytes(), &facts),
        only("strategy_experiment_phase")
    );
}

#[test]
fn strategy_review_contract_selected_needs_terminal_experiments() {
    let v2 = V2_KEEPS.as_bytes();
    let cases = [
        ("no experiments", vec![]),
        (
            "one not terminal",
            vec![
                exp(1, TASK_D, "cut-min-edges", true),
                exp(1, TASK_H, "continue-pilots", false),
            ],
        ),
        // 選んだ版自身の task（X(v2)）は前の版の実験に数えない。
        (
            "only this version",
            vec![exp(2, TASK_Z, "cut-min-edges", true)],
        ),
    ];
    for (name, experiments) in cases {
        let facts = ExecutionFacts {
            experiments,
            ..b2()
        };
        assert_eq!(
            exec_codes(v2, &facts),
            only("strategy_experiment_phase"),
            "{name}"
        );
    }
    // 結果を見てから選んだ版は execution を問わず、selected の候補か candidate 無しで通る。
    for (execution, candidate) in [
        (Some(ExecutionMode::Atomic), Some("cut-min-edges")),
        (None, None),
    ] {
        let facts = ExecutionFacts {
            execution,
            candidate: candidate.map(str::to_string),
            ..b2()
        };
        assert!(exec_codes(v2, &facts).is_empty(), "{execution:?}");
    }
    let unknown = ExecutionFacts {
        candidate: Some("no-such".into()),
        ..b2()
    };
    assert_eq!(exec_codes(v2, &unknown), only("strategy_experiment_phase"));
    // v1 で実験の前に selected を書いた版も実験段階を飛ばせない。
    let early = edit(V1, |v| v["candidates"][0]["selected"] = true.into());
    let facts = ExecutionFacts {
        experiments: vec![],
        ..b2()
    };
    assert_eq!(
        exec_codes(&early, &facts),
        only("strategy_experiment_phase")
    );
}

#[test]
fn strategy_review_contract_uncritiqued_source_not_done_or_not_subject() {
    let not_done = ExecutionFacts {
        source_done: false,
        ..b1()
    };
    assert_eq!(
        exec_codes(V1.as_bytes(), &not_done),
        only("strategy_uncritiqued")
    );
    let older = ExecutionFacts {
        critiqued: false,
        ..b2()
    };
    assert_eq!(
        exec_codes(V2_KEEPS.as_bytes(), &older),
        only("strategy_uncritiqued")
    );
}

#[test]
fn strategy_review_contract_failure_unread_terminal_experiment() {
    let v2 = V2_KEEPS.as_bytes();
    let mut unread = b2();
    unread
        .experiments
        .push(exp(1, TASK_Z, "cut-min-edges", true));
    assert_eq!(exec_codes(v2, &unread), only("strategy_failure_unread"));
    // 終わっていない実験は読む対象でない（ただし選んだ版は終わるまで通らない）。
    let mut running = b2();
    running
        .experiments
        .push(exp(1, TASK_Z, "cut-min-edges", false));
    assert_eq!(exec_codes(v2, &running), only("strategy_experiment_phase"));
    // 再評価の v3: v2 の本実装 TASK_Z が終わった。読まない v3 は拒否、証拠で参照した v3 は通る。
    let mut after = b2();
    after
        .experiments
        .push(exp(2, TASK_Z, "cut-min-edges", true));
    assert_eq!(
        exec_codes(&v3(|_| {}), &after),
        only("strategy_failure_unread")
    );
    let read = v3(|v| {
        v["evidence"][0]["references"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"kind": "task", "task_id": TASK_Z}));
    });
    assert!(exec_codes(&read, &after).is_empty());
}

#[test]
fn strategy_review_contract_ablation() {
    let base = b2();
    let v2 = V2_KEEPS.as_bytes();
    assert!(check(v2, Intent::Execute).is_empty(), "B2 の文書は受理");
    assert!(exec_codes(v2, &base).is_empty(), "B2 は受理");
    let mut phase = b2();
    phase.experiments[1].terminal = false;
    let crit = ExecutionFacts {
        critiqued: false,
        ..b2()
    };
    let revisit = ExecutionFacts {
        candidate: Some("continue-pilots".into()),
        ..b2()
    };
    let mut unread = b2();
    unread
        .experiments
        .push(exp(1, TASK_Z, "cut-min-edges", true));
    let rows = [
        (
            "N-phase",
            "v1 の実験 H が終端でない",
            phase,
            "strategy_experiment_phase",
        ),
        (
            "N-crit",
            "参照した版が Critic の対象にない",
            crit,
            "strategy_uncritiqued",
        ),
        (
            "N-revisit",
            "却下した continue-pilots の task",
            revisit,
            "strategy_revisit_rejected",
        ),
        (
            "N-unread",
            "v1 の終端の実験 Z を v2 が参照しない",
            unread,
            "strategy_failure_unread",
        ),
    ];
    println!("| 負の状態 | B2 から変えた 1 か所 | 返した code |");
    println!("|---|---|---|");
    for (name, label, facts, code) in rows {
        let got = exec_codes(v2, &facts);
        println!("| {name} | {label} | {got:?} |");
        assert_eq!(got, only(code), "{name}");
    }
}
