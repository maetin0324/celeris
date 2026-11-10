---
title: 健全性v2 G4 fix-s1 — 未評価の reviewer の項目が決定的な不合格の修復を妨げないようにする
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-10
---
# 健全性v2 G4 fix-s1 — 未評価の reviewer の項目が決定的な不合格の修復を妨げないようにする

完了日: 2026-11-10

欠け G4-S1 を直した。`Check::Reviewer` のある task で決定的な条件が落ちると、review は reviewer run を起こさず reviewer の
項目を「未評価」の不合格にする。修復の分類はこの項目も数えていたので、fmt・lint などの修復できる不合格も health の Fail
（`health:<rule_id>`）も repair に回らず、`ReviewFail`（retry 切れなら failed）になっていた。今は未評価の項目を分類から外す。
基準は `$CELERIS_WU_BASE`。`task_core::classify_review_failure`・`health_review/`・検査器・policy は変えていない。

## 実装前 brief
1. 根本原因: `crates/task-dispatch/src/review.rs` は決定的な条件が 1 つでも落ちると、`Check::Reviewer` の項目を reason
   「not evaluated: a deterministic check failed」の不合格にする。`Dispatcher::try_review_repair`
   （`dispatcher/review_verdict.rs`）はこの項目も `FailedCheck` に入れて `classify_review_failure` に渡す。reviewer の項目は
   class を持たない（`classify_reviewer_reason` の対象の語も無い）ので、混ざると `Substantive` になる。症状は review の後の
   遷移（`review_fail`）に出る。
2. 責務 owner: 未評価の reason を作るのは `review.rs`（ADR-0007 D5 2.）。verdict を分類の入力に写す意味（暗黙の条件の
   `Verdict::implicit_check`、health の Error の `Verdict::health_check_error`）も review module が持つ。分類そのものは
   task-core が持ち、変えない。
3. 既存の再利用先: `Verdict::implicit_check`、`task_core::classify_review_failure`、試験の土台の `ParallelWuAdapter`
   （`with_review`・`with_file`・`with_action`）、`instant_done_dispatcher`、`health_enforce.rs` の `commit_base_checker`・
   `commit_fixed`・`non_celeris_dispatcher`、`target_sync.rs` の `reviewing_task`・`set_check`・`commit_file`。
4. 選択肢: (i) task-core の分類で reason を見る案は採らなかった。reason の意味は review の持ち物で、objective も task-core を
   変えないとしている。(ii) `try_review_repair` の中に filter を足す案は、最初の commit で実装したが、dispatcher module の本体の
   行が 35120 → 35130 に増え、`size.responsibility-growth` が worsened になった（check-health の exit 1）。(iii) 採用: 不合格の
   一覧を作る処理を `review::repair_failing`（reason の const と同じ file）に移し、`try_review_repair` はそれを呼ぶ。暗黙の
   条件の写し（`implicit_check`）と未評価の除外は、どちらも「review の verdict が分類に対して何を意味するか」で、review
   module の責務に当たる。行を別 file に付け替えただけの移動ではなく、reason を作る module が、その reason を分類から外す
   規則も持つようにした。
5. 新しい状態・分岐・fallback・port: 分岐 1 つ（決定的な不合格があり、`Check::Reviewer` で reason が const の項目を外す）。
   状態・fallback・port は無し。
6. 撤去対象: reason の文字列リテラルの 2 か所目（`dispatcher/tests/ownerless_runs.rs` の試験の比較）を const に置き換えた。
   `try_review_repair` の中の `FailedCheck` の組み立ては `repair_failing` に移して消した。
7. 不変条件: 下の 3 試験。reviewer の判定を飛ばして done にはしない（修復の後の review で reviewer run がもう一度走る。
   試験 (1) と (3) は reviewer の合格の `review.json` を書いてから done になることを確かめる）。

## 変更
- `phase 1: keep not-evaluated reviewer items out of the review repair classification`
  - `crates/task-dispatch/src/review.rs`: `pub(crate) const REVIEWER_NOT_EVALUATED_REASON` を置き、未評価の verdict はこれを使う。
  - `crates/task-dispatch/src/dispatcher/tests/review.rs`: 試験 (1)・(2) と helper `fmt_and_reviewer_task`・`review_repair_classes`。
  - `crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`: 試験 (3)。
  - `crates/task-dispatch/src/dispatcher/tests/ownerless_runs.rs`: reason の比較と冒頭の注釈を const に置き換えた（期待は同じ）。
  - `agent-docs/adr/0072-task-execution-decomposition.md`: D16 の後に付記（2026-11-10、G4-S1）。
