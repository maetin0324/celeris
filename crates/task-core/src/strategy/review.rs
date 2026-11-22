//! ADR 2026-11-22-strategy-review-critic D1・D2・D4・D5: 戦略の候補の識別実験・期待情報価値・停止条件・保留する前提と、
//! 却下した候補・実験段階・Critic・失敗の記録の規則（純関数。store を読まない）。
//!
//! 文書の規則（D1・D4.1 の鎖と `selected`）は [`super::validate_document`] の検査の表に入り、Record・Execute の両方に掛かる。
//! 実行の入口の規則（D2.4・D3.7・D4.1 の `candidate`・D4.2）は [`check_execution`] で、呼び出し側が store から集めた
//! [`ExecutionFacts`] を受ける。段階は文書から導き、保存しない（D2.1）。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    Input, LIST_ITEM_MAX_CHARS, Reference, Rule, Stance, StrategyDocument, TEXT_MAX_CHARS,
    Violation, is_id, limit_count, limit_text, push,
};
use crate::ExecutionMode;

/// D1.3: `open` の候補の上限と、`selected` の無い版（比べる代替案）の下限。
pub const OPEN_MAX: usize = 3;
pub const OPEN_MIN_WITHOUT_SELECTED: usize = 2;
pub const REJECTED_BY_MAX: usize = 16;
pub const DISCRIMINATES_MAX: usize = 8;
pub const STOP_WHEN_MAX: usize = 8;
pub const HELD_ASSUMPTIONS_MAX: usize = 16;

/// D1.1: 候補の採否（文書の承認の状態とは別）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Open,
    Rejected,
}

/// D1.1: 最小の識別実験。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub content: String,
    /// 区別する仮説の id。
    pub discriminates: Vec<String>,
    pub budget: ExperimentBudget,
}

/// 実験の予算（候補の `cost` 以下）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentBudget {
    pub usd_max: f64,
    pub wall_hours_max: f64,
}

/// D1.2: 保留する前提（今は確かめずに置く前提と、確かめに戻る条件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldAssumption {
    pub id: String,
    pub statement: String,
    pub revisit_when: String,
}

/// D2.3: 鎖の版の実験 task（`cos_operations` の `task.create`・`applied` の行の `target_id`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentTask {
    /// 起票が参照した版。
    pub version: u32,
    pub task_id: String,
    /// 起票の `strategy.candidate`。
    pub candidate: Option<String>,
    /// gate の時点で `done`・`failed`・`cancelled`。
    pub terminal: bool,
}

/// D5: 実行の入口の事実（呼び出し側が store から集める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFacts {
    /// 文書を作った task（`strategy.task_id`）の status が `done`。
    pub source_done: bool,
    /// 参照した `(name, sha256)` が文書を作った task の Critic の対象にある。
    pub critiqued: bool,
    /// 参照した版と鎖の前の版の実験 task の全件。
    pub experiments: Vec<ExperimentTask>,
    /// 起票の本文の `execution`（省略は `None`）。
    pub execution: Option<ExecutionMode>,
    /// 起票の `strategy.candidate`。
    pub candidate: Option<String>,
    /// 起票の本文の `max_wall_secs`。
    pub max_wall_secs: Option<u64>,
}

/// D2.1: `selected` の候補が無い版は実験段階、ある版は実行段階。
pub fn experiment_phase(doc: &StrategyDocument) -> bool {
    !doc.candidates.iter().any(|c| c.selected)
}

/// D1.1・D1.3 の参照と予算の形（[`Rule::Shape`]）。
pub(super) fn check_candidate_shape(input: &Input<'_>, out: &mut Vec<Violation>) {
    let doc = input.doc;
    let has_hypothesis = |id: &String| doc.hypotheses.iter().any(|h| &h.id == id);
    for (i, c) in doc.candidates.iter().enumerate() {
        let at = |k: &str| format!("candidates[{i}].{k}");
        let refutes = |id: &String| {
            doc.evidence.iter().any(|e| {
                &e.id == id && e.stance == Stance::Refute && c.hypotheses.contains(&e.hypothesis)
            })
        };
        match c.state {
            CandidateState::Open if !c.rejected_by.is_empty() => push(
                out,
                Rule::Shape,
                at("rejected_by"),
                "open の候補に rejected_by がある",
            ),
            CandidateState::Rejected if !c.rejected_by.iter().all(refutes) => push(
                out,
                Rule::Shape,
                at("rejected_by"),
                "rejected_by が、候補の仮説に対する同じ文書の refute の証拠でない",
            ),
            _ => {}
        }
        let ex = &c.experiment;
        if !ex.discriminates.iter().all(has_hypothesis) {
            push(
                out,
                Rule::Shape,
                at("experiment.discriminates"),
                "無い仮説を指す",
            );
        }
        if !ex.discriminates.is_empty()
            && !ex.discriminates.iter().any(|h| c.hypotheses.contains(h))
        {
            push(
                out,
                Rule::Shape,
                at("experiment.discriminates"),
                "候補の仮説を 1 件も区別しない",
            );
        }
        let b = &ex.budget;
        let usd_ok = b.usd_max.is_finite() && b.usd_max >= 0.0 && b.usd_max <= c.cost.usd_max;
        let wall_ok = b.wall_hours_max.is_finite()
            && b.wall_hours_max > 0.0
            && b.wall_hours_max <= c.cost.wall_hours_max;
        if !(usd_ok && wall_ok) {
            push(
                out,
                Rule::Shape,
                at("experiment.budget"),
                "実験の予算が候補の cost の内でない（0 ≤ usd_max ≤ cost.usd_max、0 < wall_hours_max ≤ cost.wall_hours_max）",
            );
        }
    }
    let mut seen = BTreeSet::new();
    for (i, a) in doc.held_assumptions.iter().enumerate() {
        if !is_id(&a.id) || !seen.insert(a.id.as_str()) {
            push(
                out,
                Rule::Shape,
                format!("held_assumptions[{i}].id"),
                "id が [a-z0-9-]{1,40} でないか重複する",
            );
        }
    }
}

