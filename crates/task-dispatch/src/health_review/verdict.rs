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
//! `health` を渡していない review（`[review]` の無い repo、WU の review）は、従来どおり `ReviewOutput` 全体を
//! 1 回で読み、`health` を見ない。

use task_core::execution::ReviewRepairHint;
use task_worker::ReviewOutput;
use task_worker::protocol::{HealthFindingOut, HealthSeverity, HealthVerdictOut};

use crate::review::Verdict;

/// I7 の不合格の reason（`{tag}: ` の後ろ）。
pub(crate) const HEALTH_VERDICT_INVALID: &str = "health verdict missing or invalid in review.json";

/// 局所修復に写らない blocking の finding に付ける repair の scope。`classify_review_failure` は `"local"` 以外の
/// scope を substantive として扱う。repair を `None` にすると reason の文面（`fmt`・`lint` など）から局所修復と
/// 推測されうるので、D5.1 の表の replan を文面に依らず固定するために明示する。
pub(crate) const SUBSTANTIVE_SCOPE: &str = "substantive";

// `review.rs` からは `Verdict` の関数として呼ぶ（`policy` の `impl Dispatcher` と同じ形）。`review` → `health_review`
// の path の参照を作ると、`health_review` → `dispatcher` → `review` と合わせて module の循環（`deps.cycle`）になる。
impl Verdict {
    /// `review.json` の本文 `text` を、reviewer の受け入れ条件 `criteria` の verdict にする。`Err` は全 criterion を
    /// 不合格にする reason（呼び出し側の `fail_all` に渡す）。`health_requested` は `ReviewRequest.health` を渡したか。
    pub(crate) fn from_review_json(
        text: &str,
        health_requested: bool,
        criteria: &[usize],
        tag: &str,
        review_rel: &str,
    ) -> Result<Vec<Verdict>, String> {
        let (output, health) = match parse(text, health_requested) {
            Ok(parsed) => parsed,
            Err(Unreadable::Output(e)) => {
                return Err(format!(
                    "{tag}: {review_rel} is not a valid ReviewOutput: {e}"
                ));
            }
            Err(Unreadable::Health) => return Err(format!("{tag}: {HEALTH_VERDICT_INVALID}")),
        };
        let verdicts = criterion_verdicts(&output, criteria, tag, review_rel);
        Ok(match health {
            Some(health) => with_health(verdicts, &health, criteria, tag),
            None => verdicts,
        })
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
