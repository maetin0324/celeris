---
title: アーキ v4 H R1 review-tests — design review と final review の prompt に長い objective の末尾が載る試験を足す
tasks: [01M7T5KM1VYFDW1RD0B90YHAZ6]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# アーキ v4 H R1 review-tests

H R1（task 01M7T5KM1VYFDW1RD0B90YHAZ6）の WorkUnit `review-tests` の記録。設計は
[ADR 2026-11-19-planner-reviewer-prompt-coverage](../../adr/2026-11-19-planner-reviewer-prompt-coverage.md) の D2。
planner の分は [planner-prompt.md](planner-prompt.md)。

完了日: 2026-11-19

## 結論

- task-dispatch に `prompt_full_objective_*` の試験を 3 件足した。どれも 3,000 文字の objective の末尾の印
  `END-OF-REVIEW-OBJECTIVE` と、全受入条件の text（3,000 文字の text の末尾の印 `END-OF-REVIEW-ACCEPTANCE` を含む 3 件）が
  reviewer の prompt に切れずに載ることを確かめる。今の code で 3 件とも通る。
- `synthetic_review_task` の objective を一時的に 1,500 文字に切ると、3 件とも落ちた（exit 100）。`review.rs` は
  `git checkout -- crates/task-dispatch/src/review.rs` で戻し、WU の base と差分が無い。
- prompt が末尾を落とす不具合は見つからなかった（repair の申告は無し）。production の code と task-worker は変えていない。

## 試験

| 試験 | file | 経路 |
| --- | --- | --- |
| `prompt_full_objective_final_review_keeps_objective_tail_and_every_criterion` | `crates/task-dispatch/src/review/tests.rs` | `synthetic_review_task` → `ReviewRequest::default()`（design 無し）の `RunContext` → `task_worker::claude_code::build_prompt` |
| `prompt_full_objective_final_review_run_passes_the_tail_to_the_reviewer` | `crates/task-dispatch/src/review/tests.rs` | `review_task` → `run_reviewer_inner` が stub の reviewer adapter に渡した `RunRequest` → `build_prompt`（request に `design` が無いことも確かめる） |
| `prompt_full_objective_design_review_keeps_objective_tail` | `crates/task-dispatch/src/health_review/design/tests.rs` | `synthetic_review_task` → `ReviewRequest { design: Some(..) }`（`DesignReviewRequest` は既存の fixture `spec`・`leaf`・`policy`・`brief_with_new_state` と `merge_risk`・`plan_summary` で組む）→ `build_prompt`。設計 review の節（`DESIGN_HEADING`）が出ることも確かめる |

- 共通の fixture `long_objective_subject` と確認の helper `assert_review_prompt_keeps_subject`（`#[track_caller]`）は
  `review/tests.rs` に置き、design の試験は `crate::review::tests` から使う。`mod` の宣言は既存のまま（production の file は
  変えていない）。
- 各試験の doc comment に、守る不変条件と、既存の試験で検出できない理由を書いた。要点: `prompt_boundary_` の 3 件
  （task-worker）のうち 3,000 文字の objective の末尾を見るのは worker の prompt だけで、`synthetic_review_task` を通らない。
  review の既存の試験は objective が 1 文字の `task_with` の task しか使わないので、objective や受入条件が途中で切れても落ちない。
- `run_design_reviewer`（`design_gate.rs`）は private の async fn で、`design/tests.rs` から呼べない。design review の試験は
  `run_design_reviewer` と同じ組み立て（`synthetic_review_task` と `ReviewRequest { design: Some(..) }`）を試験の中で行う。
  `run_design_reviewer` が `synthetic_review_task` の後で objective を変えた場合は、この試験では検出できない（未対応の範囲）。

## 検出の確認（一時的な変更。commit していない）

`crates/task-dispatch/src/review.rs` の `synthetic_review_task` の `objective: subject_task.objective.clone()` を
`objective: subject_task.objective.chars().take(1500).collect()` に変えて実行した。

- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/prompt_full_objective_/)'` → exit 100、
  `3 tests run: 0 passed, 3 failed`。3 件とも同じ assert で落ちた:
  - `final review: the objective tail mark is missing (objective line ends with "ああ…あ")`
  - `final review run: the objective tail mark is missing (objective line ends with "ああ…あ")`
  - `design review: the objective tail mark is missing (objective line ends with "ああ…あ")`
- `git checkout -- crates/task-dispatch/src/review.rs` で戻した。`git diff --quiet $CELERIS_WU_BASE -- crates/task-dispatch/src/review.rs`
  → 差分なし。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/prompt_full_objective_|boundary_case_|synthetic_review_task|reviewer_uses_department/)'`
  → exit 0、`11 tests run: 11 passed`（新しい 3 件、C の `boundary_case_` 6 件、既存の review の 2 件）
- `bash scripts/dev/test-affected.sh` → exit 0、`3941 tests run: 3941 passed (11 slow), 11 skipped`（対象 crate: celeris・
  celeris-campaign・celerisctl・llm-proxy・task-dispatch）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `git diff --name-only $CELERIS_WU_BASE` → `crates/task-dispatch/src/review/tests.rs`・
  `crates/task-dispatch/src/health_review/design/tests.rs`・この file だけ

## 変更（commit の件名）

- `arch-v4 H R1 review-tests: final review と design review の prompt に 3,000 文字の objective の末尾が載る試験 prompt_full_objective_* を task-dispatch に足す`

## 未解決

- 無し。task-worker の `prompt_boundary_` 3 件は task-worker の変更が無いのでこの WU の test-affected の対象外（task-worker を
  含む task 全体の確認は WU `record` と統合で行う）。
