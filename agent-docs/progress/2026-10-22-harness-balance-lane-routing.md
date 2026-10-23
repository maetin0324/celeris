---
title: harness の残量選択の復活、frontier lane の規則、完了単位の根拠表
updated: 2026-10-22
tasks: [01M5H8CYQHDBSCB3VQRB60K8PJ]
status: done
completed: 2026-10-22
---
# harness の残量選択の復活、frontier lane の規則、完了単位の根拠表

ADR: `agent-docs/adr/2026-10-22-harness-balance-and-lane-first-routing.md`
（ADR-0049 D2 を復活、`2026-10-11-llm-source-harness-selection` と `…-model-routing-evaluation` に付記）

## 実装
- **D1 harness 並べ替え**: `task-core` `route_select/balance.rs`（`order_harnesses`、純粋）、`task-dispatch`
  `route_select.rs` の `order_by_headroom`（`routing_decided` の `route_decision.harness_order` に入力と結果）。
  `route_state` が account book から `QuotaSnapshot.source_remaining` / `source_headroom` を埋める
  （`accounts::remaining_fraction`、ADR-0024 D3 の `evaluate` スコア）。
- **D2 guardrail**: `balance.rs::evaluate_balance_guardrail`、`completion_outcome::evaluate_harness_balance`、
  daemon `completion_evidence_tick.rs`（15 分ごと）、`harness_balance_trips` 表、
  `celerisctl routing balance trips|reset <class>`。
- **D3/D4 frontier lane**: `policy.rs::select_frontier_lane`（lane の `route_defaults` が根拠表に勝つ。ε は frontier lane の中だけ）。
- **D5〜D7 完了単位**: migration 0090（`completion_outcomes`、`routing_evidence` に lane・`wall_p90_s`・`usd_per_pass`・
  `quota_cost_per_pass`）、`store/completion_outcomes.rs`（`record_task_decision_log` の末尾で投影）、
  `completion_outcome::refresh_routing_evidence`（daemon が 6 時間ごと）。
- **撤去**: `routing-eval aggregate --write-evidence`（評価 trial から根拠表を書く経路）。`wall_median_s`/`usd_list_median` は改名。

## 試験
- task-dispatch `route_select::harness_balance`: 残量が少ない claude で codex が先／許容幅内は宣言順／止まったクラスと
  無効クラスは宣言順／席が埋まれば次へ／frontier lane は根拠表の同等判定に勝つ／ε は frontier lane の中だけ。
  `route_state_fills_source_remaining_and_headroom_from_the_account_book`（本番経路で source_remaining が入る）。
- task-core `store::completion_outcomes::tests`: 完了単位の記録（token・USD・quota・wall・試行・差し戻し・Jev・版）、
  完了から lane ごとの根拠表→選択器、guardrail の停止と人の reset、lane の無い単位は根拠にしない。
- task-core `route_select::balance::tests`、migration を戻す既存試験は新表の DROP を追加。

## 本番への移行（operator が人に確認して適用）
1. release 後、migration 0090 が自動で当たる。旧い `routing_evidence` の行は lane 空のまま残り、**どの lane の選択にも効かない**。
   完了単位の記録が貯まる（daemon が 6 時間ごとに `completion-<時刻>` の版を書く）まで、claude_code の enforce は
   根拠表が無い lane で `route_defaults` の順になる。急ぐ場合は人が `celerisctl routing evidence` で版を確認する。
2. harness の残量切替を有効にする（**人が決める**。初期は planner / review の想定）:
   ```toml
   [route_selection]
   harness_balance_classes = ["planner", "review"]
   harness_balance_tolerance = 0.15      # 既定
   harness_balance_guard_min = 10        # 既定
   harness_balance_guard_margin = 0.15   # 既定
   ```
   無効（既定）のクラスは従来どおり宣言順。止まったら受信箱に通知が出る。再開は
   `celerisctl routing balance reset <class> --db <db>`。
3. frontier lane に規則で opus を当てるには `[route_defaults.claude_code.lanes.frontier] candidates = [{ source = "…", model = "claude-opus-5-5" }]`
   が宣言されていること（既存の書式）。宣言が無ければ frontier lane でも従来の選択。

## 未解決・提案
- guardrail の wall 比較（`GuardrailInput`）は run 窓の中央値と根拠表の p90 を比べる（p90 ≥ 中央値なので緩い側）。次の整理で窓側も p90 にする。
- 根拠表の「quota 加重」の窓の重み（7d・1 か月 1.0、5h 0.25）は初期値。本番の完了結果が貯まってから見直す。
- Jev の判定別（specificity × lane）の実績表は `completion_outcomes` から SQL で引ける。画面は別 task。
