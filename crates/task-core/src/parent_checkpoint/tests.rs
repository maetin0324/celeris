//! ADR 2026-11-22-parent-checkpoint-evaluator D1.2・D1.3・D3.2・D4・D5.2 の契約の試験（prefix `parent_checkpoint_contract_`）。
//!
//! base には `task_core::parent_checkpoint` が無く、親の途中評価の発火・評価の検査・行動の表を確かめる試験も無い。
//! 既存の試験（`strategy_*`・`strategy_critic_*`・`project_controller_*`）は戦略の文書の形・Critic の対象・controller の
//! 既存の行動だけを見るので、ここで守る不変条件（発火の条件、参照の無い主張を on_track にしない、goal の到達を評価が
//! 書けない、写しの表）が壊れても検出できない。各試験の doc comment に守る不変条件を書く。

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::*;
use crate::Status;
use crate::project_proposal::{ProjectAction, WaitPhase};
use crate::strategy::{Reference, artifact_name};

const V1: &str = include_str!("../strategy/fixtures/baseline-v1.json");
const V2: &str = include_str!("../strategy/fixtures/v2-keeps-refute.json");
const SID: &str = "arch-v4-cycle";

// ---------------------------------------------------------------- 発火（D1.2・D1.3）

fn child(id: &str, steps: &[(Status, &str)]) -> ChildFacts {
    ChildFacts {
        task_id: id.into(),
        transitions: steps
            .iter()
            .map(|(to, at)| ChildTransition {
                to: *to,
                at: (*at).into(),
            })
            .collect(),
    }
}

fn done(id: &str, at: &str) -> ChildFacts {
    child(id, &[(Status::Running, at), (Status::Done, at)])
}

fn strategy(documents: Vec<StrategyDocumentFacts>) -> Option<StrategyFacts> {
    Some(StrategyFacts {
        strategy_id: SID.into(),
        version: 1,
        documents,
    })
}

fn facts(children: Vec<ChildFacts>) -> TriggerFacts {
    TriggerFacts {
        strategy: strategy(Vec::new()),
        children,
        tokens_used: 0,
        max_tokens: None,
        evaluated: BTreeSet::new(),
        requested: BTreeSet::new(),
    }
}

fn due(ts: &[Trigger]) -> Vec<String> {
    ts.iter()
        .filter(|t| t.state == TriggerState::Due)
        .map(|t| t.key.clone())
        .collect()
}

fn find<'a>(ts: &'a [Trigger], key: &str) -> Option<&'a Trigger> {
    ts.iter().find(|t| t.key == key)
}

/// 不変条件: 子の終端の数 c が N の倍数 k に達したときだけ `children:<k>` が発火し、N 未満では発火しない。
/// 評価済みの k は再び due にならず、次の倍数で新しい key が due になる。`at` は k 件目の終端の時刻。
#[test]
fn parent_checkpoint_contract_children_every_n() {
    let s = CheckpointSettings::default();
    let one = facts(vec![done("A", "2026-11-17T18:45:41Z")]);
    assert!(triggers(&one, &s).is_empty(), "N 未満（1 < 2）で発火した");

    let mut two = facts(vec![
        done("A", "2026-11-17T18:45:41Z"),
        done("B", "2026-11-17T21:55:57Z"),
    ]);
    let ts = triggers(&two, &s);
    assert_eq!(due(&ts), vec!["children:2"]);
    let t = find(&ts, "children:2").expect("children:2");
    assert_eq!(t.kind, TriggerKind::Children);
    assert_eq!(t.at.as_deref(), Some("2026-11-17T21:55:57Z"));

    two.evaluated.insert("children:2".into());
    two.children.push(done("C", "2026-11-18T10:48:28Z"));
    let ts = triggers(&two, &s);
    assert!(
        due(&ts).is_empty(),
        "3 件目で新しい checkpoint が出た: {ts:?}"
    );
    assert_eq!(
        find(&ts, "children:2").map(|t| t.state),
        Some(TriggerState::Evaluated)
    );

    two.children.push(done("D", "2026-11-18T20:39:25Z"));
    let ts = triggers(&two, &s);
    assert_eq!(due(&ts), vec!["children:4"]);
    assert_eq!(
        find(&ts, "children:4").and_then(|t| t.at.as_deref()),
        Some("2026-11-18T20:39:25Z")
    );

    let n3 = CheckpointSettings::new(3, vec![50, 80], 2).expect("N = 3");
    let two_children = facts(vec![
        done("A", "2026-11-17T18:45:41Z"),
        done("B", "2026-11-17T21:55:57Z"),
    ]);
    assert!(
        triggers(&two_children, &n3).is_empty(),
        "N = 3 で 2 件で発火した"
    );
}

/// 不変条件: 終端は `done`・`failed`・`cancelled` の 3 つで、c が一度に 2 段以上進んだら最大の k だけが出る
/// （古い k を後から評価しない）。
#[test]
fn parent_checkpoint_contract_children_jump_max_only() {
    let f = facts(vec![
        done("A", "2026-11-18T01:00:00Z"),
        done("B", "2026-11-18T01:00:01Z"),
        child("C", &[(Status::Cancelled, "2026-11-18T01:00:02Z")]),
        done("D", "2026-11-18T01:00:03Z"),
        done("E", "2026-11-18T01:00:04Z"),
    ]);
    let ts = triggers(&f, &CheckpointSettings::default());
    let children: Vec<&str> = ts
        .iter()
        .filter(|t| t.kind == TriggerKind::Children)
        .map(|t| t.key.as_str())
        .collect();
    assert_eq!(children, vec!["children:4"]);
}

/// 不変条件: `review_fail` で `ready` に戻る再試行は終端にも失敗にも数えない（局所の review で、親の反証ではない）。
/// 子の今の状態は最後の遷移で決まる。
#[test]
fn parent_checkpoint_contract_review_fail_retry_not_counted() {
    let s = CheckpointSettings::default();
    let mut f = facts(vec![
        child(
            "A",
            &[
                (Status::Running, "2026-11-17T16:00:00Z"),
                (Status::Reviewing, "2026-11-17T17:00:00Z"),
                (Status::Ready, "2026-11-17T17:10:00Z"),
                (Status::Running, "2026-11-17T17:20:00Z"),
            ],
        ),
        done("B", "2026-11-17T21:55:57Z"),
    ]);
    let ts = triggers(&f, &s);
    assert!(ts.is_empty(), "review_fail の再試行を数えた: {ts:?}");

    f.children[0].transitions.push(ChildTransition {
        to: Status::Done,
        at: "2026-11-17T22:00:00Z".into(),
    });
    assert_eq!(due(&triggers(&f, &s)), vec!["children:2"]);
}

