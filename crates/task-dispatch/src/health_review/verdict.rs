//! ADR 2026-11-07-health-review-integration D5: reviewer run の `review.json` を受け入れ条件ごとの [`Verdict`] に
//! し、構造化 health verdict（`ReviewOutput.health`）を既存の repair/replan の分類へ写す。
//!
//! - D5.2・I6・I7: `ReviewRequest.health` を渡した review で `health` が無い・壊れている（未知の kind・severity）
//!   なら、`review.json` が読めないときと同じく全 criterion を不合格にする（reason は [`HEALTH_VERDICT_INVALID`]）。
//! - D5.3: blocking の finding 1 件ごとに、reviewer の受け入れ条件の最小の index へ不合格の [`Verdict`] を足す。
//!   `classify_review_failure` は変えない。全部が局所なら `RepairClass::ReviewerLocal`、1 件でも substantive なら
//!   `Trigger::ReviewFail`（replan）になる。
//! - advisory は合否に使わない。合格の [`Verdict`] として `Event::ReviewVerdict` と次の run の `prior_review` に残す。
//!
//! - D6.1: candidate の report が `policy.changed` で、変更 path に policy の file 以外も含めば、blocking と同じ扱いの
//!   `{tag}: health:policy_mixed: …`（substantive → replan）を足す（[`policy_mixed`]）。
//! - D6.3・I7: `policy.changed` の repo がある review で `health.policy` が無ければ全 criterion を不合格にする。
//!   `approved = false` は substantive の不合格。`approved = true` は合格の記録だけで、人の承認（D6.4・I8）は
//!   `dispatcher::policy_review` が待つ。
//!
//! `health` を渡していない review（`[review]` の無い repo、WU の review）は、従来どおり `ReviewOutput` 全体を
//! 1 回で読み、`health` を見ない。

use task_core::execution::ReviewRepairHint;
use task_worker::ReviewOutput;
use task_worker::protocol::{
    HealthFindingOut, HealthRepoInput, HealthReviewInput, HealthSeverity, HealthVerdictOut,
    PolicyVerdictOut,
};

use crate::review::Verdict;

/// I7 の不合格の reason（`{tag}: ` の後ろ）。
pub(crate) const HEALTH_VERDICT_INVALID: &str = "health verdict missing or invalid in review.json";

/// D6.3 の不合格の reason（`{tag}: ` の後ろ。後ろに変わった policy の file が続く）。
pub(crate) const POLICY_VERDICT_MISSING: &str =
    "health.policy verdict missing in review.json for the changed policy files";

/// 局所修復に写らない blocking の finding に付ける repair の scope。`classify_review_failure` は `"local"` 以外の
/// scope を substantive として扱う。repair を `None` にすると reason の文面（`fmt`・`lint` など）から局所修復と
/// 推測されうるので、D5.1 の表の replan を文面に依らず固定するために明示する。
pub(crate) const SUBSTANTIVE_SCOPE: &str = "substantive";

// `review.rs` からは `Verdict` の関数として呼ぶ（`policy` の `impl Dispatcher` と同じ形）。`review` → `health_review`
// の path の参照を作ると、`health_review` → `dispatcher` → `review` と合わせて module の循環（`deps.cycle`）になる。
impl Verdict {
    /// `review.json` の本文 `text` を、reviewer の受け入れ条件 `criteria` の verdict にする。`Err` は全 criterion を
    /// 不合格にする reason（呼び出し側の `fail_all` に渡す）。`health_input` は reviewer に渡した `ReviewRequest.health`。
    pub(crate) fn from_review_json(
        text: &str,
        health_input: Option<&HealthReviewInput>,
        criteria: &[usize],
        tag: &str,
        review_rel: &str,
    ) -> Result<Vec<Verdict>, String> {
        let (output, health) = match parse(text, health_input.is_some()) {
            Ok(parsed) => parsed,
            Err(Unreadable::Output(e)) => {
                return Err(format!(
                    "{tag}: {review_rel} is not a valid ReviewOutput: {e}"
                ));
            }
            Err(Unreadable::Health) => return Err(format!("{tag}: {HEALTH_VERDICT_INVALID}")),
        };
        let verdicts = criterion_verdicts(&output, criteria, tag, review_rel);
        match (health, health_input) {
            (Some(health), Some(input)) => {
                let verdicts = with_health(verdicts, &health, criteria, tag);
                with_policy(verdicts, input, health.policy.as_ref(), criteria, tag)
            }
            _ => Ok(verdicts),
        }
    }
}