- `phase 1: build the review repair failing list in review.rs next to the not-evaluated reason`
  - `review.rs` に `repair_failing(task, verdicts)` を置き、`dispatcher/review_verdict.rs` の `try_review_repair` はそれを呼ぶ。
    ADR の付記も置き場所に合わせて直した。
- `phase 1: record fix-s1 progress` — この file。

## 試験
| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| (1) `review_repair_not_evaluated_reviewer_does_not_block_a_format_repair`（`tests/review.rs`） | `Check::Reviewer` と fmt の検査の task で fmt だけが落ちると、`RepairScheduled` の class `format` になり、`review_fail` にならない。repair の objective に未評価の reason が入らない。修復の後の review で reviewer が合格し、done・attempts 0 | `a_format_only_review_failure_*` は `Check::Reviewer` の無い task |
| (2) `review_repair_not_evaluated_only_skips_the_not_evaluated_reason`（`tests/review.rs`） | reviewer が実際に不合格にした項目（reason が const でない）と fmt の不合格が混ざると、今までどおり repair にならず `review_fail`（max_retries 0 で failed）。WU も作らない | (1) だけでは「`Check::Reviewer` の項目をすべて外す」誤った直し方を検出できない |
| (3) `review_repair_not_evaluated_reviewer_does_not_block_a_health_repair`（`tests/health_enforce.rs`） | base の stub の検査器の health の Fail が、`Check::Reviewer` のある task でも `RepairScheduled` の class `health:deps.violation` になり、修復の後に health が Pass、reviewer の合格で done・attempts 0 | `health_enforce_non_celeris_direct_review_routes_to_repair` は `Check::Reviewer` の無い task |

試験が欠けを検出することの確認（提出前に一時的に本体を変えて実行し、元に戻した。sha256 で照合）:
- 除外を無効にすると (1)・(3) が落ち、(2) は合格した。
- `Check::Reviewer` の項目をすべて外すように変えると (2) だけが落ちた。

既存の試験の期待は変わっていない。`ownerless_runs.rs` の `a_reviewer_run_not_launched_because_a_check_failed_is_closed` は
決定的な検査が `false`（修復できない class）なので、今も `review_fail` になる。比較の文字列を const にしただけ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_generic_) | test(review_repair_not_evaluated_)'`: 22 passed（health_generic_ 19 件、うち 4 件は G4-R1 の `should_panic` のまま panic して合格。review_repair_not_evaluated_ 3 件）。
- `bash scripts/dev/test-affected.sh`（`phase 1: record fix-s1 progress` の HEAD。この後の commit はこの file だけ）: exit 0。
  check-health は pass（`verdict=pass fail_grade=0 new=0 worsened=0`）。対象 crate は celeris・celeris-campaign・celerisctl・
  llm-proxy・task-dispatch。nextest は 3728 passed・11 skipped（577 秒、load average 23〜65）。health の unittest は 94 件 OK、
  `OK (expected failures=2)`（G1・G2）。
- 1 つ前の実行（2 つ目の commit の HEAD、同じ 3728 本）は exit 100 だった。1 failed・2 timed out で、どれも今回の分岐を通らない。
  - failed: `dispatcher::tests::git_workspace::the_repository_check_commands_are_the_reviewers_default`（`ReviewVerdict` が 0 件。
    `run_until_idle(&mut d, 60)` の 60 tick の内に review まで進まなかった）。単独で 3 回再実行し、3 回とも合格（2〜3 秒）。
  - timed out: `celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two` と
    `celeris-campaign::k_cases_grade k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one`（600 秒の上限）。
    実 K4 原票の採点。高負荷で上限を超えることは `agent-docs/progress/2026-11-09-planner-turn-limit.md` に記録がある。
    最後の実行では 3 件とも合格した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- reason の文字列: `grep -rn 'not evaluated: a deterministic check failed' crates/` は `review.rs` の const の 1 行だけ。

## 未解決
- 無し。campaign の実 K4 原票の試験が高負荷で 600 秒の上限を超えうることは既知（2026-11-09 の進捗の提案）で、この task の範囲の外。

## 提案
- 無し。