/// 不変条件: strategy の無い root（低抽象度の root）は、子・失敗・予算がどうであっても trigger を 1 つも持たない
/// （D9: 従来の task で reviewer run を増やさない）。
#[test]
fn parent_checkpoint_contract_no_strategy_root_no_triggers() {
    let mut f = facts(vec![
        done("A", "2026-11-18T01:00:00Z"),
        done("B", "2026-11-18T01:00:01Z"),
        done("C", "2026-11-18T01:00:02Z"),
        child("D", &[(Status::Failed, "2026-11-18T01:00:03Z")]),
    ]);
    f.strategy = None;
    f.tokens_used = 950;
    f.max_tokens = Some(1_000);
    assert!(triggers(&f, &CheckpointSettings::default()).is_empty());
}

/// 不変条件: 子が一度でも `failed` に遷移したら `child_failed:<id>` が 1 つだけ due になる（再開して走っていても、
/// 2 回失敗しても 1 つ）。再開した子は終端に数えない。
#[test]
fn parent_checkpoint_contract_child_failed() {
    let f = facts(vec![child(
        "X",
        &[
            (Status::Running, "2026-11-18T01:00:00Z"),
            (Status::Failed, "2026-11-18T02:00:00Z"),
            (Status::Ready, "2026-11-18T03:00:00Z"),
            (Status::Running, "2026-11-18T03:10:00Z"),
            (Status::Failed, "2026-11-18T04:00:00Z"),
            (Status::Ready, "2026-11-18T05:00:00Z"),
        ],
    )]);
    let ts = triggers(&f, &CheckpointSettings::default());
    assert_eq!(due(&ts), vec!["child_failed:X"]);
    let t = find(&ts, "child_failed:X").expect("child_failed");
    assert_eq!(t.kind, TriggerKind::ChildFailed);
    assert_eq!(t.at.as_deref(), Some("2026-11-18T02:00:00Z"));
}

fn doc(version: u32, extra_refute: Option<&str>) -> Vec<u8> {
    let mut v: Value = serde_json::from_str(if version == 1 { V1 } else { V2 }).expect("fixture");
    v["version"] = json!(version);
    if let Some(id) = extra_refute {
        let mut e = v["evidence"][0].clone();
        e["id"] = json!(id);
        e["stance"] = json!("refute");
        v["evidence"].as_array_mut().expect("evidence").push(e);
    }
    serde_json::to_vec(&v).expect("serialize")
}

fn produced(version: u32, bytes: Option<Vec<u8>>, at: &str) -> StrategyDocumentFacts {
    StrategyDocumentFacts {
        name: artifact_name(SID, version),
        bytes,
        at: at.into(),
    }
}

/// 不変条件: 反証の trigger は、root が参照した版（v1）より新しい版に、参照した版に無い `stance: refute` の証拠が
/// あるときだけ出る。v1 から持ち越した反証（`e-pilot-refute`）・参照した版そのもの・別の strategy の文書は数えない。
/// 同じ証拠を後の版が持ち越しても、最初に現れた版の key 1 つだけ（同じ反証で 2 回評価しない）。
#[test]
fn parent_checkpoint_contract_refute_new_evidence_only() {
    let mut other: Value = serde_json::from_slice(&doc(2, Some("e-other"))).expect("doc");
    other["strategy_id"] = json!("other");
    let f = TriggerFacts {
        strategy: strategy(vec![
            produced(1, Some(doc(1, None)), "2026-11-18T00:00:00Z"),
            produced(
                2,
                Some(doc(2, Some("e-new-refute"))),
                "2026-11-18T06:00:00Z",
            ),
            produced(
                3,
                Some(doc(3, Some("e-new-refute"))),
                "2026-11-18T07:00:00Z",
            ),
            StrategyDocumentFacts {
                name: artifact_name("other", 2),
                bytes: Some(serde_json::to_vec(&other).expect("other")),
                at: "2026-11-18T06:30:00Z".into(),
            },
        ]),
        ..facts(Vec::new())
    };
    let ts = triggers(&f, &CheckpointSettings::default());
    assert_eq!(due(&ts), vec!["refute:arch-v4-cycle:v2:e-new-refute"]);
    let t = &ts[0];
    assert_eq!(t.kind, TriggerKind::Refute);
    assert_eq!(t.at.as_deref(), Some("2026-11-18T06:00:00Z"));

    // 持ち越しだけの新しい版（v2-keeps-refute のまま）は発火しない。
    let carried = TriggerFacts {
        strategy: strategy(vec![
            produced(1, Some(doc(1, None)), "2026-11-18T00:00:00Z"),
            produced(2, Some(doc(2, None)), "2026-11-18T06:00:00Z"),
        ]),
        ..facts(Vec::new())
    };
    assert!(triggers(&carried, &CheckpointSettings::default()).is_empty());
}

/// 不変条件: 読めない文書（記録が引けない・JSON でない・name と中身の版が違う）は反証に数えない（D1.2）。
#[test]
fn parent_checkpoint_contract_refute_unreadable_document_ignored() {
    let mut mismatched: Value = serde_json::from_slice(&doc(2, Some("e-x"))).expect("doc");
    mismatched["version"] = json!(5);
    let f = TriggerFacts {
        strategy: strategy(vec![
            produced(1, Some(doc(1, None)), "2026-11-18T00:00:00Z"),
            produced(2, None, "2026-11-18T06:00:00Z"),
            produced(3, Some(b"{not json".to_vec()), "2026-11-18T07:00:00Z"),
            produced(
                4,
                Some(serde_json::to_vec(&mismatched).expect("m")),
                "2026-11-18T08:00:00Z",
            ),
        ]),
        ..facts(Vec::new())
    };
    assert!(triggers(&f, &CheckpointSettings::default()).is_empty());
}

