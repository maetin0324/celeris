//! ADR 2026-11-22-strategy-review-critic D3.1・D7.2 の対象の導出の試験（prefix `strategy_critic_`）。

use super::*;
use crate::{ArtifactRef, Event};

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn produced(seq: u64, run_id: &str, name: &str, sha256: &str) -> (u64, Event) {
    (
        seq,
        Event::ArtifactProduced {
            run_id: run_id.into(),
            artifact: ArtifactRef {
                name: name.into(),
                path: format!("artifacts/{name}"),
                sha256: sha256.into(),
                kind: "file".into(),
                declared: true,
            },
        },
    )
}

/// 不変条件: 対象は task のすべての run（WorkUnit の run を含む）の `ArtifactProduced` から集め、`strategy_id` ごとに最大の版
/// だけを返す。review する run が文書を作り直さなくても対象に入る。base には対象を導く関数が無い。
#[test]
fn strategy_critic_subjects_latest_version_over_all_runs() {
    let events = vec![
        produced(
            1,
            "wu-run-1",
            &crate::strategy::artifact_name("arch", 1),
            SHA_A,
        ),
        produced(
            2,
            "wu-run-2",
            &crate::strategy::artifact_name("arch", 2),
            SHA_B,
        ),
        produced(3, "run-final", "summary.md", SHA_A),
        produced(
            4,
            "wu-run-1",
            &crate::strategy::artifact_name("other", 1),
            SHA_A,
        ),
    ];
    let subjects = critic_subjects(&events);
    let got: Vec<(&str, u32, &str, bool)> = subjects
        .iter()
        .map(|s| {
            (
                s.strategy_id.as_str(),
                s.version,
                s.sha256.as_str(),
                s.conflict,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![("arch", 2, SHA_B, false), ("other", 1, SHA_A, false)]
    );
    assert_eq!(subjects[0].name, "strategy/arch/v2.json");
    assert_eq!(subjects[0].path, "artifacts/strategy/arch/v2.json");
}

/// 不変条件: 戦略の成果物の名前（`artifact_name` の形）に一致しない成果物しか無い task は対象が空で、Critic と無関係。
/// 似た名前（拡張子・先頭の dir・版の書き方の違い）は対象にしない。
#[test]
fn strategy_critic_subjects_empty_without_strategy_artifact() {
    let events = vec![
        produced(1, "r", "summary.md", SHA_A),
        produced(2, "r", "strategy/arch/v1.md", SHA_A),
        produced(3, "r", "notes/strategy/arch/v1.json", SHA_A),
        produced(4, "r", "strategy/arch/v01.json", SHA_A),
        produced(5, "r", "strategy/arch/1.json", SHA_A),
        produced(6, "r", "strategy//v1.json", SHA_A),
        produced(7, "r", "strategy/a/b/v1.json", SHA_A),
    ];
    assert!(critic_subjects(&events).is_empty());
    assert!(critic_subjects(&[]).is_empty());
}

/// 不変条件: 同じ name に 2 つの sha256 があれば、その対象は衝突として印が付く（子 2 D1.4 と同じ規則）。`sha256:` の
/// 前置きの有無は同じ値とみなす。
#[test]
fn strategy_critic_same_name_two_sha_is_conflict() {
    let name = crate::strategy::artifact_name("arch", 1);
    let events = vec![
        produced(1, "r1", &name, SHA_A),
        produced(2, "r2", &name, &format!("sha256:{SHA_A}")),
    ];
    assert!(!critic_subjects(&events)[0].conflict);
    let events = vec![
        produced(1, "r1", &name, SHA_A),
        produced(2, "r2", &name, SHA_B),
    ];
    let subjects = critic_subjects(&events);
    assert_eq!(subjects.len(), 1);
    assert!(subjects[0].conflict);
}

/// 不変条件: 基準は C1〜C5 の固定の 5 項目で、文書や run から変えられない定数である。
#[test]
fn strategy_critic_rubric_is_fixed_c1_to_c5() {
    let ids: Vec<&str> = CRITIC_RUBRIC.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec!["C1", "C2", "C3", "C4", "C5"]);
    assert!(CRITIC_RUBRIC.iter().all(|(_, text)| !text.is_empty()));
}
