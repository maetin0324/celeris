---
title: planner run の turn の上限と comment の割り込みの数え方
tasks: [01M6ZE0AJQJVE240W7DEWSE561]
status: done
updated: 2026-11-09
---
# planner run の turn の上限と comment の割り込みの数え方

完了日: 2026-11-09

ADR: `agent-docs/adr/2026-11-09-planner-turn-limit-and-comment-interrupt.md`（実装より先に commit。ADR-0074 D5.3 に付記）。

## 変更
- `ADR: planner run の turn の上限で残った計画 file を検証にかけ、comment の割り込みを試行に数えない` — 本番の事実
  （13 件・01M6YJF7HJ の計画 file・01M6ZDF9Q4 の割り込み）から原因 3 つと対処 D1〜D4、採らなかった案、ADR-0074 D5.3・
  ADR-0072 D14・ADR-0079 D9・ADR 2026-10-27-review-resync… との関係を書いた。
- `planner の turn の上限: 残った計画 file の検証・割り込みを試行から除く・既定 40/1200・prompt の予算節` — 実装:
  - D1: `crates/task-dispatch/src/health_review/design_gate.rs` の `plan_file_checked`（`Done` と `BudgetExhausted{Turns}`）。
    `dispatcher/planner_flow.rs` の `on_planner_finished` は旧い `matches!(… Terminal::Done …)` の門を消してこれを呼ぶ。
    file が無いときの理由に run の終わり方を添える（`… が見つからない（budget_exhausted(Turns): …）`）。
  - D2: `planner_attempts_in_window` は `continuation_session::interrupted_by_comment`（ADR-0140 付記の判定。`pub(super)` に
    しただけで中身は変えない）に当たる planner run を数えない。
  - D3: `task_core::PlannerConfig::default` を 40 turn / 1,200 秒に。`crates/celeris/src/config/execution.rs` の
    `default_planner_max_turns`・`default_planner_max_wall_secs` は別に持っていた 24・900 を消し、task_core の既定から導く。
  - D4: `crates/task-worker/src/claude_code/prompt.rs` の `planner_budget_section`（planner の prompt の「## 予算 (budget)」）。
- 試験（すべて偽のアダプタ。run の終わり方は試験の中で差し替え、本物の LLM は呼ばない）:
  - `crates/task-dispatch/src/dispatcher/tests/planner_budget.rs`: 偽アダプタを planner run ごとの振る舞い
    （`Plan`・`TurnsOut`・`CommentInterrupt`）の列に広げた。新しい試験 3 本:
    `planner_budget_turns_out_with_a_complete_plan_is_validated_and_adopted`（file あり → 採用、`end` は Turns のまま）、
    `planner_budget_turns_out_without_a_valid_plan_is_an_invalid_attempt`（不正な file → 拒否して attempt-1 に移す、
    file 無し → 理由に Turns、2 回で `plan_invalid`）、
    `planner_budget_comment_interrupt_is_not_a_planner_attempt`（割り込み → 上限・file 無し → 妥当な計画で採用、
    `plan_invalid` は出ない）。既存の 2 本は既定の検査を 40 / 1,200 に改めた。
  - `crates/task-worker/src/claude_code/tests.rs`: `planner_budget_prompt_states_the_turn_budget_and_the_plan_file_rule`。
  - D2 の条件を一時的に外すと `planner_budget_comment_interrupt_is_not_a_planner_attempt` が落ちる（not idle の panic）
    ことを確かめ、元に戻した。

## 旧い経路の撤去
- `on_planner_finished` の「`Terminal::Done` のときだけ計画を検証する」門は消し、`plan_file_checked` 1 か所にした。
- celeris の config の planner の既定（24・900 の重複）を消した。既定の正本は `task_core::PlannerConfig::default`。
- 互換分岐・切替 flag は置いていない。config の欄は増減していない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p task-dispatch -E 'test(planner_budget) | … | test(/planner/) | test(/comment/) | test(/session_resume/)'` → 61 passed
- `cargo nextest run -p task-worker -E 'test(planner_budget) | test(/planner_prompt/) | test(/execution_plan/)'` → 11 passed
- `python3 scripts/dev/repo-health/repo_health.py --base auto --head worktree` → exit 0、verdict pass、new 0・worsened 0
  （dispatcher module のコード行は純減。`on_planner_finished` も短くなった）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh`（実装の commit の HEAD。この後の commit は本ファイルだけ）→ exit 100。
  check-health は pass。nextest は 7985 本のうち 7984 passed・23 skipped・**1 本 timed out**:
  `celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`（600 秒の上限で打ち切り。
  実行中の host の load average は 66〜72）。web の typecheck+unit と test-ts-verify（20 passed）は通った。
- 上の 1 本を単独で再実行: `cargo nextest run -p celeris-campaign --test k4_cells_validity -E 'test(real_k4_archive_grades_thirty_of_thirty_two)'`
  → 1 passed（398 秒、load average 42〜71）。この試験は実 K4 原票の採点で、今回の変更（planner の判定・既定値・prompt）を
  通らない。task-core の逆依存として対象に入った。

## 本番への移行
- 本番 config に `[execution.planner]` の上書きは無い（operator の集計）。promote すると既定 40 turn / 1,200 秒が効く。
  config の書き換えは要らない。上書きを置いた host では、その値が優先する。
- 凍結した評価（K2・K4・IEEE Access）の release・設定には触れていない。

## promote の後に operator が見る値
- planner run のうち `WorkerFinished.end` が `BudgetExhausted{Turns}`（`error_max_turns`）で終わった件数。
  2026-11-06〜08 の 13 件（約 23 USD）と比べる。
- そのうち outcome が `done: … planned`（D1 で採用された）件数。
- `plan_invalid` の決定の件数と、その直前の planner run に `interrupted: comment` が含まれる件数（D2 の後は 0 のはず）。

## 未解決
- `real_k4_archive_grades_thirty_of_thirty_two` は単独でも約 400 秒かかり、全体の並列実行と高い load の下では
  nextest の一律の上限（`.config/nextest.toml` の 60 秒 × 10 = 600 秒）を超える。reviewer の test-affected.sh の再実行でも、
  host の負荷しだいで同じ打ち切りが起きうる（main で同じかは未確認）。凍結した評価（K4）の試験なので、この task では触れていない。
- cancel・pause・daemon の停止・孤児の回収で止まった planner run は、今も試行に数える（ADR D2 の範囲外。本番の事実が無い）。

## 提案
- `real_k4_archive_grades_thirty_of_thirty_two` に nextest の個別の上限（`[[profile.default.overrides]]` の slow-timeout）を
  置くか、試験を軽くする task を立てる（K4 の凍結に触れない範囲で、持ち主の判断が要る）。
- 上の値で `error_max_turns` が減らなければ、planner の prompt に入る G2 の文脈（規約・design_brief・health）の量を測り、
  planner に渡す範囲を絞る（turn をさらに上げる前に）。
- pause で止まった planner run も計画の失敗ではないので、D2 と同じく数えない案を検討する。