/// 不変条件: 予算の trigger は root の `max_tokens` に対する使った token の割合が各閾値以上で出る。評価済みの閾値は
/// 再び due にならない。`max_tokens` が無い・0 の root では出ない。
#[test]
fn parent_checkpoint_contract_budget_thresholds() {
    let s = CheckpointSettings::default();
    let mut f = facts(Vec::new());
    f.max_tokens = Some(1_000);
    f.tokens_used = 499;
    assert!(triggers(&f, &s).is_empty(), "50% 未満で発火した");
    f.tokens_used = 500;
    let ts = triggers(&f, &s);
    assert_eq!(due(&ts), vec!["budget:50"]);
    assert_eq!(ts[0].kind, TriggerKind::Budget);
    f.tokens_used = 800;
    assert_eq!(due(&triggers(&f, &s)), vec!["budget:50", "budget:80"]);
    f.evaluated.insert("budget:50".into());
    assert_eq!(due(&triggers(&f, &s)), vec!["budget:80"]);

    f.max_tokens = None;
    f.tokens_used = u64::MAX;
    assert!(
        triggers(&f, &s).is_empty(),
        "max_tokens の無い root で発火した"
    );
    f.max_tokens = Some(0);
    assert!(triggers(&f, &s).is_empty(), "max_tokens = 0 で発火した");

    let custom = CheckpointSettings::new(2, vec![25], 2).expect("[25]");
    let mut g = facts(Vec::new());
    g.max_tokens = Some(u64::MAX);
    g.tokens_used = u64::MAX / 4 + 1;
    assert_eq!(due(&triggers(&g, &custom)), vec!["budget:25"]);
}

/// 不変条件: trigger の状態は evaluated → requested → due の順に導く（D1.3）。shadow の `suppressed` を含む
/// requested は due にならないので、同じ trigger の判断は 2 回作られない。出力は時刻順（時刻の無い予算は後ろ）。
#[test]
fn parent_checkpoint_contract_trigger_state_order() {
    let mut f = facts(vec![
        done("A", "2026-11-18T01:00:00Z"),
        done("B", "2026-11-18T02:00:00Z"),
        child("C", &[(Status::Failed, "2026-11-18T00:30:00Z")]),
    ]);
    f.max_tokens = Some(100);
    f.tokens_used = 60;
    f.requested.insert("children:2".into());
    f.requested.insert("budget:50".into());
    f.evaluated.insert("budget:50".into());
    let ts = triggers(&f, &CheckpointSettings::default());
    let got: Vec<(&str, TriggerState)> = ts.iter().map(|t| (t.key.as_str(), t.state)).collect();
    assert_eq!(
        got,
        vec![
            ("child_failed:C", TriggerState::Due),
            ("children:2", TriggerState::Requested),
            ("budget:50", TriggerState::Evaluated),
        ]
    );
}

/// 不変条件: 設定の値の範囲（N 1〜8、閾値は昇順・1〜99・4 件まで、試行 1〜3）は 1 か所で検査し、範囲外の値は
/// 設定エラーになる（`[project_controller.checkpoint]`）。既定は N = 2・[50, 80]・2。
#[test]
fn parent_checkpoint_contract_settings_ranges() {
    let d = CheckpointSettings::default();
    assert_eq!(d.children_per_checkpoint(), 2);
    assert_eq!(d.budget_thresholds_pct(), &[50, 80]);
    assert_eq!(d.max_evaluation_attempts(), 2);
    assert!(CheckpointSettings::new(1, vec![], 1).is_ok());
    assert!(CheckpointSettings::new(8, vec![1, 2, 3, 99], 3).is_ok());
    let bad: Vec<(u32, Vec<u8>, u32, &str)> = vec![
        (0, vec![50], 2, "children_per_checkpoint"),
        (9, vec![50], 2, "children_per_checkpoint"),
        (2, vec![80, 50], 2, "budget_thresholds_pct"),
        (2, vec![50, 50], 2, "budget_thresholds_pct"),
        (2, vec![0], 2, "budget_thresholds_pct"),
        (2, vec![100], 2, "budget_thresholds_pct"),
        (2, vec![10, 20, 30, 40, 50], 2, "budget_thresholds_pct"),
        (2, vec![50], 0, "max_evaluation_attempts"),
        (2, vec![50], 4, "max_evaluation_attempts"),
    ];
    for (n, p, a, field) in bad {
        let err = CheckpointSettings::new(n, p.clone(), a).expect_err("範囲外");
        assert_eq!(err.field, field, "({n}, {p:?}, {a})");
    }
}

/// 不変条件: アーキv4 の子の終端の時刻（ADR D8.1）で N = 2 の checkpoint は B・D・F の done で発火し、
/// `review_fail` で `ready` に戻った A・C の再試行は数えない。評価ごとに evaluated に入れると同じ key は 2 回出ない。
#[test]
fn parent_checkpoint_contract_arch_v4_children_times() {
    let children = vec![
        child(
            "A",
            &[
                (Status::Reviewing, "2026-11-17T17:00:00Z"),
                (Status::Ready, "2026-11-17T17:10:00Z"),
                (Status::Done, "2026-11-17T18:45:41Z"),
            ],
        ),
        done("B", "2026-11-17T21:55:57Z"),
        child(
            "C",
            &[
                (Status::Reviewing, "2026-11-18T08:00:00Z"),
                (Status::Ready, "2026-11-18T08:10:00Z"),
                (Status::Done, "2026-11-18T10:48:28Z"),
            ],
        ),
        done("D", "2026-11-18T20:39:25Z"),
        done("E", "2026-11-18T22:54:25Z"),
        done("F", "2026-11-19T04:54:54Z"),
        done("G", "2026-11-19T10:41:53Z"),
        child("H", &[(Status::Running, "2026-11-19T11:00:00Z")]),
    ];
    let mut cutoffs: Vec<String> = children
        .iter()
        .flat_map(|c| c.transitions.iter().map(|t| t.at.clone()))
        .collect();
    cutoffs.sort();
    let mut evaluated = BTreeSet::new();
    let mut fired = Vec::new();
    for cutoff in cutoffs {
        let slice: Vec<ChildFacts> = children
            .iter()
            .map(|c| ChildFacts {
                task_id: c.task_id.clone(),
                transitions: c
                    .transitions
                    .iter()
                    .filter(|t| t.at <= cutoff)
                    .cloned()
                    .collect(),
            })
            .collect();
        let f = TriggerFacts {
            evaluated: evaluated.clone(),
            ..facts(slice)
        };
        for key in due(&triggers(&f, &CheckpointSettings::default())) {
            fired.push((cutoff.clone(), key.clone()));
            evaluated.insert(key);
        }
    }
    assert_eq!(
        fired,
        vec![
            ("2026-11-17T21:55:57Z".to_string(), "children:2".to_string()),
            ("2026-11-18T20:39:25Z".to_string(), "children:4".to_string()),
            ("2026-11-19T04:54:54Z".to_string(), "children:6".to_string()),
        ]
    );
}

