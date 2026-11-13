---
title: "健全性v2 R4 settle-replan: run_settlement を作り、wu-failure の Replan か Ask の上限判定を replan_or_ask の 1 つにする"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: running
updated: 2026-11-13
---
# 健全性v2 R4 settle-replan

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D4 の wu-failure の行（担当 unit settle）の共有の関数 `run_settlement::replan_or_ask` を、
  新しい `crates/task-dispatch/src/run_settlement.rs` に 1 つだけ定義した（`lib.rs` に `pub(crate) mod run_settlement;`）。
  - 入力 `ReplanLimits`: `stalled`（`task_ops::plan_gate::stalled_replans` の結果）・`replans`（`counted_replans`）・
    `max_replans`（`effective_max_replans`）・`ban`（replan を禁止する理由 `ReplanBan::HealthStop`。統合の `stops_without_replan`）。
  - 返す値 `ReplanOrAsk`: `Replan` か `Ask(AskReason)`。理由は `Stalled`・`Banned`・`Exhausted`。優先は stalled → 禁止 → 使い切り
    （統合の失敗で health の禁止と stalled が重なると stalled の質問文を出す今の挙動を保つ）。数は入力の `ReplanLimits` から読む。
- 次の比較を `replan_or_ask` の呼び出しに置き換え、旧い比較（`stalled >= STALLED_REPLAN_LIMIT`・`replans_so_far < effective_max_replans`）を消した。
  - `dispatcher/worker_finish.rs`: WU の失敗の分岐（`failed`・`limit`・`plan_issue`）。
  - `dispatcher/work_units.rs`: 遅延 WU の分岐（`deferred_work_unit_trigger`）と `replan_gate`。
  - `dispatcher/phase_integration.rs`: 統合の失敗の分岐（`integration_gives_up_with`）。`can_replan` は `Replan` のときだけ。
  - `dispatcher/tree_units.rs`: 木の liveness の事実 `replans_left`。
- replan の実行（`AutoReplanRequested`・`Continue{Replan}`）・event の追記・質問の文（`replan_stalled_question`・`replan_exhausted_ask`）・
  決定の要求（`raise_node_replan_limit`）・`PLAN_ISSUE_REPLAN_HINT` は各経路に残した。transaction の範囲は変えていない（D3）。
- `run_settlement` は入力の値だけを読む純粋な module。永続化・file・process・`dispatcher` module に触れない。
- `dispatcher.rs` の module map に `run_settlement` の行を、`docs/architecture-map.md` に run_settlement の行を足した。ADR は変えていない。

## replan_gate と replans_left の扱い

- `work_units.rs` の `replan_gate`: 仕事の残らない計画で「replan の余地があれば planner を起こし、無ければ木の節点の決定
  `limit:max_replans` を求めて `Skip`」。上限の比較は wu-failure と同じ述語（`replans < max_replans`）で、Ask の側も
  `replan_exhausted_ask` の木の分岐と同じ `raise_node_replan_limit`。意味が同じなので `replan_or_ask` に置き換えた。失敗の署名が無いので
  `stalled` は 0、禁止の理由は無い。`work_units.rs` に残る上限の比較は 0 か所。
- `tree_units.rs` の `replans_left`（liveness の事実）: 「この節点の WU が失敗したら自動 replan が来るか」を見る値で、wu-failure の
  `Replan` と同じ述語。意味が同じなので `replan_or_ask(..) == Replan` から導く（stalled 0・禁止無し）。
- `planner_flow.rs` の `replans_left`・`node_replans_left`（prompt に渡す残り回数の数）は比較でなく引き算の表示なので、この unit の範囲外として変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo build -p task-dispatch`: exit 0（警告なし）。
- `cargo nextest run -p task-dispatch -E 'test(run_settlement) | test(cex_) | test(stall) | test(replan) | test(liveness)'`:
  exit 0、95 passed（`run_settlement::tests` 3 件、cex_ の反例試験、stalled_replan・tree_replan・replan_rebase を含む）。cex_ の試験は書き換えていない。
- `grep` で `worker_finish.rs`・`phase_integration.rs`・`work_units.rs`・`tree_units.rs` に `replans_so_far`・`STALLED_REPLAN_LIMIT` の比較・
  `< self.effective_max_replans` が無いことを確かめた（`phase_integration.rs` の doc comment の `STALLED_REPLAN_LIMIT` への参照 1 件だけ）。

## 足した試験

`crates/task-dispatch/src/run_settlement/tests.rs` の 3 件。守る不変条件と、既存の試験で検出できない理由は各試験の doc comment に書いた。

- `replan_or_ask_replans_only_below_the_limit`: 上限ちょうどと上限 0 は使い切り（比較が `<=` に変わると落ちる）。
- `replan_or_ask_stalled_wins_over_ban_and_limit`: `STALLED_REPLAN_LIMIT` 回で上限に余りがあっても Ask、禁止とも重なると stalled が先。
- `replan_or_ask_ban_wins_over_the_limit`: 禁止があれば上限に余りがあっても Ask、使い切りと重なると禁止が先。

## 未解決事項

- 無し。

## 提案

- 無し。
