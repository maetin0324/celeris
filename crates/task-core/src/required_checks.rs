//! ADR 2026-10-23-required-checks-bound-to-candidate: 必須検査の結果を候補 SHA・基底 SHA に束縛する純粋関数。
//!
//! review は `Event::RequiredChecksEvaluated` に `(target_sha, reviewed_sha, 検査ごとの合否)` を残す。
//! 束縛（repo・target・reviewed）が違う証跡は無いものとして扱うので、候補・基底が動けば証跡は失効する。
//!
//! ADR 2026-11-08-health-enforcement-loop D2.4〜D2.7: repo health の項目（`cmd` が [`HEALTH_CHECK_ID`]）は
//! 定義指紋に検査器の版を入れ（[`definition_digest`]）、再利用は [`reusable_health`] だけが決め、delivery の照合
//! （[`verify_required_checks`]）は [`HealthRequirement`] に従って health の証跡を確かめる。
use crate::{Event, PhaseCheckResult, RepoId};
use health::{CheckerId, HEALTH_CHECK_ID, HealthEvidence, HealthVerdict};
use std::collections::BTreeSet;

pub mod health;

/// 必須検査の定義の指紋（D2.4）。domain `celeris.required-checks/3` で、cmd の数・長さ付きの cmd 列と、
/// `health`（適用判定が `Applies` の検査器）があれば続けて [`HEALTH_CHECK_ID`] と `checker.sha256` を入れた
/// sha256 hex。cmd の数を先に入れるので、cmd 列と health の項目を取り違えない。
pub fn definition_digest(cmds: &[String], health: Option<&CheckerId>) -> String {
    let count = cmds.len().to_string();
    let health = health
        .map(|checker| [HEALTH_CHECK_ID, checker.sha256.as_str()])
        .into_iter()
        .flatten();
    health::domain_sha256(
        "celeris.required-checks/3",
        std::iter::once(count.as_str())
            .chain(cmds.iter().map(String::as_str))
            .chain(health),
    )
}

/// delivery の照合で health の項目を要るか（D2.6）。適用判定（[`health::applicability`]）から呼び出し元が作る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthRequirement<'a> {
    /// health の項目を要らない（定義指紋に検査器の版を入れない）。
    NotRequired,
    /// 適用判定が `Applies`。`checker` は今の適用判定の検査器、`reviewed_tree` は候補 `reviewed_sha` の tree
    /// （`git rev-parse <reviewed_sha>^{tree}`）。
    Applies {
        checker: &'a CheckerId,
        reviewed_tree: &'a str,
    },
}

impl<'a> HealthRequirement<'a> {
    /// 定義指紋に入れる検査器（`NotRequired` は `None`）。
    pub fn checker(&self) -> Option<&'a CheckerId> {
        match self {
            HealthRequirement::NotRequired => None,
            HealthRequirement::Applies { checker, .. } => Some(checker),
        }
    }
}

/// [`reusable_health`] の鍵（D2.5）。head の commit ではなく tree で引くので、同じ tree の別の commit の証跡を写せる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthReuseKey<'a> {
    pub repo_id: RepoId,
    pub base_sha: &'a str,
    pub head_tree: &'a str,
    pub policy_sha256: &'a str,
    pub checker_sha256: &'a str,
}

impl HealthReuseKey<'_> {
    fn binds(&self, e: &HealthEvidence) -> bool {
        e.repo_id == self.repo_id
            && e.base_sha == self.base_sha
            && e.head_tree == self.head_tree
            && e.policy_sha256 == self.policy_sha256
            && e.checker
                .as_ref()
                .is_some_and(|c| c.sha256 == self.checker_sha256)
    }
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