enum Unreadable {
    /// `review.json` が `ReviewOutput` として読めない（従来の不合格）。
    Output(serde_json::Error),
    /// health を求めた review で `health` が無い・壊れている（I7）。
    Health,
}

fn parse(
    text: &str,
    health_requested: bool,
) -> Result<(ReviewOutput, Option<HealthVerdictOut>), Unreadable> {
    if !health_requested {
        return serde_json::from_str(text)
            .map(|output| (output, None))
            .map_err(Unreadable::Output);
    }
    // `health` を外して別に読む。未知の kind・severity を「ReviewOutput が読めない」でなく I7 の不合格にするため。
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(Unreadable::Output)?;
    let health = value.as_object_mut().and_then(|o| o.remove("health"));
    let output: ReviewOutput = serde_json::from_value(value).map_err(Unreadable::Output)?;
    let health = health
        .filter(|h| !h.is_null())
        .and_then(|h| serde_json::from_value::<HealthVerdictOut>(h).ok())
        .ok_or(Unreadable::Health)?;
    Ok((output, Some(health)))
}

/// 受け入れ条件ごとに reviewer の最後の判定を採る（ADR-0007 D1/D5）。判定の無い条件は不合格。
fn criterion_verdicts(
    output: &ReviewOutput,
    criteria: &[usize],
    tag: &str,
    review_rel: &str,
) -> Vec<Verdict> {
    criteria
        .iter()
        .map(
            |&idx| match output.verdicts.iter().rev().find(|v| v.criterion == idx) {
                Some(v) => Verdict {
                    criterion_idx: idx,
                    pass: v.pass,
                    reason: format!("{tag}: {}", v.reason),
                    // ADR-0072 D16（Phase E4）: reviewer 自身が申告した repair ヒントをそのまま写す
                    // （`pass = true` のときは呼び出し側〈`classify_review_failure`〉が使わない）。
                    repair_hint: v.repair.as_ref().map(|r| ReviewRepairHint {
                        scope: r.scope.clone(),
                        class: r.class.clone(),
                    }),
                },
                None => Verdict {
                    criterion_idx: idx,
                    pass: false,
                    reason: format!("{tag}: no verdict for criterion {idx} in {review_rel}"),
                    repair_hint: None,
                },
            },
        )
        .collect()
}

/// D5.3: health の finding を verdict に足す。advisory（合格）を先頭、blocking（不合格）を末尾に置く。
/// 同じ criterion の最後の verdict を採る読み手（`root_done`・task の画面）が advisory を最後の判定と
/// 読まないように、advisory は reviewer 自身の判定より前に置く（呼び出し側の並べ替えは安定 sort）。
fn with_health(
    verdicts: Vec<Verdict>,
    health: &HealthVerdictOut,
    criteria: &[usize],
    tag: &str,
) -> Vec<Verdict> {
    let Some(&idx) = criteria.iter().min() else {
        return verdicts;
    };
    let of = |severity: HealthSeverity| {
        health
            .findings
            .iter()
            .filter(move |f| f.severity == severity)
    };
    let advisory = of(HealthSeverity::Advisory).map(|f| Verdict {
        criterion_idx: idx,
        pass: true,
        reason: format!("{tag}: health:{} (advisory): {}", f.kind.as_str(), f.reason),
        repair_hint: None,
    });
    let blocking = of(HealthSeverity::Blocking).map(|f| Verdict {
        criterion_idx: idx,
        pass: false,
        reason: format!("{tag}: health:{}: {}", f.kind.as_str(), f.reason),
        repair_hint: Some(blocking_repair_hint(f)),
    });
    advisory.chain(verdicts).chain(blocking).collect()
}

