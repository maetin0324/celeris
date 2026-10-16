---
tasks: [01M530Y2NDFDSJ1PB3N9MHF1K8]
updated: 2026-10-16
status: done
completed: 2026-10-16
title: "[監査修正3] 比較条件の継承と実行構成の照合"
---

# 監査修正 3: routing の継承と実測照合

## 再確認
着手時の main は 0e73be29 の系列（449412b2）。`task_routing_override_impl` は自身の行のみを読む状態を確認し、修正した。ADR: 2026-10-16-mlsys-routing-inheritance-reconciliation。

## 項目別の状態
- 実装済み・mock/単体検証済み
  - 継承: `crates/task-core/src/store/routing_overrides.rs`（祖先鎖。試験: 最も近い祖先が勝つ）、`routing_outcome.rs`（online_update=false を子でも尊重）。
  - dispatcher 試験 `root_routing_pin_is_inherited_by_child_grandchild_and_retry_whatever_the_config`: 全体 config を B/C と逆にしても root・子・孫・再試行が各 arm の mode・根拠表・online_update を維持し、固定の無い task は config に従う。
  - 実測: `ObservedRun`（Celeris 木の全 run／A は起動 command）。checker の照合（別 model・別 harness・online_update=true・全体 config への fallback を違反）。export は実測の model/harness/source/routing、宣言は declared_*。試験は `crates/campaign/tests/routing_pin.rs`・`harness_exec.rs`。
  - 比較条件: B/C の同一 lower_policy/models/run_harnesses を final で検査。`no_evidence_table` で根拠表なしの C を明示。
- 実経路検証済み: なし（実 LLM・本番 daemon は使っていない）。
- 承認待ち: なし。本測定の開始・予算増額はしていない。

## 本番への移行
manifest（final.json）に `routing.models` / `routing.run_harnesses` を B/C 同一で宣言する運用を推奨（未宣言でも routing の照合は効く）。config 欄の削除は無い。

## 提案
- `ArmDispatch` は `usage` を転送していない（実行中の上限判定が B/C/A で効かない）。別 task で確認を。
- Celeris の実 daemon で routing_resolved が子 run の basis に載ることの実経路確認は本測定前の preflight で行う。