// ---------------------------------------------------------------- 評価の検査（D3.2・D4）

fn r(v: Value) -> Reference {
    serde_json::from_value(v).expect("reference")
}

fn task_ref(id: &str) -> Value {
    json!({"kind": "task", "task_id": id})
}

fn artifact_ref(task: &str, name: &str) -> Value {
    json!({"kind": "artifact", "task_id": task, "name": name, "sha256": "a".repeat(64)})
}

fn evaluation(verdict: &str, next: &str, axes: [&str; 3]) -> Value {
    json!({
        "schema": EVALUATION_SCHEMA,
        "verdict": verdict,
        "next": next,
        "axes": {"execution": axes[0], "epistemic": axes[1], "goal": axes[2]},
        "measured": [],
        "learned": [],
        "refuted": [],
        "uncertainty": [],
        "reason": "根拠は参照の task と成果物",
    })
}

fn measured(refs: Vec<Value>) -> Value {
    json!({"claim": "SCC の単位が減った", "metric": "task-core SCC", "before": "55", "after": "55", "references": refs})
}

fn learned(refs: Vec<Value>) -> Value {
    json!({"statement": "辺の除去だけでは SCC が減らない", "references": refs})
}

fn refuted(refs: Vec<Value>) -> Value {
    json!({"hypothesis": "h-cycle-edges", "statement": "D の後も 55 のまま", "references": refs})
}

fn passed(n: u32) -> ChildOutcomes {
    ChildOutcomes {
        done_passed: n,
        ..ChildOutcomes::default()
    }
}

fn ev_facts(resolved: Vec<Value>, goal_reached: bool, children: ChildOutcomes) -> EvaluationFacts {
    EvaluationFacts {
        resolved: resolved.into_iter().map(r).collect(),
        goal_reached,
        children,
    }
}

fn has_rule(c: &CheckedEvaluation, rule: Rule) -> bool {
    c.violations.iter().any(|v| v.rule == rule)
}

/// 不変条件: 参照の無い・解決しない改善の主張は unsupported に数え、そのような評価の有効な verdict は on_track に
/// ならない（R2・R3）。on_track・continue の組は unknown になり、R7 で next は ask_human になる。
#[test]
fn parent_checkpoint_contract_unsupported_not_on_track() {
    let mut e = evaluation("on_track", "continue", ["succeeded", "unknown", "unknown"]);
    e["measured"] = json!([measured(vec![]), measured(vec![task_ref("T-unresolved")])]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![], false, passed(2)));
    assert_eq!(c.status, RecordStatus::Checked);
    assert_eq!(c.effective.unsupported, 2);
    assert_eq!(c.effective.verdict, Verdict::Unknown);
    assert_eq!(c.effective.next, Next::AskHuman);
    assert!(has_rule(&c, Rule::Unsupported));
    assert!(has_rule(&c, Rule::OnTrackUnsupported));
    assert!(has_rule(&c, Rule::NextNotAllowed));
}

/// 不変条件: 外部の参照（`url`）・`knowledge`・`chat` だけの主張は、解決の事実に載っていても支持に数えない（untrusted）。
#[test]
fn parent_checkpoint_contract_url_only_is_unsupported() {
    let url = json!({"kind": "url", "url": "https://example.org/claim"});
    let knowledge = json!({"kind": "knowledge", "path": "projects/x.md", "sha256": "b".repeat(64)});
    let mut e = evaluation("on_track", "continue", ["succeeded", "advanced", "unknown"]);
    e["measured"] = json!([measured(vec![url.clone(), knowledge.clone()])]);
    e["learned"] = json!([learned(vec![url.clone()])]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![url, knowledge], false, passed(1)));
    assert_eq!(c.effective.unsupported, 2);
    assert_eq!(c.effective.verdict, Verdict::Unknown);
    assert_eq!(c.effective.axes.epistemic, EpistemicAxis::Unknown);
}

/// 不変条件: 解決した `task`・`artifact`・`commit` の参照を持つ測定が 1 件以上で unsupported が 0 のときだけ
/// on_track が有効。1 件でも参照の無い知見が混ざれば unknown に落ちる。
#[test]
fn parent_checkpoint_contract_on_track_with_resolved_measure() {
    let a = artifact_ref("T1", "scc.tsv");
    let mut e = evaluation("on_track", "continue", ["succeeded", "advanced", "unknown"]);
    e["measured"] = json!([measured(vec![a.clone()])]);
    e["learned"] = json!([learned(vec![task_ref("T1")])]);
    let f = ev_facts(vec![a.clone(), task_ref("T1")], false, passed(2));
    let c = check_evaluation(Some(&e), &f);
    assert_eq!(c.status, RecordStatus::Checked);
    assert_eq!(c.effective.unsupported, 0);
    assert_eq!(c.effective.verdict, Verdict::OnTrack);
    assert_eq!(c.effective.next, Next::Continue);
    assert_eq!(c.effective.axes.epistemic, EpistemicAxis::Advanced);
    assert!(c.violations.is_empty(), "{:?}", c.violations);

    e["learned"] = json!([learned(vec![task_ref("T1")]), learned(vec![])]);
    let c = check_evaluation(Some(&e), &f);
    assert_eq!(c.effective.unsupported, 1);
    assert_eq!(c.effective.verdict, Verdict::Unknown);
}

