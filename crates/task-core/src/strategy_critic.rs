//! ADR 2026-11-22-strategy-review-critic D3.1・D3.4: Critic の対象と固定の基準。
//!
//! 対象は task の events の `ArtifactProduced` から毎回導き、保存しない（規約 4）。成果物の名前の形は
//! [`crate::strategy::artifact_name`] が唯一の定義で、ここはその逆を取り、同じ関数で作り直して一致したものだけを採る。
//! 純関数だけで、I/O も LLM も無い。

use std::collections::BTreeMap;

use crate::Event;
use crate::strategy::artifact_name;

/// D3.4 の固定の基準（id と本文）。文書や run は変えられない。
pub const CRITIC_RUBRIC: [(&str, &str); 5] = [
    (
        "C1",
        "欠陥: 候補ごとに、根本原因の仮説・approach・費用・risk・保留する前提の欠陥を探して挙げる。commit の参照が実在するかも確かめる。人の承認（strategy-approve-v<n>）の sha256 と対象の sha256 が違えば欠陥として挙げる。",
    ),
    (
        "C2",
        "反証条件: 候補の仮説の falsified_by が観測で確かめられる条件か。",
    ),
    (
        "C3",
        "識別実験: experiment が discriminates の仮説を区別でき、予算が最小か。",
    ),
    (
        "C4",
        "比較: 候補を期待情報価値・費用・risk で比べたか。selected があれば、実験の結果と証拠から選んだ理由が成り立つか。",
    ),
    (
        "C5",
        "逆戻り: open の候補が、同じ文書に残る rejected の候補の approach を名前を変えて出していないか。前の版の実験の失敗・反証を証拠が読んでいるか。",
    ),
];

/// Critic の対象 1 件（D3.1）。`strategy_id` ごとに最大の版。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriticSubject {
    pub name: String,
    /// workspace からの相対 path（最後の `ArtifactProduced` のもの）。
    pub path: String,
    /// 最後の `ArtifactProduced` の sha256（記録のまま）。
    pub sha256: String,
    pub strategy_id: String,
    pub version: u32,
    /// 同じ name に 2 つの sha256 がある（子 2 D1.4 と同じ規則。Critic の条件を不合格にする）。
    pub conflict: bool,
}

/// name が戦略の成果物（`artifact_name(id, v)` の形）なら `(id, v)`。
pub fn strategy_artifact(name: &str) -> Option<(String, u32)> {
    let (id, version) = name.strip_prefix("strategy/")?.rsplit_once("/v")?;
    let version: u32 = version.strip_suffix(".json")?.parse().ok()?;
    if id.is_empty() || id.contains('/') || artifact_name(id, version) != name {
        return None;
    }
    Some((id.to_string(), version))
}

/// `sha256:` の前置きを除いた値（同じ値の書き方の違いを衝突にしない）。
pub fn bare_sha256(sha: &str) -> &str {
    sha.strip_prefix("sha256:").unwrap_or(sha)
}

/// D3.1: 対象 task の events（すべての run）の戦略の成果物から、`strategy_id` ごとに最大の版を返す（`strategy_id` の順）。
/// 空なら task は Critic と無関係。
pub fn critic_subjects(events: &[(u64, Event)]) -> Vec<CriticSubject> {
    let mut latest: BTreeMap<String, CriticSubject> = BTreeMap::new();
    for (_, event) in events {
        let Event::ArtifactProduced { artifact, .. } = event else {
            continue;
        };
        let Some((strategy_id, version)) = strategy_artifact(&artifact.name) else {
            continue;
        };
        match latest.get_mut(&strategy_id) {
            Some(s) if s.version > version => {}
            Some(s) if s.version == version => {
                s.conflict |= bare_sha256(&s.sha256) != bare_sha256(&artifact.sha256);
                s.path = artifact.path.clone();
                s.sha256 = artifact.sha256.clone();
            }
            _ => {
                latest.insert(
                    strategy_id.clone(),
                    CriticSubject {
                        name: artifact.name.clone(),
                        path: artifact.path.clone(),
                        sha256: artifact.sha256.clone(),
                        strategy_id,
                        version,
                        conflict: false,
                    },
                );
            }
        }
    }
    latest.into_values().collect()
}

#[cfg(test)]
mod tests;