/// D6.1・D6.3: policy の file を変えた repo の扱い。`Err` は全 criterion を不合格にする reason（D6.3 の欠落）。
fn with_policy(
    mut verdicts: Vec<Verdict>,
    input: &HealthReviewInput,
    policy: Option<&PolicyVerdictOut>,
    criteria: &[usize],
    tag: &str,
) -> Result<Vec<Verdict>, String> {
    let changed: Vec<&HealthRepoInput> = input.repos.iter().filter(|r| r.policy.changed).collect();
    let Some(&idx) = criteria.iter().min() else {
        return Ok(verdicts);
    };
    if changed.is_empty() {
        return Ok(verdicts);
    }
    let files = changed
        .iter()
        .flat_map(|r| r.policy.files.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(", ");
    let Some(policy) = policy else {
        return Err(format!("{tag}: {POLICY_VERDICT_MISSING}: {files}"));
    };
    let blocking = |reason: String, class: &str| Verdict {
        criterion_idx: idx,
        pass: false,
        reason,
        repair_hint: Some(ReviewRepairHint {
            scope: SUBSTANTIVE_SCOPE.to_string(),
            class: class.to_string(),
        }),
    };
    for mixed in changed.iter().filter_map(|r| policy_mixed(r)) {
        verdicts.push(blocking(
            format!("{tag}: health:policy_mixed: {mixed}"),
            "policy_mixed",
        ));
    }
    if policy.approved {
        // 合格の記録（advisory と同じく reviewer 自身の判定より前）。合否は人の承認で決まる（D6.4）。
        verdicts.insert(
            0,
            Verdict {
                criterion_idx: idx,
                pass: true,
                reason: format!(
                    "{tag}: health:policy (approved by reviewer; human approval required): {files}: {}",
                    policy.reason
                ),
                repair_hint: None,
            },
        );
    } else {
        verdicts.push(blocking(
            format!("{tag}: health:policy: {files}: {}", policy.reason),
            "policy",
        ));
    }
    Ok(verdicts)
}

/// D6.1: `policy.changed` の repo の変更 path に policy の file 以外が含まれていれば、その説明を返す。`diff.paths`
/// は上限で切り詰めるので、総数（`diff.files`）から見えない path の数も数える。
pub(crate) fn policy_mixed(repo: &HealthRepoInput) -> Option<String> {
    if !repo.policy.changed {
        return None;
    }
    let is_policy = |path: &str| repo.policy.files.iter().any(|f| f == path);
    let others: Vec<&str> = repo
        .diff
        .paths
        .iter()
        .map(|p| p.path.as_str())
        .filter(|p| !is_policy(p))
        .collect();
    let listed_policy = repo
        .diff
        .paths
        .iter()
        .filter(|p| is_policy(&p.path))
        .count();
    let unlisted = usize::try_from(repo.diff.files)
        .unwrap_or(usize::MAX)
        .saturating_sub(repo.diff.paths.len());
    let unlisted_others =
        unlisted.saturating_sub(repo.policy.files.len().saturating_sub(listed_policy));
    if others.is_empty() && unlisted_others == 0 {
        return None;
    }
    let more = if unlisted_others > 0 {
        format!(" (+{unlisted_others} more)")
    } else {
        String::new()
    };
    Some(format!(
        "{}: policy files [{}] changed together with other files [{}]{more}; move the policy change into its own task",
        repo.repo_id,
        repo.policy.files.join(", "),
        others.join(", ")
    ))
}

/// D5.1 の表: `removal_missing`・`test_change_unjustified` で `repair.scope = "local"` のものだけ reviewer の
/// ヒントを写す（局所修復）。ほかは [`SUBSTANTIVE_SCOPE`]（replan）。
fn blocking_repair_hint(finding: &HealthFindingOut) -> ReviewRepairHint {
    match &finding.repair {
        Some(r) if finding.kind.allows_local_repair() && r.scope == "local" => ReviewRepairHint {
            scope: r.scope.clone(),
            class: r.class.clone(),
        },
        _ => ReviewRepairHint {
            scope: SUBSTANTIVE_SCOPE.to_string(),
            class: finding.kind.as_str().to_string(),
        },
    }
}

#[cfg(test)]
mod tests;