/// 不変条件: goal の `met` は Rust の D6（`goal_reached`）だけが決める。評価の自己申告の `met` は unknown にして
/// unsupported に数え、on_track も無効にする。D6 で到達していれば評価が何と書いても `met`。
#[test]
fn parent_checkpoint_contract_goal_met_needs_d6() {
    let a = artifact_ref("T1", "scc.tsv");
    let mut e = evaluation("on_track", "continue", ["succeeded", "unknown", "met"]);
    e["measured"] = json!([measured(vec![a.clone()])]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![a.clone()], false, passed(2)));
    assert_eq!(c.effective.axes.goal, GoalAxis::Unknown);
    assert_eq!(c.effective.unsupported, 1);
    assert_eq!(c.effective.verdict, Verdict::Unknown);
    assert!(has_rule(&c, Rule::GoalNotD6));

    for claimed in ["not_met", "unknown", "met"] {
        let mut e = evaluation("at_risk", "continue", ["succeeded", "unknown", claimed]);
        e["measured"] = json!([measured(vec![a.clone()])]);
        let c = check_evaluation(Some(&e), &ev_facts(vec![a.clone()], true, passed(2)));
        assert_eq!(c.effective.axes.goal, GoalAxis::Met, "claimed {claimed}");
    }
    let e = evaluation("off_track", "pivot", ["succeeded", "unknown", "not_met"]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![], false, passed(2)));
    assert_eq!(c.effective.axes.goal, GoalAxis::NotMet);
}

/// 不変条件: `axes.execution` は子の終端の事実で決まり、評価の値と違えば違反に記録する（判定は事実の側。R5）。
#[test]
fn parent_checkpoint_contract_execution_from_facts() {
    let cases = [
        (ChildOutcomes::default(), ExecutionAxis::Unknown),
        (passed(3), ExecutionAxis::Succeeded),
        (
            ChildOutcomes {
                done_passed: 2,
                failed: 1,
                ..ChildOutcomes::default()
            },
            ExecutionAxis::Failed,
        ),
        (
            ChildOutcomes {
                done_passed: 2,
                done_unpassed: 1,
                ..ChildOutcomes::default()
            },
            ExecutionAxis::Partial,
        ),
        (
            ChildOutcomes {
                done_passed: 2,
                cancelled: 1,
                ..ChildOutcomes::default()
            },
            ExecutionAxis::Partial,
        ),
    ];
    for (children, want) in cases {
        let e = evaluation("at_risk", "continue", ["succeeded", "unknown", "unknown"]);
        let c = check_evaluation(Some(&e), &ev_facts(vec![], false, children.clone()));
        assert_eq!(c.effective.axes.execution, want, "{children:?}");
        assert_eq!(
            has_rule(&c, Rule::ExecutionMismatch),
            want != ExecutionAxis::Succeeded,
            "{children:?}"
        );
        assert_eq!(c.status, RecordStatus::Checked);
    }
}

/// 不変条件: 負の研究結果は {execution: succeeded, epistemic: advanced, goal: not_met} と supported の反証で表し、
/// `negative_result` は事実と参照から導く（LLM の申告ではない）。反証の参照が解決しなければ負の結果にしない。
#[test]
fn parent_checkpoint_contract_negative_result_axes() {
    let a = artifact_ref("TD", "struct-deps.tsv");
    let mut e = evaluation("off_track", "pivot", ["succeeded", "advanced", "not_met"]);
    e["refuted"] = json!([refuted(vec![a.clone()])]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![a.clone()], false, passed(4)));
    assert_eq!(
        c.effective.axes,
        Axes {
            execution: ExecutionAxis::Succeeded,
            epistemic: EpistemicAxis::Advanced,
            goal: GoalAxis::NotMet,
        }
    );
    assert!(c.effective.negative_result);
    assert_eq!(c.effective.verdict, Verdict::OffTrack);
    assert_eq!(c.effective.next, Next::Pivot);

    let c = check_evaluation(Some(&e), &ev_facts(vec![], false, passed(4)));
    assert!(!c.effective.negative_result);
    assert_eq!(c.effective.axes.epistemic, EpistemicAxis::Unknown);
}

/// 不変条件: `epistemic = advanced` は supported の知見・反証があるときだけ。無ければ申告の advanced は unknown、
/// 申告の none はそのまま（R6）。
#[test]
fn parent_checkpoint_contract_epistemic_advanced_needs_support() {
    let e = evaluation("at_risk", "continue", ["succeeded", "advanced", "unknown"]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![], false, passed(1)));
    assert_eq!(c.effective.axes.epistemic, EpistemicAxis::Unknown);
    assert!(has_rule(&c, Rule::EpistemicMismatch));
    let e = evaluation("at_risk", "continue", ["succeeded", "none", "unknown"]);
    let c = check_evaluation(Some(&e), &ev_facts(vec![], false, passed(1)));
    assert_eq!(c.effective.axes.epistemic, EpistemicAxis::None);
}

/// 不変条件: 節が無い・schema が違う・未知の欄・未知の値・空の主張は `invalid` で、有効な判定は unknown・ask_human。
#[test]
fn parent_checkpoint_contract_shape_invalid() {
    let f = ev_facts(vec![], false, passed(2));
    let c = check_evaluation(None, &f);
    assert_eq!(c.status, RecordStatus::Invalid);
    assert_eq!(c.effective.verdict, Verdict::Unknown);
    assert_eq!(c.effective.next, Next::AskHuman);
    assert_eq!(c.effective.axes.execution, ExecutionAxis::Succeeded);

    let mut cases = Vec::new();
    let mut v = evaluation("on_track", "continue", ["succeeded", "none", "unknown"]);
    v["schema"] = json!("celeris.parent-evaluation/2");
    cases.push(v);
    let mut v = evaluation("on_track", "continue", ["succeeded", "none", "unknown"]);
    v["achieved"] = json!(true);
    cases.push(v);
    cases.push(evaluation(
        "great",
        "continue",
        ["succeeded", "none", "unknown"],
    ));
    cases.push(evaluation(
        "at_risk",
        "publish",
        ["succeeded", "none", "unknown"],
    ));
    let mut v = evaluation("at_risk", "continue", ["succeeded", "none", "unknown"]);
    v["measured"] =
        json!([{"claim": "", "metric": "m", "before": "1", "after": "2", "references": []}]);
    cases.push(v);
    let mut v = evaluation("at_risk", "continue", ["succeeded", "none", "unknown"]);
    v["reason"] = json!("");
    cases.push(v);
    for v in cases {
        let c = check_evaluation(Some(&v), &f);
        assert_eq!(c.status, RecordStatus::Invalid, "{v}");
        assert!(has_rule(&c, Rule::Shape), "{v}: {:?}", c.violations);
        assert_eq!(c.effective.verdict, Verdict::Unknown);
        assert_eq!(c.effective.next, Next::AskHuman);
    }
}

