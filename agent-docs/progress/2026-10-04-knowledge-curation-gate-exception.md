---
title: knowledge-curation の cron task は Complexity Gate を通さず atomic で dispatch する
tasks: [01M448S9BYW7T3X4N95M5DCF90]
status: done
updated: 2026-10-04
---

# PROGRESS — knowledge-curation harness の Complexity Gate 例外

正本: [ADR-0131](../adr/0131-cron-jobs.md) の「付記（2026-10-04、Complexity Gate 例外）」。

## 背景

本番 task 01M448KR2GJ8RJ4NKHGKPWZKMR（一回きりの全体整理、kb-full-cleanup、mode apply）は objective が
長いため Complexity Gate が compound と判定し、planner が `curate` / `land` の 2 WorkUnit に分けた。
daemon（`celeris::knowledge_curation`）は task 直下の `artifacts/curation-plan.json` と `inputs/` でしか
検証・適用しないため、WU が自分の `work-units/<key>/artifacts/` に書いた計画は daemon から見えず反映され
ない。planner がこれに気づいて毎回同じ decision（copy）を出し、人が毎回答える無駄な運用になっていた。

## Phase 1（完了 2026-10-04、単一 phase）

- `task_core::execution_gate::out_of_scope_rule` に 7 番目の条件を足した: genre が
  `knowledge-curation`（harness）かつ label `cron` を両方持つ task は、強制規則・規則表のスコアを
  評価する前に専用の rule_id `atomic/knowledge-curation` で atomic が決まる
  （`task_ops::knowledge_curation::is_curation_task` と同じ判定）。
- 判別に使う文字列の正本を `task_core::cron::CRON_TASK_LABEL` / `KNOWLEDGE_CURATION_HARNESS` に置き、
  `task_ops::cron_jobs::CRON_TASK_LABEL` と `task_ops::knowledge_curation::CURATION_HARNESS` はそれを
  re-export する（task-ops は task-core に依存するが逆はできないため。値が離れて drift するのを防ぐ）。
- `out_of_scope_rule` が早期 return するため、`dispatch_one` の `is_planner_dispatch` 判定
  （`decision_is_compound` が false のまま）が自動的に false になり、planner run・WorkUnit 分割は
  一切起きない（既存の `execution_gate_if_needed` / `dispatch_run.rs` のロジックは変更していない）。
- `task-ops::regate::out_of_scope_reason` と `task-dispatch::dispatcher::work_units` のコメントを
  7 条件に更新した。
- ADR-0131 に付記を足した（理由・判定の置き場所・試験の一覧）。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p task-core execution_gate::tests::knowledge_curation_cron_tasks_are_always_atomic` | 1 passed（genre/label 両方そろったときだけ `atomic/knowledge-curation`、長い objective・強制規則を満たす features でも atomic） |
| `cargo test -p task-dispatch dispatcher::tests::planning_and_gate::knowledge_curation_cron_task_skips_the_gate_and_runs_atomic` | 1 passed（偽アダプタで 1 run だけ起き、planner run が無く、`Event::ExecutionGated{rule_id: "atomic/knowledge-curation"}` が events に残る） |
| `bash scripts/dev/test-parallel.sh` | exit 0、`CELERIS_TEST_SUMMARY {"passed": 0, "failed": 0, "ignored": 13, "nextest_exit": 0, "doctest_exit": 0, ...}`、nextest 本体のログは「3893 tests run: 3893 passed (1 slow), 12 skipped」 |
| `cargo clippy --workspace -- -D warnings` | exit 0、warning 無し |
| `cargo fmt --all -- --check` | exit 0（1 箇所 `cargo fmt --all` で整形してから確認） |

## 未解決事項

- 本番昇格は未実施。この修正を含む release が current になるまで、本番の task 01M448KR2GJ8RJ4NKHGKPWZKMR
  系統（次回以降の全体整理 cron 発火）は引き続き旧判定（compound → copy の decision）で走る。
- 既に `curate`/`land` に分かれて走っている進行中の knowledge-curation task（もしあれば）は、この修正を
  含む release への昇格後も既存の `routing.execution`（Compound と記録済み）を書き換えない
  （`execution_gate_if_needed` は `routing.execution.is_some()` なら何もしない）。次回の cron 発火で
  新しく作られる task から atomic になる。

## 提案

- 木の子 task（`tree.parent_unit` を持つ）が knowledge-curation harness を名乗ることは現状無いが、もし
  将来そうなった場合もこの例外は `out_of_scope_rule` 経由で深さに関わらず適用される（`decide_at` の
  out-of-scope チェックは深さの閾値より先に評価するため）。
