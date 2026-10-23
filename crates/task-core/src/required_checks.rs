//! ADR 2026-10-23-required-checks-bound-to-candidate: 必須検査の結果を候補 SHA・基底 SHA に束縛する純粋関数。
//!
//! review は `Event::RequiredChecksEvaluated` に `(target_sha, reviewed_sha, 検査ごとの合否)` を残す。
//! 束縛（repo・target・reviewed）が違う証跡は無いものとして扱うので、候補・基底が動けば証跡は失効する。
use crate::{Event, PhaseCheckResult, RepoId};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// 必須検査の定義の指紋（v2 domain と長さ付き cmd 列の sha256 hex）。
pub fn definition_digest(cmds: &[String]) -> String {
    let mut hasher = Sha256::new();
    // Invalidate pre-v2 evidence, which could copy one repository's results to another.
    hasher.update(b"celeris.required-checks/2\0");
    for cmd in cmds {
        hasher.update((cmd.len() as u64).to_be_bytes());
        hasher.update(cmd.as_bytes());
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
) -> Option<(&'a [PhaseCheckResult], &'a str)> {
    events.iter().rev().find_map(|e| match e {
        Event::RequiredChecksEvaluated {
            repo_id,
            target_sha: t,
            reviewed_sha: r,
            results,
            definition_sha256,
            ..
        } if *repo_id == repo && t == target_sha && r == reviewed_sha => {
            Some((results.as_slice(), definition_sha256.as_str()))
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
    required: &[String],
) -> BTreeSet<String> {
    latest_bound(events, repo, target_sha, reviewed_sha)
        .filter(|(_, digest)| *digest == definition_digest(required))
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
/// 証跡・束縛・定義指紋・必須検査の網羅・合否を repo ごとに照合する。
pub fn verify_required_checks(
    events: &[Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
    required: &[String],
) -> Result<(), String> {
    let Some((results, digest)) = latest_bound(events, repo, target_sha, reviewed_sha) else {
        return Err(format!(
            "基底 {target_sha}・候補 {reviewed_sha} に束縛された必須検査の証跡がありません"
        ));
    };
    if digest != definition_digest(required) {
        return Err("必須検査の定義指紋が候補と一致しません".into());
    }
    if let Some(failed) = results.iter().find(|r| !r.pass) {
        return Err(format!("必須検査が不合格です: {}", failed.cmd));
    }
    if let Some(missing) = required
        .iter()
        .find(|cmd| !results.iter().any(|r| &r.cmd == *cmd))
    {
        return Err(format!("必須検査の結果が証跡にありません: {missing}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