/// 不変条件: 入力長の上限（各列 16 件、文 2,000 文字、metric・before・after 200 文字、参照 16 件、全体の byte 数）を
/// 超えた評価は `invalid`。文字数は byte でなく文字で数える（日本語の 2,000 文字は通る）。
#[test]
fn parent_checkpoint_contract_input_limits() {
    let a = artifact_ref("T1", "scc.tsv");
    let f = ev_facts(vec![a.clone()], false, passed(2));
    let base = || evaluation("at_risk", "continue", ["succeeded", "unknown", "unknown"]);

    let mut ok = base();
    ok["uncertainty"] = json!(vec![
        json!({"statement": "あ".repeat(TEXT_MAX_CHARS), "resolve_by": "い".repeat(TEXT_MAX_CHARS)});
        LIST_MAX
    ]);
    ok["measured"] = json!([{"claim": "c", "metric": "m".repeat(SHORT_MAX_CHARS), "before": "b", "after": "a", "references": vec![a.clone(); REFS_PER_ITEM_MAX]}]);
    let c = check_evaluation(Some(&ok), &f);
    assert_eq!(c.status, RecordStatus::Checked, "{:?}", c.violations);

    let mut over = Vec::new();
    let mut v = base();
    v["learned"] = json!(vec![learned(vec![a.clone()]); LIST_MAX + 1]);
    over.push(v);
    let mut v = base();
    v["uncertainty"] = json!([{"statement": "あ".repeat(TEXT_MAX_CHARS + 1), "resolve_by": "x"}]);
    over.push(v);
    let mut v = base();
    v["measured"] = json!([{"claim": "c", "metric": "m".repeat(SHORT_MAX_CHARS + 1), "before": "b", "after": "a", "references": [a.clone()]}]);
    over.push(v);
    let mut v = base();
    v["refuted"] = json!([refuted(vec![a.clone(); REFS_PER_ITEM_MAX + 1])]);
    over.push(v);
    let mut v = base();
    v["reason"] = json!("r".repeat(TEXT_MAX_CHARS + 1));
    over.push(v);
    for v in over {
        let c = check_evaluation(Some(&v), &f);
        assert_eq!(c.status, RecordStatus::Invalid);
        assert!(has_rule(&c, Rule::Limit), "{:?}", c.violations);
    }

    let mut huge = base();
    huge["reason"] = json!("x".repeat(EVALUATION_MAX_BYTES));
    let c = check_evaluation(Some(&huge), &f);
    assert_eq!(c.status, RecordStatus::Invalid);
    assert_eq!(
        c.violations.len(),
        1,
        "byte の上限は形の検査より先: {:?}",
        c.violations
    );
    assert_eq!(c.violations[0].rule, Rule::Limit);
}

/// 不変条件: 呼び出し側が引くべき参照は `task`・`artifact`・`commit` だけで、重複しない（`url` などは引かない）。
#[test]
fn parent_checkpoint_contract_references_to_resolve() {
    let a = artifact_ref("T1", "scc.tsv");
    let commit = json!({"kind": "commit", "repo": "agent-platform", "sha": "4".repeat(40)});
    let mut e = evaluation("at_risk", "continue", ["succeeded", "unknown", "unknown"]);
    e["measured"] = json!([measured(vec![
        a.clone(),
        json!({"kind": "url", "url": "https://example.org"})
    ])]);
    e["learned"] = json!([learned(vec![a.clone(), task_ref("T1")])]);
    e["refuted"] = json!([refuted(vec![commit.clone()])]);
    assert_eq!(
        references_to_resolve(&e),
        vec![r(a), r(task_ref("T1")), r(commit)]
    );
    assert!(references_to_resolve(&json!({"schema": "x"})).is_empty());
}

// ---------------------------------------------------------------- 写しの表（D5.2）と記録（D3.4）

fn effective(verdict: Verdict, next: Next) -> Effective {
    Effective {
        verdict,
        next,
        axes: Axes {
            execution: ExecutionAxis::Succeeded,
            epistemic: EpistemicAxis::Unknown,
            goal: GoalAxis::Unknown,
        },
        unsupported: 0,
        negative_result: false,
    }
}

/// 不変条件: 写していない評価に対して許す行動は D5.2 の表だけ。off_track・unknown から continue は出ず、replan の
/// reason_code は next に対応する 1 つ、escalate の question_key は `checkpoint:<判断 id>`。invalid の記録は escalate だけ。
#[test]
fn parent_checkpoint_contract_allowed_actions_table() {
    use AllowedAction::*;
    let rows = [
        (Verdict::OnTrack, Next::Continue, vec![Continue, Wait]),
        (Verdict::AtRisk, Next::Continue, vec![Continue, Wait]),
        (Verdict::OffTrack, Next::Continue, vec![Escalate]),
        (Verdict::Unknown, Next::Continue, vec![Escalate]),
        (
            Verdict::OnTrack,
            Next::DiscriminatingExperiment,
            vec![
                Replan {
                    reason_code: REASON_EXPERIMENT,
                },
                Escalate,
            ],
        ),
        (
            Verdict::OffTrack,
            Next::Pivot,
            vec![
                Replan {
                    reason_code: REASON_PIVOT,
                },
                Escalate,
            ],
        ),
        (Verdict::AtRisk, Next::AskHuman, vec![Escalate]),
    ];
    for (verdict, next, want) in rows {
        assert_eq!(
            allowed_actions(RecordStatus::Checked, &effective(verdict, next)),
            want,
            "{verdict:?} {next:?}"
        );
    }
    assert_eq!(
        allowed_actions(
            RecordStatus::Invalid,
            &effective(Verdict::OnTrack, Next::Continue)
        ),
        vec![Escalate]
    );

    let pivot = allowed_actions(
        RecordStatus::Checked,
        &effective(Verdict::OffTrack, Next::Pivot),
    );
    let key = checkpoint_question_key("D9");
    assert_eq!(key, "checkpoint:D9");
    assert!(action_allowed(
        &pivot,
        "D9",
        "replan",
        Some(REASON_PIVOT),
        None
    ));
    assert!(!action_allowed(
        &pivot,
        "D9",
        "replan",
        Some(REASON_EXPERIMENT),
        None
    ));
    assert!(!action_allowed(&pivot, "D9", "continue", None, None));
    assert!(!action_allowed(&pivot, "D9", "launch_root", None, None));
    assert!(action_allowed(&pivot, "D9", "escalate", None, Some(&key)));
    assert!(!action_allowed(
        &pivot,
        "D9",
        "escalate",
        None,
        Some("no_progress:3")
    ));
    let cont = allowed_actions(
        RecordStatus::Checked,
        &effective(Verdict::AtRisk, Next::Continue),
    );
    assert!(action_allowed(&cont, "D9", "wait", None, None));
    assert!(!action_allowed(&cont, "D9", "mark_milestone", None, None));
}

