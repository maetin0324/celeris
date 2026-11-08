//! reviewer prompt の commit の参照の節（ADR 2026-10-23-commit-refs-by-subject D1・D6）。
//!
//! celeris は review 前にどの repo の task branch も rebase する（ADR-0118）ので、worker が書けた
//! task branch の commit hash は review の時点で古い。この節は reviewer の判定の規則（hash の食い違いを
//! 理由に不合格にしない、件名で照合する）だけを伝える。worker が書く側の規則は repo の必須規則が持ち、
//! ここに写さない（ADR 2026-11-07-health-review-integration 付記 E2）。

/// `build_review_prompt` が review context の有無に依らず出す節。
pub(super) const REVIEW_SECTION: &str = "## Commit references in progress files and reports\n\
     celeris rebases the task branch onto main before review, so every task-branch commit hash the worker \
     could have written is stale by now. Do not fail a criterion because a commit hash, a tested SHA or a fixed \
     merge-base hash is missing or differs from the current HEAD. Judge a commit listed by subject by whether a \
     commit with that subject is in `git log $(git merge-base HEAD main)..HEAD`, and judge check results by \
     rerunning them or by the deterministic checks below.\n\n";
