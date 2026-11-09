//! ADR 2026-11-08-health-enforcement-loop D4.3: health の同じ指摘の往復の判定。経路への配線（Blocked・受信箱）は呼び出し側
//! （段 wire）で、ここは task の events を読んで [`RepeatVerdict`] を返すだけの純粋関数（I/O も LLM も無い）。
//!
//! 範囲（[`HealthScope`]）ごとに、health の評価の列と、その間の修復・再試行（以下「修復」）を読む。
//!
//! | 範囲 | 評価（health の証跡の正本。D2.2） | 修復 |
//! |---|---|---|
//! | WU の key | `WorkUnitCheckFinished.health` | 同じ key の `WorkUnitChecksFailed`（daemon が run を WU の retry にすり替えた） |
//! | 段の key | 統合 WU（`integrate-<段>`）の `IntegrationCheckFinished.health` | `RepairScheduled{origin: integration}` |
//! | task の review | `RequiredChecksEvaluated.results[].health` | `RepairScheduled{origin: review}`・`Transitioned{reason: review_fail}` |
//!
//! 評価の集合は blocking の fingerprint（Pass は空）。比べるのは repo ごと（review は repo ごとに証跡を残す）。今の評価
//! （repo ごとに範囲の最後の証跡）が Fail で、次のどちらかなら [`RepeatVerdict::Repeat`]、それ以外は
//! [`RepeatVerdict::Proceed`]:
//!
//! 1. 最後の修復の前の評価と同じ集合が残る。
//! 2. 前のどれかの修復で消えた fingerprint（修復の直前の評価にあり、直後の評価に無い）が再び現れた。
//!
//! health の Error・NotApplicable の行は判定の外: 今の評価なら Proceed（Error の扱いは D3.4 の経路が決める）、前の評価
//! としては読み飛ばす（測れなかった fingerprint を「消えた」と数えない）。PolicyOnly は集合を持つ評価として数えるが、
//! repair に回らないので今の評価なら Proceed。
//!
//! 人の再開（`task_ops::plan_gate::is_human_resume`。人の replan の依頼・回答・comment・reopen）で連なりを切る: それより
//! 前の評価と修復は読まない。人が Blocked の源を再開した後の最初の評価は Proceed になる。
//!
//! `task_ops::plan_gate::stalled_replans` とは別の仕様にする。あちらは同じ `(key, 失敗の署名)` の自動 replan が変更の
//! ある commit を挟まずに続く回数を数えるが、health の修復は必ず変更を commit するので、その進展の判定では往復を
//! 検出できない。ここでは修復の前後の blocking の fingerprint の集合を比べる。

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use task_core::execution::RepairOrigin;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthEvidence, HealthVerdict};
use task_core::{Event, RepoId, Trigger};
use task_ops::plan_gate::is_human_resume;

/// 往復を数える範囲（D4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthScope<'a> {
    /// WU の check。値は WU の key（replan で WU の id が変わっても同じ key は同じ範囲）。
    WorkUnit(&'a str),
    /// 段の統合。値は段の key（統合 WU の key は `task_core::integrate_key(段)`）。
    Phase(&'a str),
    /// task の review。
    Review,
}

/// 往復の判定（D4.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepeatVerdict {
    /// 往復ではない（初回・別の集合・Pass・判定の外の Error など）。repair に回すかは呼び出し側の既存の判定のまま。
    Proceed,
    /// 同じ指摘の往復。repair をもう起こさず、Blocked にして木の root の受信箱に 1 件だけ出す。
    Repeat {
        /// 往復した repo の今の評価の blocking の fingerprint（昇順・重複なし）。
        fingerprints: Vec<String>,
        /// `fingerprints` を改行でつないだ byte 列の sha256 の hex（受信箱の項目の鍵の一部）。
        set_sha256: String,
    },
}

/// 範囲の列の 1 つ。
enum Step<'a> {
    /// health の評価 1 件。
    Evaluated(&'a HealthEvidence),
    /// 修復・再試行。
    Repaired,
}

/// repo の列の 1 つ（[`Step`] を repo で絞ったもの）。
enum Item {
    /// 集合を持つ評価（Pass・Fail・PolicyOnly）。
    Measured {
        fail: bool,
        set: BTreeSet<String>,
    },
    /// 判定の外の評価（Error・NotApplicable）。
    Outside,
    Repaired,
}

