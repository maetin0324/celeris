//! ADR 2026-11-11 dispatcher-state-ownership D3・D4: review の判定と review の起動の判断（純粋関数）。
//!
//! review の結論・repair の選択・再同期の可否・reviewer の合格の再利用の可否を、呼び出し側が集めた fact から決める。
//! fact は既存の純粋関数の結果をそのまま受ける（写さない）: 判定の食い違いは `review_verdict.rs` の `review_discrepancy`、
//! stale の数と記録は `review_spawn.rs` の `pre_review_stale_state`、同期前の候補に束縛された reviewer の合格は
//! `bound_reviewer_pass`、repair の bucket と同じ class の数は `health_review::check_item::repair_count`、health の
//! 往復は `health_review::limits` の判定（`review_limits` が使う）。
//! git の読み出し・event の追記・delivery の保存・run の起動は適用側（`dispatcher/review_verdict.rs`・
//! `dispatcher/review_spawn.rs`）が、ここが返した提案の順に行う。
//!
//! D4 review-result: [`failed_checks`] は決定的 check の結果の列から失敗一覧（`FailedCheck`）を作る唯一の関数で、
//! 最終 review（verdict）・WU の checks・段の統合の checks が使う。一覧が空でなければどの経路も成功にしない
//! （最終 review は [`judge_review`]、WU は `WorkUnitCheckRun::failure`、段の統合は repair か replan へ回す）。
//! check の実行は今の共通実装（`review::run_work_unit_checks` など）のまま。

use task_core::{
    Check, Criterion, Event, FailedCheck, RepairClass, RepairDecision, RepoId, TaskKind,
    execution::ReviewRepairHint,
};

/// ADR-0007 D5 2.: 決定的な条件が 1 つでも落ちて reviewer run を起動しなかったときの、`Check::Reviewer` の項目の
/// reason。文字列の正本はここ 1 か所（verdict を作る `review::review_task` もこれを使う）。reviewer の判定ではないので、
/// [`review_repair_class`] は決定的な不合格があるときこの項目を修復の分類から外す（ADR-0072 D16 付記 G4-S1）。
pub(crate) const REVIEWER_NOT_EVALUATED_REASON: &str =
    "not evaluated: a deterministic check failed";

/// review の verdict の読み方。`review::Verdict` が実装する（この module は `review` に依存しない）。
pub(crate) trait VerdictView {
    fn criterion_idx(&self) -> usize;
    fn pass(&self) -> bool;
    fn reason(&self) -> &str;
    /// 受け入れ条件に無い暗黙の条件の check（`Verdict::implicit_check`）。
    fn implicit_check(&self) -> Check;
    fn repair_hint(&self) -> Option<&ReviewRepairHint>;
}

/// 1 件の決定的 check の結果。最終 review の verdict・WU の check・段の統合の check のどれも同じ形にして
/// [`failed_checks`] に渡す。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckResult {
    /// 落ちたときに repair の分類（`classify_review_failure`）へ渡す check。
    pub(crate) check: Check,
    pub(crate) pass: bool,
    pub(crate) reason: String,
    /// reviewer が `review.json` に添えた repair のヒント（health の Fail は検査器のヒント）。他は `None`。
    pub(crate) repair_hint: Option<ReviewRepairHint>,
}

/// D4 review-result: 決定的 check の結果から失敗一覧を作る（結果の順を保つ）。空でなければ成功にしない。
pub(crate) fn failed_checks(results: impl IntoIterator<Item = CheckResult>) -> Vec<FailedCheck> {
    (results.into_iter().filter(|r| !r.pass))
        .map(|r| FailedCheck {
            check: r.check,
            reason: r.reason,
            repair_hint: r.repair_hint,
        })
        .collect()
}

/// 最終 review の verdict を [`CheckResult`] にする（ADR-0072 D16）。check は task の受け入れ条件の check。暗黙の条件
/// （plan・aggregate・必須検査・research）は受け入れ条件に無いので `Verdict::implicit_check`（health の Fail だけ
/// `Check::Command`、他は修復できない `Check::Human`）。
pub(crate) fn verdict_results<'a, V: VerdictView>(
    acceptance: &'a [Criterion],
    verdicts: &'a [V],
) -> impl Iterator<Item = CheckResult> + 'a {
    verdicts.iter().map(move |v| CheckResult {
        check: (acceptance.get(v.criterion_idx()))
            .map(|c| c.check.clone())
            .unwrap_or_else(|| v.implicit_check()),
        pass: v.pass(),
        reason: v.reason().to_string(),
        repair_hint: v.repair_hint().cloned(),
    })
}

/// 1 件の command の check の結果（WU の checks・段の統合の checks。repair のヒントは無い）。
impl CheckResult {
    pub(crate) fn command(cmd: String, expect_exit: i32, pass: bool, reason: String) -> Self {
        let check = Check::Command { cmd, expect_exit };
        CheckResult {
            check,
            pass,
            reason,
            repair_hint: None,
        }
    }
}

/// ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D1: reviewer の条件（`Check::Reviewer`）が 1 つ以上あり、
/// すべて合格なら、その条件の idx（昇順）。適用側はこれを review の snapshot に束ねて `ReviewerPassed` に残す。
pub(crate) fn reviewer_criteria_passed<V: VerdictView>(
    acceptance: &[Criterion],
    verdicts: &[V],
) -> Option<Vec<usize>> {
    let criteria: Vec<usize> = (acceptance.iter().enumerate())
        .filter(|(_, c)| matches!(c.check, Check::Reviewer))
        .map(|(idx, _)| idx)
        .collect();
    let all_passed = criteria.iter().all(|idx| {
        verdicts
            .iter()
            .any(|v| v.criterion_idx() == *idx && v.pass())
    });
    (!criteria.is_empty() && all_passed).then_some(criteria)
}

/// [`judge_review`] の入力。`discrepancy` は `review_discrepancy` の結果（reviewer は合格・決定的 check は不合格）。
pub(crate) struct ReviewFacts<'a, V> {
    pub(crate) acceptance: &'a [Criterion],
    pub(crate) verdicts: &'a [V],
    pub(crate) discrepancy: Option<String>,
}

/// 最終 review の結論。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewJudgement {
    /// 失敗一覧（[`failed_checks`]）が空のときだけ true。reviewer が合格でも決定的 check が落ちていれば false。
    pub(crate) passed: bool,
    /// 不合格の verdict の理由を `; ` でつないだもの（`Status::Failed` に直結したときの bad_news の材料）。合格なら `None`。
    pub(crate) fail_message: Option<String>,
    /// 食い違いの記録文（ADR 2026-11-05-a9-llm-pilot-findings D4。event に残す）。
    pub(crate) discrepancy: Option<String>,
}

/// D3 `judge_review`: 決定的 check が不合格なら成功にしない。
pub(crate) fn judge_review<V: VerdictView>(facts: ReviewFacts<'_, V>) -> ReviewJudgement {
    let failing = failed_checks(verdict_results(facts.acceptance, facts.verdicts));
    let passed = failing.is_empty();
    let fail_message = (!passed).then(|| {
        (failing.iter())
            .map(|f| f.reason.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    });
    ReviewJudgement {
        passed,
        fail_message,
        discrepancy: facts.discrepancy,
    }
}

/// delivery の行のうち、review の結論を書き込んでよいかの照合に使う値。
pub(crate) struct DeliveryRow<'a> {
    pub(crate) worker_run: &'a str,
    pub(crate) review_run: &'a str,
    /// 行の状態が `Reviewing` か。
    pub(crate) reviewing: bool,
    pub(crate) criterion_idx: usize,
}

/// delivery の行に書く review の結論。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryVerdict {
    /// true なら `MergeQueued`、false なら `Blocked`。
    pub(crate) pass: bool,
    pub(crate) detail: String,
}

/// ADR 2026-10-27 D2・ADR 2026-11-06-reuse-pass-delivery-review-run: delivery の行がこの review の候補のもの（worker run と
/// review run が一致し、`Reviewing` のまま）で、行の条件の verdict があるときだけ結論を返す。合格は review 全体の結論と
/// 行の条件の verdict の両方が合格のとき。detail は不合格の理由と行の条件の理由（reviewer が `[needs-human]` で
/// 始めた理由は先頭にも置く）。
pub(crate) fn delivery_verdict<V: VerdictView>(
    row: &DeliveryRow<'_>,
    run_id: &str,
    review_run: &str,
    passed: bool,
    verdicts: &[V],
) -> Option<DeliveryVerdict> {
    if row.worker_run != run_id || row.review_run != review_run || !row.reviewing {
        return None;
    }
    let verdict = verdicts
        .iter()
        .find(|v| v.criterion_idx() == row.criterion_idx)?;
    let detail = (verdicts.iter())
        .filter(|v| !v.pass() || v.criterion_idx() == row.criterion_idx)
        .map(|v| v.reason())
        .collect::<Vec<_>>()
        .join("\n");
    let review_reason = (verdict.reason())
        .strip_prefix(&format!("reviewer({}): ", row.review_run))
        .unwrap_or(verdict.reason());
    let detail = if review_reason.trim_start().starts_with("[needs-human]") {
        format!("{review_reason}\n{detail}")
    } else {
        detail
    };
    Some(DeliveryVerdict {
        pass: passed && verdict.pass(),
        detail,
    })
}