/// D1.3 の数と文の上限（[`Rule::Limit`]）。候補の全体の数は [`super::check_limits`] が見る。
pub(super) fn check_candidate_limits(input: &Input<'_>, out: &mut Vec<Violation>) {
    let doc = input.doc;
    let open = doc
        .candidates
        .iter()
        .filter(|c| c.state == CandidateState::Open)
        .count();
    let open_min = if experiment_phase(doc) {
        OPEN_MIN_WITHOUT_SELECTED
    } else {
        0
    };
    if !(open_min..=OPEN_MAX).contains(&open) {
        push(
            out,
            Rule::Limit,
            "candidates",
            format!(
                "open の候補が {open} 件で範囲 {open_min}〜{OPEN_MAX} の外（selected の無い版は比べる代替案が 2〜3 件）"
            ),
        );
    }
    for (i, c) in doc.candidates.iter().enumerate() {
        let at = |k: &str| format!("candidates[{i}].{k}");
        if c.state == CandidateState::Rejected {
            limit_count(
                out,
                at("rejected_by"),
                c.rejected_by.len(),
                1,
                REJECTED_BY_MAX,
            );
        }
        let ex = &c.experiment;
        limit_text(out, at("experiment.content"), &ex.content, TEXT_MAX_CHARS);
        limit_count(
            out,
            at("experiment.discriminates"),
            ex.discriminates.len(),
            1,
            DISCRIMINATES_MAX,
        );
        limit_text(
            out,
            at("expected_information_value"),
            &c.expected_information_value,
            TEXT_MAX_CHARS,
        );
        limit_count(out, at("stop_when"), c.stop_when.len(), 1, STOP_WHEN_MAX);
        for (j, s) in c.stop_when.iter().enumerate() {
            limit_text(out, at(&format!("stop_when[{j}]")), s, LIST_ITEM_MAX_CHARS);
        }
    }
    limit_count(
        out,
        "held_assumptions".into(),
        doc.held_assumptions.len(),
        0,
        HELD_ASSUMPTIONS_MAX,
    );
    for (i, a) in doc.held_assumptions.iter().enumerate() {
        let at = |k: &str| format!("held_assumptions[{i}].{k}");
        limit_text(out, at("statement"), &a.statement, TEXT_MAX_CHARS);
        limit_text(
            out,
            at("revisit_when"),
            &a.revisit_when,
            LIST_ITEM_MAX_CHARS,
        );
    }
}

/// D4.1: 却下した候補は選べず、次の版で同じ id・同じ内容のまま残る（[`Rule::RevisitRejected`]）。
pub(super) fn check_revisit(input: &Input<'_>, out: &mut Vec<Violation>) {
    for (i, c) in input.doc.candidates.iter().enumerate() {
        if c.selected && c.state == CandidateState::Rejected {
            push(
                out,
                Rule::RevisitRejected,
                format!("candidates[{i}].selected"),
                "却下した候補は選べない",
            );
        }
    }
    for (at, newer, older) in input.adjacent_versions() {
        for c in older
            .candidates
            .iter()
            .filter(|c| c.state == CandidateState::Rejected)
        {
            if !newer.candidates.contains(c) {
                push(
                    out,
                    Rule::RevisitRejected,
                    format!("candidates.{}", c.id),
                    format!(
                        "{at} が前の版 v{} の却下した候補を消したか、open に戻したか、書き換えた",
                        older.version
                    ),
                );
            }
        }
    }
}

/// 実行の入口の検査（D2.4・D3.7・D4.1 の `candidate`・D4.2）。違反が無ければ空の `Vec`。
/// 文書そのものの検査（[`super::validate_document`]）は呼び出し側が先に行う。
pub fn check_execution(doc: &StrategyDocument, facts: &ExecutionFacts) -> Vec<Violation> {
    let mut out = Vec::new();
    check_critiqued(facts, &mut out);
    check_phase(doc, facts, &mut out);
    check_failures_read(doc, facts, &mut out);
    out
}