/// D4.3: `events`（task の events、古い順）から、`scope` の今の評価が同じ指摘の往復かを決める。
pub fn judge(events: &[(u64, Event)], scope: HealthScope<'_>) -> RepeatVerdict {
    let start = events
        .iter()
        .rposition(|(_, e)| is_human_resume(e))
        .map_or(0, |i| i + 1);
    let steps: Vec<Step<'_>> = events[start..]
        .iter()
        .flat_map(|(_, e)| steps_of(e, scope))
        .collect();
    let repos: BTreeSet<RepoId> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Evaluated(h) => Some(h.repo_id),
            Step::Repaired => None,
        })
        .collect();
    let repeated: BTreeSet<String> = repos
        .into_iter()
        .filter_map(|repo| repeated_set(&items_for(&steps, repo)))
        .flatten()
        .collect();
    if repeated.is_empty() {
        return RepeatVerdict::Proceed;
    }
    let fingerprints: Vec<String> = repeated.into_iter().collect();
    let set_sha256 = set_sha256(&fingerprints);
    RepeatVerdict::Repeat {
        fingerprints,
        set_sha256,
    }
}

/// 1 件の event が `scope` の列に足すもの。
fn steps_of<'a>(event: &'a Event, scope: HealthScope<'_>) -> Vec<Step<'a>> {
    let evaluated = |h: &'a Option<Box<HealthEvidence>>| -> Vec<Step<'a>> {
        h.as_deref().map(Step::Evaluated).into_iter().collect()
    };
    match (scope, event) {
        (HealthScope::WorkUnit(wu), Event::WorkUnitCheckFinished { key, health, .. })
            if key == wu =>
        {
            evaluated(health)
        }
        (HealthScope::WorkUnit(wu), Event::WorkUnitChecksFailed { key, .. }) if key == wu => {
            vec![Step::Repaired]
        }
        (HealthScope::Phase(phase), Event::IntegrationCheckFinished { key, health, .. })
            if *key == task_core::integrate_key(phase) =>
        {
            evaluated(health)
        }
        (
            HealthScope::Phase(_),
            Event::RepairScheduled {
                origin: RepairOrigin::Integration,
                ..
            },
        )
        | (
            HealthScope::Review,
            Event::RepairScheduled {
                origin: RepairOrigin::Review,
                ..
            },
        ) => vec![Step::Repaired],
        (HealthScope::Review, Event::RequiredChecksEvaluated { results, .. }) => results
            .iter()
            .filter(|r| r.cmd == HEALTH_CHECK_ID)
            .filter_map(|r| r.health.as_deref().map(Step::Evaluated))
            .collect(),
        (HealthScope::Review, Event::Transitioned { reason, .. })
            if reason == Trigger::ReviewFail.name() =>
        {
            vec![Step::Repaired]
        }
        _ => Vec::new(),
    }
}

/// 範囲の列を repo `repo` の列にする（修復は範囲全体のものなので、どの repo の列にも入る）。
fn items_for(steps: &[Step<'_>], repo: RepoId) -> Vec<Item> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::Repaired => Some(Item::Repaired),
            Step::Evaluated(h) if h.repo_id != repo => None,
            Step::Evaluated(h) => Some(match h.verdict {
                HealthVerdict::Pass | HealthVerdict::Fail | HealthVerdict::PolicyOnly => {
                    Item::Measured {
                        fail: h.verdict == HealthVerdict::Fail,
                        set: h.blocking.iter().cloned().collect(),
                    }
                }
                HealthVerdict::Error | HealthVerdict::NotApplicable => Item::Outside,
            }),
        })
        .collect()
}

/// 1 つの repo の列の今の評価（最後の評価）が往復なら、その集合を返す。
fn repeated_set(items: &[Item]) -> Option<BTreeSet<String>> {
    let now = items.iter().rposition(|i| !matches!(i, Item::Repaired))?;
    let Item::Measured { fail: true, set } = &items[now] else {
        return None;
    };
    if set.is_empty() {
        return None;
    }
    let measured_before = |at: usize| {
        items[..at].iter().rev().find_map(|i| match i {
            Item::Measured { set, .. } => Some(set),
            _ => None,
        })
    };
    let measured_after = |at: usize| {
        items[at + 1..now].iter().find_map(|i| match i {
            Item::Measured { set, .. } => Some(set),
            _ => None,
        })
    };
    let repairs: Vec<usize> = (0..now)
        .filter(|&i| matches!(items[i], Item::Repaired))
        .collect();
    // 1. 最後の修復の前の評価と同じ集合が残る。
    if let Some(&last) = repairs.last()
        && measured_before(last) == Some(set)
    {
        return Some(set.clone());
    }
    // 2. 前のどれかの修復で消えた fingerprint が再び現れた。
    let reappeared = repairs.iter().any(|&r| {
        let (Some(before), Some(after)) = (measured_before(r), measured_after(r)) else {
            return false;
        };
        before.difference(after).any(|f| set.contains(f))
    });
    reappeared.then(|| set.clone())
}

/// D4.3: 昇順・重複なしの fingerprint を改行でつないだ byte 列の sha256 の hex。
fn set_sha256(fingerprints: &[String]) -> String {
    format!("{:x}", Sha256::digest(fingerprints.join("\n").as_bytes()))
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
