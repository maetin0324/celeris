//! ADR 2026-10-23-required-checks-bound-to-candidate: 必須検査の結果を候補 SHA・基底 SHA に束縛する純粋関数。
//!
//! review は `Event::RequiredChecksEvaluated` に `(target_sha, reviewed_sha, 検査ごとの合否)` を残す。
//! 束縛（repo・target・reviewed）が違う証跡は無いものとして扱うので、候補・基底が動けば証跡は失効する。
use crate::{Event, PhaseCheckResult, RepoId};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// 必須検査の定義の指紋（cmd を順に改行で連ねた sha256 の hex）。
pub fn definition_digest(cmds: &[String]) -> String {
    let mut hasher = Sha256::new();
    for cmd in cmds {
        hasher.update(cmd.as_bytes());
        hasher.update(b"\n");
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `(repo, target_sha, reviewed_sha)` に束縛された最新の証跡の結果（無ければ `None`）。
fn latest_bound<'a>(
    events: &'a [Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
) -> Option<(&'a [PhaseCheckResult], RepoId)> {
    events.iter().rev().find_map(|e| match e {
        Event::RequiredChecksEvaluated {
            repo_id,
            target_sha: t,
            reviewed_sha: r,
            results,
            definition_repo_id,
            ..
        } if *repo_id == repo && t == target_sha && r == reviewed_sha => {
            Some((results.as_slice(), *definition_repo_id))
        }
        _ => None,
    })
}

/// 同じ束縛で既に合格した必須検査の cmd（再 review で再実行しない）。
pub fn reusable_passes(
    events: &[Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
) -> BTreeSet<String> {
    latest_bound(events, repo, target_sha, reviewed_sha)
        .map(|(results, _)| {
            results
                .iter()
                .filter(|r| r.pass)
                .map(|r| r.cmd.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// main へ取り込む前の照合。`required` は候補の HEAD にある必須検査の定義。
/// 証跡が無い・束縛が違う・1 件でも不合格・定義を読んだ repo で必須検査の結果が欠けている、は `Err`（理由つき）。
/// 定義を別の repo から読んだ証跡（複数 repo の task の 2 番目以降）は、定義の網羅を求めず合否だけを見る。
pub fn verify_required_checks(
    events: &[Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
    required: &[String],
) -> Result<(), String> {
    let Some((results, definition_repo)) = latest_bound(events, repo, target_sha, reviewed_sha)
    else {
        return Err(format!(
            "基底 {target_sha}・候補 {reviewed_sha} に束縛された必須検査の証跡がありません"
        ));
    };
    if let Some(failed) = results.iter().find(|r| !r.pass) {
        return Err(format!("必須検査が不合格です: {}", failed.cmd));
    }
    if definition_repo == repo
        && let Some(missing) = required
            .iter()
            .find(|cmd| !results.iter().any(|r| &r.cmd == *cmd))
    {
        return Err(format!("必須検査の結果が証跡にありません: {missing}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