/// 不変条件: 表の行動の kind は `ProjectAction::kind()` と同じ文字列（controller の検証が同じ名前で照合できる）。
#[test]
fn parent_checkpoint_contract_action_kinds_match_project_action() {
    let actions = [
        (
            AllowedAction::Continue,
            ProjectAction::Continue {
                watch_root_ids: vec![],
                next_wake_after_secs: 0,
            },
        ),
        (
            AllowedAction::Wait,
            ProjectAction::Wait {
                waiting_on: vec![],
                next_wake_after_secs: 0,
                phase: WaitPhase::Waiting,
            },
        ),
        (
            AllowedAction::Replan {
                reason_code: REASON_PIVOT,
            },
            ProjectAction::Replan {
                reason_code: REASON_PIVOT.into(),
                question: "q".into(),
                context_refs: vec![],
            },
        ),
        (
            AllowedAction::Escalate,
            ProjectAction::Escalate {
                category: "project_controller".into(),
                question_key: checkpoint_question_key("D1"),
                summary: "s".into(),
                options: vec![],
                evidence_refs: vec![],
            },
        ),
    ];
    for (allowed, action) in actions {
        assert_eq!(allowed.kind(), action.kind());
    }
}

/// 不変条件: 評価の成果物の name は `parent-evaluation/<判断 id>.json` の 1 つの定義から作り・読み、記録は
/// `celeris.parent-evaluation-record/1` の形で書いて読み戻せる。記録は trigger の一覧を持たない（D3.4。trigger の正本は判断の行）。
#[test]
fn parent_checkpoint_contract_record_roundtrip() {
    assert_eq!(evaluation_artifact_name("D1"), "parent-evaluation/D1.json");
    assert_eq!(
        evaluation_decision_id("parent-evaluation/D1.json"),
        Some("D1")
    );
    for bad in [
        "parent-evaluation/.json",
        "parent-evaluation/a/b.json",
        "parent-evaluation/D1.md",
        "strategy/x/v1.json",
    ] {
        assert_eq!(evaluation_decision_id(bad), None, "{bad}");
    }

    let a = artifact_ref("T1", "scc.tsv");
    let mut e = evaluation("on_track", "continue", ["succeeded", "unknown", "unknown"]);
    e["measured"] = json!([measured(vec![a.clone()])]);
    let checked = check_evaluation(Some(&e), &ev_facts(vec![a], false, passed(2)));
    let record = ParentEvaluationRecord::new("D1", "ROOT", Some(e.clone()), &checked);
    let v = serde_json::to_value(&record).expect("record");
    assert_eq!(v["schema"], json!(RECORD_SCHEMA));
    assert_eq!(v["status"], json!("checked"));
    assert!(v.get("triggers").is_none(), "記録が trigger の一覧を持った");
    assert_eq!(v["evaluation"], e);
    assert_eq!(v["effective"]["verdict"], json!("on_track"));
    assert_eq!(v["effective"]["axes"]["execution"], json!("succeeded"));
    let back: ParentEvaluationRecord = serde_json::from_value(v).expect("read back");
    assert_eq!(back, record);

    let invalid = check_evaluation(None, &ev_facts(vec![], false, passed(2)));
    let record = ParentEvaluationRecord::new("D2", "ROOT", None, &invalid);
    let v = serde_json::to_value(&record).expect("record");
    assert_eq!(v["status"], json!("invalid"));
    assert_eq!(v["evaluation"], Value::Null);
    assert_eq!(v["effective"]["next"], json!("ask_human"));
    assert!(!record.violations.is_empty());
}

/// 不変条件: rubric は E1〜E6 の固定の定数で、E6 は自己申告を証拠に数えないことと到達を `reached` だけが決めることを言う。
#[test]
fn parent_checkpoint_contract_rubric_fixed() {
    let ids: Vec<&str> = PARENT_EVALUATION_RUBRIC.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec!["E1", "E2", "E3", "E4", "E5", "E6"]);
    assert!(
        PARENT_EVALUATION_RUBRIC
            .iter()
            .all(|(_, text)| !text.is_empty())
    );
    assert!(PARENT_EVALUATION_RUBRIC[5].1.contains("reached"));
}

// ---------------------------------------------------------------- 状態（D1.3・D2.3）

fn decision(id: &str, triggers: &[&str], effect: DecisionEffect) -> EvaluateDecision {
    EvaluateDecision {
        id: id.into(),
        triggers: triggers.iter().map(|t| (*t).to_string()).collect(),
        effect,
    }
}

/// 不変条件: 試行の記録の name は `parent-evaluation/<判断 id>.attempt-<n>.json`（n ≥ 1）の 1 つの定義から作り・読み、
/// 最終の記録の name と取り違えない（D2.3・D3.4）。
#[test]
fn parent_checkpoint_contract_attempt_name_roundtrip() {
    assert_eq!(
        attempt_artifact_name("D1", 2),
        "parent-evaluation/D1.attempt-2.json"
    );
    assert_eq!(
        attempt_of("parent-evaluation/D1.attempt-2.json"),
        Some(("D1", 2))
    );
    for bad in [
        "parent-evaluation/D1.json",
        "parent-evaluation/D1.attempt-0.json",
        "parent-evaluation/D1.attempt-x.json",
        "parent-evaluation/D1.attempt-02.json",
        "parent-evaluation/a/b.attempt-1.json",
        "parent-evaluation/.attempt-1.json",
    ] {
        assert_eq!(attempt_of(bad), None, "{bad}");
    }
    assert_eq!(
        evaluation_decision_id("parent-evaluation/D1.attempt-2.json"),
        None,
        "試行の記録を最終の記録と読んだ"
    );
}