/// 同じ束縛で既に合格した必須検査の cmd（再 review で再実行しない）。health の項目は返さない（health の再利用は
/// [`reusable_health`] だけが決める）。
pub fn reusable_passes(
    events: &[Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
    required: &[String],
) -> BTreeSet<String> {
    latest_bound(events, repo, target_sha, reviewed_sha)
        .filter(|(_, digest)| *digest == definition_digest(required, None))
        .map(|(results, _)| {
            results
                .iter()
                .filter(|r| r.pass && r.cmd != HEALTH_CHECK_ID)
                .map(|r| r.cmd.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// event に残った health の証跡（D2.2 の正本 3 か所）。`PhaseIntegrated.checks[].health` は段の統合の記録の写しで、
/// 正本の `IntegrationCheckFinished.health` と二重に数えないので読まない。
fn recorded_health(event: &Event) -> Vec<&HealthEvidence> {
    match event {
        Event::RequiredChecksEvaluated { results, .. } => {
            results.iter().filter_map(|r| r.health.as_deref()).collect()
        }
        Event::WorkUnitCheckFinished { health, .. }
        | Event::IntegrationCheckFinished { health, .. } => health.as_deref().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// 写してよい health の証跡（D2.5〜D2.7）。`key` に束縛された最新の証跡が、verdict が Pass・report が
/// reviewable・report の digest がある、のすべてを満たすときだけ返す。最新の証跡が満たさなければ（Fail・Error・
/// PolicyOnly・dirty な作業ツリー・report が無い）、それより前の Pass も返さない。返った証跡を今の
/// `(base_sha, head_sha)` に束縛し直して `reused_from` を書くのは呼び出し元。
pub fn reusable_health<'a>(
    events: &'a [Event],
    key: &HealthReuseKey<'_>,
) -> Option<&'a HealthEvidence> {
    events
        .iter()
        .rev()
        .find_map(|e| recorded_health(e).into_iter().rev().find(|h| key.binds(h)))
        .filter(|h| h.verdict == HealthVerdict::Pass && h.reviewable && !h.report_sha256.is_empty())
}

/// main へ取り込む前の照合。`required` は候補の HEAD にある必須検査の定義、`health` は今の適用判定から作った
/// health の要求。証跡・束縛・定義指紋・health の項目・必須検査の網羅・合否を repo ごとに照合する。
pub fn verify_required_checks(
    events: &[Event],
    repo: RepoId,
    target_sha: &str,
    reviewed_sha: &str,
    required: &[String],
    health: HealthRequirement<'_>,
) -> Result<(), String> {
    let Some((results, digest)) = latest_bound(events, repo, target_sha, reviewed_sha) else {
        return Err(format!(
            "基底 {target_sha}・候補 {reviewed_sha} に束縛された必須検査の証跡がありません"
        ));
    };
    if digest != definition_digest(required, health.checker()) {
        return Err("必須検査の定義指紋が候補と一致しません".into());
    }
    if let HealthRequirement::Applies {
        checker,
        reviewed_tree,
    } = health
    {
        verify_health(results, repo, checker, reviewed_tree)?;
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

/// D2.6・D2.7: 束縛された証跡の health の項目を照合する。PolicyOnly は G2 の `policy_review_gate` の承認が
/// 束縛に結び付いた event として残らない（承認は `Approval` の子 task の状態）ので、合格にしない。
fn verify_health(
    results: &[PhaseCheckResult],
    repo: RepoId,
    checker: &CheckerId,
    reviewed_tree: &str,
) -> Result<(), String> {
    let Some(item) = results.iter().find(|r| r.cmd == HEALTH_CHECK_ID) else {
        return Err(format!(
            "health の項目（{HEALTH_CHECK_ID}）が証跡にありません"
        ));
    };
    let Some(e) = item.health.as_deref() else {
        return Err("health の項目に証跡がありません".into());
    };
    if e.repo_id != repo {
        return Err("health の証跡の repo が束縛と一致しません".into());
    }
    if reviewed_tree.is_empty() || e.head_tree != reviewed_tree {
        return Err(format!(
            "health の証跡の tree {} が候補の tree {reviewed_tree} と一致しません",
            e.head_tree
        ));
    }
    if e.checker.as_ref().map(|c| c.sha256.as_str()) != Some(checker.sha256.as_str()) {
        return Err("health の証跡の検査器の版が今の適用判定の検査器と一致しません".into());
    }
    if e.report_sha256.is_empty() {
        return Err("health の証跡に report の digest がありません".into());
    }
    if !e.reviewable {
        return Err("health の証跡は dirty な作業ツリーの report から作られています".into());
    }
    match e.verdict {
        HealthVerdict::Pass => Ok(()),
        HealthVerdict::PolicyOnly => {
            Err("health は policy の変更だけの不合格で、承認を束縛から判定できません".into())
        }
        other => Err(format!("health が合格していません: {other:?}")),
    }
}

/// 候補 `candidate` の tree にある必須検査の定義。`git` は repo で git を実行して stdout を返す。
/// file が無いと確かめられたときだけ必須検査なし。読めない・parse できない定義は拒否（fail closed）。
pub fn candidate_required_checks(
    candidate: &str,
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let path = crate::workspace_config::WORKSPACE_CONFIG_PATH;
    let object = format!("{candidate}:{path}");
    let text = match git(&["show", &object]) {
        Ok(text) => text,
        Err(error) => {
            // A failed cat-file alone cannot distinguish absence from repository corruption.
            // Confirm that the candidate tree can be listed and has no entry for this path.
            if git(&["cat-file", "-e", &object]).is_err()
                && git(&["ls-tree", "--name-only", candidate, "--", path])?
                    .trim()
                    .is_empty()
            {
                return Ok(Vec::new());
            }
            return Err(format!("cannot read required checks: {error}"));
        }
    };
    crate::workspace_config::parse(&text)
        .map(|cfg| cfg.commands.check)
        .map_err(|e| format!("invalid required checks definition at {object}: {e}"))
}

#[cfg(test)]
mod tests;
