//! ADR 2026-11-27-r1-stall-shadow の R1（[`stall`]）の試験（prefix `parent_checkpoint_stall_`）。
//!
//! base には [`stall`] が無く、R1 の規則は Python（`agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py`・
//! `r1_bench.py`）にしか無い。既存の `parent_checkpoint_contract_` の試験は trigger（子 N 件ごと・失敗・反証・予算）だけを
//! 見るので、ここで守る不変条件（K = 3 の境界、Outcome の到達で連続が 0 に戻る、strategy root 以外は値を出さない、
//! Python の記録と同じ発火）が壊れても検出できない。

use std::collections::BTreeSet;

use super::*;
use crate::Status;

fn done(id: &str, at: &str) -> ChildFacts {
    ChildFacts {
        task_id: id.into(),
        transitions: vec![
            ChildTransition {
                to: Status::Running,
                at: at.into(),
            },
            ChildTransition {
                to: Status::Done,
                at: at.into(),
            },
        ],
    }
}

fn running(id: &str, at: &str) -> ChildFacts {
    ChildFacts {
        task_id: id.into(),
        transitions: vec![ChildTransition {
            to: Status::Running,
            at: at.into(),
        }],
    }
}

fn root(children: Vec<ChildFacts>) -> TriggerFacts {
    TriggerFacts {
        strategy: Some(StrategyFacts {
            strategy_id: "s1".into(),
            version: 1,
            documents: Vec::new(),
        }),
        children,
        tokens_used: 0,
        max_tokens: None,
        evaluated: BTreeSet::new(),
        requested: BTreeSet::new(),
    }
}

/// `n` 件の終端の子（event の順は 1..=n の 20 桁）。
fn terminal(n: usize) -> Vec<ChildFacts> {
    (1..=n)
        .map(|i| done(&format!("c{i}"), &format!("{i:020}")))
        .collect()
}