/// ADR-0072 D16（Phase E4）・D16 付記 G4-S1: 最終 review の失敗一覧から repair の class を選ぶ。Execute の task だけが
/// 修復の対象。決定的な不合格があれば、reviewer run を起こさなかった `Check::Reviewer` の項目
/// （[`REVIEWER_NOT_EVALUATED_REASON`]）は reviewer の判定ではないので外す。修復できない種類が混ざれば `None`
/// （適用側は `ReviewFail` へ）。返す失敗一覧は分類と repair の objective に使った後のもの。
pub(crate) fn review_repair_class(
    kind: TaskKind,
    mut failing: Vec<FailedCheck>,
) -> Option<(RepairClass, Vec<FailedCheck>)> {
    if kind != TaskKind::Execute {
        return None;
    }
    if failing.iter().any(|f| f.check != Check::Reviewer) {
        failing
            .retain(|f| !(f.check == Check::Reviewer && f.reason == REVIEWER_NOT_EVALUATED_REASON));
    }
    match task_core::classify_review_failure(&failing) {
        RepairDecision::Repairable(class) => Some((class, failing)),
        RepairDecision::Substantive => None,
    }
}

/// repair の使用量と上限（`[execution] max_repairs`・`max_repairs_per_class`）。`same_class` は
/// `repair_count` が数えた同じ bucket の repair の数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepairUsage {
    pub(crate) repairs: usize,
    pub(crate) max_repairs: u32,
    pub(crate) same_class: usize,
    pub(crate) max_per_class: u32,
}

/// ADR-0072 D16/D18・health enforcement loop D4.4: repair の上限を使い切ったか（task 全体か同じ bucket のどちらか）。
/// 最終 review と段の統合が同じ数え方で使う。
pub(crate) fn repair_cap_reached(usage: RepairUsage) -> bool {
    usage.repairs as u64 >= u64::from(usage.max_repairs)
        || usage.same_class as u64 >= u64::from(usage.max_per_class)
}

/// 自動の再同期の上限で止めた review の記録（`ReviewResyncHalted` と snapshot の材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HaltedReview {
    pub(crate) review_run: String,
    pub(crate) repo_id: RepoId,
    pub(crate) reviewed_sha: String,
    pub(crate) target_sha: String,
    pub(crate) resyncs: u32,
}

/// review の入口で再同期を続けるかの提案（ADR-0118 D4 付記・ADR 2026-10-27 D3・D4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResyncGate {
    /// 再同期して review へ進む。`clear_halt` なら止めた記録の snapshot を消す（上限の内に戻った）。
    Proceed { clear_halt: bool },
    /// 上限を超えた。`first` なら止めた最初の回（`ReviewResyncHalted` と通知を残す）。そうでなければ止めている間で、
    /// 適用側は target の先頭 commit が静かか（[`target_quiet`]）を確かめ、静かなら再開する。
    Halt { first: bool, halted: HaltedReview },
}

/// stale の数・最後の stale・止めた記録（`pre_review_stale_state` の結果）と上限から、再同期を続けるかを決める。
pub(crate) fn resync_gate(
    stale: (u32, Option<&Event>, Option<&Event>),
    max_resyncs: u32,
) -> ResyncGate {
    let (count, last, halted) = stale;
    if count <= max_resyncs {
        return ResyncGate::Proceed { clear_halt: true };
    }
    let (review_run, repo_id, reviewed_sha, target_sha) = match halted.or(last) {
        Some(Event::ReviewTargetAdvanced {
            review_run,
            repo_id,
            reviewed_sha,
            target_sha,
            ..
        })
        | Some(Event::ReviewResyncHalted {
            review_run,
            repo_id,
            reviewed_sha,
            target_sha,
            ..
        }) => (review_run, *repo_id, reviewed_sha, target_sha),
        _ => return ResyncGate::Proceed { clear_halt: false },
    };
    ResyncGate::Halt {
        first: halted.is_none(),
        halted: HaltedReview {
            review_run: review_run.clone(),
            repo_id,
            reviewed_sha: reviewed_sha.clone(),
            target_sha: target_sha.clone(),
            resyncs: count,
        },
    }
}

/// ADR 2026-10-27 D4: target の先頭 commit（committer 時刻、UNIX 秒）から `quiet_secs` 以上たったか。
pub(crate) fn target_quiet(now_unix: i64, committed_unix: i64, quiet_secs: u64) -> bool {
    i128::from(now_unix) - i128::from(committed_unix) >= i128::from(quiet_secs)
}

/// ADR 2026-10-27 D2: 同期前の候補に束縛された reviewer の合格を再利用できるかの照合。合格の基底から今の target まで
/// に変わった file（`target_files`）と、今の target から候補までに変わった file（`task_files`）の重なり。空なら再利用
/// できる（決定的 check だけをやり直す）。重なれば reviewer をやり直す。
pub(crate) fn reviewer_reuse_overlap<'a>(
    target_files: &'a [String],
    task_files: &[String],
) -> Vec<&'a str> {
    (target_files.iter())
        .filter(|f| task_files.contains(f))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests;