/// D3.7（[`Rule::Uncritiqued`]）。
fn check_critiqued(facts: &ExecutionFacts, out: &mut Vec<Violation>) {
    if !facts.source_done {
        push(
            out,
            Rule::Uncritiqued,
            "strategy.task_id",
            "文書を作った task が done でない（final review の Critic を通っていない）",
        );
    }
    if !facts.critiqued {
        push(
            out,
            Rule::Uncritiqued,
            "strategy.sha256",
            "参照した版が文書を作った task の Critic の対象でない（古い版か衝突した版）",
        );
    }
}

/// D2.4 の 1〜6（[`Rule::ExperimentPhase`]）と、却下した候補の起票（[`Rule::RevisitRejected`]）。
fn check_phase(doc: &StrategyDocument, facts: &ExecutionFacts, out: &mut Vec<Violation>) {
    let phase = |out: &mut Vec<Violation>, path: &str, detail: String| {
        push(out, Rule::ExperimentPhase, path, detail)
    };
    let named = facts
        .candidate
        .as_deref()
        .map(|id| (id, doc.candidates.iter().find(|c| c.id == id)));
    let target = match named {
        None => None,
        Some((id, None)) => {
            phase(out, "strategy.candidate", format!("候補 {id} が文書に無い"));
            None
        }
        Some((id, Some(c))) if c.state == CandidateState::Rejected => {
            push(
                out,
                Rule::RevisitRejected,
                "strategy.candidate",
                format!("却下した候補 {id} の task は起票できない"),
            );
            None
        }
        Some((_, Some(c))) => Some(c),
    };
    match doc.candidates.iter().find(|c| c.selected) {
        None => {
            if facts.execution != Some(ExecutionMode::Atomic) {
                phase(
                    out,
                    "execution",
                    "実験段階の版（selected が無い）は execution: atomic の識別実験だけを起票できる"
                        .into(),
                );
            }
            let Some(c) = target else {
                if named.is_none() {
                    phase(
                        out,
                        "strategy.candidate",
                        "実験段階の版は strategy.candidate に open の候補を書く".into(),
                    );
                }
                return;
            };
            let started = facts
                .experiments
                .iter()
                .any(|e| e.version == doc.version && e.candidate.as_deref() == Some(c.id.as_str()));
            if started {
                phase(
                    out,
                    "strategy.candidate",
                    format!("候補 {} の実験 task はこの版で起票済み", c.id),
                );
            }
            let budget_secs = c.experiment.budget.wall_hours_max * 3600.0;
            if let Some(secs) = facts.max_wall_secs
                && secs as f64 > budget_secs
            {
                phase(
                    out,
                    "max_wall_secs",
                    format!(
                        "max_wall_secs {secs} が候補 {} の実験の予算 {budget_secs} 秒を超える",
                        c.id
                    ),
                );
            }
        }
        Some(selected) => {
            if let Some(c) = target
                && c.id != selected.id
            {
                phase(
                    out,
                    "strategy.candidate",
                    format!("selected でない候補 {} は実行段階で起票できない", c.id),
                );
            }
            let earlier: Vec<&ExperimentTask> = facts
                .experiments
                .iter()
                .filter(|e| e.version < doc.version)
                .collect();
            if earlier.is_empty() {
                phase(
                    out,
                    "strategy",
                    "前の版の実験 task が無い（実験の結果を見てから選ぶ）".into(),
                );
            }
            let running: Vec<&str> = earlier
                .iter()
                .filter(|e| !e.terminal)
                .map(|e| e.task_id.as_str())
                .collect();
            if !running.is_empty() {
                phase(
                    out,
                    "strategy",
                    format!("前の版の実験 task {} が終わっていない", running.join(", ")),
                );
            }
        }
    }
}

/// D4.2（[`Rule::FailureUnread`]）: 前の版の終端の実験 task はすべて証拠の `task`・`artifact` の参照に現れる。
fn check_failures_read(doc: &StrategyDocument, facts: &ExecutionFacts, out: &mut Vec<Violation>) {
    let cited: BTreeSet<&str> = doc
        .evidence
        .iter()
        .flat_map(|e| &e.references)
        .filter_map(|r| match r {
            Reference::Task { task_id } | Reference::Artifact { task_id, .. } => {
                Some(task_id.as_str())
            }
            _ => None,
        })
        .collect();
    for e in facts
        .experiments
        .iter()
        .filter(|e| e.version < doc.version && e.terminal && !cited.contains(e.task_id.as_str()))
    {
        push(
            out,
            Rule::FailureUnread,
            "evidence",
            format!(
                "前の版 v{} の終端の task {} を証拠が参照しない（結果・失敗の記録を読んでから版を作る）",
                e.version, e.task_id
            ),
        );
    }
}