fn reach(required: usize, reached_at: &[&str]) -> OutcomeReach {
    OutcomeReach {
        required,
        reached_at: reached_at.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// 不変条件: escalate は終端の子と連続がともに K（= 3）以上で初めて真になり、`at` は K 個目の終端の順。
/// 終端でない子は数えない。
#[test]
fn parent_checkpoint_stall_k_boundary() {
    assert_eq!(STALL_K, 3);
    let mut children = terminal(2);
    children.push(running("r", &format!("{:020}", 9)));
    let s = stall(&root(children.clone()), &reach(1, &[])).expect("strategy root");
    assert_eq!((s.terminal_children, s.no_progress_streak), (2, 2));
    assert!(!s.escalate);
    assert_eq!(s.at, None);

    children.push(done("c3", &format!("{:020}", 3)));
    let s = stall(&root(children), &reach(1, &[])).expect("strategy root");
    assert_eq!((s.terminal_children, s.no_progress_streak), (3, 3));
    assert!(s.escalate);
    assert_eq!(s.at.as_deref(), Some(format!("{:020}", 3).as_str()));
}

/// 不変条件: Outcome の到達の数が増えると、それより前に終端になった子は連続から外れる（到達の直後の連続は 0）。
/// 到達より後の終端が K 件たまれば、Outcome が全部は到達していない限り再び発火する。全部到達なら発火しない。
#[test]
fn parent_checkpoint_stall_reach_resets_streak() {
    let third = format!("{:020}", 3);
    // 子 3 件の終端の後に 2 件中 1 件の Outcome が到達。
    let s = stall(&root(terminal(3)), &reach(2, &[&third])).expect("strategy root");
    assert_eq!((s.terminal_children, s.no_progress_streak), (3, 0));
    assert!(!s.outcome_reached);
    assert!(!s.escalate);

    // 到達の後に 2 件: 連続 2 で発火しない。
    let s = stall(&root(terminal(5)), &reach(2, &[&third])).expect("strategy root");
    assert_eq!((s.terminal_children, s.no_progress_streak), (5, 2));
    assert!(!s.escalate);

    // 到達の後に 3 件: 連続 3 で発火し、`at` は到達の後の 3 件目（全体の 6 件目）。
    let s = stall(&root(terminal(6)), &reach(2, &[&third])).expect("strategy root");
    assert_eq!((s.terminal_children, s.no_progress_streak), (6, 3));
    assert!(s.escalate);
    assert_eq!(s.at.as_deref(), Some(format!("{:020}", 6).as_str()));

    // 最後の到達（2 件目）より後だけを数える。空文字列の到達（終端の子より前の到達）は全部を後に数える。
    let fifth = format!("{:020}", 5);
    let s = stall(&root(terminal(6)), &reach(3, &["", &fifth])).expect("strategy root");
    assert_eq!(s.no_progress_streak, 1);
    let s = stall(&root(terminal(6)), &reach(3, &[""])).expect("strategy root");
    assert_eq!(s.no_progress_streak, 6);
    assert!(s.escalate);

    // 全部の Outcome が到達: 連続が K 以上でも発火しない。Outcome 0 件は未到達。
    let s = stall(&root(terminal(6)), &reach(1, &[&third])).expect("strategy root");
    assert!(s.outcome_reached);
    assert_eq!(s.no_progress_streak, 3);
    assert!(!s.escalate);
    let s = stall(&root(terminal(3)), &reach(0, &[])).expect("strategy root");
    assert!(!s.outcome_reached);
    assert!(s.escalate);
}

/// 不変条件: strategy root でない root（`strategy` が `None`）は子が何件終端でも値を出さない（triggers と同じ範囲）。
#[test]
fn parent_checkpoint_stall_non_strategy_root_is_none() {
    let mut facts = root(terminal(7));
    facts.strategy = None;
    assert_eq!(stall(&facts, &reach(1, &[])), None);
    assert_eq!(stall(&facts, &reach(0, &[])), None);
}

/// 不変条件: r1-replay の 19 点（子 0 の replay 12 slice と研究ケース 7 slice）で、Rust の K = 3 の発火が Python の記録と
/// 一致する。
///
/// 値の出典: `agent-docs/progress/2026-11-27-strategy-v1/r1-result.json`（schema `celeris.r1-escalate-result/1`）の
/// `replay.points[]`・`research.points[]` の `id`・`outcomes`・`outcomes_reached`・`terminal_children`・
/// `no_progress_streak`・`escalate["3"]`。19 点とも到達の記録が無い（`outcomes_reached` 0）ので連続は終端の子の数と同じ。
#[test]
fn parent_checkpoint_stall_r1_result_points_match() {
    // (id, outcomes, terminal_children, no_progress_streak, escalate["3"])
    const POINTS: [(&str, usize, u32, u32, bool); 19] = [
        ("A-mid", 1, 0, 0, false),
        ("A-done", 1, 1, 1, false),
        ("C-mid", 1, 2, 2, false),
        ("C-done", 1, 3, 3, true),
        ("D-mid", 1, 3, 3, true),
        ("E-mid", 1, 3, 3, true),
        ("D-done", 1, 4, 4, true),
        ("E-done", 1, 5, 5, true),
        ("F-mid", 1, 5, 5, true),
        ("F-done", 1, 6, 6, true),
        ("G-mid", 1, 6, 6, true),
        ("G-done", 1, 7, 7, true),
        ("11.15", 0, 1, 1, false),
        ("11.16", 0, 2, 2, false),
        ("11.17", 0, 3, 3, true),
        ("11.18", 0, 4, 4, true),
        ("11.19", 0, 5, 5, true),
        ("11.20", 0, 6, 6, true),
        ("11.21", 0, 7, 7, true),
    ];
    for (id, outcomes, terminal_children, streak, fired) in POINTS {
        let s = stall(
            &root(terminal(terminal_children as usize)),
            &reach(outcomes, &[]),
        )
        .expect("strategy root");
        assert_eq!(s.terminal_children, terminal_children, "{id}");
        assert_eq!(s.no_progress_streak, streak, "{id}");
        assert!(!s.outcome_reached, "{id}");
        assert_eq!(s.escalate, fired, "{id}");
    }
}

/// 不変条件: r1-bench の trial の終わりの状態で、Rust の発火・非発火が記録と一致する。
///
/// 値の出典: `agent-docs/progress/2026-11-27-strategy-v1/r1-bench.json`（schema `celeris.r1-bench-result/1`）の
/// `trials[]` の `trial`・`evidence.terminal_roots`・`evidence.achieved_at`・`fired`。r1-bench は 44 trial を全部除外した
/// （root の終端と Outcome ごとの到達の時刻が facts に無い。`inconclusive: true`）ので、点ごとの比較はできない。
/// ここで比べるのは trial の終わりの状態だけ: `achieved_at` があれば必要な Outcome が全部到達（Outcome 1 件・到達 1 件で
/// 表す）、無ければ未到達。記録の `fired` は全 trial で空（非発火）。
/// 一致するのは 44 件中 42 件（下の 5 件はその例）。残る 2 件（a9 の C08 の new、終端の root 4・`achieved_at` 無し）は
/// Rust が発火し、記録は除外のため空で、一致しない（下で別に確かめる。ADR の「根拠」に書いた）。
#[test]
fn parent_checkpoint_stall_r1_bench_trials_match() {
    // (trial, terminal_roots, achieved_at がある, 記録の発火)
    const MATCHED: [(&str, usize, bool, bool); 5] = [
        ("095c3dec279f5c10", 3, true, false),  // a8b/replay new C04
        ("8aedb500651f5788", 4, true, false),  // a8b/replay new C08
        ("4db780ad3b129a1a", 4, true, false),  // a8b/replay old.proactive C10
        ("948a48ff0789a75f", 0, false, false), // a8b/replay ctl.null C01（root 0）
        ("49a77b52eb2fcefa", 2, false, false), // a9/llm new C06
    ];
    for (trial, roots, achieved, fired) in MATCHED {
        let r = if achieved {
            reach(1, &[""])
        } else {
            reach(1, &[])
        };
        let s = stall(&root(terminal(roots)), &r).expect("strategy root");
        assert_eq!(s.escalate, fired, "{trial}");
    }
    // a9/llm・a9/stub の new C08（trial 7420dfe789f99af7）: 終端の root 4・achieved_at 無し。記録は除外で空。
    let s = stall(&root(terminal(4)), &reach(1, &[])).expect("strategy root");
    assert!(s.escalate);
}
