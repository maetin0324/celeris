---
title: 決定的な検査が落ちたら reviewer の合格でも done にしない（review-check）
tasks: [01M6MJEG72MD982T211EKAM74G]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# review-check（問題 4）

## 結論
ADR D4 の「`on_review_finished` が検査結果を見ず verdict だけで done にする」という説明は、コードの読みと試験では当たらなかった。決定的な検査が落ちた task は、reviewer が合格を返しても元から done にならない。足りなかったのは食い違いの記録で、これを足した。

## 原因（確かめたこと）
- `crates/task-dispatch/src/review.rs` の `review_task` は `Check::Command` を reviewer より先に実行し、1 つでも落ちると reviewer を起動せず、reviewer の基準を `not evaluated: a deterministic check failed` で不合格にする。
- `on_review_finished`（`dispatcher/review_verdict.rs`）の `all_pass` は全 verdict の AND で、command の fail が残る限り `ReviewFail` になる。
- したがって C06・C08 の pilot で root が done になったのは、acceptance に command の検査が無く（`check` が Reviewer だけ）、検査が落ちる余地が無かったため。これは D2 の pilot 課題側（`check-public-<id>.sh` を command で持たせる）の問題で、本 WU の範囲外。
- 試験の先行確認: 実装前でも `failed_command_check_wins_over_reviewer_pass` の「done にならない」部分は通った（上の読みを裏づける）。

## 修正
- `review_verdict.rs`: reviewer の基準が pass で reviewer 以外の判定が fail の食い違いを検出し、`review discrepancy: reviewer passed but a deterministic check failed; …` を `WorkerProgress` として残す。文面に reviewer の reason と落ちた検査の reason を含める。新しい event 種は足していない。
- reviewer の reason は `ReviewVerdict` event に元から残っている（`reviewer(<run>): <reason>`）。試験で固定した。

## 試験（fake の判定を `on_review_finished` に渡す。LLM 不使用）
- `dispatcher/tests/review_check_wins.rs`
  - `failed_command_check_wins_over_reviewer_pass`: command fail + reviewer pass で done にならず、食い違い event と reviewer の reason が残る。
  - `reviewer_only_task_is_unchanged`: 検査の無い task は従来どおり done、食い違い event なし。

## 未解決
- 実運用では、検査が落ちると reviewer は起動しないので、食い違い event が出るのは reviewer を再利用する経路（`reviewer_reuse`）などに限る。
- ADR D4 の 4（stub reviewer を「検査できない基準は pass にしない」にする）と `driver.py`・`stub_harness.py` の変更は、本 WU の範囲外（pilot-git・別 WU）で未実施。
- `build_review_prompt` の `## Deterministic checks` 節の変更は不要と判断（command の結果は既に渡る）。実 LLM の reviewer での確認は未実施。

## 検査（提出時の HEAD で実行）
- `cargo test -p task-dispatch review_check_wins`: 2 passed。
- clippy・fmt・test-affected は下の「検査結果」に記す。

## 検査結果（提出時の HEAD で実行）
- `cargo fmt --all -- --check`: exit 0。`cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`: 2 回実行。1 回目は `celeris::script_runtime script_thread_count_skips_joined_but_listed_thread`、2 回目は `celeris::script_generation_e2e script_generation_e2e_switch_error_rollback_restart` が落ちた（2 回目は 3351 件中 3350 pass・1 fail）。落ちる試験が回ごとに違い、script runtime の子 process・zombie 周りで、task-dispatch の変更とは無関係に見える（base での再現は未確認）。