/// 不変条件（D1.3）: 状態は判断の行（id・triggers・effect）と root の成果物の name だけから導く。`suppressed` は発火済みに
/// 数えるが実行中にしない、最終の記録がある判断は評価済み、それ以外の効果の判断は発火済みに数えない、試行は同じ n を 1 回に数える。
/// base には状態を導く関数が無く、呼び出し側が記録の中身から evaluated を集めていた（二重の正本）。
#[test]
fn parent_checkpoint_contract_states_from_decisions_and_names() {
    let decisions = vec![
        decision("D1", &["children:2"], DecisionEffect::Applied),
        decision("D2", &["budget:50"], DecisionEffect::Suppressed),
        decision("D3", &["child_failed:T9"], DecisionEffect::Other),
        decision("D4", &["children:4", "budget:80"], DecisionEffect::Pending),
    ];
    let names = [
        "parent-evaluation/D1.attempt-1.json",
        "parent-evaluation/D1.json",
        "parent-evaluation/D4.attempt-1.json",
        "parent-evaluation/D4.attempt-1.json",
        "parent-evaluation/D4.attempt-2.json",
        "strategy/x/v1.json",
    ];
    let st = checkpoint_states(&decisions, &names);
    let set = |xs: &[&str]| xs.iter().map(|x| (*x).to_string()).collect::<BTreeSet<_>>();
    assert_eq!(
        st.fired,
        set(&["budget:50", "budget:80", "children:2", "children:4"])
    );
    assert_eq!(st.evaluated, set(&["children:2"]));
    assert_eq!(
        st.requested(),
        set(&["budget:50", "budget:80", "children:4"])
    );
    assert_eq!(st.running, vec!["D4".to_string()]);
    assert_eq!(st.evaluated_decisions, vec!["D1".to_string()]);
    assert_eq!(st.attempts("D4"), 2);
    assert_eq!(st.attempts("D1"), 1);
    assert_eq!(st.attempts("D2"), 0);
}

/// 不変条件（D1.3）: shadow の `suppressed` の後、同じ trigger は due にならず（2 回目の判断を作らない）、実行中にもならない。
#[test]
fn parent_checkpoint_contract_suppressed_not_due_not_running() {
    let decisions = vec![decision("D1", &["children:2"], DecisionEffect::Suppressed)];
    let st = checkpoint_states(&decisions, &[]);
    assert!(st.running.is_empty(), "suppressed を実行中に数えた");
    let mut f = TriggerFacts {
        strategy: strategy(Vec::new()),
        children: vec![
            done("C1", "2026-11-18T01:00:00Z"),
            done("C2", "2026-11-18T02:00:00Z"),
        ],
        tokens_used: 0,
        max_tokens: None,
        evaluated: BTreeSet::new(),
        requested: BTreeSet::new(),
    };
    st.fill(&mut f);
    assert!(due(&triggers(&f, &CheckpointSettings::default())).is_empty());
}

// ---- D3.1.1 子の報告の本文（`child_reports`）

fn finished(outcome: &str, role: Option<crate::RunRole>) -> crate::Event {
    crate::Event::WorkerFinished {
        run_id: "r".to_string(),
        outcome: outcome.to_string(),
        usage: None,
        role,
        metrics: None,
        end: None,
    }
}

/// 守る不変条件: 評価器に渡す子の報告は checkpoint の時点以前の worker・reviewer の run の本文だけで、1 件・子ごと・合計の
/// 上限を超えない。後ろの子の最新の報告は前の子の古い報告で落ちない。既存の試験は報告の欄を持たないので検出できない。
#[test]
fn parent_checkpoint_blind_child_reports_cutoff_and_caps() {
    use crate::RunRole;
    let long = "あ".repeat(REPORT_MAX_BYTES); // 3 byte × 2,000 字
    let worker = finished(&long, None);
    let reviewer = finished("合格", Some(RunRole::Reviewer));
    let planner = finished("計画", Some(RunRole::Planner));
    let later = finished("時点より後", None);
    let a: Vec<(u64, &str, &crate::Event)> = (0..6)
        .map(|i| (i, "2026-11-18T10:00:00Z", &worker))
        .chain([
            (6, "2026-11-18T11:00:00Z", &reviewer),
            (7, "2026-11-18T11:30:00Z", &planner),
            (8, "not a time", &reviewer),
            (9, "2026-11-18T20:00:00.000000001Z", &later),
        ])
        .collect();
    let b: Vec<(u64, &str, &crate::Event)> = vec![(3, "2026-11-18T09:00:00Z", &reviewer)];
    let cutoff = "2026-11-18T20:00:00Z";
    let out = child_reports(&[a.clone(), b.clone()], cutoff);
    // 時点より後・時刻の読めない・planner は入らない。子ごとに新しい順に 4 件（seq の昇順で返す）。
    let seqs: Vec<u64> = out[0].iter().map(|r| r.seq).collect();
    assert!(!seqs.contains(&9) && !seqs.contains(&8) && !seqs.contains(&7));
    assert!(out[0].len() <= REPORTS_PER_CHILD_MAX);
    assert_eq!(
        out[0].last().map(|r| (r.seq, r.role)),
        Some((6, ReportRole::Reviewer))
    );
    // 1 件の上限: char の境で切り、truncated を立てる。
    for r in out[0].iter().filter(|r| r.role == ReportRole::Worker) {
        assert!(
            r.truncated && r.text.len() <= REPORT_MAX_BYTES && r.text.len() > REPORT_MAX_BYTES - 3
        );
    }
    // 合計の上限と、後ろの子の最新の報告が残ること。
    let total: usize = out.iter().flatten().map(|r| r.text.len()).sum();
    assert!(total <= REPORTS_MAX_BYTES, "{total}");
    assert_eq!(out[1].iter().map(|r| r.seq).collect::<Vec<_>>(), vec![3]);
    let many: Vec<Vec<(u64, &str, &crate::Event)>> = vec![a; 12];
    let capped = child_reports(&many, cutoff);
    let total: usize = capped.iter().flatten().map(|r| r.text.len()).sum();
    assert!(total <= REPORTS_MAX_BYTES, "{total}");
    assert!(
        capped.iter().all(|c| !c.is_empty()),
        "全部の子の最新の 1 件が先に入る"
    );
    // 時点の読めない判断は報告を入れない。
    assert!(child_reports(&[b], "yesterday").iter().all(Vec::is_empty));
}
